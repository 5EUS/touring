use anyhow::{Context, Result};
use directories::ProjectDirs;
use sqlx::{any::AnyConnectOptions, AnyPool, ConnectOptions, migrate::Migrator};
use sqlx::any::AnyPoolOptions;
use std::{path::PathBuf, str::FromStr};
use std::sync::Once;

// Ensure drivers are installed exactly once for sqlx::any
static INSTALL_DRIVERS: Once = Once::new();

// Embed SQL migrations from the migrations/ directory
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

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
        MIGRATOR.run(&self.pool).await.context("running migrations")
    }

    pub fn pool(&self) -> &AnyPool { &self.pool }
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
    if path_str.contains(' ') {
        path_str = path_str.replace(' ', "%20");
    }
    Ok(format!("sqlite:///{path_str}?mode=rwc"))
}
