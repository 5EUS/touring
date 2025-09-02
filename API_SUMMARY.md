# Touring Library - UI API Extensions

This document summarizes the new API methods added to support UI development.

## New Data Structures

### Core Types
- `SeriesInfo` - Complete series information including metadata and statistics
- `SeriesMetadataUpdate` - Structure for updating series metadata
- `SeriesSource` - External source mapping for a series
- `ChapterInfo` - Detailed chapter information with download status
- `EpisodeInfo` - Detailed episode information with stream status
- `DownloadProgress` - Progress tracking for downloads
- `DownloadResult` - Result of download operations
- `LibraryStats` - Overall library statistics

## Download API

### Individual Downloads
- `download_chapter_images(chapter_id, output_dir, force_overwrite)` - Download chapter images to directory
- `download_chapter_cbz(chapter_id, output_file, force_overwrite)` - Download chapter as CBZ archive

### Batch Downloads
- `download_series_chapters(series_id, base_dir, as_cbz, force_overwrite)` - Download all chapters for a series
- `download_series_chapters_with_progress(series_id, base_dir, as_cbz, force_overwrite, progress_callback)` - Download with progress tracking
- `get_series_download_status(series_id, base_dir, as_cbz)` - Check how many chapters are already downloaded

## Series Management API

### Series Information
- `get_series_info(series_id)` - Get complete series information
- `update_series_metadata(series_id, updates)` - Update series metadata
- `search_local_series(query, kind, limit)` - Search local series database
- `refresh_series_metadata(series_id)` - Refresh metadata from sources

### Source Management
- `get_series_sources(series_id)` - Get all source mappings for a series
- `add_series_source(series_id, source_id, external_id)` - Add new source mapping
- `remove_series_source(series_id, source_id, external_id)` - Remove source mapping

### Content Information
- `get_chapter_info(chapter_id)` - Get detailed chapter information
- `get_episode_info(episode_id)` - Get detailed episode information

## Library Statistics
- `get_library_stats()` - Get overall library statistics (series count, chapters, episodes, cache stats)

## Usage Examples

### Download with Progress Tracking
```rust
use std::path::Path;
use touring::prelude::*;

let touring = Touring::connect(None, true).await?;
let result = touring.download_series_chapters_with_progress(
    "series-id",
    Path::new("/downloads/manga"),
    true, // as CBZ
    false, // don't force overwrite
    |progress| {
        println!("Downloading {} ({}/{})", progress.current_item, progress.current, progress.total);
    }
).await?;

println!("Downloaded {}/{} chapters", result.items_downloaded, result.items_processed);
```

### Series Management
```rust
// Search local series
let series = touring.search_local_series("yotsuba", Some("manga"), Some(10)).await?;

// Get detailed info
if let Some(info) = touring.get_series_info(&series[0].id).await? {
    println!("Series: {} ({} chapters, {} episodes)", info.title, info.chapters_count, info.episodes_count);
}

// Update metadata
let updates = SeriesMetadataUpdate {
    title: Some("New Title".to_string()),
    description: Some(Some("New description".to_string())),
    cover_url: None,
    status: Some(Some("completed".to_string())),
};
touring.update_series_metadata(&series[0].id, updates).await?;
```

### Library Statistics
```rust
let stats = touring.get_library_stats().await?;
println!("Library: {} series ({} manga, {} anime), {} chapters, {} episodes", 
    stats.total_series, stats.manga_series, stats.anime_series, 
    stats.total_chapters, stats.total_episodes);
```

## Key Features for UI Development

1. **Progress Tracking** - Download operations support progress callbacks for UI updates
2. **Error Handling** - All methods return `Result<T>` for proper error handling
3. **Async/Await** - All operations are async for non-blocking UI
4. **Serializable Types** - All data structures support Serde for JSON/API responses
5. **Flexible Queries** - Search and filter methods support optional parameters
6. **Status Checking** - Methods to check download status without actually downloading
7. **Metadata Management** - Full CRUD operations for series metadata
8. **Source Management** - Multi-source support with mapping management

All new methods are available through the `touring::prelude` module for easy importing.
