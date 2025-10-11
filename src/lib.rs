pub mod aggregator;
pub mod dao;
pub mod db;
pub mod mapping;
pub mod plugins;
pub mod storage;
pub mod types;

// --- Library API for embedding ---

/// Convenience re-exports for embedders.
pub mod prelude {
    pub use crate::plugins::{
        Asset, AssetKind, Media, MediaType, ProviderCapabilities, Unit, UnitKind,
    };
    pub use crate::{
        ChapterInfo, DownloadProgress, DownloadResult, EpisodeInfo, LibraryStats, SeriesInfo,
        SeriesMetadataUpdate, SeriesSource,
    };
}

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::aggregator::Aggregator;
use crate::plugins::{Asset, Media, ProviderCapabilities, Unit};

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
pub struct ChapterProgress {
    pub chapter_id: String,
    pub series_id: String,
    pub page_index: i64,
    pub total_pages: Option<i64>,
    pub updated_at: i64,
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

/// High-level façade for embedders. Delegates all media/search/cache logic to `Aggregator`.
pub struct Touring {
    agg: Aggregator,
}

impl Touring {
    /// Initialize database and (optionally) run migrations. Does not start any internal runtimes.
    pub async fn connect(database_url: Option<&str>, run_migrations: bool) -> Result<Self> {
        let agg = Aggregator::new(database_url, run_migrations).await?;
        Ok(Self { agg })
    }

    /// Load all plugins from a directory.
    pub async fn load_plugins_from_directory(&mut self, dir: &Path) -> Result<()> {
        self.agg.load_plugins_from_directory(dir).await
    }

    /// Rebuild plugin runtime from a directory, replacing any previously loaded plugins.
    pub async fn reload_plugins_from_directory(&mut self, dir: &Path) -> Result<()> {
        self.agg.reload_plugins_from_directory(dir).await
    }

    /// List loaded plugin names.
    pub fn list_plugins(&self) -> Vec<String> {
        self.agg.list_plugins()
    }

    /// Get plugin capabilities (cached by default, or refresh).
    pub async fn get_capabilities(
        &self,
        refresh: bool,
    ) -> Result<Vec<(String, ProviderCapabilities)>> {
        self.agg.get_capabilities(refresh).await
    }

    /// Get allowed hosts per plugin.
    pub async fn get_allowed_hosts(&self) -> Result<Vec<(String, Vec<String>)>> {
        self.agg.get_allowed_hosts().await
    }

    /// Search manga with per-source caching; upserts series + mappings. Returns (source, media).
    pub async fn search_manga_cached_with_sources(
        &self,
        query: &str,
        refresh: bool,
    ) -> Result<Vec<(String, Media)>> {
        self.agg
            .search_manga_cached_with_sources(query, refresh)
            .await
    }

    /// Search manga without persisting to database (UI display only). Returns (source, media).
    pub async fn search_manga_no_persist(
        &self,
        query: &str,
        refresh: bool,
    ) -> Result<Vec<(String, Media)>> {
        self.agg.search_manga_no_persist(query, refresh).await
    }

    /// Search anime with per-source caching; upserts series + mappings. Returns (source, media).
    pub async fn search_anime_cached_with_sources(
        &self,
        query: &str,
        refresh: bool,
    ) -> Result<Vec<(String, Media)>> {
        self.agg
            .search_anime_cached_with_sources(query, refresh)
            .await
    }

    /// Fetch chapters for a manga id; upserts chapters linked to canonical series id.
    pub async fn get_manga_chapters(&self, external_manga_id: &str) -> Result<Vec<Unit>> {
        self.agg.get_manga_chapters(external_manga_id).await
    }

    /// Fetch chapters without persisting them (used for preview flows)
    pub async fn preview_manga_chapters(&self, external_manga_id: &str) -> Result<Vec<Unit>> {
        self.agg.preview_manga_chapters(external_manga_id).await
    }

    /// Fetch episode list for an anime id; upserts and returns episodes.
    pub async fn get_anime_episodes(&self, external_anime_id: &str) -> Result<Vec<Unit>> {
        self.agg.get_anime_episodes(external_anime_id).await
    }

    /// Fetch episodes without persisting them (used for preview flows)
    pub async fn preview_anime_episodes(&self, external_anime_id: &str) -> Result<Vec<Unit>> {
        self.agg.preview_anime_episodes(external_anime_id).await
    }

    /// Fetch episode streams for an episode id; persists streams (dedupe by (episode_id, url)).
    pub async fn get_episode_streams(&self, external_episode_id: &str) -> Result<Vec<Asset>> {
        self.agg.get_episode_streams(external_episode_id).await
    }

    /// Fetch chapter images (URLs) with caching and optional refresh. Accepts canonical or external chapter id.
    pub async fn get_chapter_images_with_refresh(
        &self,
        chapter_id: &str,
        refresh: bool,
    ) -> Result<Vec<String>> {
        self.agg
            .get_chapter_images_with_refresh(chapter_id, refresh)
            .await
    }

    // Convenience: accepts canonical or external chapter id
    pub async fn get_chapter_images(&self, chapter_id: &str) -> Result<Vec<String>> {
        self.agg.get_chapter_images(chapter_id).await
    }

    // --- Series management APIs ---

    pub async fn list_series(&self, kind: Option<&str>) -> Result<Vec<(String, String)>> {
        let pool = self.agg.database().pool().clone();
        crate::dao::list_series(&pool, kind).await
    }

    pub async fn list_chapters_for_series(
        &self,
        series_id: &str,
    ) -> Result<Vec<(String, Option<f64>, Option<String>, Option<String>, Option<String>)>> {
        let pool = self.agg.database().pool().clone();
        crate::dao::list_chapters_for_series(&pool, series_id).await
    }

    pub async fn list_episodes_for_series(
        &self,
        series_id: &str,
    ) -> Result<Vec<(String, Option<f64>, Option<String>, Option<String>, Option<String>)>> {
        let pool = self.agg.database().pool().clone();
        crate::dao::list_episodes_for_series(&pool, series_id).await
    }

    pub async fn get_chapter_progress(&self, chapter_id: &str) -> Result<Option<ChapterProgress>> {
        let pool = self.agg.database().pool().clone();
        if let Some((canonical_id, _series_id)) =
            crate::dao::find_chapter_identity(&pool, chapter_id).await?
        {
            crate::dao::get_chapter_progress(&pool, &canonical_id).await
        } else {
            Ok(None)
        }
    }

    pub async fn get_chapter_progress_for_series(
        &self,
        series_id: &str,
    ) -> Result<Vec<ChapterProgress>> {
        let pool = self.agg.database().pool().clone();
        crate::dao::get_chapter_progress_for_series(&pool, series_id).await
    }

    pub async fn set_chapter_progress(
        &self,
        chapter_id: &str,
        page_index: i64,
        total_pages: Option<i64>,
    ) -> Result<()> {
        let pool = self.agg.database().pool().clone();
        if let Some((canonical_id, series_id)) =
            crate::dao::find_chapter_identity(&pool, chapter_id).await?
        {
            crate::dao::upsert_chapter_progress(
                &pool,
                &canonical_id,
                &series_id,
                page_index,
                total_pages,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn clear_chapter_progress(&self, chapter_id: &str) -> Result<()> {
        let pool = self.agg.database().pool().clone();
        if let Some((canonical_id, _series_id)) =
            crate::dao::find_chapter_identity(&pool, chapter_id).await?
        {
            let _ = crate::dao::clear_chapter_progress(&pool, &canonical_id).await?;
        }
        Ok(())
    }

    pub async fn clear_series_progress(&self, series_id: &str) -> Result<u64> {
        let pool = self.agg.database().pool().clone();
        let deleted = sqlx::query("DELETE FROM chapter_progress WHERE series_id = ?")
            .bind(series_id)
            .execute(&pool)
            .await?
            .rows_affected();
        Ok(deleted)
    }

    pub async fn get_series_download_path(&self, series_id: &str) -> Result<Option<String>> {
        let pool = self.agg.database().pool().clone();
        Ok(crate::dao::get_series_pref(&pool, series_id)
            .await?
            .and_then(|p| p.download_path))
    }

    pub async fn set_series_download_path(
        &self,
        series_id: &str,
        path: Option<&str>,
    ) -> Result<()> {
        let pool = self.agg.database().pool().clone();
        crate::dao::set_series_download_path(&pool, series_id, path).await
    }

    pub async fn delete_series(&self, series_id: &str) -> Result<u64> {
        let pool = self.agg.database().pool().clone();
        crate::dao::delete_series(&pool, series_id).await
    }

    pub async fn delete_chapter(&self, chapter_id: &str) -> Result<u64> {
        let pool = self.agg.database().pool().clone();
        crate::dao::delete_chapter(&pool, chapter_id).await
    }

    pub async fn delete_episode(&self, episode_id: &str) -> Result<u64> {
        let pool = self.agg.database().pool().clone();
        crate::dao::delete_episode(&pool, episode_id).await
    }

    /// Resolve the canonical series id from a source id and the plugin's external media id
    pub async fn resolve_series_id(
        &self,
        source_id: &str,
        external_id: &str,
    ) -> Result<Option<String>> {
        let pool = self.agg.database().pool().clone();
        crate::dao::find_series_id_by_source_external(&pool, source_id, external_id).await
    }

    /// Get series_id and naming info for a chapter
    pub async fn get_chapter_meta(
        &self,
        chapter_id: &str,
    ) -> Result<Option<(String, Option<f64>, Option<String>)>> {
        let pool = self.agg.database().pool().clone();
        // Try canonical id first
        let row: Option<(String, Option<f64>, Option<String>)> =
            sqlx::query_as("SELECT series_id, number_num, number_text FROM chapters WHERE id = ?")
                .bind(chapter_id)
                .fetch_optional(&pool)
                .await?;
        if row.is_some() {
            return Ok(row);
        }
        // Fallback: treat provided id as external id
        let row2: Option<(String, Option<f64>, Option<String>)> = sqlx::query_as(
            "SELECT series_id, number_num, number_text FROM chapters WHERE external_id = ?",
        )
        .bind(chapter_id)
        .fetch_optional(&pool)
        .await?;
        Ok(row2)
    }

    /// Get series_id and naming info for an episode
    pub async fn get_episode_meta(
        &self,
        episode_id: &str,
    ) -> Result<Option<(String, Option<f64>, Option<String>)>> {
        let pool = self.agg.database().pool().clone();
        let row: Option<(String, Option<f64>, Option<String>)> =
            sqlx::query_as("SELECT series_id, number_num, number_text FROM episodes WHERE id = ?")
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
        self.agg
            .clear_cache_prefix(prefix)
            .await
            .map_err(Into::into)
    }

    /// Vacuum/compact the database (SQLite only; no-op on others).
    pub async fn vacuum_db(&self) -> Result<()> {
        self.agg.vacuum_db().await
    }

    /// Clear all data from the database (WARNING: This deletes all series, chapters, episodes, and sources).
    /// Returns the number of series deleted (chapters/episodes cascade automatically via foreign keys).
    pub async fn clear_database(&self) -> Result<u64> {
        let pool = self.agg.database().pool().clone();

        // Delete all series first (this will cascade to chapters and episodes via foreign keys)
        let series_deleted = sqlx::query("DELETE FROM series")
            .execute(&pool)
            .await?
            .rows_affected();

        // Delete all sources
        sqlx::query("DELETE FROM sources").execute(&pool).await?;

        // Clear cache as well
        self.clear_cache_prefix(None).await?;

        Ok(series_deleted)
    }

    // --- Download API for UI ---

    /// Download chapter images to a directory. Returns number of images downloaded.
    pub async fn download_chapter_images(
        &self,
        chapter_id: &str,
        output_dir: &Path,
        force_overwrite: bool,
    ) -> Result<usize> {
        let urls = self
            .get_chapter_images_with_refresh(chapter_id, false)
            .await?;
        if urls.is_empty() {
            return Ok(0);
        }

        tokio::fs::create_dir_all(output_dir).await.ok();
        let client = reqwest::Client::builder()
            .user_agent("touring/0.1")
            .build()?;
        let mut downloaded = 0;

        for (i, url) in urls.iter().enumerate() {
            if url.starts_with("mock://") {
                let fname = format!("{:04}.jpg", i + 1);
                let path = output_dir.join(fname);
                if !force_overwrite && tokio::fs::try_exists(&path).await.unwrap_or(false) {
                    continue;
                }
                tokio::fs::write(&path, b"MOCK").await?;
                downloaded += 1;
                continue;
            }

            let fname = format!("{:04}.jpg", i + 1);
            let path = output_dir.join(fname);
            if !force_overwrite && tokio::fs::try_exists(&path).await.unwrap_or(false) {
                continue;
            }

            let resp = client.get(url).send().await?;
            if !resp.status().is_success() {
                continue;
            }
            let bytes = resp.bytes().await?;
            tokio::fs::write(&path, &bytes).await?;
            downloaded += 1;
        }
        Ok(downloaded)
    }

    /// Download chapter as CBZ archive. Returns true if downloaded successfully.
    pub async fn download_chapter_cbz(
        &self,
        chapter_id: &str,
        output_file: &Path,
        force_overwrite: bool,
    ) -> Result<bool> {
        if !force_overwrite && tokio::fs::try_exists(output_file).await.unwrap_or(false) {
            return Ok(false);
        }

        let urls = self
            .get_chapter_images_with_refresh(chapter_id, false)
            .await?;
        if urls.is_empty() {
            return Ok(false);
        }

        let tmp_dir = output_file.with_extension("tmpdir");
        let downloaded = self
            .download_chapter_images(chapter_id, &tmp_dir, true)
            .await?;
        if downloaded == 0 {
            return Ok(false);
        }

        // Create CBZ
        let file = std::fs::File::create(output_file)?;
        let mut zip = zip::ZipWriter::new(file);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        let mut entries: Vec<_> = std::fs::read_dir(&tmp_dir)?
            .filter_map(|e| e.ok())
            .collect();
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
    pub async fn download_series_chapters(
        &self,
        series_id: &str,
        base_dir: &Path,
        as_cbz: bool,
        force_overwrite: bool,
    ) -> Result<(usize, usize)> {
        let chapters = self.list_chapters_for_series(series_id).await?;
        let mut processed = 0;
        let mut downloaded = 0;

        tokio::fs::create_dir_all(base_dir).await.ok();

        for (chapter_id, number_num, number_text, upload_group, _title) in chapters {
            processed += 1;
            let name = number_text
                .or_else(|| number_num.map(|n| format!("{:.3}", n)))
                .unwrap_or_else(|| format!("chapter_{}", processed));

            if as_cbz {
                let output_file = base_dir.join(format!("{}.cbz", name));
                if self
                    .download_chapter_cbz(&chapter_id, &output_file, force_overwrite)
                    .await?
                {
                    downloaded += 1;
                }
            } else {
                let output_dir = base_dir.join(name);
                let count = self
                    .download_chapter_images(&chapter_id, &output_dir, force_overwrite)
                    .await?;
                if count > 0 {
                    downloaded += 1;
                }
            }
        }

        Ok((processed, downloaded))
    }

    /// Download series with progress callback. Callback receives (current, total, item_name).
    pub async fn download_series_chapters_with_progress<F>(
        &self,
        series_id: &str,
        base_dir: &Path,
        as_cbz: bool,
        force_overwrite: bool,
        mut progress_callback: F,
    ) -> Result<DownloadResult>
    where
        F: FnMut(DownloadProgress),
    {
        let chapters = self.list_chapters_for_series(series_id).await?;
        let total = chapters.len();
        let mut processed = 0;
        let mut downloaded = 0;

        tokio::fs::create_dir_all(base_dir).await.ok();

        for (chapter_id, number_num, number_text, upload_group, _title) in chapters {
            processed += 1;
            let name = number_text
                .or_else(|| number_num.map(|n| format!("{:.3}", n)))
                .unwrap_or_else(|| format!("chapter_{}", processed));

            progress_callback(DownloadProgress {
                current: processed,
                total,
                current_item: name.clone(),
            });

            let success = if as_cbz {
                let output_file = base_dir.join(format!("{}.cbz", name));
                self.download_chapter_cbz(&chapter_id, &output_file, force_overwrite)
                    .await
                    .unwrap_or(false)
            } else {
                let output_dir = base_dir.join(name);
                let count = self
                    .download_chapter_images(&chapter_id, &output_dir, force_overwrite)
                    .await
                    .unwrap_or(0);
                count > 0
            };

            if success {
                downloaded += 1;
            }
        }

        Ok(DownloadResult {
            success: true,
            items_processed: processed,
            items_downloaded: downloaded,
            error: None,
        })
    }

    /// Get download status for a series (how many chapters are already downloaded).
    pub async fn get_series_download_status(
        &self,
        series_id: &str,
        base_dir: &Path,
        as_cbz: bool,
    ) -> Result<(usize, usize)> {
        let chapters = self.list_chapters_for_series(series_id).await?;
        let total = chapters.len();
        let mut downloaded = 0;

        for (_, number_num, number_text, upload_group) in chapters
            .iter()
            .enumerate()
            .map(|(_i, (id, num, text, upload_group, _title))| (id, num, text, upload_group))
        {
            let name = number_text
                .clone()
                .or_else(|| number_num.map(|n| format!("{:.3}", n)))
                .unwrap_or_else(|| format!("chapter_{}", downloaded + 1));

            let exists = if as_cbz {
                let output_file = base_dir.join(format!("{}.cbz", name));
                tokio::fs::try_exists(&output_file).await.unwrap_or(false)
            } else {
                let output_dir = base_dir.join(name);
                tokio::fs::try_exists(&output_dir).await.unwrap_or(false)
            };

            if exists {
                downloaded += 1;
            }
        }

        Ok((downloaded, total))
    }

    // --- Series Management API for UI ---

    /// Get full series information including metadata and preferences.
    pub async fn get_series_info(&self, series_id: &str) -> Result<Option<SeriesInfo>> {
        let pool = self.agg.database().pool().clone();
        // Use COALESCE to handle NULL values properly with sqlx::Any driver
        let row: Option<(String, String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, kind, title, COALESCE(description, ''), COALESCE(cover_url, ''), COALESCE(status, '') FROM series WHERE id = ?"
        )
        .bind(series_id)
        .fetch_optional(&pool)
        .await?;

        let Some((id, kind, title, description, cover_url, status)) = row else {
            return Ok(None);
        };

        // Convert empty strings back to None
        let description = if description.is_empty() {
            None
        } else {
            Some(description)
        };
        let cover_url = if cover_url.is_empty() {
            None
        } else {
            Some(cover_url)
        };
        let status = if status.is_empty() {
            None
        } else {
            Some(status)
        };

        let pref = crate::dao::get_series_pref(&pool, series_id).await?;
        let download_path = pref.and_then(|p| p.download_path);

        let chapters_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM chapters WHERE series_id = ?")
                .bind(series_id)
                .fetch_one(&pool)
                .await?;

        let episodes_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE series_id = ?")
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
    pub async fn update_series_metadata(
        &self,
        series_id: &str,
        updates: SeriesMetadataUpdate,
    ) -> Result<()> {
        let pool = self.agg.database().pool().clone();

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
        let pool = self.agg.database().pool().clone();
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT source_id, external_id FROM series_sources WHERE series_id = ?")
                .bind(series_id)
                .fetch_all(&pool)
                .await?;

        Ok(rows
            .into_iter()
            .map(|(source_id, external_id)| SeriesSource {
                source_id,
                external_id,
            })
            .collect())
    }

    /// Add a new source mapping for a series.
    pub async fn add_series_source(
        &self,
        series_id: &str,
        source_id: &str,
        external_id: &str,
    ) -> Result<()> {
        let pool = self.agg.database().pool().clone();
        let link = crate::dao::SeriesSourceInsert {
            series_id: series_id.to_string(),
            source_id: source_id.to_string(),
            external_id: external_id.to_string(),
        };
        crate::dao::upsert_series_source(&pool, &link).await
    }

    /// Remove a source mapping for a series.
    pub async fn remove_series_source(
        &self,
        series_id: &str,
        source_id: &str,
        external_id: &str,
    ) -> Result<u64> {
        let pool = self.agg.database().pool().clone();
        let res = sqlx::query(
            "DELETE FROM series_sources WHERE series_id = ? AND source_id = ? AND external_id = ?",
        )
        .bind(series_id)
        .bind(source_id)
        .bind(external_id)
        .execute(&pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Get detailed chapter information including download status.
    pub async fn get_chapter_info(&self, chapter_id: &str) -> Result<Option<ChapterInfo>> {
        let pool = self.agg.database().pool().clone();
        let row: Option<(String, String, String, Option<String>, Option<f64>, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, series_id, external_id, number_text, number_num, title, lang, volume FROM chapters WHERE id = ?"
        )
        .bind(chapter_id)
        .fetch_optional(&pool)
        .await?;

        let Some((id, series_id, external_id, number_text, number_num, title, lang, volume)) = row
        else {
            return Ok(None);
        };

        // Check if images are cached
        let images = self
            .get_chapter_images(chapter_id)
            .await
            .unwrap_or_default();
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
        let pool = self.agg.database().pool().clone();
        let row: Option<(String, String, String, Option<String>, Option<f64>, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, series_id, external_id, number_text, number_num, title, lang, season FROM episodes WHERE id = ?"
        )
        .bind(episode_id)
        .fetch_optional(&pool)
        .await?;

        let Some((id, series_id, external_id, number_text, number_num, title, lang, season)) = row
        else {
            return Ok(None);
        };

        // Check for streams
        let stream_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM streams WHERE episode_id = ?")
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
    pub async fn search_local_series(
        &self,
        query: &str,
        kind: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<SeriesInfo>> {
        let pool = self.agg.database().pool().clone();
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

            let chapters_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM chapters WHERE series_id = ?")
                    .bind(&id)
                    .fetch_one(&pool)
                    .await?;

            let episodes_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE series_id = ?")
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
        let pool = self.agg.database().pool().clone();

        let total_series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
            .fetch_one(&pool)
            .await?;
        let manga_series: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE kind = 'manga'")
                .fetch_one(&pool)
                .await?;
        let anime_series: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE kind = 'anime'")
                .fetch_one(&pool)
                .await?;
        let total_chapters: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapters")
            .fetch_one(&pool)
            .await?;
        let total_episodes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episodes")
            .fetch_one(&pool)
            .await?;
        let total_sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources")
            .fetch_one(&pool)
            .await?;

        // Cache stats
        let cache_entries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cache")
            .fetch_one(&pool)
            .await?;
        let expired_cache: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM cache WHERE expires_at < ?")
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
            // Try to fetch fresh metadata from the source (manga domain)
            let media_list = self
                .agg
                .search_manga_cached_with_sources(&source.external_id, true)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|(s, _)| s == &source.source_id)
                .map(|(_, m)| m)
                .collect::<Vec<_>>();

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
}

impl Touring {
    /// Direct access to underlying Aggregator (advanced use).
    pub fn aggregator(&self) -> &Aggregator {
        &self.agg
    }
}

// Local helper needed for stats (avoid reaching into aggregator internals)
fn current_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
