use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task;
use tracing::{debug, error, warn};
use wasmtime::{Config, Engine};

// Generate WIT bindings from shared plugin-interface (generic library world)
wasmtime::component::bindgen!({
    world: "library",
    path: "wit/",
});

mod config;
mod host;
mod plugin;

use plugin::Plugin;

// Commands routed to a dedicated worker thread per plugin
enum PluginCmd {
    FetchMediaList {
        kind: MediaType,
        query: String,
        reply: oneshot::Sender<anyhow::Result<Vec<Media>>>,
    },
    FetchUnits {
        media_id: String,
        reply: oneshot::Sender<anyhow::Result<Vec<Unit>>>,
    },
    FetchAssets {
        unit_id: String,
        reply: oneshot::Sender<anyhow::Result<Vec<Asset>>>,
    },
    GetCapabilities {
        refresh: bool,
        reply: oneshot::Sender<anyhow::Result<ProviderCapabilities>>,
    },
    GetAllowedHosts {
        reply: oneshot::Sender<anyhow::Result<Vec<String>>>,
    },
}

#[derive(Clone)]
struct PluginWorker {
    tx: mpsc::Sender<PluginCmd>,
    call_timeout: Duration,
}

struct PluginArtifacts {
    primary: PathBuf,
    fallback: Option<PathBuf>,
}

struct PluginSlot {
    name: String,
    artifacts: PluginArtifacts,
    engine: Arc<Engine>,
    epoch_ticks: Arc<AtomicU64>,
    epoch_interval: Duration,
    state: Mutex<Option<PluginWorker>>,
}

#[derive(Default)]
struct ArtifactSet {
    wasm: Option<PathBuf>,
    cwasm: Option<PathBuf>,
}

impl ArtifactSet {
    fn into_artifacts(self, prefer_precompiled: bool) -> Option<PluginArtifacts> {
        match (self.cwasm, self.wasm, prefer_precompiled) {
            (Some(cwasm), Some(wasm), true) => Some(PluginArtifacts {
                primary: cwasm,
                fallback: Some(wasm),
            }),
            (Some(cwasm), Some(wasm), false) => Some(PluginArtifacts {
                primary: wasm,
                fallback: Some(cwasm),
            }),
            (Some(cwasm), None, _) => Some(PluginArtifacts {
                primary: cwasm,
                fallback: None,
            }),
            (None, Some(wasm), _) => Some(PluginArtifacts {
                primary: wasm,
                fallback: None,
            }),
            _ => None,
        }
    }
}

impl PluginSlot {
    fn new(
        name: String,
        artifacts: PluginArtifacts,
        engine: Arc<Engine>,
        epoch_ticks: Arc<AtomicU64>,
        epoch_interval: Duration,
    ) -> Self {
        Self {
            name,
            artifacts,
            engine,
            epoch_ticks,
            epoch_interval,
            state: Mutex::new(None),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn worker(&self) -> Result<PluginWorker> {
        let mut guard = self.state.lock().await;
        if let Some(worker) = guard.as_ref() {
            return Ok(worker.clone());
        }

        let primary_path = self.artifacts.primary.clone();
        match self.instantiate(&primary_path).await {
            Ok(worker) => {
                *guard = Some(worker.clone());
                return Ok(worker);
            }
            Err(mut err) => {
                warn!(plugin=%self.name, path=%primary_path.display(), error=?err, "failed to load plugin artifact");
                if let Some(fallback_path) = &self.artifacts.fallback {
                    warn!(plugin=%self.name, path=%fallback_path.display(), error=?err, "attempting fallback artifact");
                    match self.instantiate(fallback_path).await {
                        Ok(worker) => {
                            *guard = Some(worker.clone());
                            return Ok(worker);
                        }
                        Err(fallback_err) => {
                            error!(plugin=%self.name, path=%fallback_path.display(), error=?fallback_err, "fallback plugin load failed");
                            err = fallback_err;
                        }
                    }
                }
                Err(err)
            }
        }
    }

    async fn instantiate(&self, path: &Path) -> Result<PluginWorker> {
        let path_buf = path.to_path_buf();
        if !path_buf.exists() {
            return Err(anyhow!("plugin artifact missing: {}", path_buf.display()));
        }
        let cfg_path = path_buf.with_extension("toml");
        if !cfg_path.exists() {
            return Err(anyhow!("missing plugin config: {}", cfg_path.display()));
        }

        let slot_name = self.name.clone();
        let engine = self.engine.clone();
        let epoch_ticks = self.epoch_ticks.clone();
        let interval = self.epoch_interval;
        let path_to_load = path_buf.clone();

        let plugin = task::spawn_blocking(move || -> Result<Plugin> {
            let worker_threads = if cfg!(target_os = "ios") || cfg!(target_os = "android") {
                1
            } else {
                2
            };
            let rt_arc = std::sync::Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(worker_threads)
                    .build()?,
            );
            let fut = Plugin::new_async(
                &engine,
                &path_to_load,
                epoch_ticks,
                interval,
                rt_arc.clone(),
            );
            rt_arc.block_on(fut)
        })
        .await
        .map_err(|e| {
            anyhow!(
                "failed to join plugin loader thread for {}: {}",
                slot_name,
                e
            )
        })??;

        let call_timeout = plugin.call_timeout;
        let (tx, mut rx) = mpsc::channel::<PluginCmd>(64);
        std::thread::spawn(move || {
            let mut plugin = plugin;
            while let Some(cmd) = rx.blocking_recv() {
                match cmd {
                    PluginCmd::FetchMediaList { kind, query, reply } => {
                        let _ = reply.send(plugin.fetch_media_list(kind, &query));
                    }
                    PluginCmd::FetchUnits { media_id, reply } => {
                        let _ = reply.send(plugin.fetch_units(&media_id));
                    }
                    PluginCmd::FetchAssets { unit_id, reply } => {
                        let _ = reply.send(plugin.fetch_assets(&unit_id));
                    }
                    PluginCmd::GetCapabilities { refresh, reply } => {
                        let res = if refresh {
                            plugin.get_capabilities_refresh()
                        } else {
                            plugin.get_capabilities_cached()
                        };
                        let _ = reply.send(res);
                    }
                    PluginCmd::GetAllowedHosts { reply } => {
                        let hosts = plugin.allowed_hosts.clone().unwrap_or_default();
                        let _ = reply.send(Ok(hosts));
                    }
                }
            }
        });
        println!("Loaded plugin: {}", path_buf.display());
        Ok(PluginWorker { tx, call_timeout })
    }
}

// Simplified plugin manager - generic
#[allow(dead_code)] // Some fields (_epoch_stop/_epoch_thread) reserved for future coordinated shutdown
pub struct PluginManager {
    engine: Arc<Engine>,
    slots: Vec<Arc<PluginSlot>>,
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

        #[cfg(not(target_os = "ios"))]
        {
            use wasmtime::OptLevel;
            config.strategy(wasmtime::Strategy::Cranelift);
            config.cranelift_opt_level(OptLevel::Speed);
        }
        #[cfg(target_os = "ios")]
        {
            use wasmtime::Collector;
            config.collector(Collector::DeferredReferenceCounting);
            config
                .target("pulley64")
                .map_err(|e| anyhow!("failed to set Pulley target: {}", e))?;
        }
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
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(epoch_interval);
                eng.increment_epoch();
                ticks.fetch_add(1, Ordering::Relaxed);
            }
        });

        Ok(Self {
            engine,
            slots: Vec::new(),
            epoch_ticks,
            epoch_interval,
            _epoch_stop: epoch_stop,
            _epoch_thread: Some(handle),
        })
    }

    pub async fn load_plugins_from_directory(&mut self, dir: &Path) -> Result<()> {
        self.slots.clear();
        if !dir.exists() {
            println!("Plugin directory does not exist: {}", dir.display());
            return Ok(());
        }
        let prefer_precompiled = !cfg!(target_os = "android");
        let mut artifacts_by_name: HashMap<String, ArtifactSet> = HashMap::new();

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let entry = artifacts_by_name.entry(stem.to_string()).or_default();
            match ext {
                "cwasm" => entry.cwasm = Some(path),
                "wasm" => entry.wasm = Some(path),
                _ => {}
            }
        }

        for (name, artifact_set) in artifacts_by_name {
            let Some(artifacts) = artifact_set.into_artifacts(prefer_precompiled) else {
                warn!(plugin=%name, "skipping plugin - no valid artifacts found");
                continue;
            };
            let cfg_path = artifacts.primary.with_extension("toml");
            if !cfg_path.exists() {
                warn!(plugin=%name, config=%cfg_path.display(), "rejecting plugin: missing .toml config");
                continue;
            }
            let slot = PluginSlot::new(
                name.clone(),
                artifacts,
                self.engine.clone(),
                self.epoch_ticks.clone(),
                self.epoch_interval,
            );
            debug!(plugin=%name, "registered plugin for lazy loading");
            self.slots.push(Arc::new(slot));
        }

        self.slots.sort_by(|a, b| a.name().cmp(b.name()));
        Ok(())
    }

    pub fn list_plugins(&self) -> Vec<String> {
        self.slots
            .iter()
            .map(|slot| slot.name().to_string())
            .collect()
    }

    pub async fn get_capabilities(
        &self,
        refresh: bool,
    ) -> Result<Vec<(String, ProviderCapabilities)>> {
        let mut out = Vec::new();
        for slot_arc in &self.slots {
            let slot = slot_arc.clone();
            let worker = match slot.worker().await {
                Ok(worker) => worker,
                Err(e) => {
                    warn!(plugin=%slot.name(), error=%e, "failed to initialize plugin");
                    continue;
                }
            };
            let name = slot.name().to_string();
            let call_timeout = worker.call_timeout;
            let tx = worker.tx.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = tx
                .send(PluginCmd::GetCapabilities {
                    refresh,
                    reply: reply_tx,
                })
                .await
            {
                warn!(plugin=%name, error=%e, "send error get_capabilities");
                continue;
            }
            match tokio::time::timeout(call_timeout, reply_rx).await {
                Ok(Ok(Ok(c))) => out.push((name.clone(), c)),
                Ok(Ok(Err(e))) => warn!(plugin=%name, error=%e, "get_capabilities failed"),
                Ok(Err(_canceled)) => warn!(plugin=%name, "get_capabilities sender dropped"),
                Err(_elapsed) => warn!(plugin=%name, "get_capabilities timeout"),
            }
        }
        Ok(out)
    }

    pub async fn get_allowed_hosts(&self) -> Result<Vec<(String, Vec<String>)>> {
        let mut out = Vec::new();
        for slot_arc in &self.slots {
            let slot = slot_arc.clone();
            let worker = match slot.worker().await {
                Ok(worker) => worker,
                Err(e) => {
                    warn!(plugin=%slot.name(), error=%e, "failed to initialize plugin");
                    continue;
                }
            };
            let name = slot.name().to_string();
            let call_timeout = worker.call_timeout;
            let tx = worker.tx.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = tx
                .send(PluginCmd::GetAllowedHosts { reply: reply_tx })
                .await
            {
                warn!(plugin=%name, error=%e, "send error get_allowed_hosts");
                continue;
            }
            match tokio::time::timeout(call_timeout, reply_rx).await {
                Ok(Ok(Ok(hosts))) => out.push((name.clone(), hosts)),
                Ok(Ok(Err(e))) => warn!(plugin=%name, error=%e, "get_allowed_hosts failed"),
                Ok(Err(_)) => warn!(plugin=%name, "get_allowed_hosts sender dropped"),
                Err(_) => warn!(plugin=%name, "get_allowed_hosts timeout"),
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
    async fn search_with_sources(
        &self,
        kind: MediaType,
        query: &str,
    ) -> Result<Vec<(String, Media)>> {
        let mut futures = Vec::new();
        for slot_arc in &self.slots {
            let slot = slot_arc.clone();
            let kind_clone = kind.clone();
            let query_string = query.to_string();
            futures.push(async move {
                let worker = match slot.worker().await {
                    Ok(worker) => worker,
                    Err(e) => {
                        warn!(plugin=%slot.name(), error=%e, kind=?kind_clone, "failed to initialize plugin");
                        return None;
                    }
                };
                let name = slot.name().to_string();
                let call_timeout = worker.call_timeout;
                let tx = worker.tx.clone();
                let (reply_tx, reply_rx) = oneshot::channel();
                if let Err(e) =
                    tx.send(PluginCmd::FetchMediaList {
                        kind: kind_clone.clone(),
                        query: query_string.clone(),
                        reply: reply_tx,
                    })
                    .await
                {
                    warn!(plugin=%name, error=%e, kind=?kind_clone, "send error search_with_sources");
                    return None;
                }
                match tokio::time::timeout(call_timeout, reply_rx).await {
                    Ok(Ok(Ok(list))) => Some((name, list)),
                    Ok(Ok(Err(e))) => {
                        warn!(plugin=%name, error=%e, "fetchmedialist failed");
                        None
                    }
                    Ok(Err(_)) => {
                        warn!(plugin=%name, "fetchmedialist sender dropped");
                        None
                    }
                    Err(_) => {
                        warn!(plugin=%name, "fetchmedialist timeout");
                        None
                    }
                }
            });
        }
        let mut all = Vec::new();
        for r in futures::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
        {
            let (name, list) = r;
            debug!(plugin=%name, kind=?kind, query, count=list.len(), "search results");
            for m in list {
                all.push((name.clone(), m));
            }
        }
        Ok(all)
    }

    async fn search_for(&self, kind: MediaType, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(slot) = self
            .slots
            .iter()
            .find(|slot| slot.name() == source)
            .cloned()
        {
            let worker = slot
                .worker()
                .await
                .map_err(|e| anyhow!("failed to initialize plugin {}: {}", source, e))?;
            let call_timeout = worker.call_timeout;
            let tx = worker.tx.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            tx.send(PluginCmd::FetchMediaList {
                kind: kind.clone(),
                query: query.to_string(),
                reply: reply_tx,
            })
            .await
            .map_err(|e| anyhow!("send error: {}", e))?;
            match tokio::time::timeout(call_timeout, reply_rx).await {
                Ok(Ok(Ok(v))) => {
                    debug!(plugin=%source, kind=?kind, query, count=v.len(), "search_for results");
                    Ok(v)
                }
                Ok(Ok(Err(e))) => Err(anyhow!("{}", e)),
                Ok(Err(_)) => Err(anyhow!("sender dropped")),
                Err(_) => Err(anyhow!("timeout after {:?}", call_timeout)),
            }
        } else {
            Ok(Vec::new())
        }
    }
    pub async fn get_manga_chapters_with_source(
        &self,
        manga_id: &str,
    ) -> Result<(Option<String>, Vec<Unit>)> {
        for slot_arc in &self.slots {
            let slot = slot_arc.clone();
            let worker = match slot.worker().await {
                Ok(worker) => worker,
                Err(e) => {
                    warn!(plugin=%slot.name(), error=%e, "failed to initialize plugin");
                    continue;
                }
            };
            let name = slot.name().to_string();
            let call_timeout = worker.call_timeout;
            let tx = worker.tx.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = tx
                .send(PluginCmd::FetchUnits {
                    media_id: manga_id.to_string(),
                    reply: reply_tx,
                })
                .await
            {
                warn!(plugin=%name, error=%e, "send error get_manga_chapters_with_source");
                continue;
            }
            match tokio::time::timeout(call_timeout, reply_rx).await {
                Ok(Ok(Ok(units))) => {
                    let chapters: Vec<Unit> = units
                        .into_iter()
                        .filter(|u| matches!(u.kind, UnitKind::Chapter))
                        .collect();
                    if !chapters.is_empty() {
                        return Ok((Some(name), chapters));
                    }
                }
                Ok(Ok(Err(e))) => warn!(plugin=%name, error=%e, "fetchunits failed"),
                Ok(Err(_)) => warn!(plugin=%name, "fetchunits sender dropped"),
                Err(_) => warn!(plugin=%name, "fetchunits timeout"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub async fn get_chapter_images_with_source(
        &self,
        chapter_id: &str,
    ) -> Result<(Option<String>, Vec<String>)> {
        for slot_arc in &self.slots {
            let slot = slot_arc.clone();
            let worker = match slot.worker().await {
                Ok(worker) => worker,
                Err(e) => {
                    warn!(plugin=%slot.name(), error=%e, "failed to initialize plugin");
                    continue;
                }
            };
            let name = slot.name().to_string();
            let call_timeout = worker.call_timeout;
            let tx = worker.tx.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = tx
                .send(PluginCmd::FetchAssets {
                    unit_id: chapter_id.to_string(),
                    reply: reply_tx,
                })
                .await
            {
                warn!(plugin=%name, error=%e, "send error get_chapter_images_with_source");
                continue;
            }
            match tokio::time::timeout(call_timeout, reply_rx).await {
                Ok(Ok(Ok(assets))) => {
                    let urls: Vec<String> = assets
                        .into_iter()
                        .filter(|a| matches!(a.kind, AssetKind::Page | AssetKind::Image))
                        .map(|a| a.url)
                        .collect();
                    if !urls.is_empty() {
                        return Ok((Some(name), urls));
                    }
                }
                Ok(Ok(Err(e))) => warn!(plugin=%name, error=%e, "fetchassets failed"),
                Ok(Err(_)) => warn!(plugin=%name, "fetchassets sender dropped"),
                Err(_) => warn!(plugin=%name, "fetchassets timeout"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub async fn get_anime_episodes_with_source(
        &self,
        anime_id: &str,
    ) -> Result<(Option<String>, Vec<Unit>)> {
        for slot_arc in &self.slots {
            let slot = slot_arc.clone();
            let worker = match slot.worker().await {
                Ok(worker) => worker,
                Err(e) => {
                    warn!(plugin=%slot.name(), error=%e, "failed to initialize plugin");
                    continue;
                }
            };
            let name = slot.name().to_string();
            let call_timeout = worker.call_timeout;
            let tx = worker.tx.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = tx
                .send(PluginCmd::FetchUnits {
                    media_id: anime_id.to_string(),
                    reply: reply_tx,
                })
                .await
            {
                warn!(plugin=%name, error=%e, "send error get_anime_episodes_with_source");
                continue;
            }
            match tokio::time::timeout(call_timeout, reply_rx).await {
                Ok(Ok(Ok(units))) => {
                    let eps: Vec<Unit> = units
                        .into_iter()
                        .filter(|u| matches!(u.kind, UnitKind::Episode))
                        .collect();
                    if !eps.is_empty() {
                        return Ok((Some(name), eps));
                    }
                }
                Ok(Ok(Err(e))) => warn!(plugin=%name, error=%e, "fetchunits failed"),
                Ok(Err(_)) => warn!(plugin=%name, "fetchunits sender dropped"),
                Err(_) => warn!(plugin=%name, "fetchunits timeout"),
            }
        }
        Ok((None, Vec::new()))
    }

    pub async fn get_episode_streams_with_source(
        &self,
        episode_id: &str,
    ) -> Result<(Option<String>, Vec<Asset>)> {
        for slot_arc in &self.slots {
            let slot = slot_arc.clone();
            let worker = match slot.worker().await {
                Ok(worker) => worker,
                Err(e) => {
                    warn!(plugin=%slot.name(), error=%e, "failed to initialize plugin");
                    continue;
                }
            };
            let name = slot.name().to_string();
            let call_timeout = worker.call_timeout;
            let tx = worker.tx.clone();
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(e) = tx
                .send(PluginCmd::FetchAssets {
                    unit_id: episode_id.to_string(),
                    reply: reply_tx,
                })
                .await
            {
                warn!(plugin=%name, error=%e, "send error get_episode_streams_with_source");
                continue;
            }
            match tokio::time::timeout(call_timeout, reply_rx).await {
                Ok(Ok(Ok(assets))) => {
                    let vids: Vec<Asset> = assets
                        .into_iter()
                        .filter(|a| matches!(a.kind, AssetKind::Video))
                        .collect();
                    if !vids.is_empty() {
                        return Ok((Some(name), vids));
                    }
                }
                Ok(Ok(Err(e))) => warn!(plugin=%name, error=%e, "fetchassets failed"),
                Ok(Err(_)) => warn!(plugin=%name, "fetchassets sender dropped"),
                Err(_) => warn!(plugin=%name, "fetchassets timeout"),
            }
        }
        Ok((None, Vec::new()))
    }
}

// Graceful shutdown of epoch ticker thread
impl Drop for PluginManager {
    fn drop(&mut self) {
        self._epoch_stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self._epoch_thread.take() {
            let _ = handle.join();
        }
    }
}
