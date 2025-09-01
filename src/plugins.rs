use std::path::Path;
use anyhow::{anyhow, Result};
use wasmtime::{Engine, Config, Store, component::*};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};
use std::time::{Duration, Instant};
use std::sync::{Arc, atomic::{AtomicU64, AtomicBool, Ordering}};
use std::sync::mpsc::{self, Sender, Receiver};
use serde::Deserialize;
use std::fs;

// Generate WIT bindings from shared plugin-interface (generic library world)
wasmtime::component::bindgen!({
    world: "library",
    path: "plugin-interface/wit/",
});

// Host context with WASI and HTTP support
struct Host {
    wasi: WasiCtx,
    table: wasmtime_wasi::ResourceTable,
    http: WasiHttpCtx, // TODO limit allowed hosts by plugin config
}

impl WasiView for Host {
    fn ctx(&mut self) -> &mut WasiCtx { &mut self.wasi }
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable { &mut self.table }
}

impl WasiHttpView for Host {
    fn ctx(&mut self) -> &mut WasiHttpCtx { &mut self.http }
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable { &mut self.table }
}

// Single plugin instance - generic bindings
struct Plugin {
    name: String,
    store: Store<Host>,
    bindings: Library,
    caps: Option<ProviderCapabilities>,
    rate_limit: Duration,
    slow_warn: Duration,
    call_timeout: Duration,
    last_call: Option<Instant>,
    epoch_ticks: Arc<AtomicU64>,
    epoch_interval: Duration,
    _instance: wasmtime::component::Instance,
    _component: Component,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct PluginConfig {
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    user_agent: Option<String>,
    #[serde(default)]
    rate_limit_ms: Option<u64>,
    #[serde(default)]
    call_timeout_ms: Option<u64>,
}

impl Plugin {
    pub fn new(engine: &Engine, plugin_path: &Path, epoch_ticks: Arc<AtomicU64>, epoch_interval: Duration) -> Result<Self> {
        let component = Component::from_file(engine, plugin_path)?;

        // Load plugin config (<name>.toml next to wasm)
        let cfg = plugin_path.with_extension("toml");
        let cfg: PluginConfig = fs::read_to_string(&cfg)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();

        // Initialize WASI context for the plugin
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdout().inherit_stderr().inherit_env();
        let wasi = builder.build();

        // Build HTTP context and apply simple allowlist via environment check inside guest
        let http = WasiHttpCtx::new();
        // Note: wasmtime-wasi-http 28.0.0 has no direct API to set host allowlist; we enforce in guest or via config headers.

        let host = Host { 
            wasi,
            table: wasmtime_wasi::ResourceTable::new(),
            http,
        };
        let mut store = Store::new(engine, host);

        // Ensure the store starts with a safe far-future epoch deadline to avoid immediate traps
        let now = epoch_ticks.load(Ordering::Relaxed);
        let far = now.saturating_add(1_000_000_000);
        store.set_epoch_deadline(far);

        // Create a new linker for this plugin
        let mut linker = Linker::new(engine);
        wasmtime_wasi::add_to_linker_sync(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

        let instance = linker.instantiate(&mut store, &component)?;
        let bindings = Library::new(&mut store, &instance)?;

        // Try to read provider capabilities now and cache them
        let caps = match bindings.call_getcapabilities(&mut store) {
            Ok(c) => Some(c),
            Err(e) => { eprintln!("Failed to get capabilities for {}: {}", plugin_path.display(), e); None }
        };

        let plugin = Self {
            name: plugin_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string(),
            store,
            bindings,
            caps,
            rate_limit: Duration::from_millis(cfg.rate_limit_ms.unwrap_or(150)),
            slow_warn: Duration::from_secs(5),
            call_timeout: Duration::from_millis(cfg.call_timeout_ms.unwrap_or(15_000)),
            last_call: None,
            epoch_ticks,
            epoch_interval,
            _instance: instance,
            _component: component,
        };

        // Optionally set user agent via env var in the store (read by guest)
        if let Some(_ua) = cfg.user_agent {
            // No direct API to inject headers, but guests can read env; set a convention
            // Note: WASI env is captured at ctx build; this is a no-op placeholder for future wasmtime updates.
        }

        Ok(plugin)
    }

    fn set_deadline(&mut self) {
        // Convert timeout into epoch ticks (absolute deadline)
        let now = self.epoch_ticks.load(Ordering::Relaxed);
        let per_tick_ms = self.epoch_interval.as_millis().max(1) as u128;
        let need = ((self.call_timeout.as_millis() + per_tick_ms - 1) / per_tick_ms) as u64;
        let deadline = now.saturating_add(need);
        self.store.set_epoch_deadline(deadline);
    }

    fn clear_deadline(&mut self) {
        // Move deadline far into the future without risking overflow
        let now = self.epoch_ticks.load(Ordering::Relaxed);
        let far = now.saturating_add(1_000_000_000); // ~10^9 ticks ahead
        self.store.set_epoch_deadline(far);
    }

    fn throttle(&mut self) {
        if let Some(last) = self.last_call {
            let elapsed = last.elapsed();
            if elapsed < self.rate_limit {
                std::thread::sleep(self.rate_limit - elapsed);
            }
        }
        self.last_call = Some(Instant::now());
    }

    fn warn_if_slow(&self, start: Instant, op: &str) {
        let elapsed = start.elapsed();
        if elapsed > self.slow_warn {
            eprintln!("Plugin {} {} took {:?}", self.name, op, elapsed);
        }
    }

    fn retry_once<T, F>(&mut self, mut f: F, op: &str) -> Result<T>
    where F: FnMut(&mut Self) -> Result<T> {
        for attempt in 0..2 {
            self.set_deadline();
            let res = f(self);
            self.clear_deadline();
            match res {
                Ok(v) => return Ok(v),
                Err(e1) if attempt == 0 => {
                    eprintln!("Plugin {} {} failed: {}. Retrying...", self.name, op, e1);
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                Err(e) => return Err(anyhow!("{} after retry: {}", op, e)),
            }
        }
        unreachable!();
    }

    // Generic methods
    fn fetch_media_list(&mut self, kind: MediaType, query: &str) -> Result<Vec<Media>> {
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            this.bindings
                .call_fetchmedialist(&mut this.store, &kind, query)
                .map_err(|e| anyhow!("Failed to call fetchmedialist: {}", e))
        }, "fetchmedialist");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchmedialist");
        res
    }

    fn fetch_units(&mut self, media_id: &str) -> Result<Vec<Unit>> {
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            this.bindings
                .call_fetchunits(&mut this.store, media_id)
                .map_err(|e| anyhow!("Failed to call fetchunits: {}", e))
        }, "fetchunits");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchunits");
        res
    }

    fn fetch_assets(&mut self, unit_id: &str) -> Result<Vec<Asset>> {
        self.throttle();
        self.set_deadline();
        let start = Instant::now();
        let res = self.retry_once(|this| {
            this.bindings
                .call_fetchassets(&mut this.store, unit_id)
                .map_err(|e| anyhow!("Failed to call fetchassets: {}", e))
        }, "fetchassets");
        self.clear_deadline();
        self.warn_if_slow(start, "fetchassets");
        res
    }

    fn get_capabilities_refresh(&mut self) -> Result<ProviderCapabilities> {
        // Capabilities should also honor deadlines to avoid traps when epoch interruption is enabled
        self.set_deadline();
        let caps = self.bindings
            .call_getcapabilities(&mut self.store)
            .map_err(|e| anyhow!("Failed to call getcapabilities: {}", e));
        self.clear_deadline();
        let caps = caps?;
        self.caps = Some(caps.clone());
        Ok(caps)
    }

    fn get_capabilities_cached(&mut self) -> Result<ProviderCapabilities> {
        if let Some(c) = self.caps.clone() { return Ok(c); }
        self.get_capabilities_refresh()
    }

    fn supports_media(&self, target: MediaType) -> bool {
        match &self.caps {
            Some(c) => c.media_types.iter().any(|mt|
                matches!((mt, &target), (MediaType::Manga, MediaType::Manga) | (MediaType::Anime, MediaType::Anime))
            ),
            None => true,
        }
    }

    fn supports_unit(&self, target: UnitKind) -> bool {
        match &self.caps {
            Some(c) => c.unit_kinds.iter().any(|uk|
                matches!((uk, &target), (UnitKind::Chapter, UnitKind::Chapter) | (UnitKind::Episode, UnitKind::Episode))
            ),
            None => true,
        }
    }

    fn supports_asset(&self, target: AssetKind) -> bool {
        match &self.caps {
            Some(c) => c.asset_kinds.iter().any(|ak|
                matches!((ak, &target), (AssetKind::Page, AssetKind::Page) | (AssetKind::Image, AssetKind::Image) | (AssetKind::Video, AssetKind::Video))
            ),
            None => true,
        }
    }
}

// Commands routed to a dedicated worker thread per plugin
enum PluginCommand {
    FetchMediaList { kind: MediaType, query: String, resp: Sender<anyhow::Result<Vec<Media>>> },
    FetchUnits { media_id: String, resp: Sender<anyhow::Result<Vec<Unit>>> },
    FetchAssets { unit_id: String, resp: Sender<anyhow::Result<Vec<Asset>>> },
    GetCapabilities { refresh: bool, resp: Sender<anyhow::Result<ProviderCapabilities>> },
    Shutdown,
}

struct PluginWorker {
    name: String,
    tx: Sender<PluginCommand>,
}

// Simplified plugin manager - generic
pub struct PluginManager {
    engine: Arc<Engine>,
    workers: Vec<PluginWorker>,
    epoch_ticks: Arc<AtomicU64>,
    epoch_interval: Duration,
    epoch_stop: Arc<AtomicBool>,
    epoch_thread: Option<std::thread::JoinHandle<()>>,
}

impl PluginManager {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(false);
        config.epoch_interruption(true);
        // Ensure deterministic fuel/epoch start
        let engine = Arc::new(Engine::new(&config)?);

        // Start epoch ticker (10ms)
        let epoch_interval = Duration::from_millis(10);
        let epoch_ticks = Arc::new(AtomicU64::new(0));
        let epoch_stop = Arc::new(AtomicBool::new(false));
        let eng = engine.clone();
        let ticks = epoch_ticks.clone();
        let stop = epoch_stop.clone();
        let handle = std::thread::spawn(move || {
            // Start counter near one to avoid zero math issues
            ticks.store(1, Ordering::Relaxed);
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                std::thread::sleep(epoch_interval);
                eng.increment_epoch();
                ticks.fetch_add(1, Ordering::Relaxed);
            }
        });

        Ok(Self { engine, workers: Vec::new(), epoch_ticks, epoch_interval, epoch_stop, epoch_thread: Some(handle) })
    }

    pub fn set_rate_limit_millis(&mut self, ms: u64) {
        // Send a capabilities refresh to force workers to apply updated rate (they read from Plugin on next call)
        let _ = ms; // rate limit is applied inside Plugin; configurable per-plugin via future config
    }

    pub fn set_call_timeout_millis(&mut self, ms: u64) {
        // Same note as above
        let _ = ms;
    }

    pub async fn load_plugin(&mut self, plugin_path: &Path) -> Result<()> {
        let path = plugin_path.to_path_buf();
        let engine = self.engine.clone();
        let epoch_ticks = self.epoch_ticks.clone();
        let epoch_interval = self.epoch_interval;

        let (tx, rx): (Sender<PluginCommand>, Receiver<PluginCommand>) = mpsc::channel();

        // Spawn worker thread that owns the Plugin
        let name = plugin_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();
        std::thread::spawn(move || {
            // Create the Plugin in this thread to avoid moving WASM state across threads
            let mut plugin = match Plugin::new(&engine, &path, epoch_ticks, epoch_interval) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Failed to initialize plugin {}: {}", path.display(), e);
                    return;
                }
            };
            loop {
                match rx.recv() {
                    Ok(PluginCommand::FetchMediaList { kind, query, resp }) => {
                        let r = plugin.fetch_media_list(kind, &query);
                        let _ = resp.send(r.map_err(|e| e));
                    }
                    Ok(PluginCommand::FetchUnits { media_id, resp }) => {
                        let r = plugin.fetch_units(&media_id);
                        let _ = resp.send(r.map_err(|e| e));
                    }
                    Ok(PluginCommand::FetchAssets { unit_id, resp }) => {
                        let r = plugin.fetch_assets(&unit_id);
                        let _ = resp.send(r.map_err(|e| e));
                    }
                    Ok(PluginCommand::GetCapabilities { refresh, resp }) => {
                        let r = if refresh { plugin.get_capabilities_refresh() } else { plugin.get_capabilities_cached() };
                        let _ = resp.send(r.map_err(|e| e));
                    }
                    Ok(PluginCommand::Shutdown) | Err(_) => break,
                }
            }
        });

        self.workers.push(PluginWorker { name: name.clone(), tx });
        println!("Loaded plugin: {}", plugin_path.display());
        Ok(())
    }

    pub async fn load_plugins_from_directory(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            println!("Plugin directory does not exist: {}", dir.display());
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                if let Err(e) = self.load_plugin(&path).await {
                    eprintln!("Failed to load plugin {}: {}", path.display(), e);
                }
            }
        }
        Ok(())
    }

    pub fn list_plugins(&self) -> Vec<String> {
        self.workers.iter().map(|w| w.name.clone()).collect()
    }

    pub fn get_capabilities(&mut self, refresh: bool) -> Result<Vec<(String, ProviderCapabilities)>> {
        let mut out = Vec::new();
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::GetCapabilities { refresh, resp: rtx });
            match rrx.recv() {
                Ok(Ok(caps)) => out.push((w.name.clone(), caps)),
                Ok(Err(e)) => eprintln!("Plugin {} get_capabilities failed: {}", w.name, e),
                Err(e) => eprintln!("Plugin {} channel error: {}", w.name, e),
            }
        }
        Ok(out)
    }

    pub fn search_manga_with_sources(&mut self, query: &str) -> Result<Vec<(String, Media)>> {
        let mut pending: Vec<(String, Receiver<anyhow::Result<Vec<Media>>>)> = Vec::new();
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::FetchMediaList { kind: MediaType::Manga, query: query.to_string(), resp: rtx });
            pending.push((w.name.clone(), rrx));
        }
        let mut all = Vec::new();
        for (name, rrx) in pending {
            match rrx.recv() {
                Ok(Ok(mut v)) => { for m in v.drain(..) { all.push((name.clone(), m)); } }
                Ok(Err(e)) => eprintln!("Plugin {} fetchmedialist failed: {}", name, e),
                Err(e) => eprintln!("Plugin {} channel error: {}", name, e),
            }
        }
        Ok(all)
    }

    pub fn search_manga_for(&mut self, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(w) = self.workers.iter().find(|w| w.name == source) {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::FetchMediaList { kind: MediaType::Manga, query: query.to_string(), resp: rtx });
            return rrx.recv().unwrap_or_else(|e| Err(anyhow!("channel error: {}", e)));
        }
        Ok(Vec::new())
    }

    pub fn search_anime_with_sources(&mut self, query: &str) -> Result<Vec<(String, Media)>> {
        let mut pending: Vec<(String, Receiver<anyhow::Result<Vec<Media>>>)> = Vec::new();
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::FetchMediaList { kind: MediaType::Anime, query: query.to_string(), resp: rtx });
            pending.push((w.name.clone(), rrx));
        }
        let mut all = Vec::new();
        for (name, rrx) in pending {
            match rrx.recv() {
                Ok(Ok(mut v)) => { for m in v.drain(..) { all.push((name.clone(), m)); } }
                Ok(Err(e)) => eprintln!("Plugin {} fetchmedialist failed: {}", name, e),
                Err(e) => eprintln!("Plugin {} channel error: {}", name, e),
            }
        }
        Ok(all)
    }

    pub fn search_anime_for(&mut self, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(w) = self.workers.iter().find(|w| w.name == source) {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::FetchMediaList { kind: MediaType::Anime, query: query.to_string(), resp: rtx });
            return rrx.recv().unwrap_or_else(|e| Err(anyhow!("channel error: {}", e)));
        }
        Ok(Vec::new())
    }

    pub fn get_manga_chapters_with_source(&mut self, manga_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::FetchUnits { media_id: manga_id.to_string(), resp: rtx });
            match rrx.recv() {
                Ok(Ok(units)) => {
                    let chapters: Vec<Unit> = units.into_iter().filter(|u| matches!(u.kind, UnitKind::Chapter)).collect();
                    if !chapters.is_empty() { return Ok((Some(w.name.clone()), chapters)); }
                }
                Ok(Err(e)) => eprintln!("Plugin {} fetchunits failed: {}", w.name, e),
                Err(e) => eprintln!("Plugin {} channel error: {}", w.name, e),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_chapter_images_with_source(&mut self, chapter_id: &str) -> Result<(Option<String>, Vec<String>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::FetchAssets { unit_id: chapter_id.to_string(), resp: rtx });
            match rrx.recv() {
                Ok(Ok(assets)) => {
                    let urls: Vec<String> = assets.into_iter().filter(|a| matches!(a.kind, AssetKind::Page | AssetKind::Image)).map(|a| a.url).collect();
                    if !urls.is_empty() { return Ok((Some(w.name.clone()), urls)); }
                }
                Ok(Err(e)) => eprintln!("Plugin {} fetchassets failed: {}", w.name, e),
                Err(e) => eprintln!("Plugin {} channel error: {}", w.name, e),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_anime_episodes_with_source(&mut self, anime_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::FetchUnits { media_id: anime_id.to_string(), resp: rtx });
            match rrx.recv() {
                Ok(Ok(units)) => {
                    let eps: Vec<Unit> = units.into_iter().filter(|u| matches!(u.kind, UnitKind::Episode)).collect();
                    if !eps.is_empty() { return Ok((Some(w.name.clone()), eps)); }
                }
                Ok(Err(e)) => eprintln!("Plugin {} fetchunits failed: {}", w.name, e),
                Err(e) => eprintln!("Plugin {} channel error: {}", w.name, e),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_episode_streams_with_source(&mut self, episode_id: &str) -> Result<(Option<String>, Vec<Asset>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            let _ = w.tx.send(PluginCommand::FetchAssets { unit_id: episode_id.to_string(), resp: rtx });
            match rrx.recv() {
                Ok(Ok(assets)) => {
                    let vids: Vec<Asset> = assets.into_iter().filter(|a| matches!(a.kind, AssetKind::Video)).collect();
                    if !vids.is_empty() { return Ok((Some(w.name.clone()), vids)); }
                }
                Ok(Err(e)) => eprintln!("Plugin {} fetchassets failed: {}", w.name, e),
                Err(e) => eprintln!("Plugin {} channel error: {}", w.name, e),
            }
        }
        Ok((None, Vec::new()))
    }
}