use anyhow::Result;
use std::path::Path;

use crate::db::Database;
use crate::plugins::{PluginManager, Media, Unit, UnitKind, MediaType};
use crate::storage::Storage;
use crate::dao;
use crate::mapping::{series_insert_from_media, series_source_from, chapter_insert_from_unit};

/// Aggregator decouples media aggregation and persistence from the CLI/backend.
/// It owns the database and the plugin manager and provides a narrow API.
pub struct Aggregator {
    db: Database,
    pm: PluginManager,
    rt: tokio::runtime::Runtime,
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
        Ok(Self { db, pm, rt })
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
                    let cid = uuid::Uuid::new_v4().to_string();
                    let ch = chapter_insert_from_unit(&cid, &series_id, &source_id, u);
                    let _ = dao::upsert_chapter(&pool, &ch).await;
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
        Ok(units)
    }

    /// Fetch chapter images (URLs) for a chapter id with caching via Storage trait.
    pub fn get_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
        let key = format!("all|pages|{}", chapter_id);
        let now = current_epoch();

        // Try cache
        if let Some(payload) = self.rt.block_on(self.db.get_cache(&key, now)).ok().flatten() {
            if let Ok(urls) = serde_json::from_str::<Vec<String>>(&payload) {
                return Ok(urls);
            }
        }

        // Miss -> plugins
        let (_src_opt, urls) = self.pm.get_chapter_images_with_source(chapter_id)?;

        // Write-through with TTL
        let payload = serde_json::to_string(&urls)?;
        let expires_at = now + 24 * 60 * 60;
        let _ = self.rt.block_on(self.db.put_cache(&key, &payload, expires_at));

        Ok(urls)
    }

    /// Access to the underlying database for future extensions.
    #[allow(dead_code)]
    pub fn database(&self) -> &Database { &self.db }

    /// Example upsert hooks (to be called from future cache-integrated paths)
    pub fn upsert_source(&self, id: &str, version: &str) -> Result<()> {
        let pool = self.db.pool().clone();
        self.rt.block_on(async move { dao::upsert_source(&pool, &dao::SourceInsert { id: id.to_string(), version: version.to_string() }).await })
    }
}

fn current_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
