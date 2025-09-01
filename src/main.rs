mod cli;
mod plugins;
mod db;
mod aggregator;
mod storage;
mod dao;
mod mapping;

use cli::{Cli, Commands};
use clap::Parser;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create runtime only for async plugin loading
    let rt = tokio::runtime::Runtime::new()?;

    let mut cli = Cli::parse();

    // Fallback to environment variables if CLI flags not provided
    if cli.database_url.is_none() {
        if let Ok(v) = std::env::var("TOURING_DATABASE_URL") { if !v.is_empty() { cli.database_url = Some(v); } }
    }
    if !cli.no_migrations {
        if let Ok(v) = std::env::var("TOURING_NO_MIGRATIONS") {
            let v = v.to_ascii_lowercase();
            if v == "1" || v == "true" || v == "yes" { cli.no_migrations = true; }
        }
    }

    // Initialize Aggregator synchronously (it owns an internal runtime for DB I/O)
    let mut agg = aggregator::Aggregator::new(cli.database_url.as_deref(), !cli.no_migrations)?;

    // Load plugins with the outer runtime
    rt.block_on(async { agg.load_plugins_from_directory(Path::new("plugins")).await })?;

    match cli.command {
        Commands::Plugins { name } => {
            if let Some(name) = name {
                println!("Filtering plugins by name: {}", name);
                for plugin_name in agg.list_plugins().into_iter().filter(|p| p.contains(&name)) {
                    println!("  - {}", plugin_name);
                }
            } else {
                println!("Listing all plugins...");
                for plugin_name in agg.list_plugins() { println!("  - {}", plugin_name); }
            }
        }
        Commands::Manga { query } => {
            println!("Fetching manga list for query: {}", query);
            if agg.list_plugins().is_empty() { eprintln!("No plugins loaded"); return Ok(()); }
            match agg.search_manga(&query) {
                Ok(manga_list) => {
                    for manga in manga_list {
                        println!("Manga: {} (ID: {})", manga.title, manga.id);
                        if let Some(description) = &manga.description { println!("  Description: {}", description); }
                        if let Some(url) = &manga.url { println!("  URL: {}", url); }
                        if let Some(cover) = &manga.cover_url { println!("  Cover: {}", cover); }
                    }
                }
                Err(e) => eprintln!("Error fetching manga list: {}", e),
            }
        }
        Commands::Chapters { manga_id } => {
            println!("Fetching chapters for manga ID: {}", manga_id);
            if agg.list_plugins().is_empty() { eprintln!("No plugins loaded"); return Ok(()); }
            match agg.get_manga_chapters(&manga_id) {
                Ok(units) => {
                    if units.is_empty() { println!("No chapters found for manga ID: {}", manga_id); }
                    else {
                        println!("Found {} chapters for manga {}:", units.len(), manga_id);
                        for u in units {
                            let num = u.number.map(|n| n.to_string()).or(u.number_text.clone()).unwrap_or_default();
                            println!("  {}: {}{}", u.id, if num.is_empty() { "".to_string() } else { format!("Ch. {} ", num) }, u.title);
                            if let Some(lang) = &u.lang { println!("    lang: {}", lang); }
                            if let Some(g) = &u.group { println!("    group: {}", g); }
                            if let Some(p) = &u.published_at { println!("    published: {}", p); }
                            if let Some(uurl) = &u.url { println!("    url: {}", uurl); }
                        }
                    }
                }
                Err(e) => eprintln!("Error fetching chapters: {}", e),
            }
        }
        Commands::Chapter { chapter_id } => {
            println!("Fetching chapter images for chapter ID: {}", chapter_id);
            if agg.list_plugins().is_empty() { eprintln!("No plugins loaded"); return Ok(()); }
            match agg.get_chapter_images(&chapter_id) {
                Ok(image_urls) => {
                    if image_urls.is_empty() { println!("No images found for chapter ID: {}", chapter_id); }
                    else {
                        println!("Found {} images for chapter {}:", image_urls.len(), chapter_id);
                        for (index, url) in image_urls.iter().enumerate() { println!("  {}: {}", index + 1, url); }
                    }
                }
                Err(e) => eprintln!("Error fetching chapter images: {}", e),
            }
        }
    }

    Ok(())
}