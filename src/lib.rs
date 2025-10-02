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
    pub use crate::{SeriesInfo, SeriesMetadataUpdate, SeriesSource, ChapterInfo, EpisodeInfo, DownloadProgress, DownloadResult, LibraryStats};
}

use anyhow::Result;
use std::path::Path;
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};



use crate::db::Database;
use crate::plugins::{PluginManager, Media, Unit, MediaType, Asset, ProviderCapabilities};
use crate::storage::Storage;
use crate::mapping::{series_insert_from_media, series_source_from, chapter_insert_from_unit};
use crate::types::{MediaCache, SearchEntry, media_to_cache, media_from_cache};

// --- Data structures for UI API ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesInfo {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub status: Option<String>,
    pub download_path: Option<String>,
    pub chapters_count: usize,
    pub episodes_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesMetadataUpdate {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub cover_url: Option<Option<String>>,
    pub status: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesSource {
    pub source_id: String,
    pub external_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterInfo {
    pub id: String,
    pub series_id: String,
    pub external_id: String,
    pub number_text: Option<String>,
    pub number_num: Option<f64>,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub volume: Option<String>,
    pub has_images: bool,
    pub image_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeInfo {
    pub id: String,
    pub series_id: String,
    pub external_id: String,
    pub number_text: Option<String>,
    pub number_num: Option<f64>,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub season: Option<String>,
    pub has_streams: bool,
    pub stream_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub current: usize,
    pub total: usize,
    pub current_item: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadResult {
    pub success: bool,
    pub items_processed: usize,
    pub items_downloaded: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStats {
    pub total_series: usize,
    pub manga_series: usize,
    pub anime_series: usize,
    pub total_chapters: usize,
    pub total_episodes: usize,
    pub total_sources: usize,
    pub cache_entries: usize,
    pub expired_cache_entries: usize,
}

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
        pm.load_plugins_from_directory(dir)
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

    /// Get allowed hosts per plugin.
    pub async fn get_allowed_hosts(&self) -> Result<Vec<(String, Vec<String>)>> {
        let pm = self.pm.clone();
        tokio::task::spawn_blocking(move || pm.lock().unwrap().get_allowed_hosts())
            .await
            .unwrap()
    }

    /// Search manga with per-source caching; upserts series + mappings. Returns (source, media).
    pub async fn search_manga_cached_with_sources(&self, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        let norm = norm_query(query);
        let now = current_epoch();
        let sources = self.list_plugins();
        let mut all = Vec::with_capacity(sources.len() * 10); // Pre-allocate based on expected results

        for source in sources {
            let key = format!("{}|search|manga|{}", source, norm);
            let mut hit: Option<Vec<Media>> = None;
            
            // Try cache first if not refreshing
            if !refresh {
                if let Some(payload) = self.db.get_cache(&key, now).await.ok().flatten() {
                    hit = Self::try_deserialize_media_cache(&payload, MediaType::Manga);
                }
            }

            let list = if let Some(medias) = hit {
                medias
            } else {
                // Cache miss -> query plugin (avoid spawn_blocking overhead for single calls)
                let pm = self.pm.clone();
                let src = source.clone();
                let q = query.to_string();
                let results = tokio::task::spawn_blocking(move || pm.lock().unwrap().search_manga_for(&src, &q)).await.unwrap()?;
                
                // Cache the results  
                let payload = serde_json::to_string(&results.iter().map(media_to_cache).collect::<Vec<_>>())?;
                let _ = self.db.put_cache(&key, &payload, now + self.search_ttl_secs).await;
                results
            };

            // Upsert series mappings for all results from this source
            for m in &list {
                let _ = self.upsert_source(&source, "unknown").await;
                let _ = self.get_or_create_series_id(&source, &m.id, m).await;
            }
            
            // Add to results
            for m in list {
                all.push((source.clone(), m));
            }
        }
        Ok(all)
    }

    /// Search anime with per-source caching; upserts series + mappings. Returns (source, media).
    pub async fn search_anime_cached_with_sources(&self, query: &str, refresh: bool) -> Result<Vec<(String, Media)>> {
        let norm = norm_query(query);
        let now = current_epoch();
        let sources = self.list_plugins();
        let mut all = Vec::with_capacity(sources.len() * 10); // Pre-allocate

        for source in sources {
            let key = format!("{}|search|anime|{}", source, norm);
            let mut hit: Option<Vec<Media>> = None;
            
            // Try cache first if not refreshing
            if !refresh {
                if let Some(payload) = self.db.get_cache(&key, now).await.ok().flatten() {
                    hit = Self::try_deserialize_media_cache(&payload, MediaType::Anime);
                }
            }

            let list = if let Some(medias) = hit {
                medias
            } else {
                // Cache miss -> query plugin
                let pm = self.pm.clone();
                let src = source.clone();
                let q = query.to_string();
                let mut results = tokio::task::spawn_blocking(move || pm.lock().unwrap().search_anime_for(&src, &q)).await.unwrap()?;
                
                // Ensure correct media type for anime
                for m in &mut results { 
                    m.mediatype = MediaType::Anime; 
                }
                
                // Cache the results
                let payload = serde_json::to_string(&results.iter().map(media_to_cache).collect::<Vec<_>>())?;
                let _ = self.db.put_cache(&key, &payload, now + self.search_ttl_secs).await;
                results
            };

            // Upsert series mappings for all results from this source
            for m in &list {
                let _ = self.upsert_source(&source, "unknown").await;
                let _ = self.get_or_create_series_id(&source, &m.id, m).await;
            }
            
            // Add to results
            for m in list {
                all.push((source.clone(), m));
            }
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
                    let ch = chapter_insert_from_unit(existing, series_id.clone(), source_id.clone(), u);
                    let _ = crate::dao::upsert_chapter(&pool, &ch).await;
                } else {
                    let cid = uuid::Uuid::new_v4().to_string();
                    let ch = chapter_insert_from_unit(cid, series_id.clone(), source_id.clone(), u);
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
        let eid_input = external_episode_id.to_string();
        // Resolve canonical -> external if needed
        let eid = self.resolve_episode_external_id(&eid_input).await?;
        let eid_for_call = eid.clone();
        let (src_opt, vids) = tokio::task::spawn_blocking(move || pm.lock().unwrap().get_episode_streams_with_source(&eid_for_call)).await.unwrap()?;
        if let Some(source_id) = src_opt {
            let pool = self.db.pool().clone();
            if let Some(canonical_eid) = crate::dao::find_episode_id_by_source_external(&pool, &source_id, &eid).await? {
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

    /// Fetch chapter images (URLs) with caching and optional refresh. Accepts canonical or external chapter id.
    pub async fn get_chapter_images_with_refresh(&self, chapter_id: &str, refresh: bool) -> Result<Vec<String>> {
        // Resolve canonical -> external if possible, but keep original too
        let original_id = chapter_id.to_string();
        let external_id = self.resolve_chapter_external_id(chapter_id).await?;
        let key_orig = format!("all|pages|{}", original_id);
        let key_ext = format!("all|pages|{}", external_id);
        let now = current_epoch();

        if !refresh {
            // First try cache with the exact id the user provided (back-compat)
            if let Some(payload) = self.db.get_cache(&key_orig, now).await.ok().flatten() {
                if let Ok(urls) = serde_json::from_str::<Vec<String>>(&payload) { return Ok(urls); }
            }
            // Then try the external-id-based cache key
            if key_ext != key_orig {
                if let Some(payload) = self.db.get_cache(&key_ext, now).await.ok().flatten() {
                    if let Ok(urls) = serde_json::from_str::<Vec<String>>(&payload) { return Ok(urls); }
                }
            }
        }

        // Miss -> call plugins; prefer trying with the ID the user provided first
        let mut urls: Vec<String> = Vec::new();
        for try_id in [original_id.as_str(), external_id.as_str()] {
            let pm = self.pm.clone();
            let cid = try_id.to_string();
            let (_src_opt, u) = tokio::task::spawn_blocking(move || pm.lock().unwrap().get_chapter_images_with_source(&cid)).await.unwrap()?;
            if !u.is_empty() { urls = u; break; }
            // If first attempt returns empty, try the other id
            if try_id == external_id.as_str() { break; }
        }

        // Write-through with TTL into both keys for future hits
        let payload = serde_json::to_string(&urls)?;
        let expires_at = now + self.pages_ttl_secs;
        let _ = self.db.put_cache(&key_ext, &payload, expires_at).await;
        if key_orig != key_ext { let _ = self.db.put_cache(&key_orig, &payload, expires_at).await; }

        Ok(urls)
    }

    // Convenience: accepts canonical or external chapter id
    pub async fn get_chapter_images(&self, chapter_id: &str) -> Result<Vec<String>> {
        self.get_chapter_images_with_refresh(chapter_id, false).await
    }

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

    /// Resolve the canonical series id from a source id and the plugin's external media id
    pub async fn resolve_series_id(&self, source_id: &str, external_id: &str) -> Result<Option<String>> {
        let pool = self.db.pool().clone();
        crate::dao::find_series_id_by_source_external(&pool, source_id, external_id).await
    }

    /// Get series_id and naming info for a chapter
    pub async fn get_chapter_meta(&self, chapter_id: &str) -> Result<Option<(String, Option<f64>, Option<String>)>> {
        let pool = self.db.pool().clone();
        // Try canonical id first
        let row: Option<(String, Option<f64>, Option<String>)> = sqlx::query_as(
            "SELECT series_id, number_num, number_text FROM chapters WHERE id = ?"
        )
        .bind(chapter_id)
        .fetch_optional(&pool)
        .await?;
        if row.is_some() { return Ok(row); }
        // Fallback: treat provided id as external id
        let row2: Option<(String, Option<f64>, Option<String>)> = sqlx::query_as(
            "SELECT series_id, number_num, number_text FROM chapters WHERE external_id = ?"
        )
        .bind(chapter_id)
        .fetch_optional(&pool)
        .await?;
        Ok(row2)
    }

    /// Get series_id and naming info for an episode
    pub async fn get_episode_meta(&self, episode_id: &str) -> Result<Option<(String, Option<f64>, Option<String>)>> {
        let pool = self.db.pool().clone();
        let row: Option<(String, Option<f64>, Option<String>)> = sqlx::query_as(
            "SELECT series_id, number_num, number_text FROM episodes WHERE id = ?"
        )
        .bind(episode_id)
        .fetch_optional(&pool)
        .await?;
        Ok(row)
    }

    /// Get stored download path for a series id
    pub async fn get_series_path(&self, series_id: &str) -> Result<Option<String>> {
        self.get_series_download_path(series_id).await
    }

    /// Clear cache entries by prefix. Returns number of rows removed.
    pub async fn clear_cache_prefix(&self, prefix: Option<&str>) -> Result<u64> {
        self.db.clear_cache_prefix(prefix).await.map_err(Into::into)
    }

    /// Vacuum/compact the database (SQLite only; no-op on others).
    pub async fn vacuum_db(&self) -> Result<()> { self.db.vacuum().await.map_err(Into::into) }

    // --- Download API for UI ---

    /// Download chapter images to a directory. Returns number of images downloaded.
    pub async fn download_chapter_images(&self, chapter_id: &str, output_dir: &Path, force_overwrite: bool) -> Result<usize> {
        let urls = self.get_chapter_images_with_refresh(chapter_id, false).await?;
        if urls.is_empty() { return Ok(0); }
        
        tokio::fs::create_dir_all(output_dir).await.ok();
        let client = reqwest::Client::builder().user_agent("touring/0.1").build()?;
        let mut downloaded = 0;
        
        for (i, url) in urls.iter().enumerate() {
            if url.starts_with("mock://") {
                let fname = format!("{:04}.jpg", i + 1);
                let path = output_dir.join(fname);
                if !force_overwrite && tokio::fs::try_exists(&path).await.unwrap_or(false) { continue; }
                tokio::fs::write(&path, b"MOCK").await?;
                downloaded += 1;
                continue;
            }
            
            let fname = format!("{:04}.jpg", i + 1);
            let path = output_dir.join(fname);
            if !force_overwrite && tokio::fs::try_exists(&path).await.unwrap_or(false) { continue; }
            
            let resp = client.get(url).send().await?;
            if !resp.status().is_success() { continue; }
            let bytes = resp.bytes().await?;
            tokio::fs::write(&path, &bytes).await?;
            downloaded += 1;
        }
        Ok(downloaded)
    }

    /// Download chapter as CBZ archive. Returns true if downloaded successfully.
    pub async fn download_chapter_cbz(&self, chapter_id: &str, output_file: &Path, force_overwrite: bool) -> Result<bool> {
        if !force_overwrite && tokio::fs::try_exists(output_file).await.unwrap_or(false) { return Ok(false); }
        
        let urls = self.get_chapter_images_with_refresh(chapter_id, false).await?;
        if urls.is_empty() { return Ok(false); }
        
        let tmp_dir = output_file.with_extension("tmpdir");
        let downloaded = self.download_chapter_images(chapter_id, &tmp_dir, true).await?;
        if downloaded == 0 { return Ok(false); }
        
        // Create CBZ
        let file = std::fs::File::create(output_file)?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        
        let mut entries: Vec<_> = std::fs::read_dir(&tmp_dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        
        for entry in entries {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                zip.start_file(name, options)?;
                let data = std::fs::read(&path)?;
                use std::io::Write;
                zip.write_all(&data)?;
            }
        }
        zip.finish()?;
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp_dir);
        Ok(true)
    }

    /// Download all chapters for a series to a base directory. Returns (chapters_processed, chapters_downloaded).
    pub async fn download_series_chapters(&self, series_id: &str, base_dir: &Path, as_cbz: bool, force_overwrite: bool) -> Result<(usize, usize)> {
        let chapters = self.list_chapters_for_series(series_id).await?;
        let mut processed = 0;
        let mut downloaded = 0;
        
        tokio::fs::create_dir_all(base_dir).await.ok();
        
        for (chapter_id, number_num, number_text) in chapters {
            processed += 1;
            let name = number_text.or_else(|| number_num.map(|n| format!("{:.3}", n))).unwrap_or_else(|| format!("chapter_{}", processed));
            
            if as_cbz {
                let output_file = base_dir.join(format!("{}.cbz", name));
                if self.download_chapter_cbz(&chapter_id, &output_file, force_overwrite).await? {
                    downloaded += 1;
                }
            } else {
                let output_dir = base_dir.join(name);
                let count = self.download_chapter_images(&chapter_id, &output_dir, force_overwrite).await?;
                if count > 0 { downloaded += 1; }
            }
        }
        
        Ok((processed, downloaded))
    }

    /// Download series with progress callback. Callback receives (current, total, item_name).
    pub async fn download_series_chapters_with_progress<F>(&self, series_id: &str, base_dir: &Path, as_cbz: bool, force_overwrite: bool, mut progress_callback: F) -> Result<DownloadResult>
    where
        F: FnMut(DownloadProgress),
    {
        let chapters = self.list_chapters_for_series(series_id).await?;
        let total = chapters.len();
        let mut processed = 0;
        let mut downloaded = 0;
        
        tokio::fs::create_dir_all(base_dir).await.ok();
        
        for (chapter_id, number_num, number_text) in chapters {
            processed += 1;
            let name = number_text.or_else(|| number_num.map(|n| format!("{:.3}", n))).unwrap_or_else(|| format!("chapter_{}", processed));
            
            progress_callback(DownloadProgress {
                current: processed,
                total,
                current_item: name.clone(),
            });
            
            let success = if as_cbz {
                let output_file = base_dir.join(format!("{}.cbz", name));
                self.download_chapter_cbz(&chapter_id, &output_file, force_overwrite).await.unwrap_or(false)
            } else {
                let output_dir = base_dir.join(name);
                let count = self.download_chapter_images(&chapter_id, &output_dir, force_overwrite).await.unwrap_or(0);
                count > 0
            };
            
            if success { downloaded += 1; }
        }
        
        Ok(DownloadResult {
            success: true,
            items_processed: processed,
            items_downloaded: downloaded,
            error: None,
        })
    }

    /// Get download status for a series (how many chapters are already downloaded).
    pub async fn get_series_download_status(&self, series_id: &str, base_dir: &Path, as_cbz: bool) -> Result<(usize, usize)> {
        let chapters = self.list_chapters_for_series(series_id).await?;
        let total = chapters.len();
        let mut downloaded = 0;
        
        for (_, number_num, number_text) in chapters.iter().enumerate().map(|(_i, (id, num, text))| (id, num, text)) {
            let name = number_text.clone().or_else(|| number_num.map(|n| format!("{:.3}", n))).unwrap_or_else(|| format!("chapter_{}", downloaded + 1));
            
            let exists = if as_cbz {
                let output_file = base_dir.join(format!("{}.cbz", name));
                tokio::fs::try_exists(&output_file).await.unwrap_or(false)
            } else {
                let output_dir = base_dir.join(name);
                tokio::fs::try_exists(&output_dir).await.unwrap_or(false)
            };
            
            if exists { downloaded += 1; }
        }
        
        Ok((downloaded, total))
    }

    // --- Series Management API for UI ---

    /// Get full series information including metadata and preferences.
    pub async fn get_series_info(&self, series_id: &str) -> Result<Option<SeriesInfo>> {
        let pool = self.db.pool().clone();
        let row: Option<(String, String, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, kind, title, description, cover_url, status FROM series WHERE id = ?"
        )
        .bind(series_id)
        .fetch_optional(&pool)
        .await?;
        
        let Some((id, kind, title, description, cover_url, status)) = row else { return Ok(None); };
        
        let pref = crate::dao::get_series_pref(&pool, series_id).await?;
        let download_path = pref.and_then(|p| p.download_path);
        
        let chapters_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapters WHERE series_id = ?")
            .bind(series_id)
            .fetch_one(&pool)
            .await?;
            
        let episodes_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE series_id = ?")
            .bind(series_id)
            .fetch_one(&pool)
            .await?;
        
        Ok(Some(SeriesInfo {
            id,
            kind,
            title,
            description,
            cover_url,
            status,
            download_path,
            chapters_count: chapters_count as usize,
            episodes_count: episodes_count as usize,
        }))
    }

    /// Update series metadata (title, description, status, etc.).
    pub async fn update_series_metadata(&self, series_id: &str, updates: SeriesMetadataUpdate) -> Result<()> {
        let pool = self.db.pool().clone();
        
        // Build dynamic query based on provided fields
        let mut query = "UPDATE series SET updated_at = CURRENT_TIMESTAMP".to_string();
        let mut bindings = Vec::new();
        
        if let Some(title) = &updates.title {
            query.push_str(", title = ?");
            bindings.push(title.as_str());
        }
        if let Some(description) = &updates.description {
            query.push_str(", description = ?");
            bindings.push(description.as_deref().unwrap_or(""));
        }
        if let Some(cover_url) = &updates.cover_url {
            query.push_str(", cover_url = ?");
            bindings.push(cover_url.as_deref().unwrap_or(""));
        }
        if let Some(status) = &updates.status {
            query.push_str(", status = ?");
            bindings.push(status.as_deref().unwrap_or(""));
        }
        
        query.push_str(" WHERE id = ?");
        
        // Execute update if we have any fields to update
        if !bindings.is_empty() {
            let mut q = sqlx::query(&query);
            for binding in bindings {
                q = q.bind(binding);
            }
            q = q.bind(series_id);
            q.execute(&pool).await?;
        }
        
        Ok(())
    }

    /// Get all sources and external IDs for a series.
    pub async fn get_series_sources(&self, series_id: &str) -> Result<Vec<SeriesSource>> {
        let pool = self.db.pool().clone();
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT source_id, external_id FROM series_sources WHERE series_id = ?"
        )
        .bind(series_id)
        .fetch_all(&pool)
        .await?;
        
        Ok(rows.into_iter().map(|(source_id, external_id)| SeriesSource { source_id, external_id }).collect())
    }

    /// Add a new source mapping for a series.
    pub async fn add_series_source(&self, series_id: &str, source_id: &str, external_id: &str) -> Result<()> {
        let pool = self.db.pool().clone();
        let link = crate::dao::SeriesSourceInsert {
            series_id: series_id.to_string(),
            source_id: source_id.to_string(),
            external_id: external_id.to_string(),
        };
        crate::dao::upsert_series_source(&pool, &link).await
    }

    /// Remove a source mapping for a series.
    pub async fn remove_series_source(&self, series_id: &str, source_id: &str, external_id: &str) -> Result<u64> {
        let pool = self.db.pool().clone();
        let res = sqlx::query("DELETE FROM series_sources WHERE series_id = ? AND source_id = ? AND external_id = ?")
            .bind(series_id)
            .bind(source_id)
            .bind(external_id)
            .execute(&pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// Get detailed chapter information including download status.
    pub async fn get_chapter_info(&self, chapter_id: &str) -> Result<Option<ChapterInfo>> {
        let pool = self.db.pool().clone();
        let row: Option<(String, String, String, Option<String>, Option<f64>, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, series_id, external_id, number_text, number_num, title, lang, volume FROM chapters WHERE id = ?"
        )
        .bind(chapter_id)
        .fetch_optional(&pool)
        .await?;
        
        let Some((id, series_id, external_id, number_text, number_num, title, lang, volume)) = row else { return Ok(None); };
        
        // Check if images are cached
        let images = self.get_chapter_images(chapter_id).await.unwrap_or_default();
        let has_images = !images.is_empty();
        
        Ok(Some(ChapterInfo {
            id,
            series_id,
            external_id,
            number_text,
            number_num,
            title,
            lang,
            volume,
            has_images,
            image_count: images.len(),
        }))
    }

    /// Get detailed episode information.
    pub async fn get_episode_info(&self, episode_id: &str) -> Result<Option<EpisodeInfo>> {
        let pool = self.db.pool().clone();
        let row: Option<(String, String, String, Option<String>, Option<f64>, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, series_id, external_id, number_text, number_num, title, lang, season FROM episodes WHERE id = ?"
        )
        .bind(episode_id)
        .fetch_optional(&pool)
        .await?;
        
        let Some((id, series_id, external_id, number_text, number_num, title, lang, season)) = row else { return Ok(None); };
        
        // Check for streams
        let stream_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM streams WHERE episode_id = ?")
            .bind(episode_id)
            .fetch_one(&pool)
            .await?;
        
        Ok(Some(EpisodeInfo {
            id,
            series_id,
            external_id,
            number_text,
            number_num,
            title,
            lang,
            season,
            has_streams: stream_count > 0,
            stream_count: stream_count as usize,
        }))
    }

    /// Search series in local database (for UI autocomplete/filtering).
    pub async fn search_local_series(&self, query: &str, kind: Option<&str>, limit: Option<usize>) -> Result<Vec<SeriesInfo>> {
        let pool = self.db.pool().clone();
        let search_term = format!("%{}%", query);
        let limit_val = limit.unwrap_or(50) as i64;
        
        let rows = if let Some(k) = kind {
            sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>, Option<String>)>(
                "SELECT id, kind, title, description, cover_url, status FROM series WHERE title LIKE ? AND kind = ? ORDER BY title LIMIT ?"
            )
            .bind(&search_term)
            .bind(k)
            .bind(limit_val)
            .fetch_all(&pool)
            .await?
        } else {
            sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>, Option<String>)>(
                "SELECT id, kind, title, description, cover_url, status FROM series WHERE title LIKE ? ORDER BY title LIMIT ?"
            )
            .bind(&search_term)
            .bind(limit_val)
            .fetch_all(&pool)
            .await?
        };
        
        let mut result = Vec::new();
        
        for (id, kind, title, description, cover_url, status) in rows {
            let pref = crate::dao::get_series_pref(&pool, &id).await?;
            let download_path = pref.and_then(|p| p.download_path);
            
            let chapters_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapters WHERE series_id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await?;
                
            let episodes_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE series_id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await?;
            
            result.push(SeriesInfo {
                id,
                kind,
                title,
                description,
                cover_url,
                status,
                download_path,
                chapters_count: chapters_count as usize,
                episodes_count: episodes_count as usize,
            });
        }
        
        Ok(result)
    }

    /// Get statistics about the library (total series, chapters, episodes, etc.).
    pub async fn get_library_stats(&self) -> Result<LibraryStats> {
        let pool = self.db.pool().clone();
        
        let total_series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series").fetch_one(&pool).await?;
        let manga_series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE kind = 'manga'").fetch_one(&pool).await?;
        let anime_series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE kind = 'anime'").fetch_one(&pool).await?;
        let total_chapters: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapters").fetch_one(&pool).await?;
        let total_episodes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episodes").fetch_one(&pool).await?;
        let total_sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources").fetch_one(&pool).await?;
        
        // Cache stats
        let cache_entries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cache").fetch_one(&pool).await?;
        let expired_cache: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cache WHERE expires_at < ?")
            .bind(current_epoch())
            .fetch_one(&pool)
            .await?;
        
        Ok(LibraryStats {
            total_series: total_series as usize,
            manga_series: manga_series as usize,
            anime_series: anime_series as usize,
            total_chapters: total_chapters as usize,
            total_episodes: total_episodes as usize,
            total_sources: total_sources as usize,
            cache_entries: cache_entries as usize,
            expired_cache_entries: expired_cache as usize,
        })
    }

    /// Refresh metadata for a series from all its sources.
    pub async fn refresh_series_metadata(&self, series_id: &str) -> Result<bool> {
        let sources = self.get_series_sources(series_id).await?;
        let mut updated = false;
        
        for source in sources {
            // Try to fetch fresh metadata for this series from the source
            let pm = self.pm.clone();
            let external_id = source.external_id.clone();
            let source_id = source.source_id.clone();
            
            let media_list = tokio::task::spawn_blocking(move || {
                pm.lock().unwrap().search_manga_for(&source_id, &external_id)
            }).await.unwrap().unwrap_or_default();
            
            // If we find a match, update the series metadata
            if let Some(media) = media_list.into_iter().find(|m| m.id == source.external_id) {
                let updates = SeriesMetadataUpdate {
                    title: Some(media.title),
                    description: Some(media.description),
                    cover_url: Some(media.cover_url),
                    status: None, // Don't override status from search results
                };
                self.update_series_metadata(series_id, updates).await?;
                updated = true;
            }
        }
        
        Ok(updated)
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
        let s = series_insert_from_media(new_id.clone(), media);
        crate::dao::upsert_series(&pool, &s).await?;
        let link = series_source_from(new_id.clone(), source_id.to_string(), external_id.to_string());
        crate::dao::upsert_series_source(&pool, &link).await?;
        Ok(new_id)
    }

    async fn resolve_chapter_external_id(&self, id: &str) -> Result<String> {
        let pool = self.db.pool().clone();
        let ext: Option<String> = sqlx::query_scalar("SELECT external_id FROM chapters WHERE id = ? LIMIT 1")
            .bind(id)
            .fetch_optional(&pool)
            .await?;
        Ok(ext.unwrap_or_else(|| id.to_string()))
    }

    async fn resolve_episode_external_id(&self, id: &str) -> Result<String> {
        let pool = self.db.pool().clone();
        let ext: Option<String> = sqlx::query_scalar("SELECT external_id FROM episodes WHERE id = ? LIMIT 1")
            .bind(id)
            .fetch_optional(&pool)
            .await?;
        Ok(ext.unwrap_or_else(|| id.to_string()))
    }
    /// Helper to deserialize cache payload with fallback to legacy format
    fn try_deserialize_media_cache(payload: &str, media_type: MediaType) -> Option<Vec<Media>> {
        // Try new format first
        if let Ok(items) = serde_json::from_str::<Vec<MediaCache>>(payload) {
            let mut medias: Vec<Media> = items.into_iter().map(media_from_cache).collect();
            // Ensure correct media type for anime from cache
            if matches!(media_type, MediaType::Anime) {
                for m in &mut medias {
                    m.mediatype = MediaType::Anime;
                }
            }
            return Some(medias);
        }
        
        // Fallback to legacy format
        if let Ok(entries) = serde_json::from_str::<Vec<SearchEntry>>(payload) {
            let mut medias: Vec<Media> = entries.into_iter().map(|e| media_from_cache(e.media)).collect();
            if matches!(media_type, MediaType::Anime) {
                for m in &mut medias {
                    m.mediatype = MediaType::Anime;
                }
            }
            return Some(medias);
        }
        
        None
    }
}

// --- Helper functions (eliminate duplication from aggregator) ---

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
