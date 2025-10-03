use std::path::Path;
use anyhow::{anyhow, Result};
use wasmtime::{Engine, Config};
use std::time::Duration;
use std::sync::{Arc, atomic::{AtomicU64, AtomicBool, Ordering}};
use tracing::{debug, warn, error};
use tokio::sync::{mpsc, oneshot};

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
enum PluginCmd {
    FetchMediaList { kind: MediaType, query: String, reply: oneshot::Sender<anyhow::Result<Vec<Media>>> },
    FetchUnits { media_id: String, reply: oneshot::Sender<anyhow::Result<Vec<Unit>>> },
    FetchAssets { unit_id: String, reply: oneshot::Sender<anyhow::Result<Vec<Asset>>> },
    GetCapabilities { refresh: bool, reply: oneshot::Sender<anyhow::Result<ProviderCapabilities>> },
    GetAllowedHosts { reply: oneshot::Sender<anyhow::Result<Vec<String>>> },
}

struct PluginWorker {
    name: String,
    tx: mpsc::Sender<PluginCmd>,
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

    pub async fn load_plugin(&mut self, plugin_path: &Path) -> Result<()> {
        let path = plugin_path.to_path_buf();
        debug!(plugin_path=%path.display(), "instantiating plugin");
        let engine = self.engine.clone();
        let epoch_ticks = self.epoch_ticks.clone();
        let interval = self.epoch_interval;
        // Instantiate plugin (async)
        // Use a small multi-thread runtime per plugin only for its internal async host calls
        // On mobile, use 1 worker thread to reduce overhead; desktop uses 2
        let worker_threads = if cfg!(target_os = "ios") || cfg!(target_os = "android") {
            1
        } else {
            2
        };
        let rt_arc = std::sync::Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(worker_threads)
                .build()?
        );
        // Instantiate plugin using current async context
        let plugin = Plugin::new_async(&engine, &path, epoch_ticks, interval, rt_arc.clone()).await?;
        let name = plugin.name.clone();
        let call_timeout = plugin.call_timeout;
        // Channel capacity small; backpressure signals overload
        let (tx, mut rx) = mpsc::channel::<PluginCmd>(64);
        // Move plugin into an OS thread hosting its single-thread runtime local executor
        std::thread::spawn(move || {
            // Reuse the same runtime for invoking async Wasm calls inside plugin methods
            let mut plugin = plugin;
            // Process messages sequentially
            while let Some(cmd) = rx.blocking_recv() { // blocking_recv since we are already on a dedicated thread
                match cmd {
                    PluginCmd::FetchMediaList { kind, query, reply } => { let _ = reply.send(plugin.fetch_media_list(kind, &query)); }
                    PluginCmd::FetchUnits { media_id, reply } => { let _ = reply.send(plugin.fetch_units(&media_id)); }
                    PluginCmd::FetchAssets { unit_id, reply } => { let _ = reply.send(plugin.fetch_assets(&unit_id)); }
                    PluginCmd::GetCapabilities { refresh, reply } => { let res = if refresh { plugin.get_capabilities_refresh() } else { plugin.get_capabilities_cached() }; let _ = reply.send(res); }
                    PluginCmd::GetAllowedHosts { reply } => { let hosts = plugin.allowed_hosts.clone().unwrap_or_default(); let _ = reply.send(Ok(hosts)); }
                }
            }
        });
        self.workers.push(PluginWorker { name: name.clone(), tx, call_timeout });
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
                // Check if corresponding .toml file exists
                let toml_path = path.with_extension("toml");
                if !toml_path.exists() {
                    warn!(plugin_path=%path.display(), "rejecting plugin: missing .toml config");
                    continue;
                }
                
                if let Err(e) = self.load_plugin(&path).await {
                    error!(plugin_path=%path.display(), error=%e, "failed to load plugin");
                }
            }
        }
        Ok(())
    }

    pub fn list_plugins(&self) -> Vec<String> {
        self.workers.iter().map(|w| w.name.clone()).collect()
    }

    pub async fn get_capabilities(&self, refresh: bool) -> Result<Vec<(String, ProviderCapabilities)>> {
        let mut out = Vec::new();
        for w in &self.workers {
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = w.tx.send(PluginCmd::GetCapabilities { refresh, reply: reply_tx }).await { warn!(plugin=%w.name, error=%e, "send error get_capabilities"); continue; }
            match tokio::time::timeout(w.call_timeout, reply_rx).await {
                Ok(Ok(Ok(c))) => out.push((w.name.clone(), c)),
                Ok(Ok(Err(e))) => warn!(plugin=%w.name, error=%e, "get_capabilities failed"),
                Ok(Err(_canceled)) => warn!(plugin=%w.name, "get_capabilities sender dropped"),
                Err(_elapsed) => warn!(plugin=%w.name, "get_capabilities timeout"),
            }
        }
        Ok(out)
    }

    pub async fn get_allowed_hosts(&self) -> Result<Vec<(String, Vec<String>)>> {
        let mut out = Vec::new();
        for w in &self.workers {
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = w.tx.send(PluginCmd::GetAllowedHosts { reply: reply_tx }).await { warn!(plugin=%w.name, error=%e, "send error get_allowed_hosts"); continue; }
            match tokio::time::timeout(w.call_timeout, reply_rx).await {
                Ok(Ok(Ok(hosts))) => out.push((w.name.clone(), hosts)),
                Ok(Ok(Err(e))) => warn!(plugin=%w.name, error=%e, "get_allowed_hosts failed"),
                Ok(Err(_)) => warn!(plugin=%w.name, "get_allowed_hosts sender dropped"),
                Err(_) => warn!(plugin=%w.name, "get_allowed_hosts timeout"),
            }
        }
        Ok(out)
    }

    pub async fn search_manga_with_sources(&self, query: &str) -> Result<Vec<(String, Media)>> {
        self.search_with_sources(MediaType::Manga, query).await
    }

    pub async fn search_manga_for(&self, source: &str, query: &str) -> Result<Vec<Media>> {
        self.search_for(MediaType::Manga, source, query).await
    }

    pub async fn search_anime_with_sources(&self, query: &str) -> Result<Vec<(String, Media)>> {
        self.search_with_sources(MediaType::Anime, query).await
    }

    pub async fn search_anime_for(&self, source: &str, query: &str) -> Result<Vec<Media>> {
        self.search_for(MediaType::Anime, source, query).await
    }

    // Generic internal helpers ------------------------------------------------------
    async fn search_with_sources(&self, kind: MediaType, query: &str) -> Result<Vec<(String, Media)>> {
        let mut futures = Vec::new();
        for w in &self.workers {
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = w.tx.send(PluginCmd::FetchMediaList { kind: kind.clone(), query: query.to_string(), reply: reply_tx }).await {
                warn!(plugin=%w.name, error=%e, kind=?kind, "send error search_with_sources");
                continue;
            }
            let name = w.name.clone();
            let timeout = w.call_timeout;
            futures.push(async move {
                match tokio::time::timeout(timeout, reply_rx).await {
                    Ok(Ok(Ok(list))) => Some((name, list)),
                    Ok(Ok(Err(e))) => { warn!(plugin=%name, error=%e, "fetchmedialist failed"); None },
                    Ok(Err(_)) => { warn!(plugin=%name, "fetchmedialist sender dropped"); None },
                    Err(_) => { warn!(plugin=%name, "fetchmedialist timeout"); None },
                }
            });
        }
        let mut all = Vec::new();
        for r in futures::future::join_all(futures).await.into_iter().flatten() {
            let (name, list) = r;
            debug!(plugin=%name, kind=?kind, query, count=list.len(), "search results");
            for m in list { all.push((name.clone(), m)); }
        }
        Ok(all)
    }

    async fn search_for(&self, kind: MediaType, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(w) = self.workers.iter().find(|w| w.name == source) {
            let (reply_tx, reply_rx) = oneshot::channel();
            w.tx.send(PluginCmd::FetchMediaList { kind: kind.clone(), query: query.to_string(), reply: reply_tx }).await.map_err(|e| anyhow!("send error: {}", e))?;
            match tokio::time::timeout(w.call_timeout, reply_rx).await {
                Ok(Ok(Ok(v))) => { debug!(plugin=%source, kind=?kind, query, count=v.len(), "search_for results"); Ok(v) }
                Ok(Ok(Err(e))) => Err(anyhow!("{}", e)),
                Ok(Err(_)) => Err(anyhow!("sender dropped")),
                Err(_) => Err(anyhow!("timeout after {:?}", w.call_timeout)),
            }
        } else { Ok(Vec::new()) }
    }
    pub async fn get_manga_chapters_with_source(&self, manga_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for w in &self.workers {
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = w.tx.send(PluginCmd::FetchUnits { media_id: manga_id.to_string(), reply: reply_tx }).await { warn!(plugin=%w.name, error=%e, "send error get_manga_chapters_with_source"); continue; }
            match tokio::time::timeout(w.call_timeout, reply_rx).await {
                Ok(Ok(Ok(units))) => {
                    let chapters: Vec<Unit> = units.into_iter().filter(|u| matches!(u.kind, UnitKind::Chapter)).collect();
                    if !chapters.is_empty() { return Ok((Some(w.name.clone()), chapters)); }
                }
                Ok(Ok(Err(e))) => warn!(plugin=%w.name, error=%e, "fetchunits failed"),
                Ok(Err(_)) => warn!(plugin=%w.name, "fetchunits sender dropped"),
                Err(_) => warn!(plugin=%w.name, "fetchunits timeout"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub async fn get_chapter_images_with_source(&self, chapter_id: &str) -> Result<(Option<String>, Vec<String>)> {
        for w in &self.workers {
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = w.tx.send(PluginCmd::FetchAssets { unit_id: chapter_id.to_string(), reply: reply_tx }).await { warn!(plugin=%w.name, error=%e, "send error get_chapter_images_with_source"); continue; }
            match tokio::time::timeout(w.call_timeout, reply_rx).await {
                Ok(Ok(Ok(assets))) => {
                    let urls: Vec<String> = assets.into_iter().filter(|a| matches!(a.kind, AssetKind::Page | AssetKind::Image)).map(|a| a.url).collect();
                    if !urls.is_empty() { return Ok((Some(w.name.clone()), urls)); }
                }
                Ok(Ok(Err(e))) => warn!(plugin=%w.name, error=%e, "fetchassets failed"),
                Ok(Err(_)) => warn!(plugin=%w.name, "fetchassets sender dropped"),
                Err(_) => warn!(plugin=%w.name, "fetchassets timeout"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub async fn get_anime_episodes_with_source(&self, anime_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for w in &self.workers {
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = w.tx.send(PluginCmd::FetchUnits { media_id: anime_id.to_string(), reply: reply_tx }).await { warn!(plugin=%w.name, error=%e, "send error get_anime_episodes_with_source"); continue; }
            match tokio::time::timeout(w.call_timeout, reply_rx).await {
                Ok(Ok(Ok(units))) => {
                    let eps: Vec<Unit> = units.into_iter().filter(|u| matches!(u.kind, UnitKind::Episode)).collect();
                    if !eps.is_empty() { return Ok((Some(w.name.clone()), eps)); }
                }
                Ok(Ok(Err(e))) => warn!(plugin=%w.name, error=%e, "fetchunits failed"),
                Ok(Err(_)) => warn!(plugin=%w.name, "fetchunits sender dropped"),
                Err(_) => warn!(plugin=%w.name, "fetchunits timeout"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub async fn get_episode_streams_with_source(&self, episode_id: &str) -> Result<(Option<String>, Vec<Asset>)> {
        for w in &self.workers {
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = w.tx.send(PluginCmd::FetchAssets { unit_id: episode_id.to_string(), reply: reply_tx }).await { warn!(plugin=%w.name, error=%e, "send error get_episode_streams_with_source"); continue; }
            match tokio::time::timeout(w.call_timeout, reply_rx).await {
                Ok(Ok(Ok(assets))) => {
                    let vids: Vec<Asset> = assets.into_iter().filter(|a| matches!(a.kind, AssetKind::Video)).collect();
                    if !vids.is_empty() { return Ok((Some(w.name.clone()), vids)); }
                }
                Ok(Ok(Err(e))) => warn!(plugin=%w.name, error=%e, "fetchassets failed"),
                Ok(Err(_)) => warn!(plugin=%w.name, "fetchassets sender dropped"),
                Err(_) => warn!(plugin=%w.name, "fetchassets timeout"),
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