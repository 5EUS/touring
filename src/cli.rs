use clap::{Parser, Subcommand};

/// Extensible CLI for debugging and development
#[derive(Parser)]
#[command(name = "touring")]
#[command(about = "A CLI tool for managing plugins and sources", long_about = None)]
pub struct Cli {
    /// Database connection string (sqlite/postgres/mysql). If not provided, a sensible
    /// default is used (sqlite file in user data dir). Can also be set via TOURING_DATABASE_URL.
    #[arg(long = "database-url")]
    pub database_url: Option<String>,

    /// Skip running migrations on startup. Can also be set via TOURING_NO_MIGRATIONS.
    #[arg(long = "no-migrations", default_value_t = false)]
    pub no_migrations: bool,

    /// Directory to load plugins (.wasm) from. Can also be set via TOURING_PLUGINS_DIR.
    #[arg(long = "plugins-dir")]
    pub plugins_dir: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// List all available plugins
    Plugins {
        /// Filter plugins by name
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Show plugin capabilities (cached by default)
    Capabilities {
        /// Refresh capabilities by calling each plugin
        #[arg(long)]
        refresh: bool,
    },
    /// Search for manga
    Manga {
        /// Query to search for
        query: String,
        /// Bypass cache and force refresh
        #[arg(long)]
        refresh: bool,
        /// Output JSON for machine readability
        #[arg(long)]
        json: bool,
    },
    /// Search for anime
    Anime {
        /// Query to search for
        query: String,
        /// Bypass cache and force refresh
        #[arg(long)]
        refresh: bool,
        /// Output JSON for machine readability
        #[arg(long)]
        json: bool,
    },
    /// Get chapters for a specific manga
    Chapters {
        /// Manga ID to get chapters for
        manga_id: String,
    },
    /// Get episodes for a specific anime
    Episodes {
        /// Anime ID to get episodes for
        anime_id: String,
    },
    /// Get chapter images
    Chapter {
        /// Chapter ID to retrieve images for
        chapter_id: String,
        /// Bypass cache and force refresh
        #[arg(long)]
        refresh: bool,
    },
    /// Get video streams for an episode
    Streams {
        /// Episode ID to retrieve streams for
        episode_id: String,
    },
    /// Refresh cache for a given key prefix (e.g., search) by forcing refresh on next access
    RefreshCache {
        /// Optional key prefix to clear (defaults to all)
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Vacuum/compact the database (SQLite only; no-op for others)
    VacuumDb,
    /// Download helpers
    Download {
        #[command(subcommand)]
        cmd: DownloadCmd,
    },
    /// Manage series stored in the database
    Series {
        #[command(subcommand)]
        cmd: SeriesCmd,
    },
}

#[derive(Subcommand)]
pub enum DownloadCmd {
    /// Download all images for a chapter to a directory (or a cbz file)
    Chapter {
        /// Chapter ID
        chapter_id: String,
        /// Output directory (created if missing). If --cbz is used, this is the .cbz path.
        #[arg(long)]
        out: String,
        /// Create a .cbz instead of files on disk
        #[arg(long)]
        cbz: bool,
        /// Overwrite existing files
        #[arg(long)]
        force: bool,
    },
    /// Download a video stream (HLS/DASH not yet muxed) to a file
    Episode {
        /// Episode ID
        episode_id: String,
        /// Output file
        #[arg(long)]
        out: String,
        /// Select stream by index (default 0)
        #[arg(long, default_value_t = 0)]
        index: usize,
    },
}

#[derive(Subcommand)]
pub enum SeriesCmd {
    /// List series (optionally by kind)
    List {
        /// Filter series by kind (e.g., manga, anime)
        #[arg(long)]
        kind: Option<String>,
    },
    /// Set or clear the download path for a series
    SetPath {
        /// Series ID to set the download path for
        series_id: String,
        /// New download path (leave empty to clear)
        #[arg(long)]
        path: Option<String>,
    },
    /// Delete a series (cascades to chapters/episodes/streams/images)
    Delete {
        /// Series ID to delete
        series_id: String,
    },
    /// Delete a single chapter by id
    DeleteChapter {
        /// Chapter ID to delete
        chapter_id: String,
    },
    /// Delete a single episode by id
    DeleteEpisode {
        /// Episode ID to delete
        episode_id: String,
    },
}