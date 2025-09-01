use anyhow::Result;
use std::path::Path;

use crate::db::Database;
use crate::plugins::{PluginManager, Media, Unit, UnitKind, MediaType, Asset, AssetKind, ProviderCapabilities};
use crate::storage::Storage;
use crate::dao;
use crate::mapping::{series_insert_from_media, series_source_from, chapter_insert_from_unit};
use serde::{Serialize, Deserialize};
use crate::types::{MediaCache, SearchEntry, media_to_cache, media_from_cache};

/// Aggregator decouples media aggregation and persistence from the CLI/backend.
/// It owns the database and the plugin manager and provides a narrow API.
pub struct Aggregator {
    db: Database,
    pm: PluginManager,
    rt: tokio::runtime::Runtime,
    // Caching TTLs (seconds)
    search_ttl_secs: i64,
    pages_ttl_secs: i64,
}

impl Aggregator {
    /// Initialize database and (optionally) run migrations. Creates a blank PluginManager.
    /// Synchronous API; uses an internal Tokio runtime for DB I/O.
    pub fn new(database_url: Option<&str>, run_migrations: bool) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let db = rt.block_on(async {
            let db = Database::connect(database_url).await?;
            if run_migrations { db.run_migrations().await?; }
            Ok::<_, anyhow::Error>(db)
        })?;
        let pm = PluginManager::new()?;
        // TTLs via env with defaults
        let search_ttl_secs = std::env::var("TOURING_SEARCH_TTL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60 * 60);
        let pages_ttl_secs = std::env::var("TOURING_PAGES_TTL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(24 * 60 * 60);
        Ok(Self { db, pm, rt, search_ttl_secs, pages_ttl_secs })
    }

    /// Load all plugins from a directory (async). Caller may choose its own runtime.
    pub async fn load_plugins_from_directory(&mut self, dir: &Path) -> Result<()> {
        self.pm.load_plugins_from_directory(dir).await
    }

    /// List loaded plugins.
    pub fn list_plugins(&self) -> Vec<String> { self.pm.list_plugins() }

    /// Get or create a canonical series id by (source_id, external_id). Generates a UUID on create.
    fn get_or_create_series_id(&self, source_id: &str, external_id: &str, media: &Media) -> Result<String> {
        let pool = self.db.pool().clone();
        self.rt.block_on(async move {
            if let Some(existing) = dao::find_series_id_by_source_external(&pool, source_id, external_id).await? {
                return Ok(existing);
            }
            // Create new canonical series
            let new_id = uuid::Uuid::new_v4().to_string();
            let s = series_insert_from_media(&new_id, media);
            dao::upsert_series(&pool, &s).await?;
            let link = series_source_from(&new_id, source_id, external_id);
            dao::upsert_series_source(&pool, &link).await?;
            Ok(new_id)
        })
    }

    /// Search manga using all loaded plugins. Upserts series + mappings.
    pub fn search_manga(&mut self, query: &str) -> Result<Vec<Media>> {
        let results = self.pm.search_manga_with_sources(query)?;
        // ensure sources + series mappings exist
        for (source_id, media) in &results {
            let _ = self.upsert_source(source_id, "unknown");
            let _ = self.get_or_create_series_id(source_id, &media.id, media)?;
        }
        Ok(results.into_iter().map(|(_, m)| m).collect())
    }

    /// Search anime using all loaded plugins. Upserts series + mappings.
    pub fn search_anime(&mut self, query: &str) -> Result<Vec<Media>> {
        let results = self.pm.search_anime_with_sources(query)?;
        for (source_id, media) in &results {
            let _ = self.upsert_source(source_id, "unknown");
            let _ = self.get_or_create_series_id(source_id, &media.id, media)?;
        }
        Ok(results
            .into_iter()
            .map(|(src, mut m)| { m.mediatype = MediaType::Anime; (src, m).1 })
            .collect())
    }

    /// Search manga using per-source cache; on miss, query plugin and persist per source.
    pub fn search_manga_cached_with_sources(&mut self, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        let norm = Self::norm_query(query);
        let now = current_epoch();
        let mut all: Vec<(String, Media)> = Vec::new();

        for source in self.pm.list_plugins() {
            let key = format!("{}|search|manga|{}", source, norm);
            let mut hit: Option<Vec<Media>> = None;

            if !refresh {
                if let Some(payload) = self.rt.block_on(self.db.get_cache(&key, now)).ok().flatten() {
                    // Prefer new format: Vec<MediaCache>
                    if let Ok(items) = serde_json::from_str::<Vec<MediaCache>>(&payload) {
                        let medias: Vec<Media> = items.into_iter().map(media_from_cache).collect();
                        hit = Some(medias);
                    } else if let Ok(entries) = serde_json::from_str::<Vec<SearchEntry>>(&payload) {
                        // Back-compat from previous global cache format
                        let medias: Vec<Media> = entries.into_iter().map(|e| media_from_cache(e.media)).collect();
                        hit = Some(medias);
                    }
                }
            }

            let list = if let Some(mut medias) = hit {
                // Upsert for each media from cache
                for m in &medias {
                    let _ = self.upsert_source(&source, "unknown");
                    let _ = self.get_or_create_series_id(&source, &m.id, m);
                }
                medias
            } else {
                // Miss -> query this plugin only
                let results = self.pm.search_manga_for(&source, query)?;
                for m in &results {
                    let _ = self.upsert_source(&source, "unknown");
                    let _ = self.get_or_create_series_id(&source, &m.id, m);
                }
                // Write-through per source
                let payload = serde_json::to_string(&results.iter().map(media_to_cache).collect::<Vec<_>>())?;
                let _ = self.rt.block_on(self.db.put_cache(&key, &payload, now + self.search_ttl_secs));
                results
            };

            for m in list { all.push((source.clone(), m)); }
        }

        Ok(all)
    }

    /// Search anime using per-source cache; on miss, query plugin and persist per source.
    pub fn search_anime_cached_with_sources(&mut self, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        let norm = Self::norm_query(query);
        let now = current_epoch();
        let mut all: Vec<(String, Media)> = Vec::new();

        for source in self.pm.list_plugins() {
            let key = format!("{}|search|anime|{}", source, norm);
            let mut hit: Option<Vec<Media>> = None;

            if !refresh {
                if let Some(payload) = self.rt.block_on(self.db.get_cache(&key, now)).ok().flatten() {
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

            let list = if let Some(mut medias) = hit {
                for m in &medias {
                    let _ = self.upsert_source(&source, "unknown");
                    let _ = self.get_or_create_series_id(&source, &m.id, m);
                }
                medias
            } else {
                let mut results = self.pm.search_anime_for(&source, query)?;
                for m in &mut results { m.mediatype = MediaType::Anime; }
                for m in &results {
                    let _ = self.upsert_source(&source, "unknown");
                    let _ = self.get_or_create_series_id(&source, &m.id, m);
                }
                let payload = serde_json::to_string(&results.iter().map(media_to_cache).collect::<Vec<_>>())?;
                let _ = self.rt.block_on(self.db.put_cache(&key, &payload, now + self.search_ttl_secs));
                results
            };

            for m in list { all.push((source.clone(), m)); }
        }

        Ok(all)
    }

    /// Fetch chapters for a manga id. Upserts chapters linked to canonical series id.
    pub fn get_manga_chapters(&mut self, external_manga_id: &str) -> Result<Vec<Unit>> {
        let (source_opt, units) = self.pm.get_manga_chapters_with_source(external_manga_id)?;
        if let Some(source_id) = source_opt {
            // Build a minimal Media to compute kind for the series row
            let media_stub = Media { id: external_manga_id.to_string(), mediatype: MediaType::Manga, title: String::new(), description: None, url: None, cover_url: None };
            let series_id = self.get_or_create_series_id(&source_id, external_manga_id, &media_stub)?;
            let pool = self.db.pool().clone();
            self.rt.block_on(async {
                for u in units.iter().filter(|u| matches!(u.kind, UnitKind::Chapter)) {
                    // Get or create chapter canonical id by mapping lookup
                    if let Some(existing) = dao::find_chapter_id_by_mapping(&pool, &series_id, &source_id, &u.id).await? {
                        // Update existing
                        let ch = chapter_insert_from_unit(&existing, &series_id, &source_id, u);
                        let _ = dao::upsert_chapter(&pool, &ch).await;
                    } else {
                        let cid = uuid::Uuid::new_v4().to_string();
                        let ch = chapter_insert_from_unit(&cid, &series_id, &source_id, u);
                        let _ = dao::upsert_chapter(&pool, &ch).await;
                    }
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
        Ok(units)
    }

    /// Fetch episode for an anime id. Upserts episodes linked to canonical series id.
    pub fn get_anime_episodes(&mut self, external_anime_id: &str) -> Result<Vec<Unit>> {
        let (source_opt, units) = self.pm.get_anime_episodes_with_source(external_anime_id)?;
        if let Some(source_id) = source_opt {
            // Build minimal Media for kind
            let media_stub = Media { id: external_anime_id.to_string(), mediatype: MediaType::Anime, title: String::new(), description: None, url: None, cover_url: None };
            let series_id = self.get_or_create_series_id(&source_id, external_anime_id, &media_stub)?;
            let pool = self.db.pool().clone();
            self.rt.block_on(async {
                for u in units.iter().filter(|u| matches!(u.kind, UnitKind::Episode)) {
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
                        let _ = dao::upsert_episode(&pool, &ep).await;
                    } else {
                        let eid = uuid::Uuid::new_v4().to_string();
                        let ep = crate::dao::EpisodeInsert {
                            id: eid,
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
                        let _ = dao::upsert_episode(&pool, &ep).await;
                    }
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
        Ok(units)
    }

    /// Fetch video streams for an episode id and persist them (no cache yet).
    pub fn get_episode_streams(&mut self, external_episode_id: &str) -> Result<Vec<Asset>> {
        let (src_opt, vids) = self.pm.get_episode_streams_with_source(external_episode_id)?;
        if let Some(source_id) = src_opt {
            // Try to resolve canonical episode id by (source, external)
            let pool = self.db.pool().clone();
            self.rt.block_on(async {
                if let Some(canonical_eid) = dao::find_episode_id_by_source_external(&pool, &source_id, external_episode_id).await? {
                    // Map assets -> streams and upsert
                    let streams: Vec<crate::dao::StreamInsert> = vids.iter().map(|a| crate::dao::StreamInsert {
                        episode_id: canonical_eid.clone(),
                        url: a.url.clone(),
                        quality: None,
                        mime: a.mime.clone(),
                    }).collect();
                    let _ = dao::upsert_streams(&pool, &canonical_eid, &streams).await;
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
        Ok(vids)
    }

    /// Access to the underlying database for future extensions.
    #[allow(dead_code)]
    pub fn database(&self) -> &Database { &self.db }

    /// Example upsert hooks (to be called from future cache-integrated paths)
    pub fn upsert_source(&self, id: &str, version: &str) -> Result<()> {
        let pool = self.db.pool().clone();
        self.rt.block_on(async move { dao::upsert_source(&pool, &dao::SourceInsert { id: id.to_string(), version: version.to_string() }).await })
    }

    pub fn clear_cache_prefix(&self, prefix: Option<&str>) -> Result<u64> {
        let db = self.db.clone();
        // Database is cheap to clone; execute in internal runtime
        self.rt.block_on(async { db.clear_cache_prefix(prefix).await.map_err(Into::into) })
    }

    pub fn vacuum_db(&self) -> Result<()> {
        let db = self.db.clone();
        self.rt.block_on(async { db.vacuum().await.map_err(Into::into) })
    }

    fn norm_query(q: &str) -> String {
        let trimmed = q.trim().to_ascii_lowercase();
        let mut out = String::with_capacity(trimmed.len());
        let mut last_space = false;
        for ch in trimmed.chars() {
            if ch.is_whitespace() {
                if !last_space { out.push(' '); last_space = true; }
            } else { out.push(ch); last_space = false; }
        }
        out
    }

    /// Search manga using cache; on miss, query plugins and persist.
    pub fn search_manga_cached(&mut self, query: &str, refresh: bool) -> Result<Vec<Media>> {
        // Use per-source cache; drop source in return value for backward compat
        let pairs = self.search_manga_cached_with_sources(query, refresh)?;
        Ok(pairs.into_iter().map(|(_, m)| m).collect())
    }

    /// Search anime using cache; on miss, query plugins and persist.
    pub fn search_anime_cached(&mut self, query: &str, refresh: bool) -> Result<Vec<Media>> {
        let pairs = self.search_anime_cached_with_sources(query, refresh)?;
        Ok(pairs.into_iter().map(|(_, m)| m).collect())
    }

    /// Fetch chapter images (URLs) for a chapter id with caching via Storage trait.
    pub fn get_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
        self.get_chapter_images_with_refresh(chapter_id, false)
    }

    pub fn get_chapter_images_with_refresh(&mut self, chapter_id: &str, refresh: bool) -> Result<Vec<String>> {
        let key = format!("all|pages|{}", chapter_id);
        let now = current_epoch();

        if !refresh {
            // Try cache
            if let Some(payload) = self.rt.block_on(self.db.get_cache(&key, now)).ok().flatten() {
                if let Ok(urls) = serde_json::from_str::<Vec<String>>(&payload) {
                    return Ok(urls);
                }
            }
        }

        // Miss -> plugins
        let (_src_opt, urls) = self.pm.get_chapter_images_with_source(chapter_id)?;

        // Write-through with TTL
        let payload = serde_json::to_string(&urls)?;
        let expires_at = now + self.pages_ttl_secs;
        let _ = self.rt.block_on(self.db.put_cache(&key, &payload, expires_at));

        Ok(urls)
    }

    /// List capabilities per plugin. If refresh is true, call plugins, else use cached.
    pub fn get_capabilities(&mut self, refresh: bool) -> Result<Vec<(String, ProviderCapabilities)>> {
        self.pm.get_capabilities(refresh)
    }
}

fn current_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
