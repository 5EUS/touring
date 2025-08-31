pub mod wasmsource {
    use anyhow::{Context, Result};
    use wasmtime::{component::*, *};
    use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

    // Generate bindings for the WIT world
    wasmtime::component::bindgen!({
        world: "source",
        // Use the wit folder so package imports resolve
        path: "wit/",
    });

    // Minimal host context with WASI + resource table for component model
    struct Host {
        wasi: WasiCtx,
        table: ResourceTable,
    }

    impl WasiView for Host {
        fn ctx(&mut self) -> &mut WasiCtx {
            &mut self.wasi
        }
        fn table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }
    }

    // Type aliases for easier use
    pub type WasmManga = Media;
    pub type WasmEpisode = Episode;
    pub type WasmMediaStream = Mediastream;

    pub struct WasmSource {
        store: Store<Host>,
        bindings: Source,
        _instance: wasmtime::component::Instance,
        _component: Component,
    }

    impl WasmSource {
        pub fn new(engine: &Engine, wasm_path: &str) -> Result<Self> {
            // Load component (not a core module)
            let component = Component::from_file(engine, wasm_path)
                .with_context(|| format!("failed to load component: {}", wasm_path))?;

            // Build WASI + host
            let wasi = WasiCtxBuilder::new()
                .inherit_stdio()
                .inherit_args() // unwrap Result before build()
                .build();
            let table = ResourceTable::new();
            let host = Host { wasi, table };

            // Store and linker
            let mut store = Store::new(engine, host);
            let mut linker = wasmtime::component::Linker::<Host>::new(engine);
            wasmtime_wasi::add_to_linker_sync(&mut linker)?;

            // Instantiate via generated bindings (returns (bindings, instance))
            let instance = linker.instantiate(&mut store, &component)
                .context("failed to instantiate component")?;
            let bindings = Source::new(&mut store, &instance)
                .context("failed to create component bindings")?;

            Ok(Self {
                store,
                bindings,
                _instance: instance,
                _component: component,
            })
        }

        // Manga methods
        pub fn call_fetch_manga_list(&mut self, query: &str) -> Result<Vec<WasmManga>> {
            self.bindings
                .call_fetchmangalist(&mut self.store, query)
                .context("failed to call fetchmangalist")
        }

        pub fn call_fetch_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
            self.bindings
                .call_fetchchapterimages(&mut self.store, chapter_id)
                .context("failed to call fetchchapterimages")
        }

        // Anime methods
        pub fn call_fetch_anime_list(&mut self, query: &str) -> Result<Vec<WasmManga>> {
            self.bindings
                .call_fetchanimelist(&mut self.store, query)
                .context("failed to call fetchanimelist")
        }

        pub fn call_fetch_anime_episodes(&mut self, anime_id: &str) -> Result<Vec<WasmEpisode>> {
            self.bindings
                .call_fetchanimeepisodes(&mut self.store, anime_id)
                .context("failed to call fetchanimeepisodes")
        }

        pub fn call_fetch_episode_streams(&mut self, episode_id: &str) -> Result<Vec<WasmMediaStream>> {
            self.bindings
                .call_fetchepisodestreams(&mut self.store, episode_id)
                .context("failed to call fetchepisodestreams")
        }

        // Legacy method for backwards compatibility
        pub fn call_fetch_list(&mut self, query: &str) -> Result<Vec<WasmManga>> {
            self.call_fetch_manga_list(query)
        }
    }
}
