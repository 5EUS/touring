use std::path::Path;
use anyhow::{anyhow, Result};
use wasmtime::{Engine, Config};
use std::time::Duration;
use std::sync::{Arc, atomic::{AtomicU64, AtomicBool, Ordering}};
use std::sync::mpsc::{self, Sender, Receiver};
use tracing::{debug, warn, error};

// Generate WIT bindings from shared plugin-interface (generic library world)
wasmtime::component::bindgen!({
    world: "library",
    path: "wit/",
});

mod host;
mod config;
mod plugin;

use plugin::Plugin;

// Commands routed to a dedicated worker thread per plugin
enum PluginCommand {
    FetchMediaList { kind: MediaType, query: String, resp: Sender<anyhow::Result<Vec<Media>>> },
    FetchUnits { media_id: String, resp: Sender<anyhow::Result<Vec<Unit>>> },
    FetchAssets { unit_id: String, resp: Sender<anyhow::Result<Vec<Asset>>> },
    GetCapabilities { refresh: bool, resp: Sender<anyhow::Result<ProviderCapabilities>> },
    GetAllowedHosts { resp: Sender<anyhow::Result<Vec<String>>> },
}

struct PluginWorker {
    name: String,
    tx: Sender<PluginCommand>,
    call_timeout: Duration,
}

// Simplified plugin manager - generic
#[allow(dead_code)] // Some fields (_epoch_stop/_epoch_thread) reserved for future coordinated shutdown
pub struct PluginManager {
    engine: Arc<Engine>,
    workers: Vec<PluginWorker>,
    epoch_ticks: Arc<AtomicU64>,
    epoch_interval: Duration,
    _epoch_stop: Arc<AtomicBool>,
    _epoch_thread: Option<std::thread::JoinHandle<()>>,
}

impl PluginManager {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
    config.wasm_component_model(true);
    // Enable async support for wasi-http operations
    config.async_support(true);
        config.epoch_interruption(true);
        
        config.strategy(wasmtime::Strategy::Cranelift);
        let engine = Arc::new(Engine::new(&config)?);

        // Start epoch ticker (10ms)
        let epoch_interval = Duration::from_millis(10);
        let epoch_ticks = Arc::new(AtomicU64::new(0));
        let epoch_stop = Arc::new(AtomicBool::new(false));
        let eng = engine.clone();
        let ticks = epoch_ticks.clone();
        let stop = epoch_stop.clone();
        let handle = std::thread::spawn(move || {
            ticks.store(1, Ordering::Relaxed);
            loop {
                if stop.load(Ordering::Relaxed) { break; }
                std::thread::sleep(epoch_interval);
                eng.increment_epoch();
                ticks.fetch_add(1, Ordering::Relaxed);
            }
        });

        Ok(Self { engine, workers: Vec::new(), epoch_ticks, epoch_interval, _epoch_stop: epoch_stop, _epoch_thread: Some(handle) })
    }

    pub fn load_plugin(&mut self, plugin_path: &Path) -> Result<()> {
    let path = plugin_path.to_path_buf();
    debug!(plugin_path=%path.display(), "instantiating plugin asynchronously");
        // Always perform async instantiation on a dedicated thread with its own runtime to avoid nested block_on panics.
        let (ptx, prx) = mpsc::channel();
        let engine = self.engine.clone();
        let epoch_ticks = self.epoch_ticks.clone();
        let interval = self.epoch_interval;
        // Shared runtime for plugin async calls kept alive for plugin lifetime
        let shared_rt = std::sync::Arc::new(tokio::runtime::Builder::new_multi_thread().enable_all().build()?);
        let shared_rt_clone = shared_rt.clone();
        std::thread::spawn(move || {
            let res = (|| -> Result<Plugin> {
                let plugin = shared_rt_clone.block_on(Plugin::new_async(&engine, &path, epoch_ticks, interval, shared_rt_clone.clone()))?;
                Ok(plugin)
            })();
            let _ = ptx.send(res);
        });
        let plugin = prx.recv().map_err(|e| anyhow!("plugin instantiate thread join error: {}", e))??;
    let name = plugin.name.clone();
    // Capture per-plugin timeout before moving plugin into thread
    let call_timeout = plugin.call_timeout;
        let (tx, rx): (Sender<PluginCommand>, Receiver<PluginCommand>) = mpsc::channel();

        std::thread::spawn(move || {
            let mut plugin = plugin; // move into thread
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
                    Ok(PluginCommand::GetAllowedHosts { resp }) => {
                        let hosts = plugin.allowed_hosts.clone().unwrap_or_default();
                        let _ = resp.send(Ok(hosts));
                    }
                    Err(_) => break,
                }
            }
        });

        self.workers.push(PluginWorker { name: name.clone(), tx, call_timeout });
        println!("Loaded plugin: {}", plugin_path.display());
        Ok(())
    }

    pub fn load_plugins_from_directory(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            println!("Plugin directory does not exist: {}", dir.display());
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                // Check if corresponding .toml file exists
                let toml_path = path.with_extension("toml");
                if !toml_path.exists() {
                    warn!(plugin_path=%path.display(), "rejecting plugin: missing .toml config");
                    continue;
                }
                
                if let Err(e) = self.load_plugin(&path) {
                    error!(plugin_path=%path.display(), error=%e, "failed to load plugin");
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
            let timeout = w.call_timeout;
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::GetCapabilities { refresh, resp: rtx }) {
                warn!(plugin=%w.name, error=%e, "send error get_capabilities");
                continue;
            }
            match rrx.recv_timeout(timeout) {
                Ok(Ok(caps)) => out.push((w.name.clone(), caps)),
                Ok(Err(e)) => warn!(plugin=%w.name, error=%e, "get_capabilities failed"),
                Err(mpsc::RecvTimeoutError::Timeout) => warn!(plugin=%w.name, ?timeout, "get_capabilities timeout"),
                Err(mpsc::RecvTimeoutError::Disconnected) => warn!(plugin=%w.name, "get_capabilities channel disconnected"),
            }
        }
        Ok(out)
    }

    pub fn get_allowed_hosts(&mut self) -> Result<Vec<(String, Vec<String>)>> {
        let mut out = Vec::new();
        for w in &self.workers {
            let timeout = w.call_timeout;
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::GetAllowedHosts { resp: rtx }) {
                warn!(plugin=%w.name, error=%e, "send error get_allowed_hosts");
                continue;
            }
            match rrx.recv_timeout(timeout) {
                Ok(Ok(hosts)) => out.push((w.name.clone(), hosts)),
                Ok(Err(e)) => warn!(plugin=%w.name, error=%e, "get_allowed_hosts failed"),
                Err(mpsc::RecvTimeoutError::Timeout) => warn!(plugin=%w.name, ?timeout, "get_allowed_hosts timeout"),
                Err(mpsc::RecvTimeoutError::Disconnected) => warn!(plugin=%w.name, "get_allowed_hosts channel disconnected"),
            }
        }
        Ok(out)
    }

    pub fn search_manga_with_sources(&mut self, query: &str) -> Result<Vec<(String, Media)>> {
        self.search_with_sources(MediaType::Manga, query)
    }

    pub fn search_manga_for(&mut self, source: &str, query: &str) -> Result<Vec<Media>> {
        self.search_for(MediaType::Manga, source, query)
    }

    pub fn search_anime_with_sources(&mut self, query: &str) -> Result<Vec<(String, Media)>> {
        self.search_with_sources(MediaType::Anime, query)
    }

    pub fn search_anime_for(&mut self, source: &str, query: &str) -> Result<Vec<Media>> {
        self.search_for(MediaType::Anime, source, query)
    }

    // Generic internal helpers ------------------------------------------------------
    fn search_with_sources(&mut self, kind: MediaType, query: &str) -> Result<Vec<(String, Media)>> {
        let mut pending: Vec<(String, Receiver<anyhow::Result<Vec<Media>>>, Duration)> = Vec::new();
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchMediaList { kind: kind.clone(), query: query.to_string(), resp: rtx }) {
                warn!(plugin=%w.name, error=%e, kind=?kind, "send error search_with_sources");
                continue;
            }
            debug!(plugin=%w.name, kind=?kind, query, "dispatched search");
            pending.push((w.name.clone(), rrx, w.call_timeout));
        }
        let mut all = Vec::new();
        for (name, rrx, timeout) in pending {
            match rrx.recv_timeout(timeout) {
                Ok(Ok(mut v)) => {
                    let count = v.len();
                    debug!(plugin=%name, kind=?kind, query, count, "search results");
                    for m in v.drain(..) { all.push((name.clone(), m)); }
                }
                Ok(Err(e)) => warn!(plugin=%name, kind=?kind, error=%e, "fetchmedialist failed"),
                Err(mpsc::RecvTimeoutError::Timeout) => warn!(plugin=%name, kind=?kind, ?timeout, "fetchmedialist timeout"),
                Err(mpsc::RecvTimeoutError::Disconnected) => warn!(plugin=%name, kind=?kind, "fetchmedialist channel disconnected"),
            }
        }
        Ok(all)
    }

    fn search_for(&mut self, kind: MediaType, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(w) = self.workers.iter().find(|w| w.name == source) {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchMediaList { kind: kind.clone(), query: query.to_string(), resp: rtx }) {
                return Err(anyhow!("send error: {}", e));
            }
            let timeout = w.call_timeout;
            return match rrx.recv_timeout(timeout) {
                Ok(Ok(v)) => {
                    debug!(plugin=%source, kind=?kind, query, count=v.len(), "search_for results");
                    Ok(v)
                },
                Ok(Err(e)) => Err(anyhow!("{}", e)),
                Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow!("timeout after {:?}", timeout)),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow!("channel disconnected")),
            };
        }
        Ok(Vec::new())
    }
    pub fn get_manga_chapters_with_source(&mut self, manga_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchUnits { media_id: manga_id.to_string(), resp: rtx }) {
                warn!(plugin=%w.name, error=%e, "send error get_manga_chapters_with_source");
                continue;
            }
            let timeout = w.call_timeout;
            match rrx.recv_timeout(timeout) {
                Ok(Ok(units)) => {
                    let chapters: Vec<Unit> = units.into_iter().filter(|u| matches!(u.kind, UnitKind::Chapter)).collect();
                    if !chapters.is_empty() { return Ok((Some(w.name.clone()), chapters)); }
                }
                Ok(Err(e)) => warn!(plugin=%w.name, error=%e, "fetchunits failed"),
                Err(mpsc::RecvTimeoutError::Timeout) => warn!(plugin=%w.name, ?timeout, "fetchunits timeout"),
                Err(mpsc::RecvTimeoutError::Disconnected) => warn!(plugin=%w.name, "fetchunits channel disconnected"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_chapter_images_with_source(&mut self, chapter_id: &str) -> Result<(Option<String>, Vec<String>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchAssets { unit_id: chapter_id.to_string(), resp: rtx }) {
                warn!(plugin=%w.name, error=%e, "send error get_chapter_images_with_source");
                continue;
            }
            let timeout = w.call_timeout;
            match rrx.recv_timeout(timeout) {
                Ok(Ok(assets)) => {
                    let urls: Vec<String> = assets.into_iter().filter(|a| matches!(a.kind, AssetKind::Page | AssetKind::Image)).map(|a| a.url).collect();
                    if !urls.is_empty() { return Ok((Some(w.name.clone()), urls)); }
                }
                Ok(Err(e)) => warn!(plugin=%w.name, error=%e, "fetchassets failed"),
                Err(mpsc::RecvTimeoutError::Timeout) => warn!(plugin=%w.name, ?timeout, "fetchassets timeout"),
                Err(mpsc::RecvTimeoutError::Disconnected) => warn!(plugin=%w.name, "fetchassets channel disconnected"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_anime_episodes_with_source(&mut self, anime_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchUnits { media_id: anime_id.to_string(), resp: rtx }) {
                warn!(plugin=%w.name, error=%e, "send error get_anime_episodes_with_source");
                continue;
            }
            let timeout = w.call_timeout;
            match rrx.recv_timeout(timeout) {
                Ok(Ok(units)) => {
                    let eps: Vec<Unit> = units.into_iter().filter(|u| matches!(u.kind, UnitKind::Episode)).collect();
                    if !eps.is_empty() { return Ok((Some(w.name.clone()), eps)); }
                }
                Ok(Err(e)) => warn!(plugin=%w.name, error=%e, "fetchunits failed"),
                Err(mpsc::RecvTimeoutError::Timeout) => warn!(plugin=%w.name, ?timeout, "fetchunits timeout"),
                Err(mpsc::RecvTimeoutError::Disconnected) => warn!(plugin=%w.name, "fetchunits channel disconnected"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_episode_streams_with_source(&mut self, episode_id: &str) -> Result<(Option<String>, Vec<Asset>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchAssets { unit_id: episode_id.to_string(), resp: rtx }) {
                warn!(plugin=%w.name, error=%e, "send error get_episode_streams_with_source");
                continue;
            }
            let timeout = w.call_timeout;
            match rrx.recv_timeout(timeout) {
                Ok(Ok(assets)) => {
                    let vids: Vec<Asset> = assets.into_iter().filter(|a| matches!(a.kind, AssetKind::Video)).collect();
                    if !vids.is_empty() { return Ok((Some(w.name.clone()), vids)); }
                }
                Ok(Err(e)) => warn!(plugin=%w.name, error=%e, "fetchassets failed"),
                Err(mpsc::RecvTimeoutError::Timeout) => warn!(plugin=%w.name, ?timeout, "fetchassets timeout"),
                Err(mpsc::RecvTimeoutError::Disconnected) => warn!(plugin=%w.name, "fetchassets channel disconnected"),
            }
        }
        Ok((None, Vec::new()))
    }
}

// Graceful shutdown of epoch ticker thread
impl Drop for PluginManager {
    fn drop(&mut self) {
        self._epoch_stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self._epoch_thread.take() { let _ = handle.join(); }
    }
}