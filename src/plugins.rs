use std::path::Path;
use anyhow::{anyhow, Result};
use wasmtime::{Engine, Config, Store, component::*};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};
use std::time::{Duration, Instant};
use std::sync::{Arc, atomic::{AtomicU64, AtomicBool, Ordering}};

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

impl Plugin {
    pub async fn new(engine: &Engine, plugin_path: &Path, epoch_ticks: Arc<AtomicU64>, epoch_interval: Duration) -> Result<Self> {
        let component = Component::from_file(engine, plugin_path)?;

        // Initialize WASI context for the plugin
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdout().inherit_stderr().inherit_env();
        let wasi = builder.build();

        let host = Host { 
            wasi,
            table: wasmtime_wasi::ResourceTable::new(),
            http: WasiHttpCtx::new(),
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

        Ok(Self {
            name: plugin_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string(),
            store,
            bindings,
            caps,
            rate_limit: Duration::from_millis(150),
            slow_warn: Duration::from_secs(5),
            call_timeout: Duration::from_secs(15),
            last_call: None,
            epoch_ticks,
            epoch_interval,
            _instance: instance,
            _component: component,
        })
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

// Simplified plugin manager - generic
pub struct PluginManager {
    engine: Arc<Engine>,
    plugins: Vec<Plugin>,
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

        Ok(Self { engine, plugins: Vec::new(), epoch_ticks, epoch_interval, epoch_stop, epoch_thread: Some(handle) })
    }

    pub fn set_rate_limit_millis(&mut self, ms: u64) {
        for p in &mut self.plugins { p.rate_limit = Duration::from_millis(ms); }
    }

    pub fn set_call_timeout_millis(&mut self, ms: u64) {
        for p in &mut self.plugins { p.call_timeout = Duration::from_millis(ms); }
    }

    pub async fn load_plugin(&mut self, plugin_path: &Path) -> Result<()> {
        let plugin = Plugin::new(&self.engine, plugin_path, self.epoch_ticks.clone(), self.epoch_interval).await?;
        self.plugins.push(plugin);
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
        self.plugins.iter().map(|p| p.name.clone()).collect()
    }

    pub fn get_capabilities(&mut self, refresh: bool) -> Result<Vec<(String, ProviderCapabilities)>> {
        let mut out = Vec::new();
        for p in &mut self.plugins {
            let caps = if refresh { p.get_capabilities_refresh()? } else { p.get_capabilities_cached()? };
            out.push((p.name.clone(), caps));
        }
        Ok(out)
    }

    // Convenience wrappers for current CLI (manga-focused)
    pub fn search_manga(&mut self, query: &str) -> Result<Vec<Media>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            if !plugin.supports_media(MediaType::Manga) { continue; }
            match plugin.fetch_media_list(MediaType::Manga, query) {
                Ok(mut v) => all.append(&mut v),
                Err(e) => eprintln!("Plugin failed fetchmedialist: {}", e),
            }
        }
        Ok(all)
    }

    // With source id exposure
    pub fn search_manga_with_sources(&mut self, query: &str) -> Result<Vec<(String, Media)>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            if !plugin.supports_media(MediaType::Manga) { continue; }
            match plugin.fetch_media_list(MediaType::Manga, query) {
                Ok(v) => {
                    let source_id = plugin.name.clone();
                    for m in v { all.push((source_id.clone(), m)); }
                }
                Err(e) => eprintln!("Plugin failed fetchmedialist: {}", e),
            }
        }
        Ok(all)
    }

    pub fn get_manga_chapters(&mut self, manga_id: &str) -> Result<Vec<Unit>> {
        for plugin in &mut self.plugins {
            if !plugin.supports_unit(UnitKind::Chapter) { continue; }
            match plugin.fetch_units(manga_id) {
                Ok(units) => {
                    let chapters: Vec<Unit> = units
                        .into_iter()
                        .filter(|u| matches!(u.kind, UnitKind::Chapter))
                        .collect();
                    if !chapters.is_empty() { return Ok(chapters); }
                }
                Err(e) => eprintln!("Plugin failed fetchunits: {}", e),
            }
        }
        Ok(Vec::new())
    }

    pub fn get_manga_chapters_with_source(&mut self, manga_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for plugin in &mut self.plugins {
            if !plugin.supports_unit(UnitKind::Chapter) { continue; }
            match plugin.fetch_units(manga_id) {
                Ok(units) => {
                    let chapters: Vec<Unit> = units
                        .into_iter()
                        .filter(|u| matches!(u.kind, UnitKind::Chapter))
                        .collect();
                    if !chapters.is_empty() { return Ok((Some(plugin.name.clone()), chapters)); }
                }
                Err(e) => eprintln!("Plugin failed fetchunits: {}", e),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
        for plugin in &mut self.plugins {
            if !(plugin.supports_asset(AssetKind::Page) || plugin.supports_asset(AssetKind::Image)) { continue; }
            match plugin.fetch_assets(chapter_id) {
                Ok(assets) => {
                    let urls: Vec<String> = assets
                        .into_iter()
                        .filter(|a| matches!(a.kind, AssetKind::Page | AssetKind::Image))
                        .map(|a| a.url)
                        .collect();
                    if !urls.is_empty() { return Ok(urls); }
                }
                Err(e) => eprintln!("Plugin failed fetchassets: {}", e),
            }
        }
        Ok(Vec::new())
    }

    pub fn get_chapter_images_with_source(&mut self, chapter_id: &str) -> Result<(Option<String>, Vec<String>)> {
        for plugin in &mut self.plugins {
            if !(plugin.supports_asset(AssetKind::Page) || plugin.supports_asset(AssetKind::Image)) { continue; }
            match plugin.fetch_assets(chapter_id) {
                Ok(assets) => {
                    let urls: Vec<String> = assets
                        .into_iter()
                        .filter(|a| matches!(a.kind, AssetKind::Page | AssetKind::Image))
                        .map(|a| a.url)
                        .collect();
                    if !urls.is_empty() { return Ok((Some(plugin.name.clone()), urls)); }
                }
                Err(e) => eprintln!("Plugin failed fetchassets: {}", e),
            }
        }
        Ok((None, Vec::new()))
    }

    // Optional anime helpers using generic interface
    pub fn search_anime(&mut self, query: &str) -> Result<Vec<Media>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            if !plugin.supports_media(MediaType::Anime) { continue; }
            match plugin.fetch_media_list(MediaType::Anime, query) {
                Ok(mut v) => all.append(&mut v),
                Err(e) => eprintln!("Plugin failed fetchmedialist: {}", e),
            }
        }
        Ok(all)
    }

    pub fn search_anime_with_sources(&mut self, query: &str) -> Result<Vec<(String, Media)>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            if !plugin.supports_media(MediaType::Anime) { continue; }
            match plugin.fetch_media_list(MediaType::Anime, query) {
                Ok(v) => {
                    let source_id = plugin.name.clone();
                    for m in v { all.push((source_id.clone(), m)); }
                }
                Err(e) => eprintln!("Plugin failed fetchmedialist: {}", e),
            }
        }
        Ok(all)
    }

    pub fn get_anime_episodes(&mut self, anime_id: &str) -> Result<Vec<Unit>> {
        for plugin in &mut self.plugins {
            if !plugin.supports_unit(UnitKind::Episode) { continue; }
            match plugin.fetch_units(anime_id) {
                Ok(units) => {
                    let eps: Vec<Unit> = units
                        .into_iter()
                        .filter(|u| matches!(u.kind, UnitKind::Episode))
                        .collect();
                    if !eps.is_empty() { return Ok(eps); }
                }
                Err(e) => eprintln!("Plugin failed fetchunits: {}", e),
            }
        }
        Ok(Vec::new())
    }

    pub fn get_anime_episodes_with_source(&mut self, anime_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for plugin in &mut self.plugins {
            if !plugin.supports_unit(UnitKind::Episode) { continue; }
            match plugin.fetch_units(anime_id) {
                Ok(units) => {
                    let eps: Vec<Unit> = units
                        .into_iter()
                        .filter(|u| matches!(u.kind, UnitKind::Episode))
                        .collect();
                    if !eps.is_empty() { return Ok((Some(plugin.name.clone()), eps)); }
                }
                Err(e) => eprintln!("Plugin failed fetchunits: {}", e),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_episode_streams(&mut self, episode_id: &str) -> Result<Vec<Asset>> {
        for plugin in &mut self.plugins {
            if !plugin.supports_asset(AssetKind::Video) { continue; }
            match plugin.fetch_assets(episode_id) {
                Ok(assets) => {
                    let vids: Vec<Asset> = assets
                        .into_iter()
                        .filter(|a| matches!(a.kind, AssetKind::Video))
                        .collect();
                    if !vids.is_empty() { return Ok(vids); }
                }
                Err(e) => eprintln!("Plugin failed fetchassets: {}", e),
            }
        }
        Ok(Vec::new())
    }

    pub fn get_episode_streams_with_source(&mut self, episode_id: &str) -> Result<(Option<String>, Vec<Asset>)> {
        for plugin in &mut self.plugins {
            if !plugin.supports_asset(AssetKind::Video) { continue; }
            match plugin.fetch_assets(episode_id) {
                Ok(assets) => {
                    let vids: Vec<Asset> = assets
                        .into_iter()
                        .filter(|a| matches!(a.kind, AssetKind::Video))
                        .collect();
                    if !vids.is_empty() { return Ok((Some(plugin.name.clone()), vids)); }
                }
                Err(e) => eprintln!("Plugin failed fetchassets: {}", e),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn search_manga_for(&mut self, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(p) = self.plugins.iter_mut().find(|p| p.name == source) {
            if !p.supports_media(MediaType::Manga) { return Ok(Vec::new()); }
            return p.fetch_media_list(MediaType::Manga, query);
        }
        Ok(Vec::new())
    }

    pub fn search_anime_for(&mut self, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(p) = self.plugins.iter_mut().find(|p| p.name == source) {
            if !p.supports_media(MediaType::Anime) { return Ok(Vec::new()); }
            return p.fetch_media_list(MediaType::Anime, query);
        }
        Ok(Vec::new())
    }
}