use anyhow::Result;
use std::path::Path;

use crate::db::Database;
use crate::plugins::{PluginManager, Media, Unit};

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

    /// Search manga using all loaded plugins.
    pub fn search_manga(&mut self, query: &str) -> Result<Vec<Media>> { self.pm.search_manga(query) }

    /// Fetch chapters for a manga id.
    pub fn get_manga_chapters(&mut self, manga_id: &str) -> Result<Vec<Unit>> {
        self.pm.get_manga_chapters(manga_id)
    }

    /// Fetch chapter images (URLs) for a chapter id with caching.
    /// Uses search_cache with key = "all|pages|{chapter_id}", payload = JSON array of URLs,
    /// expires_at = epoch seconds. Synchronous API internally using the Aggregator runtime.
    pub fn get_chapter_images(&mut self, chapter_id: &str) -> Result<Vec<String>> {
        use sqlx::AnyPool;
        use sqlx::Row;

        let pool: &AnyPool = self.db.pool();
        let key = format!("all|pages|{}", chapter_id);

        let cached: Option<Vec<String>> = self.rt.block_on(async {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            if let Some(row) = sqlx::query("SELECT payload FROM search_cache WHERE key = ? AND expires_at > ?")
                .bind(&key)
                .bind(now)
                .fetch_optional(pool)
                .await
                .ok()? {
                let payload: String = row.try_get("payload").ok()?;
                serde_json::from_str::<Vec<String>>(&payload).ok()
            } else { None }
        });

        if let Some(urls) = cached { return Ok(urls); }

        // Cache miss -> query plugins synchronously
        let urls = self.pm.get_chapter_images(chapter_id)?;

        // Write-through cache with TTL (24h)
        let payload = serde_json::to_string(&urls)?;
        let ttl = 24 * 60 * 60; // 1 day
        self.rt.block_on(async {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let expires_at = now + ttl;
            let _ = sqlx::query(
                "INSERT INTO search_cache(key, payload, expires_at) VALUES (?, ?, ?)\n                 ON CONFLICT(key) DO UPDATE SET payload=excluded.payload, expires_at=excluded.expires_at",
            )
            .bind(&key)
            .bind(&payload)
            .bind(expires_at)
            .execute(pool)
            .await;
        });

        Ok(urls)
    }

    /// Access to the underlying database for future extensions.
    #[allow(dead_code)]
    pub fn database(&self) -> &Database { &self.db }
}
