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
        Commands::Chapters { manga_id } => {
            println!("Fetching chapters for manga ID: {}", manga_id);
            if pm.list_plugins().is_empty() {
                eprintln!("No plugins loaded");
                return Ok(());
            }
            match pm.get_manga_chapters(&manga_id) {
                Ok(chapters) => {
                    if chapters.is_empty() {
                        println!("No chapters found for manga ID: {}", manga_id);
                    } else {
                        println!("Found {} chapters for manga {}:", chapters.len(), manga_id);
                        for chapter in chapters {
                            println!("  {}: {}", chapter.id, chapter.title);
                            if let Some(description) = &chapter.description {
                                println!("    {}", description);
                            }
                        }
                    }
                }
                Err(e) => eprintln!("Error fetching chapters: {}", e),
            }
        }
        Commands::Chapter { chapter_id } => {
            println!("Fetching chapter images for chapter ID: {}", chapter_id);
            if pm.list_plugins().is_empty() {
                eprintln!("No plugins loaded");
                return Ok(());
            }
            match pm.get_chapter_images(&chapter_id) {
                Ok(image_urls) => {
                    if image_urls.is_empty() {
                        println!("No images found for chapter ID: {}", chapter_id);
                    } else {
                        println!("Found {} images for chapter {}:", image_urls.len(), chapter_id);
                        for (index, url) in image_urls.iter().enumerate() {
                            println!("  {}: {}", index + 1, url);
                        }
                    }
                }
                Err(e) => eprintln!("Error fetching chapter images: {}", e),
            }
        }
    }
    Ok(())
}