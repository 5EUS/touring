use clap::{Parser, Subcommand};

/// Extensible CLI for debugging and development
#[derive(Parser)]
#[command(name = "touring")]
#[command(about = "A CLI tool for managing plugins and sources", long_about = None)]
pub struct Cli {
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
    /// Search for manga
    Manga {
        /// Query to search for
        query: String,
    },
}