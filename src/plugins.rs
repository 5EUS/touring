use std::path::Path;
use anyhow::{anyhow, Result};
use wasmtime::{Engine, Config, Store, component::*};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

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
    _instance: wasmtime::component::Instance,
    _component: Component,
}

impl Plugin {
    pub async fn new(engine: &Engine, plugin_path: &Path) -> Result<Self> {
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

        // Create a new linker for this plugin
        let mut linker = Linker::new(engine);
        wasmtime_wasi::add_to_linker_sync(&mut linker)?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

        let instance = linker.instantiate(&mut store, &component)?;
        let bindings = Library::new(&mut store, &instance)?;

        Ok(Self {
            name: plugin_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string(),
            store,
            bindings,
            _instance: instance,
            _component: component,
        })
    }

    // Generic methods
    fn fetch_media_list(&mut self, kind: MediaType, query: &str) -> Result<Vec<Media>> {
        self.bindings
            .call_fetchmedialist(&mut self.store, &kind, query)
            .map_err(|e| anyhow!("Failed to call fetchmedialist: {}", e))
    }

    fn fetch_units(&mut self, media_id: &str) -> Result<Vec<Unit>> {
        self.bindings
            .call_fetchunits(&mut self.store, media_id)
            .map_err(|e| anyhow!("Failed to call fetchunits: {}", e))
    }

    fn fetch_assets(&mut self, unit_id: &str) -> Result<Vec<Asset>> {
        self.bindings
            .call_fetchassets(&mut self.store, unit_id)
            .map_err(|e| anyhow!("Failed to call fetchassets: {}", e))
    }

    fn get_capabilities(&mut self) -> Result<ProviderCapabilities> {
        self.bindings
            .call_getcapabilities(&mut self.store)
            .map_err(|e| anyhow!("Failed to call getcapabilities: {}", e))
    }
}

// Simplified plugin manager - generic
pub struct PluginManager {
    engine: Engine,
    plugins: Vec<Plugin>,
}

impl PluginManager {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(false);
        let engine = Engine::new(&config)?;
        Ok(Self { engine, plugins: Vec::new() })
    }

    pub async fn load_plugin(&mut self, plugin_path: &Path) -> Result<()> {
        let plugin = Plugin::new(&self.engine, plugin_path).await?;
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

    // Convenience wrappers for current CLI (manga-focused)
    pub fn search_manga(&mut self, query: &str) -> Result<Vec<Media>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
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
            match plugin.fetch_media_list(MediaType::Anime, query) {
                Ok(mut v) => all.append(&mut v),
                Err(e) => eprintln!("Plugin failed fetchmedialist: {}", e),
            }
        }
        Ok(all)
    }

    pub fn get_anime_episodes(&mut self, anime_id: &str) -> Result<Vec<Unit>> {
        for plugin in &mut self.plugins {
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

    pub fn get_episode_streams(&mut self, episode_id: &str) -> Result<Vec<Asset>> {
        for plugin in &mut self.plugins {
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
}