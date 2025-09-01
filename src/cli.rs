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
}