use anyhow::Result;
use std::path::Path;

pub mod wasmsource;
pub mod source;
pub mod plugins;

fn main() -> Result<()> {
    let mut pm = plugins::PluginManager::new()?;
    pm.load_plugins_from_directory(Path::new("plugins"))?;

    // Manga example
    println!("=== MANGA SEARCH ===");
    let manga_list = pm.search_manga("naruto")?;
    for m in &manga_list {
        println!("Found manga: {} (ID: {})", m.title, m.id);
        if let Some(d) = &m.description {
            println!("  Description: {}", d);
        }
        if let Some(u) = &m.url {
            println!("  URL: {}", u);
        }
        println!("  Type: {:?}", m.mediatype);
    }

    if let Some(first) = manga_list.first() {
        println!("\nFetching chapter images for manga: {}", first.title);
        let images = pm.get_chapter_images(&first.id)?;
        for (i, url) in images.iter().enumerate() {
            println!("  Image {}: {}", i + 1, url);
        }
    }

    // Anime example
    println!("\n=== ANIME SEARCH ===");
    let anime_list = pm.search_anime("bleach")?;
    for a in &anime_list {
        println!("Found anime: {} (ID: {})", a.title, a.id);
        if let Some(d) = &a.description {
            println!("  Description: {}", d);
        }
        if let Some(u) = &a.url {
            println!("  URL: {}", u);
        }
        println!("  Type: {:?}", a.mediatype);
    }

    if let Some(first_anime) = anime_list.first() {
        println!("\nFetching episodes for anime: {}", first_anime.title);
        let episodes = pm.get_anime_episodes(&first_anime.id)?;
        for ep in episodes.iter().take(5) { // Show first 5 episodes
            println!("  Episode: {} (ID: {})", ep.title, ep.id);
            if let Some(num) = ep.number {
                println!("    Number: {}", num);
            }
            if let Some(url) = &ep.url {
                println!("    URL: {}", url);
            }
        }

        // Get streams for first episode
        if let Some(first_ep) = episodes.first() {
            println!("\nFetching streams for episode: {}", first_ep.title);
            let streams = pm.get_episode_streams(&first_ep.id)?;
            for stream in &streams {
                println!("  Stream URL: {}", stream.url);
                if let Some(quality) = &stream.quality {
                    println!("    Quality: {}", quality);
                }
                if let Some(mime) = &stream.mime {
                    println!("    MIME: {}", mime);
                }
            }
        }
    }

    Ok(())
}