mod cli;
mod plugins;

use cli::{Cli, Commands};
use clap::Parser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create plugin manager WITHOUT tokio runtime
    let mut pm = plugins::PluginManager::new()?;
    
    // Load plugins synchronously in main thread (no HTTP conflicts during loading)
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        pm.load_plugins_from_directory(std::path::Path::new("plugins")).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    
    let cli = Cli::parse();

    match cli.command {
        Commands::Plugins { name } => {
            if let Some(name) = name {
                println!("Filtering plugins by name: {}", name);

            } else {
                println!("Listing all plugins...");
                for plugin_name in pm.list_plugins() {
                    println!("  - {}", plugin_name);
                }
            }
        }
        Commands::Manga { query } => {
            println!("Fetching manga list for query: {}", query);
            if pm.list_plugins().is_empty() {
                eprintln!("No plugins loaded");
                return Ok(());
            }
            // Execute plugins synchronously - HTTP requests will create their own runtime internally
            match pm.search_manga(&query) {
                Ok(manga_list) => {
                    for manga in manga_list {
                        println!("Manga: {} (ID: {})", manga.title, manga.id);
                        if let Some(description) = &manga.description {
                            println!("  Description: {}", description);
                        }
                        if let Some(url) = &manga.url {
                            println!("  URL: {}", url);
                        }
                    }
                }
                Err(e) => eprintln!("Error fetching manga list: {}", e),
            }
        }
    }
    Ok(())
}