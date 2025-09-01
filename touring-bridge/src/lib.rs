//! Flutter bridge for the touring library API.
//! Exposes a thin async wrapper around Touring suitable for flutter_rust_bridge.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use touring::prelude::*;

pub struct TouringBridge {
    inner: Arc<touring::Touring>,
}

impl TouringBridge {
    /// Create and connect the library. If database_url is None, use default.
    pub async fn new(database_url: Option<String>, run_migrations: bool) -> Result<Self> {
        let t = touring::Touring::connect(database_url.as_deref(), run_migrations).await?;
        Ok(Self { inner: Arc::new(t) })
    }

    /// Load plugins from a directory.
    pub async fn load_plugins_from_directory(&self, dir: String) -> Result<()> {
        self.inner.load_plugins_from_directory(PathBuf::from(dir).as_path()).await
    }

    pub fn list_plugins(&self) -> Vec<String> { self.inner.list_plugins() }

    pub async fn capabilities(&self, refresh: bool) -> Result<Vec<(String, ProviderCapabilities)>> {
        self.inner.get_capabilities(refresh).await
    }

    pub async fn search_manga(&self, query: String, refresh: bool) -> Result<Vec<(String, Media)>> {
        self.inner.search_manga_cached_with_sources(&query, refresh).await
    }

    pub async fn search_anime(&self, query: String, refresh: bool) -> Result<Vec<(String, Media)>> {
        self.inner.search_anime_cached_with_sources(&query, refresh).await
    }

    pub async fn get_manga_chapters(&self, manga_id: String) -> Result<Vec<Unit>> {
        self.inner.get_manga_chapters(&manga_id).await
    }

    pub async fn get_anime_episodes(&self, anime_id: String) -> Result<Vec<Unit>> {
        self.inner.get_anime_episodes(&anime_id).await
    }

    pub async fn get_chapter_images(&self, chapter_id: String, refresh: bool) -> Result<Vec<String>> {
        self.inner.get_chapter_images_with_refresh(&chapter_id, refresh).await
    }

    pub async fn get_episode_streams(&self, episode_id: String) -> Result<Vec<Asset>> {
        self.inner.get_episode_streams(&episode_id).await
    }

    pub async fn clear_cache_prefix(&self, prefix: Option<String>) -> Result<u64> {
        self.inner.clear_cache_prefix(prefix.as_deref()).await
    }

    pub async fn vacuum_db(&self) -> Result<()> { self.inner.vacuum_db().await }
}
