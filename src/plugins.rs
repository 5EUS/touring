use std::path::Path;
use anyhow::{anyhow, Result};
use wasmtime::{Engine, Config};
use std::time::Duration;
use std::sync::{Arc, atomic::{AtomicU64, AtomicBool, Ordering}};
use std::sync::mpsc::{self, Sender, Receiver};

// Generate WIT bindings from shared plugin-interface (generic library world)
wasmtime::component::bindgen!({
    world: "library",
    path: "plugin-interface/wit/",
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

        Ok(Self { engine, workers: Vec::new(), epoch_ticks, epoch_interval, epoch_stop, epoch_thread: Some(handle) })
    }

    pub async fn load_plugin(&mut self, plugin_path: &Path) -> Result<()> {
        let path = plugin_path.to_path_buf();
        let engine = self.engine.clone();
        let epoch_ticks = self.epoch_ticks.clone();
        let epoch_interval = self.epoch_interval;

        let (tx, rx): (Sender<PluginCommand>, Receiver<PluginCommand>) = mpsc::channel();
        let name = plugin_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();

        std::thread::spawn(move || {
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
        let timeout = Duration::from_secs(10);
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::GetCapabilities { refresh, resp: rtx }) {
                eprintln!("Plugin {} send error (get_capabilities): {}", w.name, e);
                continue;
            }
            match rrx.recv_timeout(timeout) {
                Ok(Ok(caps)) => out.push((w.name.clone(), caps)),
                Ok(Err(e)) => eprintln!("Plugin {} get_capabilities failed: {}", w.name, e),
                Err(mpsc::RecvTimeoutError::Timeout) => eprintln!("Plugin {} get_capabilities timed out after {:?}", w.name, timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => eprintln!("Plugin {} channel disconnected (get_capabilities)", w.name),
            }
        }
        Ok(out)
    }

    pub fn search_manga_with_sources(&mut self, query: &str) -> Result<Vec<(String, Media)>> {
        let mut pending: Vec<(String, Receiver<anyhow::Result<Vec<Media>>>)> = Vec::new();
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchMediaList { kind: MediaType::Manga, query: query.to_string(), resp: rtx }) {
                eprintln!("Plugin {} send error (search_manga_with_sources): {}", w.name, e);
                continue;
            }
            pending.push((w.name.clone(), rrx));
        }
        let mut all = Vec::new();
        let timeout = Duration::from_secs(20);
        for (name, rrx) in pending {
            match rrx.recv_timeout(timeout) {
                Ok(Ok(mut v)) => { for m in v.drain(..) { all.push((name.clone(), m)); } }
                Ok(Err(e)) => eprintln!("Plugin {} fetchmedialist failed: {}", name, e),
                Err(mpsc::RecvTimeoutError::Timeout) => eprintln!("Plugin {} fetchmedialist timed out after {:?}", name, timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => eprintln!("Plugin {} channel disconnected (fetchmedialist)", name),
            }
        }
        Ok(all)
    }

    pub fn search_manga_for(&mut self, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(w) = self.workers.iter().find(|w| w.name == source) {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchMediaList { kind: MediaType::Manga, query: query.to_string(), resp: rtx }) {
                return Err(anyhow!("send error: {}", e));
            }
            let timeout = Duration::from_secs(20);
            return match rrx.recv_timeout(timeout) {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(anyhow!("{}", e)),
                Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow!("timeout after {:?}", timeout)),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow!("channel disconnected")),
            };
        }
        Ok(Vec::new())
    }

    pub fn search_anime_with_sources(&mut self, query: &str) -> Result<Vec<(String, Media)>> {
        let mut pending: Vec<(String, Receiver<anyhow::Result<Vec<Media>>>)> = Vec::new();
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchMediaList { kind: MediaType::Anime, query: query.to_string(), resp: rtx }) {
                eprintln!("Plugin {} send error (search_anime_with_sources): {}", w.name, e);
                continue;
            }
            pending.push((w.name.clone(), rrx));
        }
        let mut all = Vec::new();
        let timeout = Duration::from_secs(20);
        for (name, rrx) in pending {
            match rrx.recv_timeout(timeout) {
                Ok(Ok(mut v)) => { for m in v.drain(..) { all.push((name.clone(), m)); } }
                Ok(Err(e)) => eprintln!("Plugin {} fetchmedialist failed: {}", name, e),
                Err(mpsc::RecvTimeoutError::Timeout) => eprintln!("Plugin {} fetchmedialist timed out after {:?}", name, timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => eprintln!("Plugin {} channel disconnected (fetchmedialist)", name),
            }
        }
        Ok(all)
    }

    pub fn search_anime_for(&mut self, source: &str, query: &str) -> Result<Vec<Media>> {
        if let Some(w) = self.workers.iter().find(|w| w.name == source) {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchMediaList { kind: MediaType::Anime, query: query.to_string(), resp: rtx }) {
                return Err(anyhow!("send error: {}", e));
            }
            let timeout = Duration::from_secs(20);
            return match rrx.recv_timeout(timeout) {
                Ok(Ok(v)) => Ok(v),
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
                eprintln!("Plugin {} send error (get_manga_chapters_with_source): {}", w.name, e);
                continue;
            }
            let timeout = Duration::from_secs(20);
            match rrx.recv_timeout(timeout) {
                Ok(Ok(units)) => {
                    let chapters: Vec<Unit> = units.into_iter().filter(|u| matches!(u.kind, UnitKind::Chapter)).collect();
                    if !chapters.is_empty() { return Ok((Some(w.name.clone()), chapters)); }
                }
                Ok(Err(e)) => eprintln!("Plugin {} fetchunits failed: {}", w.name, e),
                Err(mpsc::RecvTimeoutError::Timeout) => eprintln!("Plugin {} fetchunits timed out after {:?}", w.name, timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => eprintln!("Plugin {} channel disconnected (fetchunits)", w.name),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_chapter_images_with_source(&mut self, chapter_id: &str) -> Result<(Option<String>, Vec<String>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchAssets { unit_id: chapter_id.to_string(), resp: rtx }) {
                eprintln!("Plugin {} send error (get_chapter_images_with_source): {}", w.name, e);
                continue;
            }
            let timeout = Duration::from_secs(20);
            match rrx.recv_timeout(timeout) {
                Ok(Ok(assets)) => {
                    let urls: Vec<String> = assets.into_iter().filter(|a| matches!(a.kind, AssetKind::Page | AssetKind::Image)).map(|a| a.url).collect();
                    if !urls.is_empty() { return Ok((Some(w.name.clone()), urls)); }
                }
                Ok(Err(e)) => eprintln!("Plugin {} fetchassets failed: {}", w.name, e),
                Err(mpsc::RecvTimeoutError::Timeout) => eprintln!("Plugin {} fetchassets timed out after {:?}", w.name, timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => eprintln!("Plugin {} channel disconnected (fetchassets)", w.name),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_anime_episodes_with_source(&mut self, anime_id: &str) -> Result<(Option<String>, Vec<Unit>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchUnits { media_id: anime_id.to_string(), resp: rtx }) {
                eprintln!("Plugin {} send error (get_anime_episodes_with_source): {}", w.name, e);
                continue;
            }
            let timeout = Duration::from_secs(20);
            match rrx.recv_timeout(timeout) {
                Ok(Ok(units)) => {
                    let eps: Vec<Unit> = units.into_iter().filter(|u| matches!(u.kind, UnitKind::Episode)).collect();
                    if !eps.is_empty() { return Ok((Some(w.name.clone()), eps)); }
                }
                Ok(Err(e)) => eprintln!("Plugin {} fetchunits failed: {}", w.name, e),
                Err(mpsc::RecvTimeoutError::Timeout) => eprintln!("Plugin {} fetchunits timed out after {:?}", w.name, timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => eprintln!("Plugin {} channel disconnected (fetchunits)", w.name),
            }
        }
        Ok((None, Vec::new()))
    }

    pub fn get_episode_streams_with_source(&mut self, episode_id: &str) -> Result<(Option<String>, Vec<Asset>)> {
        for w in &self.workers {
            let (rtx, rrx) = mpsc::channel();
            if let Err(e) = w.tx.send(PluginCommand::FetchAssets { unit_id: episode_id.to_string(), resp: rtx }) {
                eprintln!("Plugin {} send error (get_episode_streams_with_source): {}", w.name, e);
                continue;
            }
            let timeout = Duration::from_secs(20);
            match rrx.recv_timeout(timeout) {
                Ok(Ok(assets)) => {
                    let vids: Vec<Asset> = assets.into_iter().filter(|a| matches!(a.kind, AssetKind::Video)).collect();
                    if !vids.is_empty() { return Ok((Some(w.name.clone()), vids)); }
                }
                Ok(Err(e)) => eprintln!("Plugin {} fetchassets failed: {}", w.name, e),
                Err(mpsc::RecvTimeoutError::Timeout) => eprintln!("Plugin {} fetchassets timed out after {:?}", w.name, timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => eprintln!("Plugin {} channel disconnected (fetchassets)", w.name),
            }
        }
        Ok((None, Vec::new()))
    }
}