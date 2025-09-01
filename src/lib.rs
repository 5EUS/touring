pub mod aggregator;
pub mod db;
pub mod plugins;
pub mod storage;
pub mod dao;
pub mod mapping;
pub mod types;

// --- Library API for embedding ---

/// Convenience re-exports for embedders.
pub mod prelude {
    pub use crate::plugins::{Media, Unit, Asset, MediaType, UnitKind, AssetKind, ProviderCapabilities};
}

use anyhow::Result;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::db::Database;
use crate::plugins::{PluginManager, Media, Unit, MediaType, Asset, ProviderCapabilities};
use crate::storage::Storage;
use crate::mapping::{series_insert_from_media, series_source_from, chapter_insert_from_unit};
use crate::types::{MediaCache, SearchEntry, media_to_cache, media_from_cache};

/// Async library entry point. Owns the database and a plugin manager.
pub struct Touring {
    db: Database,
    pm: Arc<Mutex<PluginManager>>, // plugin calls are blocking; guard with a Mutex and use spawn_blocking
    // Caching TTLs (seconds)
    search_ttl_secs: i64,
    pages_ttl_secs: i64,
}

impl Touring {
    /// Initialize database and (optionally) run migrations. Does not start any internal runtimes.
    pub async fn connect(database_url: Option<&str>, run_migrations: bool) -> Result<Self> {
        let db = Database::connect(database_url).await?;
        if run_migrations { db.run_migrations().await?; }
        let pm = PluginManager::new()?;
        // TTLs via env with defaults
        let search_ttl_secs = std::env::var("TOURING_SEARCH_TTL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60 * 60);
        let pages_ttl_secs = std::env::var("TOURING_PAGES_TTL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(24 * 60 * 60);
        Ok(Self { db, pm: Arc::new(Mutex::new(pm)), search_ttl_secs, pages_ttl_secs })
    }

    /// Load all plugins from a directory.
    pub async fn load_plugins_from_directory(&self, dir: &Path) -> Result<()> {
        let mut pm = self.pm.lock().unwrap();
        pm.load_plugins_from_directory(dir).await
    }

    /// List loaded plugin names.
    pub fn list_plugins(&self) -> Vec<String> { self.pm.lock().unwrap().list_plugins() }

    /// Get plugin capabilities (cached by default, or refresh).
    pub async fn get_capabilities(&self, refresh: bool) -> Result<Vec<(String, ProviderCapabilities)>> {
        let pm = self.pm.clone();
        tokio::task::spawn_blocking(move || pm.lock().unwrap().get_capabilities(refresh))
            .await
            .unwrap()
    }

    /// Search manga with per-source caching; upserts series + mappings. Returns (source, media).
    pub async fn search_manga_cached_with_sources(&self, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        let norm = norm_query(query);
        let now = current_epoch();
        let mut all: Vec<(String, Media)> = Vec::new();
        let sources = self.list_plugins();
        for source in sources {
            let key = format!("{}|search|manga|{}", source, norm);
            let mut hit: Option<Vec<Media>> = None;
            if !refresh {
                if let Some(payload) = self.db.get_cache(&key, now).await.ok().flatten() {
                    // New format: Vec<MediaCache>
                    if let Ok(items) = serde_json::from_str::<Vec<MediaCache>>(&payload) {
                        let medias: Vec<Media> = items.into_iter().map(media_from_cache).collect();
                        hit = Some(medias);
                    } else if let Ok(entries) = serde_json::from_str::<Vec<SearchEntry>>(&payload) {
                        // Back-compat
                        let medias: Vec<Media> = entries.into_iter().map(|e| media_from_cache(e.media)).collect();
                        hit = Some(medias);
                    }
                }
            }

            let list = if let Some(medias) = hit {
                // Upsert per cached media
                for m in &medias {
                    let _ = self.upsert_source(&source, "unknown").await;
                    let _ = self.get_or_create_series_id(&source, &m.id, m).await;
                }
                medias
            } else {
                // Miss -> query the specific plugin
                let pm = self.pm.clone();
                let src = source.clone();
                let q = query.to_string();
                let results = tokio::task::spawn_blocking(move || pm.lock().unwrap().search_manga_for(&src, &q)).await.unwrap()?;
                for m in &results {
                    let _ = self.upsert_source(&source, "unknown").await;
                    let _ = self.get_or_create_series_id(&source, &m.id, m).await;
                }
                let payload = serde_json::to_string(&results.iter().map(media_to_cache).collect::<Vec<_>>())?;
                let _ = self.db.put_cache(&key, &payload, now + self.search_ttl_secs).await;
                results
            };

            for m in list { all.push((source.clone(), m)); }
        }
        Ok(all)
    }

    /// Search anime with per-source caching; upserts series + mappings. Returns (source, media).
    pub async fn search_anime_cached_with_sources(&self, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        let norm = norm_query(query);
        let now = current_epoch();
        let mut all: Vec<(String, Media)> = Vec::new();
        let sources = self.list_plugins();
        for source in sources {
            let key = format!("{}|search|anime|{}", source, norm);
            let mut hit: Option<Vec<Media>> = None;
            if !refresh {
                if let Some(payload) = self.db.get_cache(&key, now).await.ok().flatten() {
                    if let Ok(items) = serde_json::from_str::<Vec<MediaCache>>(&payload) {
                        let mut medias: Vec<Media> = items.into_iter().map(media_from_cache).collect();
                        for m in &mut medias { m.mediatype = MediaType::Anime; }
                        hit = Some(medias);
                    } else if let Ok(entries) = serde_json::from_str::<Vec<SearchEntry>>(&payload) {
                        let mut medias: Vec<Media> = entries.into_iter().map(|e| media_from_cache(e.media)).collect();
                        for m in &mut medias { m.mediatype = MediaType::Anime; }
                        hit = Some(medias);
                    }
                }
            }

            let list = if let Some(medias) = hit {
                for m in &medias {
                    let _ = self.upsert_source(&source, "unknown").await;
                    let _ = self.get_or_create_series_id(&source, &m.id, m).await;
                }
                medias
            } else {
                let pm = self.pm.clone();
                let src = source.clone();
                let q = query.to_string();
                let mut results = tokio::task::spawn_blocking(move || pm.lock().unwrap().search_anime_for(&src, &q)).await.unwrap()?;
                for m in &mut results { m.mediatype = MediaType::Anime; }
                for m in &results {
                    let _ = self.upsert_source(&source, "unknown").await;
                    let _ = self.get_or_create_series_id(&source, &m.id, m).await;
                }
                let payload = serde_json::to_string(&results.iter().map(media_to_cache).collect::<Vec<_>>())?;
                let _ = self.db.put_cache(&key, &payload, now + self.search_ttl_secs).await;
                results
            };

            for m in list { all.push((source.clone(), m)); }
        }
        Ok(all)
    }

    /// Fetch chapters for a manga id; upserts chapters linked to canonical series id.
    pub async fn get_manga_chapters(&self, external_manga_id: &str) -> Result<Vec<Unit>> {
        let pm = self.pm.clone();
        let eid = external_manga_id.to_string();
        let (source_opt, units) = tokio::task::spawn_blocking(move || pm.lock().unwrap().get_manga_chapters_with_source(&eid)).await.unwrap()?;
        if let Some(source_id) = source_opt {
            let media_stub = Media { id: external_manga_id.to_string(), mediatype: MediaType::Manga, title: String::new(), description: None, url: None, cover_url: None };
            let series_id = self.get_or_create_series_id(&source_id, external_manga_id, &media_stub).await?;
            let pool = self.db.pool().clone();
            for u in units.iter().filter(|u| matches!(u.kind, crate::plugins::UnitKind::Chapter)) {
                if let Some(existing) = dao::find_chapter_id_by_mapping(&pool, &series_id, &source_id, &u.id).await? {
                    let ch = chapter_insert_from_unit(&existing, &series_id, &source_id, u);
                    let _ = crate::dao::upsert_chapter(&pool, &ch).await;
                } else {
                    let cid = uuid::Uuid::new_v4().to_string();
                    let ch = chapter_insert_from_unit(&cid, &series_id, &source_id, u);
                    let _ = crate::dao::upsert_chapter(&pool, &ch).await;
                }
            }
        }
        Ok(units)
    }

    /// Fetch episode list for an anime id; upserts and returns episodes.
    pub async fn get_anime_episodes(&self, external_anime_id: &str) -> Result<Vec<Unit>> {
        let pm = self.pm.clone();
        let eid = external_anime_id.to_string();
        let (source_opt, units) = tokio::task::spawn_blocking(move || pm.lock().unwrap().get_anime_episodes_with_source(&eid)).await.unwrap()?;
        if let Some(source_id) = source_opt {
            let media_stub = Media { id: external_anime_id.to_string(), mediatype: MediaType::Anime, title: String::new(), description: None, url: None, cover_url: None };
            let series_id = self.get_or_create_series_id(&source_id, external_anime_id, &media_stub).await?;
            let pool = self.db.pool().clone();
            for u in units.iter().filter(|u| matches!(u.kind, crate::plugins::UnitKind::Episode)) {
                if let Some(existing) = dao::find_episode_id_by_mapping(&pool, &series_id, &source_id, &u.id).await? {
                    let ep = crate::dao::EpisodeInsert {
                        id: existing,
                        series_id: series_id.clone(),
                        source_id: source_id.clone(),
                        external_id: u.id.clone(),
                        number_text: u.number_text.clone(),
                        number_num: u.number.map(|n| n as f64),
                        title: Some(u.title.clone()).filter(|s| !s.is_empty()),
                        lang: u.lang.clone(),
                        season: u.group.clone(),
                        published_at: u.published_at.clone(),
                    };
                    let _ = crate::dao::upsert_episode(&pool, &ep).await;
                } else {
                    let eid_new = uuid::Uuid::new_v4().to_string();
                    let ep = crate::dao::EpisodeInsert {
                        id: eid_new,
                        series_id: series_id.clone(),
                        source_id: source_id.clone(),
                        external_id: u.id.clone(),
                        number_text: u.number_text.clone(),
                        number_num: u.number.map(|n| n as f64),
                        title: Some(u.title.clone()).filter(|s| !s.is_empty()),
                        lang: u.lang.clone(),
                        season: u.group.clone(),
                        published_at: u.published_at.clone(),
                    };
                    let _ = crate::dao::upsert_episode(&pool, &ep).await;
                }
            }
        }
        Ok(units)
    }

    /// Fetch episode streams for an episode id; persists streams (dedupe by (episode_id, url)).
    pub async fn get_episode_streams(&self, external_episode_id: &str) -> Result<Vec<Asset>> {
        let pm = self.pm.clone();
        let eid = external_episode_id.to_string();
        let (src_opt, vids) = tokio::task::spawn_blocking(move || pm.lock().unwrap().get_episode_streams_with_source(&eid)).await.unwrap()?;
        if let Some(source_id) = src_opt {
            let pool = self.db.pool().clone();
            if let Some(canonical_eid) = crate::dao::find_episode_id_by_source_external(&pool, &source_id, external_episode_id).await? {
                let streams: Vec<crate::dao::StreamInsert> = vids.iter().map(|a| crate::dao::StreamInsert {
                    episode_id: canonical_eid.clone(),
                    url: a.url.clone(),
                    quality: None,
                    mime: a.mime.clone(),
                }).collect();
                let _ = crate::dao::upsert_streams(&pool, &canonical_eid, &streams).await;
            }
        }
        Ok(vids)
    }

    /// Fetch chapter images (URLs) with caching and optional refresh.
    pub async fn get_chapter_images_with_refresh(&self, chapter_id: &str, refresh: bool) -> Result<Vec<String>> {
        let key = format!("all|pages|{}", chapter_id);
        let now = current_epoch();

        if !refresh {
            if let Some(payload) = self.db.get_cache(&key, now).await.ok().flatten() {
                if let Ok(urls) = serde_json::from_str::<Vec<String>>(&payload) {
                    return Ok(urls);
                }
            }
        }

        // Miss -> call plugins for this chapter id
        let pm = self.pm.clone();
        let cid = chapter_id.to_string();
        let (_src_opt, urls) = tokio::task::spawn_blocking(move || pm.lock().unwrap().get_chapter_images_with_source(&cid)).await.unwrap()?;

        // Write-through with TTL
        let payload = serde_json::to_string(&urls)?;
        let expires_at = now + self.pages_ttl_secs;
        let _ = self.db.put_cache(&key, &payload, expires_at).await;

        Ok(urls)
    }

    /// Convenience wrapper: fetch chapter images using cache (no refresh).
    pub async fn get_chapter_images(&self, chapter_id: &str) -> Result<Vec<String>> {
        self.get_chapter_images_with_refresh(chapter_id, false).await
    }

    /// Clear cache entries by prefix. Returns number of rows removed.
    pub async fn clear_cache_prefix(&self, prefix: Option<&str>) -> Result<u64> {
        self.db.clear_cache_prefix(prefix).await.map_err(Into::into)
    }

    /// Vacuum/compact the database (SQLite only; no-op on others).
    pub async fn vacuum_db(&self) -> Result<()> { self.db.vacuum().await.map_err(Into::into) }

    // --- Series management APIs ---

    pub async fn list_series(&self, kind: Option<&str>) -> Result<Vec<(String, String)>> {
        let pool = self.db.pool().clone();
        crate::dao::list_series(&pool, kind).await
    }

    pub async fn list_chapters_for_series(&self, series_id: &str) -> Result<Vec<(String, Option<f64>, Option<String>)>> {
        let pool = self.db.pool().clone();
        crate::dao::list_chapters_for_series(&pool, series_id).await
    }

    pub async fn list_episodes_for_series(&self, series_id: &str) -> Result<Vec<(String, Option<f64>, Option<String>)>> {
        let pool = self.db.pool().clone();
        crate::dao::list_episodes_for_series(&pool, series_id).await
    }

    pub async fn get_series_download_path(&self, series_id: &str) -> Result<Option<String>> {
        let pool = self.db.pool().clone();
        Ok(crate::dao::get_series_pref(&pool, series_id).await?.and_then(|p| p.download_path))
    }

    pub async fn set_series_download_path(&self, series_id: &str, path: Option<&str>) -> Result<()> {
        let pool = self.db.pool().clone();
        crate::dao::set_series_download_path(&pool, series_id, path).await
    }

    pub async fn delete_series(&self, series_id: &str) -> Result<u64> {
        let pool = self.db.pool().clone();
        crate::dao::delete_series(&pool, series_id).await
    }

    pub async fn delete_chapter(&self, chapter_id: &str) -> Result<u64> {
        let pool = self.db.pool().clone();
        crate::dao::delete_chapter(&pool, chapter_id).await
    }

    pub async fn delete_episode(&self, episode_id: &str) -> Result<u64> {
        let pool = self.db.pool().clone();
        crate::dao::delete_episode(&pool, episode_id).await
    }

    // --- helpers ---

    async fn upsert_source(&self, id: &str, version: &str) -> Result<()> {
        let pool = self.db.pool().clone();
        crate::dao::upsert_source(&pool, &crate::dao::SourceInsert { id: id.to_string(), version: version.to_string() }).await.map_err(Into::into)
    }

    async fn get_or_create_series_id(&self, source_id: &str, external_id: &str, media: &Media) -> Result<String> {
        let pool = self.db.pool().clone();
        if let Some(existing) = crate::dao::find_series_id_by_source_external(&pool, source_id, external_id).await? {
            return Ok(existing);
        }
        let new_id = uuid::Uuid::new_v4().to_string();
        let s = series_insert_from_media(&new_id, media);
        crate::dao::upsert_series(&pool, &s).await?;
        let link = series_source_from(&new_id, source_id, external_id);
        crate::dao::upsert_series_source(&pool, &link).await?;
        Ok(new_id)
    }
}

// --- types for cache serialization (duplicated from aggregator) ---

fn norm_query(q: &str) -> String {
    let trimmed = q.trim().to_ascii_lowercase();
    let mut out = String::with_capacity(trimmed.len());
    let mut last_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() { if !last_space { out.push(' '); last_space = true; } } else { out.push(ch); last_space = false; }
    }
    out
}
fn current_epoch() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}
