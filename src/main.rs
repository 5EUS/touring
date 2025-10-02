mod cli;

use cli::{Cli, Commands, DownloadCmd, SeriesCmd};
use clap::Parser;
use std::path::{Path, PathBuf};
use touring::prelude::MediaType;
use std::io::Write; // for zip.write_all

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
        Commands::ResolveSeriesId { source, external_id } => {
            match rt.block_on(touring.resolve_series_id(&source, &external_id))? {
                Some(id) => println!("{}", id),
                None => println!("Not found. Make sure you've searched that media first so the series/mapping exists."),
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
        Commands::AllowedHosts => {
            let hosts = rt.block_on(touring.get_allowed_hosts())?;
            for (name, host_list) in hosts {
                if host_list.is_empty() {
                    println!("{}:\n  all hosts allowed", name);
                } else {
                    println!("{}:\n  {}", name, host_list.join(", "));
                }
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
            println!("Fetching chapters for manga ID (external): {}", manga_id);
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
            println!("Fetching episodes for anime ID (external): {}", anime_id);
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
            println!("Fetching chapter images for chapter ID (canonical or external): {}", chapter_id);
            let image_urls = rt.block_on(touring.get_chapter_images_with_refresh(&chapter_id, refresh))?;
            if image_urls.is_empty() { println!("No images found for chapter ID: {}", chapter_id); }
            else {
                println!("Found {} images for chapter {}:", image_urls.len(), chapter_id);
                for (index, url) in image_urls.iter().enumerate() { println!("  {}: {}", index + 1, url); }
            }
        }
        Commands::Streams { episode_id } => {
            println!("Fetching video streams for episode ID (canonical or external): {}", episode_id);
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
        Commands::Download { cmd } => match cmd {
            DownloadCmd::Chapter { chapter_id, out, cbz, force, mock } => {
                // In mock mode, synthesize dummy URLs/content
                let urls = if mock > 0 {
                    (1..=mock).map(|i| format!("mock://image/{:04}.jpg", i)).collect::<Vec<_>>()
                } else {
                    rt.block_on(touring.get_chapter_images(&chapter_id))?
                };
                if urls.is_empty() { println!("No images found."); return Ok(()); }

                // Determine output path
                let target = if let Some(o) = out {
                    PathBuf::from(o)
                } else {
                    match rt.block_on(touring.get_chapter_meta(&chapter_id))? {
                        Some((series_id, number_num, number_text)) => {
                            let base = match rt.block_on(touring.get_series_path(&series_id))? {
                                Some(p) => PathBuf::from(p),
                                None => {
                                    eprintln!("Error: no --out provided and no stored download_path for series {}.", series_id);
                                    return Ok(());
                                }
                            };
                            let name = number_text.or_else(|| number_num.map(|n| format!("{:.3}", n))).unwrap_or_else(|| "chapter".to_string());
                            if cbz { base.join(format!("{}.cbz", name)) } else { base.join(name) }
                        }
                        None => {
                            eprintln!("Error: chapter not found: {}", chapter_id);
                            return Ok(());
                        }
                    }
                };

                if cbz {
                    rt.block_on(save_cbz_mockable(&chapter_id, &urls, &target, force))?;
                } else {
                    rt.block_on(save_images_mockable(&chapter_id, &urls, &target, force))?;
                }
                println!("Saved {} images.", urls.len());
            }
            DownloadCmd::Episode { episode_id, out, index } => {
                let streams = rt.block_on(touring.get_episode_streams(&episode_id))?;
                if streams.is_empty() { println!("No streams found."); return Ok(()); }
                let idx = index.min(streams.len() - 1);
                let s = &streams[idx];

                // Determine output path
                let target = if let Some(o) = out {
                    PathBuf::from(o)
                } else {
                    match rt.block_on(touring.get_episode_meta(&episode_id))? {
                        Some((series_id, number_num, number_text)) => {
                            let base = match rt.block_on(touring.get_series_path(&series_id))? {
                                Some(p) => PathBuf::from(p),
                                None => {
                                    eprintln!("Error: no --out provided and no stored download_path for series {}.", series_id);
                                    return Ok(());
                                }
                            };
                            let name = number_text.or_else(|| number_num.map(|n| format!("{:.3}", n))).unwrap_or_else(|| "episode".to_string());
                            base.join(format!("{}.txt", name))
                        }
                        None => {
                            eprintln!("Error: episode not found: {}", episode_id);
                            return Ok(());
                        }
                    }
                };

                let out_path = target.clone();
                rt.block_on(async move { tokio::fs::write(out_path, s.url.as_bytes()).await })?;
                println!("Wrote stream URL to {}", target.display());
            }
            DownloadCmd::Series { series_id, out, cbz, force } => {
                // Resolve output base directory
                let base_out: PathBuf = match out {
                    Some(o) => PathBuf::from(o),
                    None => match rt.block_on(touring.get_series_download_path(&series_id))? {
                        Some(p) => PathBuf::from(p),
                        None => {
                            eprintln!(
                                "Error: no --out provided and no stored series download_path for {}.\nSet one with: touring series set-path {} --path <directory>",
                                series_id, series_id
                            );
                            return Ok(());
                        }
                    },
                };

                // Ensure base directory exists (for creating per-entry subdirectories/files)
                let _ = std::fs::create_dir_all(&base_out);

                // List chapters/episodes to decide kind
                let chapters = rt.block_on(touring.list_chapters_for_series(&series_id))?;
                let episodes = rt.block_on(touring.list_episodes_for_series(&series_id))?;

                if !chapters.is_empty() {
                    println!("Downloading {} chapters to {}...", chapters.len(), base_out.display());
                    for (cid, number_num, number_text) in chapters {
                        let name = number_text.clone().or_else(|| number_num.map(|n| format!("{:.3}", n))).unwrap_or_else(|| "chapter".to_string());
                        let ch_out = if cbz {
                            base_out.join(format!("{}.cbz", name))
                        } else {
                            base_out.join(name)
                        };
                        let urls = rt.block_on(touring.get_chapter_images(&cid))?;
                        if urls.is_empty() { continue; }
                        if cbz {
                            rt.block_on(save_cbz(&cid, &urls, &ch_out, force))?;
                        } else {
                            rt.block_on(save_images(&cid, &urls, &ch_out, force))?;
                        }
                    }
                    println!("Done.");
                } else if !episodes.is_empty() {
                    println!("Downloading {} episodes to {}...", episodes.len(), base_out.display());
                    for (eid, number_num, number_text) in episodes {
                        let name = number_text.clone().or_else(|| number_num.map(|n| format!("{:.3}", n))).unwrap_or_else(|| "episode".to_string());
                        let ep_out = base_out.join(format!("{}.txt", name));
                        let streams = rt.block_on(touring.get_episode_streams(&eid))?;
                        if streams.is_empty() { continue; }
                        let s = &streams[0];
                        let out_path = ep_out.clone();
                        rt.block_on(async move { tokio::fs::write(out_path, s.url.as_bytes()).await })?;
                    }
                    println!("Done.");
                } else {
                    println!("No chapters or episodes found for series {}.", series_id);
                }
            }
        },
        Commands::Series { cmd } => match cmd {
            SeriesCmd::List { kind } => {
                let rows = rt.block_on(touring.list_series(kind.as_deref()))?;
                for (id, title) in rows { println!("{}\t{}", id, title); }
            }
            SeriesCmd::SetPath { series_id, path } => {
                if let Err(e) = rt.block_on(touring.set_series_download_path(&series_id, path.as_deref())) {
                    eprintln!("Failed to set path: {}\nHint: Use 'touring resolve-series-id <source> <external_id>' to get the canonical series id.", e);
                    return Ok(());
                }
                let current = rt.block_on(touring.get_series_download_path(&series_id))?;
                println!("Series {} download_path = {:?}", series_id, current);
            }
            SeriesCmd::Delete { series_id } => {
                let n = rt.block_on(touring.delete_series(&series_id))?;
                println!("Deleted series {} (rows affected: {})", series_id, n);
            }
            SeriesCmd::DeleteChapter { chapter_id } => {
                let n = rt.block_on(touring.delete_chapter(&chapter_id))?;
                println!("Deleted chapter {} (rows affected: {})", chapter_id, n);
            }
            SeriesCmd::DeleteEpisode { episode_id } => {
                let n = rt.block_on(touring.delete_episode(&episode_id))?;
                println!("Deleted episode {} (rows affected: {})", episode_id, n);
            }
        },
    }

    Ok(())
}

async fn save_images(chapter_id: &str, urls: &[String], out_dir: &Path, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    tokio::fs::create_dir_all(out_dir).await.ok();
    let client = reqwest::Client::builder().user_agent("touring/0.1").build()?;
    for (i, url) in urls.iter().enumerate() {
        let fname = format!("{:04}.jpg", i + 1);
        let path = out_dir.join(fname);
        if !force {
            if tokio::fs::try_exists(&path).await.unwrap_or(false) { continue; }
        }
        let resp = client.get(url).send().await?;
        if !resp.status().is_success() { eprintln!("Failed to download {}: {}", url, resp.status()); continue; }
        let bytes = resp.bytes().await?;
        tokio::fs::write(&path, &bytes).await?;
    }
    Ok(())
}

async fn save_cbz(_chapter_id: &str, urls: &[String], out_file: &Path, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !force && tokio::fs::try_exists(out_file).await.unwrap_or(false) { return Ok(()); }
    let tmp_dir = out_file.with_extension("tmpdir");
    tokio::fs::create_dir_all(&tmp_dir).await.ok();
    save_images(_chapter_id, urls, &tmp_dir, true).await?;
    // Zip the directory into a CBZ
    let file = std::fs::File::create(out_file)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut entries: Vec<_> = std::fs::read_dir(&tmp_dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            zip.start_file(name, options)?;
            let data = std::fs::read(&path)?;
            zip.write_all(&data)?;
        }
    }
    zip.finish()?;
    // Cleanup tmp dir
    let _ = std::fs::remove_dir_all(&tmp_dir);
    Ok(())
}

async fn save_images_mockable(chapter_id: &str, urls: &[String], out_dir: &Path, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    tokio::fs::create_dir_all(out_dir).await.ok();
    let client = reqwest::Client::builder().user_agent("touring/0.1").build()?;
    for (i, url) in urls.iter().enumerate() {
        let fname = format!("{:04}.jpg", i + 1);
        let path = out_dir.join(fname);
        if !force {
            if tokio::fs::try_exists(&path).await.unwrap_or(false) { continue; }
        }
        if url.starts_with("mock://") {
            // write simple placeholder bytes
            tokio::fs::write(&path, b"MOCK").await?;
            continue;
        }
        let resp = client.get(url).send().await?;
        if !resp.status().is_success() { eprintln!("Failed to download {}: {}", url, resp.status()); continue; }
        let bytes = resp.bytes().await?;
        tokio::fs::write(&path, &bytes).await?;
    }
    Ok(())
}

async fn save_cbz_mockable(_chapter_id: &str, urls: &[String], out_file: &Path, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !force && tokio::fs::try_exists(out_file).await.unwrap_or(false) { return Ok(()); }
    let tmp_dir = out_file.with_extension("tmpdir");
    tokio::fs::create_dir_all(&tmp_dir).await.ok();
    save_images_mockable(_chapter_id, urls, &tmp_dir, true).await?;
    // Zip the directory into a CBZ
    let file = std::fs::File::create(out_file)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut entries: Vec<_> = std::fs::read_dir(&tmp_dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            zip.start_file(name, options)?;
            let data = std::fs::read(&path)?;
            use std::io::Write as _;
            zip.write_all(&data)?;
        }
    }
    zip.finish()?;
    // Cleanup tmp dir
    let _ = std::fs::remove_dir_all(&tmp_dir);
    Ok(())
}