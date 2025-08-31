mod cli;
mod plugins;
mod source;
mod wasmsource;

use std::path::Path;

use cli::{Cli, Commands};
use clap::Parser;

fn main() {
    let cli = Cli::parse();
    let mut pm = plugins::PluginManager::new().unwrap();
    pm.load_plugins_from_directory(Path::new("plugins"));

    match cli.command {
        Commands::Plugins { name } => {
            if let Some(name) = name {
                println!("Filtering plugins by name: {}", name);

            } else {
                println!("Listing all plugins...");
                // Call PluginHost logic here
            }
        }
        Commands::Source { query } => {
            if let Some(query) = query {
                println!("Fetching manga list for query: {}", query);
                match pm.search_manga(&query) {
                    Ok(manga_list) => {
                        for manga in manga_list {
                            println!("Manga: {} (ID: {})", manga.title, manga.id);
                        }
                    }
                    Err(e) => eprintln!("Error fetching manga list: {}", e),
                }
            } else {
                println!("No query provided. Listing all sources...");
                // Call Source logic here
            }
        }
        Commands::Debug { module } => {
            println!("Debugging module: {}", module);
            // Add debugging logic here
        }
    }
}