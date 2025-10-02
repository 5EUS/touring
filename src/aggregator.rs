use anyhow::Result;
use std::path::Path;

use crate::dao;
use crate::db::Database;
use crate::mapping::{chapter_insert_from_unit, series_insert_from_media, series_source_from};
use crate::plugins::{Asset, Media, MediaType, PluginManager, ProviderCapabilities, Unit, UnitKind};
use crate::types::{media_from_cache, media_to_cache, MediaCache, SearchEntry};
use crate::storage::Storage; // trait for get_cache/put_cache

/// Aggregator owns database + plugins and provides higher-level cached & persisted operations.
pub struct Aggregator {
    db: Database,
    pm: PluginManager,
    // TTLs (seconds)
    search_ttl_secs: i64,
    pages_ttl_secs: i64,
}

impl Aggregator {
    pub fn database(&self) -> &Database { &self.db }
    pub fn plugin_manager(&self) -> &PluginManager { &self.pm }
    pub async fn new(database_url: Option<&str>, run_migrations: bool) -> Result<Self> {
        let db = Database::connect(database_url).await?;
        if run_migrations { db.run_migrations().await?; }
        let pm = PluginManager::new()?;
        let search_ttl_secs = std::env::var("TOURING_SEARCH_TTL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(3600);
        let pages_ttl_secs = std::env::var("TOURING_PAGES_TTL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(24 * 3600);
        Ok(Self { db, pm, search_ttl_secs, pages_ttl_secs })
    }

    pub async fn load_plugins_from_directory(&mut self, dir: &Path) -> Result<()> { self.pm.load_plugins_from_directory(dir).await }
    pub fn list_plugins(&self) -> Vec<String> { self.pm.list_plugins() }

    async fn get_or_create_series_id(&self, source_id: &str, external_id: &str, media: &Media) -> Result<String> {
        let pool = self.db.pool().clone();
        if let Some(existing) = dao::find_series_id_by_source_external(&pool, source_id, external_id).await? { return Ok(existing); }
        let new_id = uuid::Uuid::new_v4().to_string();
        let s = series_insert_from_media(new_id.clone(), media);
        dao::upsert_series(&pool, &s).await?;
        let link = series_source_from(new_id.clone(), source_id.to_string(), external_id.to_string());
        dao::upsert_series_source(&pool, &link).await?;
        Ok(new_id)
    }

    pub async fn search_manga(&self, query: &str) -> Result<Vec<Media>> {
        Ok(self.pm.search_manga_with_sources(query).await?.into_iter().map(|(_, m)| m).collect())
    }
    pub async fn search_anime(&self, query: &str) -> Result<Vec<Media>> {
        Ok(self.pm.search_anime_with_sources(query).await?.into_iter().map(|(_, mut m)| { m.mediatype = MediaType::Anime; m }).collect())
    }

    pub async fn search_manga_cached_with_sources(&self, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        self.search_with_sources(MediaType::Manga, query, refresh).await
    }
    pub async fn search_anime_cached_with_sources(&self, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        self.search_with_sources(MediaType::Anime, query, refresh).await
    }

    async fn search_with_sources(&self, kind: MediaType, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        let norm = norm_query(query);
        let now = current_epoch();
        let sources = self.pm.list_plugins();
        let mut out = Vec::new();
        for source in sources {
            let key = format!("{}|search|{:?}|{}", source, kind, norm);
            let mut hit: Option<Vec<Media>> = None;
            if !refresh {
                if let Some(payload) = self.db.get_cache(&key, now).await.ok().flatten() {
                    hit = try_deserialize_media_cache(&payload, &kind);
                }
            }
            let list = if let Some(m) = hit { m } else {
                let mut list = match kind {
                    MediaType::Manga => self.pm.search_manga_for(&source, query).await?,
                    MediaType::Anime => self.pm.search_anime_for(&source, query).await?,
                    _ => Vec::new(),
                };
                if matches!(kind, MediaType::Anime) { for v in &mut list { v.mediatype = MediaType::Anime; } }
                let payload = serde_json::to_string(&list.iter().map(media_to_cache).collect::<Vec<_>>())?;
                let _ = self.db.put_cache(&key, &payload, now + self.search_ttl_secs).await;
                list
            };
            for m in &list {
                let _ = self.upsert_source(&source, "unknown").await; // ignore errors here
                let _ = self.get_or_create_series_id(&source, &m.id, m).await;
            }
            for m in list { out.push((source.clone(), m)); }
        }
        Ok(out)
    }

    pub async fn get_manga_chapters(&self, external_manga_id: &str) -> Result<Vec<Unit>> {
        let (source_opt, units) = self.pm.get_manga_chapters_with_source(external_manga_id).await?;
        if let Some(source_id) = source_opt {
            let media_stub = Media { id: external_manga_id.to_string(), mediatype: MediaType::Manga, title: String::new(), description: None, url: None, cover_url: None };
            let series_id = self.get_or_create_series_id(&source_id, external_manga_id, &media_stub).await?;
            let pool = self.db.pool().clone();
            for u in units.iter().filter(|u| matches!(u.kind, UnitKind::Chapter)) {
                if let Some(existing) = dao::find_chapter_id_by_mapping(&pool, &series_id, &source_id, &u.id).await? {
                    let ch = chapter_insert_from_unit(existing, series_id.clone(), source_id.clone(), u);
                    let _ = dao::upsert_chapter(&pool, &ch).await;
                } else {
                    let cid = uuid::Uuid::new_v4().to_string();
                    let ch = chapter_insert_from_unit(cid, series_id.clone(), source_id.clone(), u);
                    let _ = dao::upsert_chapter(&pool, &ch).await;
                }
            }
        }
        Ok(units)
    }

    pub async fn get_anime_episodes(&self, external_anime_id: &str) -> Result<Vec<Unit>> {
        let (source_opt, units) = self.pm.get_anime_episodes_with_source(external_anime_id).await?;
        if let Some(source_id) = source_opt {
            let media_stub = Media { id: external_anime_id.to_string(), mediatype: MediaType::Anime, title: String::new(), description: None, url: None, cover_url: None };
            let series_id = self.get_or_create_series_id(&source_id, external_anime_id, &media_stub).await?;
            let pool = self.db.pool().clone();
            for u in units.iter().filter(|u| matches!(u.kind, UnitKind::Episode)) {
                if let Some(existing) = dao::find_episode_id_by_mapping(&pool, &series_id, &source_id, &u.id).await? {
                    let ep = crate::dao::EpisodeInsert { id: existing, series_id: series_id.clone(), source_id: source_id.clone(), external_id: u.id.clone(), number_text: u.number_text.clone(), number_num: u.number.map(|n| n as f64), title: Some(u.title.clone()).filter(|s| !s.is_empty()), lang: u.lang.clone(), season: u.group.clone(), published_at: u.published_at.clone() };
                    let _ = dao::upsert_episode(&pool, &ep).await;
                } else {
                    let eid = uuid::Uuid::new_v4().to_string();
                    let ep = crate::dao::EpisodeInsert { id: eid, series_id: series_id.clone(), source_id: source_id.clone(), external_id: u.id.clone(), number_text: u.number_text.clone(), number_num: u.number.map(|n| n as f64), title: Some(u.title.clone()).filter(|s| !s.is_empty()), lang: u.lang.clone(), season: u.group.clone(), published_at: u.published_at.clone() };
                    let _ = dao::upsert_episode(&pool, &ep).await;
                }
            }
        }
        Ok(units)
    }

    pub async fn get_episode_streams(&self, external_episode_id: &str) -> Result<Vec<Asset>> {
        let (src_opt, vids) = self.pm.get_episode_streams_with_source(external_episode_id).await?;
        if let Some(source_id) = src_opt {
            let pool = self.db.pool().clone();
            if let Some(canonical_eid) = dao::find_episode_id_by_source_external(&pool, &source_id, external_episode_id).await? {
                let streams: Vec<crate::dao::StreamInsert> = vids.iter().map(|a| crate::dao::StreamInsert { episode_id: canonical_eid.clone(), url: a.url.clone(), quality: None, mime: a.mime.clone() }).collect();
                let _ = dao::upsert_streams(&pool, &canonical_eid, &streams).await;
            }
        }
        Ok(vids)
    }

    pub async fn get_chapter_images_with_refresh(&self, chapter_id: &str, refresh: bool) -> Result<Vec<String>> {
        let key = format!("all|pages|{}", chapter_id);
        let now = current_epoch();
        if !refresh {
            if let Some(payload) = self.db.get_cache(&key, now).await.ok().flatten() {
                if let Ok(urls) = serde_json::from_str::<Vec<String>>(&payload) { return Ok(urls); }
            }
        }
        let (_src_opt, urls) = self.pm.get_chapter_images_with_source(chapter_id).await?;
        let payload = serde_json::to_string(&urls)?;
        let _ = self.db.put_cache(&key, &payload, now + self.pages_ttl_secs).await;
        Ok(urls)
    }
    pub async fn get_chapter_images(&self, chapter_id: &str) -> Result<Vec<String>> { self.get_chapter_images_with_refresh(chapter_id, false).await }

    pub async fn get_capabilities(&self, refresh: bool) -> Result<Vec<(String, ProviderCapabilities)>> { self.pm.get_capabilities(refresh).await }
    pub async fn get_allowed_hosts(&self) -> Result<Vec<(String, Vec<String>)>> { self.pm.get_allowed_hosts().await }

    pub async fn upsert_source(&self, id: &str, version: &str) -> Result<()> {
        let pool = self.db.pool().clone();
        dao::upsert_source(&pool, &dao::SourceInsert { id: id.to_string(), version: version.to_string() }).await.map_err(Into::into)
    }
    pub async fn clear_cache_prefix(&self, prefix: Option<&str>) -> Result<u64> { self.db.clear_cache_prefix(prefix).await.map_err(Into::into) }
    pub async fn vacuum_db(&self) -> Result<()> { self.db.vacuum().await.map_err(Into::into) }
}

fn try_deserialize_media_cache(payload: &str, _kind: &MediaType) -> Option<Vec<Media>> {
    if let Ok(items) = serde_json::from_str::<Vec<MediaCache>>(payload) { return Some(items.into_iter().map(media_from_cache).collect()); }
    if let Ok(entries) = serde_json::from_str::<Vec<SearchEntry>>(payload) { return Some(entries.into_iter().map(|e| media_from_cache(e.media)).collect()); }
    None
}

fn norm_query(q: &str) -> String {
    let t = q.trim().to_ascii_lowercase();
    let mut o = String::with_capacity(t.len());
    let mut s = false;
    for c in t.chars() {
        if c.is_whitespace() {
            if !s { o.push(' '); s = true; }
        } else {
            o.push(c); s = false;
        }
    }
    o
}

fn current_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
