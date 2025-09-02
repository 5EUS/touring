use anyhow::{Context, Result};
use directories::ProjectDirs;
use sqlx::{any::AnyConnectOptions, AnyPool, ConnectOptions, migrate::Migrator};
use sqlx::any::AnyPoolOptions;
use std::{path::PathBuf, str::FromStr};
use std::sync::Once;

use crate::storage::Storage;

// Ensure drivers are installed exactly once for sqlx::any
static INSTALL_DRIVERS: Once = Once::new();

// Embed SQL migrations from the migrations/ directory
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct Database {
    pool: AnyPool,
}

impl Database {
    // Create a connection pool. If database_url is None, use a sensible default
    // (SQLite file in the user's data directory).
    pub async fn connect(database_url: Option<&str>) -> Result<Self> {
        // Register compiled-in drivers for sqlx::any
        INSTALL_DRIVERS.call_once(|| sqlx::any::install_default_drivers());

        let url = match database_url {
            Some(u) if !u.trim().is_empty() => u.to_string(),
            _ => default_sqlite_url()?,
        };

        // Parse options to tweak connection settings (e.g., logging)
        let opts = AnyConnectOptions::from_str(&url)
            .with_context(|| format!("invalid database URL: {url}"))?;
        // Quiet by default; callers can enable SQLX_LOG if they want
        let opts = opts.disable_statement_logging();

        let pool = AnyPoolOptions::new()
            .max_connections(10)
            .connect_with(opts)
            .await
            .with_context(|| format!("failed to connect to database: {url}"))?;

        Ok(Self { pool })
    }

    pub async fn run_migrations(&self) -> Result<()> {
        match MIGRATOR.run(&self.pool).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                let looks_modified = msg.contains("was previously applied but has been modified");
                let duplicate_version = msg.contains("UNIQUE constraint failed: _sqlx_migrations.version");
                if looks_modified || duplicate_version {
                    let _ = sqlx::query("DELETE FROM _sqlx_migrations").execute(&self.pool).await;
                    MIGRATOR.run(&self.pool).await.context("running migrations after ledger reset")
                } else {
                    Err(e).context("running migrations")
                }
            }
        }
    }

    pub fn pool(&self) -> &AnyPool { &self.pool }

    pub async fn clear_cache_prefix(&self, prefix: Option<&str>) -> Result<u64> {
        let result = if let Some(p) = prefix {
            let like = format!("{}%", p);
            sqlx::query("DELETE FROM search_cache WHERE key LIKE ?")
                .bind(like)
                .execute(&self.pool)
                .await?
        } else {
            sqlx::query("DELETE FROM search_cache")
                .execute(&self.pool)
                .await?
        };
        Ok(result.rows_affected())
    }

    pub async fn vacuum(&self) -> Result<()> {
        // Best-effort: works on SQLite
        let _ = sqlx::query("VACUUM").execute(&self.pool).await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Storage for Database {
    async fn get_cache(&self, key: &str, now: i64) -> Result<Option<String>> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT payload FROM search_cache WHERE key = ? AND expires_at > ?",
        )
        .bind(key)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn put_cache(&self, key: &str, payload: &str, expires_at: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO search_cache(key, payload, expires_at) VALUES (?, ?, ?)\n             ON CONFLICT(key) DO UPDATE SET payload=excluded.payload, expires_at=excluded.expires_at",
        )
        .bind(key)
        .bind(payload)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn default_sqlite_url() -> Result<String> {
    let proj = ProjectDirs::from("dev", "touring", "touring")
        .context("unable to determine data directory for default sqlite path")?;
    let mut path: PathBuf = proj.data_dir().to_path_buf();
    std::fs::create_dir_all(&path).with_context(|| format!("creating data dir: {}", path.display()))?;
    path.push("touring.db");

    // Ensure parent directory exists (double safety)
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating db parent dir: {}", parent.display()))?;
    }

    // Ensure the file exists so SQLite can open it in rw mode
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&path);

    // Encode spaces in the path for a valid sqlite URL
    let mut path_str = path.to_string_lossy().to_string();
    if path_str.contains(' ') { path_str = path_str.replace(' ', "%20"); }
    Ok(format!("sqlite:///{path_str}?mode=rwc"))
}
