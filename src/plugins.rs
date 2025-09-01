use std::path::Path;
use anyhow::{anyhow, Result};
use wasmtime::{Engine, Config, Store, component::*};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

// Generate WIT bindings from shared plugin-interface
wasmtime::component::bindgen!({
    world: "source",
    path: "plugin-interface/wit/",
});

// Host context with WASI and HTTP support
struct Host {
    wasi: WasiCtx,
    table: wasmtime_wasi::ResourceTable,
    http: WasiHttpCtx,
}

impl WasiView for Host {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
        &mut self.table
    }
}

impl WasiHttpView for Host {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
        &mut self.table
    }
}

// Single plugin instance - combines WasmSource functionality directly
struct Plugin {
    name: String,
    store: Store<Host>,
    bindings: Source,
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
        // Add WASI support (provides CLI environment and other core interfaces)
        wasmtime_wasi::add_to_linker_sync(&mut linker)?;
        // Add HTTP support using async version to avoid runtime conflicts
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;

        // Use sync instantiation 
        let instance = linker.instantiate(&mut store, &component)?;
        let bindings = Source::new(&mut store, &instance)?;
        
        Ok(Self {
            name: plugin_path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string(),
            store,
            bindings,
            _instance: instance,
            _component: component,
        })
    }

    // Direct methods that return our domain types (no more type mapping layer)
    fn fetch_manga_list(&mut self, query: &str) -> Result<Vec<Media>> {
        self.bindings.call_fetchmangalist(&mut self.store, query)
            .map_err(|e| anyhow!("Failed to call fetchmangalist: {}", e))
    }

    fn fetch_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
        self.bindings.call_fetchchapterimages(&mut self.store, chapter_id)
            .map_err(|e| anyhow!("Failed to call fetchchapterimages: {}", e))
    }

    fn fetch_anime_list(&mut self, query: &str) -> Result<Vec<Media>> {
        self.bindings.call_fetchanimelist(&mut self.store, query)
            .map_err(|e| anyhow!("Failed to call fetchanimelist: {}", e))
    }

    fn fetch_anime_episodes(&mut self, anime_id: &str) -> Result<Vec<Episode>> {
        self.bindings.call_fetchanimeepisodes(&mut self.store, anime_id)
            .map_err(|e| anyhow!("Failed to call fetchanimeepisodes: {}", e))
    }

    fn fetch_episode_streams(&mut self, episode_id: &str) -> Result<Vec<Mediastream>> {
        self.bindings.call_fetchepisodestreams(&mut self.store, episode_id)
            .map_err(|e| anyhow!("Failed to call fetchepisodestreams: {}", e))
    }
}

// Simplified plugin manager - no more unnecessary layers
pub struct PluginManager {
    engine: Engine,
    plugins: Vec<Plugin>,
}

impl PluginManager {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        // Explicitly disable async support to force sync execution
        config.async_support(false);
        let engine = Engine::new(&config)?;
        Ok(Self {
            engine,
            plugins: Vec::new(),
        })
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
        self.plugins.iter()
            .map(|p| p.name.clone())
            .collect()
    }

    pub fn search_manga(&mut self, query: &str) -> Result<Vec<Media>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            match plugin.fetch_manga_list(query) {
                Ok(mut v) => all.append(&mut v),
                Err(e) => eprintln!("Plugin failed fetchmangalist: {}", e),
            }
        }
        Ok(all)
    }

    pub fn get_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
        for plugin in &mut self.plugins {
            match plugin.fetch_chapter_images(chapter_id) {
                Ok(imgs) if !imgs.is_empty() => return Ok(imgs),
                Ok(_) => continue,
                Err(e) => eprintln!("Plugin failed fetchchapterimages: {}", e),
            }
        }
        Ok(Vec::new())
    }

    // Anime methods
    pub fn search_anime(&mut self, query: &str) -> Result<Vec<Media>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            match plugin.fetch_anime_list(query) {
                Ok(mut v) => all.append(&mut v),
                Err(e) => eprintln!("Plugin failed fetchanimelist: {}", e),
            }
        }
        Ok(all)
    }

    pub fn get_anime_episodes(&mut self, anime_id: &str) -> Result<Vec<Episode>> {
        for plugin in &mut self.plugins {
            match plugin.fetch_anime_episodes(anime_id) {
                Ok(eps) if !eps.is_empty() => return Ok(eps),
                Ok(_) => continue,
                Err(e) => eprintln!("Plugin failed fetchanimeepisodes: {}", e),
            }
        }
        Ok(Vec::new())
    }

    pub fn get_episode_streams(&mut self, episode_id: &str) -> Result<Vec<Mediastream>> {
        for plugin in &mut self.plugins {
            match plugin.fetch_episode_streams(episode_id) {
                Ok(streams) if !streams.is_empty() => return Ok(streams),
                Ok(_) => continue,
                Err(e) => eprintln!("Plugin failed fetchepisodestreams: {}", e),
            }
        }
        Ok(Vec::new())
    }
}