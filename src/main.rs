mod cli;

use cli::{Cli, Commands};
use clap::Parser;
use std::path::Path;
use touring::prelude::MediaType;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create runtime for async library API and plugin loading
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
    if cli.plugins_dir.is_none() {
        if let Ok(v) = std::env::var("TOURING_PLUGINS_DIR") { if !v.is_empty() { cli.plugins_dir = Some(v); } }
    }

    // Initialize library API
    let touring = rt.block_on(touring::Touring::connect(cli.database_url.as_deref(), !cli.no_migrations))?;

    // Load plugins with the outer runtime
    let plugins_dir = cli.plugins_dir.clone().unwrap_or_else(|| "plugins".to_string());
    rt.block_on(async { touring.load_plugins_from_directory(Path::new(&plugins_dir)).await })?;

    match cli.command {
        Commands::Plugins { name } => {
            let list = touring.list_plugins();
            if let Some(name) = name {
                println!("Filtering plugins by name: {}", name);
                for plugin_name in list.into_iter().filter(|p| p.contains(&name)) {
                    println!("  - {}", plugin_name);
                }
            } else {
                println!("Listing all plugins...");
                for plugin_name in list { println!("  - {}", plugin_name); }
            }
        }
        Commands::Capabilities { refresh } => {
            let caps = rt.block_on(touring.get_capabilities(refresh))?;
            for (name, c) in caps {
                let media: Vec<String> = c.media_types.into_iter().map(|m| format!("{:?}", m)).collect();
                let units: Vec<String> = c.unit_kinds.into_iter().map(|u| format!("{:?}", u)).collect();
                let assets: Vec<String> = c.asset_kinds.into_iter().map(|a| format!("{:?}", a)).collect();
                println!("{}:\n  media:  {}\n  units:  {}\n  assets: {}", name, media.join(", "), units.join(", "), assets.join(", "));
            }
        }
        Commands::Manga { query, refresh, json } => {
            let pairs = rt.block_on(touring.search_manga_cached_with_sources(&query, refresh))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&pairs.iter().map(|(src, m)| {
                    let mt = match &m.mediatype { MediaType::Manga => "manga", MediaType::Anime => "anime", MediaType::Other(_) => "other" };
                    serde_json::json!({
                        "source": src,
                        "id": m.id,
                        "title": m.title,
                        "description": m.description,
                        "url": m.url,
                        "cover_url": m.cover_url,
                        "mediatype": mt,
                    })
                }).collect::<Vec<_>>())?);
            } else {
                println!("Fetching manga list for query: {}{}", query, if refresh { " (refresh)" } else { "" });
                for (src, m) in pairs {
                    println!("Manga [{}]: {} (ID: {})", src, m.title, m.id);
                    if let Some(description) = &m.description { println!("  Description: {}", description); }
                    if let Some(url) = &m.url { println!("  URL: {}", url); }
                    if let Some(cover) = &m.cover_url { println!("  Cover: {}", cover); }
                }
            }
        }
        Commands::Anime { query, refresh, json } => {
            let pairs = rt.block_on(touring.search_anime_cached_with_sources(&query, refresh))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&pairs.iter().map(|(src, m)| {
                    let mt = match &m.mediatype { MediaType::Manga => "manga", MediaType::Anime => "anime", MediaType::Other(_) => "other" };
                    serde_json::json!({
                        "source": src,
                        "id": m.id,
                        "title": m.title,
                        "description": m.description,
                        "url": m.url,
                        "cover_url": m.cover_url,
                        "mediatype": mt,
                    })
                }).collect::<Vec<_>>())?);
            } else {
                println!("Fetching anime list for query: {}{}", query, if refresh { " (refresh)" } else { "" });
                for (src, m) in pairs { println!("Anime [{}]: {} (ID: {})", src, m.title, m.id); }
            }
        }
        Commands::Chapters { manga_id } => {
            println!("Fetching chapters for manga ID: {}", manga_id);
            let units = rt.block_on(touring.get_manga_chapters(&manga_id))?;
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
        Commands::Episodes { anime_id } => {
            println!("Fetching episodes for anime ID: {}", anime_id);
            let units = rt.block_on(touring.get_anime_episodes(&anime_id))?;
            if units.is_empty() { println!("No episodes found for anime ID: {}", anime_id); }
            else {
                println!("Found {} episodes for anime {}:", units.len(), anime_id);
                for u in units {
                    let num = u.number.map(|n| n.to_string()).or(u.number_text.clone()).unwrap_or_default();
                    println!("  {}: {}{}", u.id, if num.is_empty() { "".to_string() } else { format!("Ep. {} ", num) }, u.title);
                    if let Some(lang) = &u.lang { println!("    lang: {}", lang); }
                    if let Some(s) = &u.group { println!("    season: {}", s); }
                    if let Some(p) = &u.published_at { println!("    published: {}", p); }
                    if let Some(uurl) = &u.url { println!("    url: {}", uurl); }
                }
            }
        }
        Commands::Chapter { chapter_id, refresh } => {
            println!("Fetching chapter images for chapter ID: {}", chapter_id);
            let image_urls = rt.block_on(touring.get_chapter_images_with_refresh(&chapter_id, refresh))?;
            if image_urls.is_empty() { println!("No images found for chapter ID: {}", chapter_id); }
            else {
                println!("Found {} images for chapter {}:", image_urls.len(), chapter_id);
                for (index, url) in image_urls.iter().enumerate() { println!("  {}: {}", index + 1, url); }
            }
        }
        Commands::Streams { episode_id } => {
            println!("Fetching video streams for episode ID: {}", episode_id);
            let assets = rt.block_on(touring.get_episode_streams(&episode_id))?;
            if assets.is_empty() { println!("No streams found for episode ID: {}", episode_id); }
            else {
                println!("Found {} streams for episode {}:", assets.len(), episode_id);
                for a in assets { println!("  url: {}{}", a.url, a.mime.as_deref().map(|m| format!(" ({})", m)).unwrap_or_default()); }
            }
        }
        Commands::RefreshCache { prefix } => {
            let count = rt.block_on(touring.clear_cache_prefix(prefix.as_deref()))?;
            if let Some(p) = prefix { println!("Cleared {} cache entries with prefix '{}'.", count, p); }
            else { println!("Cleared {} cache entries.", count); }
        }
        Commands::VacuumDb => {
            rt.block_on(touring.vacuum_db())?;
            println!("Database vacuum completed.");
        }
    }

    Ok(())
}