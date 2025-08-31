use std::path::Path;
use anyhow::{Context, Result};
use wasmtime::{Engine, Config};
use crate::wasmsource::wasmsource::{WasmManga, WasmSource, WasmEpisode, WasmMediaStream};
use crate::source;

// Simple WASM plugin loader with component support
pub struct PluginManager {
    engine: Engine,
    plugins: Vec<PluginHost>,
}

struct PluginHost {
    // Wrap a single instantiated component
    plugin: WasmSource,
}

impl PluginHost {
    fn new(engine: &Engine, plugin_path: &Path) -> Result<Self> {
        let plugin = WasmSource::new(engine, plugin_path.to_str().unwrap())
            .with_context(|| format!("Failed to init plugin: {}", plugin_path.display()))?;
        Ok(Self { plugin })
    }

    // Manga methods
    fn call_fetch_manga_list(&mut self, query: &str) -> Result<Vec<source::Media>> {
        let results: Vec<WasmManga> = self
            .plugin
            .call_fetch_manga_list(query)
            .context("Failed to call fetchmangalist")?;

        let mapped = results
            .into_iter()
            .map(|m| source::Media {
                id: m.id,
                mediatype: match m.mediatype {
                    crate::wasmsource::wasmsource::MediaType::Anime => source::MediaType::Anime,
                    crate::wasmsource::wasmsource::MediaType::Manga => source::MediaType::Manga,
                    crate::wasmsource::wasmsource::MediaType::Other(s) => source::MediaType::Other(s),
                },
                title: m.title,
                description: m.description,
                url: m.url,
            })
            .collect();
        Ok(mapped)
    }

    fn call_fetch_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
        self.plugin
            .call_fetch_chapter_images(chapter_id)
            .context("Failed to call fetchchapterimages")
    }

    // Anime methods
    fn call_fetch_anime_list(&mut self, query: &str) -> Result<Vec<source::Media>> {
        let results: Vec<WasmManga> = self
            .plugin
            .call_fetch_anime_list(query)
            .context("Failed to call fetchanimelist")?;

        let mapped = results
            .into_iter()
            .map(|m| source::Media {
                id: m.id,
                mediatype: match m.mediatype {
                    crate::wasmsource::wasmsource::MediaType::Anime => source::MediaType::Anime,
                    crate::wasmsource::wasmsource::MediaType::Manga => source::MediaType::Manga,
                    crate::wasmsource::wasmsource::MediaType::Other(s) => source::MediaType::Other(s),
                },
                title: m.title,
                description: m.description,
                url: m.url,
            })
            .collect();
        Ok(mapped)
    }

    fn call_fetch_anime_episodes(&mut self, anime_id: &str) -> Result<Vec<source::Episode>> {
        let results: Vec<WasmEpisode> = self
            .plugin
            .call_fetch_anime_episodes(anime_id)
            .context("Failed to call fetchanimeepisodes")?;

        let mapped = results
            .into_iter()
            .map(|e| source::Episode {
                id: e.id,
                title: e.title,
                number: e.number,
                url: e.url,
            })
            .collect();
        Ok(mapped)
    }

    fn call_fetch_episode_streams(&mut self, episode_id: &str) -> Result<Vec<source::Mediastream>> {
        let results: Vec<WasmMediaStream> = self
            .plugin
            .call_fetch_episode_streams(episode_id)
            .context("Failed to call fetchepisodestreams")?;

        let mapped = results
            .into_iter()
            .map(|s| source::Mediastream {
                url: s.url,
                quality: s.quality,
                mime: s.mime,
            })
            .collect();
        Ok(mapped)
    }
}

impl PluginManager {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config)?;
        Ok(Self {
            engine,
            plugins: Vec::new(),
        })
    }

    pub fn load_plugin(&mut self, plugin_path: &Path) -> Result<()> {
        let plugin = PluginHost::new(&self.engine, plugin_path)?;
        self.plugins.push(plugin);
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
                if let Err(e) = self.load_plugin(&path) {
                    eprintln!("Failed to load plugin {}: {}", path.display(), e);
                }
            }
        }
        Ok(())
    }

    // Manga methods
    pub fn search_manga(&mut self, query: &str) -> Result<Vec<source::Media>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            match plugin.call_fetch_manga_list(query) {
                Ok(mut v) => all.append(&mut v),
                Err(e) => eprintln!("Plugin failed fetchmangalist: {}", e),
            }
        }
        Ok(all)
    }

    pub fn get_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
        for plugin in &mut self.plugins {
            match plugin.call_fetch_chapter_images(chapter_id) {
                Ok(imgs) if !imgs.is_empty() => return Ok(imgs),
                Ok(_) => continue,
                Err(e) => eprintln!("Plugin failed fetchchapterimages: {}", e),
            }
        }
        Ok(Vec::new())
    }

    // Anime methods
    pub fn search_anime(&mut self, query: &str) -> Result<Vec<source::Media>> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            match plugin.call_fetch_anime_list(query) {
                Ok(mut v) => all.append(&mut v),
                Err(e) => eprintln!("Plugin failed fetchanimelist: {}", e),
            }
        }
        Ok(all)
    }

    pub fn get_anime_episodes(&mut self, anime_id: &str) -> Result<Vec<source::Episode>> {
        for plugin in &mut self.plugins {
            match plugin.call_fetch_anime_episodes(anime_id) {
                Ok(eps) if !eps.is_empty() => return Ok(eps),
                Ok(_) => continue,
                Err(e) => eprintln!("Plugin failed fetchanimeepisodes: {}", e),
            }
        }
        Ok(Vec::new())
    }

    pub fn get_episode_streams(&mut self, episode_id: &str) -> Result<Vec<source::Mediastream>> {
        for plugin in &mut self.plugins {
            match plugin.call_fetch_episode_streams(episode_id) {
                Ok(streams) if !streams.is_empty() => return Ok(streams),
                Ok(_) => continue,
                Err(e) => eprintln!("Plugin failed fetchepisodestreams: {}", e),
            }
        }
        Ok(Vec::new())
    }
}