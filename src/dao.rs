use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::AnyPool;

use crate::ChapterProgress;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInsert {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesInsert {
    pub id: String,
    pub kind: String, // "manga" | "anime"
    pub title: String,
    pub alt_titles: Option<String>, // JSON array string
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub tags: Option<String>, // JSON array string
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesSourceInsert {
    pub series_id: String,
    pub source_id: String,
    pub external_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterInsert {
    pub id: String,
    pub series_id: String,
    pub source_id: String,
    pub external_id: String,
    pub number_text: Option<String>,
    pub number_num: Option<f64>,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub group: Option<String>,
    pub published_at: Option<String>, // ISO string
    pub upload_group: Option<String>
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterImageInsert {
    pub chapter_id: String,
    pub idx: i64,
    pub url: String,
    pub mime: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeInsert {
    pub id: String,
    pub series_id: String,
    pub source_id: String,
    pub external_id: String,
    pub number_text: Option<String>,
    pub number_num: Option<f64>,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub season: Option<String>,
    pub published_at: Option<String>,
    pub upload_group: Option<String>
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInsert {
    pub episode_id: String,
    pub url: String,
    pub quality: Option<String>,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesPref {
    pub series_id: String,
    pub download_path: Option<String>,
}

pub async fn upsert_source(pool: &AnyPool, src: &SourceInsert) -> Result<()> {
    sqlx::query(
        "INSERT INTO sources(id, version) VALUES(?, ?)\n         ON CONFLICT(id) DO UPDATE SET version=excluded.version, updated_at=CURRENT_TIMESTAMP",
    )
    .bind(&src.id)
    .bind(&src.version)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_series(pool: &AnyPool, s: &SeriesInsert) -> Result<()> {
    sqlx::query(
        "INSERT INTO series(id, kind, title, alt_titles, description, cover_url, tags, status)\n         VALUES(?, ?, ?, ?, ?, ?, ?, ?)\n         ON CONFLICT(id) DO UPDATE SET\n           kind=excluded.kind, title=excluded.title, alt_titles=excluded.alt_titles,\n           description=excluded.description, cover_url=excluded.cover_url,\n           tags=excluded.tags, status=excluded.status, updated_at=CURRENT_TIMESTAMP",
    )
    .bind(&s.id)
    .bind(&s.kind)
    .bind(&s.title)
    .bind(&s.alt_titles)
    .bind(&s.description)
    .bind(&s.cover_url)
    .bind(&s.tags)
    .bind(&s.status)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_series_source(pool: &AnyPool, ss: &SeriesSourceInsert) -> Result<()> {
    sqlx::query(
        "INSERT INTO series_sources(series_id, source_id, external_id) VALUES(?, ?, ?)\n         ON CONFLICT(series_id, source_id, external_id) DO UPDATE SET last_synced_at=CURRENT_TIMESTAMP",
    )
    .bind(&ss.series_id)
    .bind(&ss.source_id)
    .bind(&ss.external_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_chapter(pool: &AnyPool, c: &ChapterInsert) -> Result<()> {
    sqlx::query(
        "INSERT INTO chapters(\n            id, series_id, source_id, external_id, number_text, number_num, title, lang, volume, published_at, upload_group\n         ) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)\n         ON CONFLICT(series_id, source_id, external_id) DO UPDATE SET\n           id=excluded.id, number_text=excluded.number_text, number_num=excluded.number_num,\n           title=excluded.title, lang=excluded.lang, volume=excluded.volume,\n           published_at=excluded.published_at, updated_at=CURRENT_TIMESTAMP",
    )
    .bind(&c.id)
    .bind(&c.series_id)
    .bind(&c.source_id)
    .bind(&c.external_id)
    .bind(&c.number_text)
    .bind(&c.number_num)
    .bind(&c.title)
    .bind(&c.lang)
    .bind(&c.group)
    .bind(&c.published_at)
    .bind(&c.upload_group)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_chapter_images(pool: &AnyPool, images: &[ChapterImageInsert]) -> Result<()> {
    let mut tx = pool.begin().await?;
    for img in images {
        sqlx::query(
            "INSERT INTO chapter_images(chapter_id, idx, url, mime, width, height)\n             VALUES(?, ?, ?, ?, ?, ?)\n             ON CONFLICT(chapter_id, idx) DO UPDATE SET\n               url=excluded.url, mime=excluded.mime, width=excluded.width, height=excluded.height",
        )
        .bind(&img.chapter_id)
        .bind(img.idx)
        .bind(&img.url)
        .bind(&img.mime)
        .bind(&img.width)
        .bind(&img.height)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn upsert_episode(pool: &AnyPool, e: &EpisodeInsert) -> Result<()> {
    sqlx::query(
        "INSERT INTO episodes(\n            id, series_id, source_id, external_id, number_text, number_num, title, lang, season, published_at, upload_group\n         ) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)\n         ON CONFLICT(series_id, source_id, external_id) DO UPDATE SET\n           id=excluded.id, number_text=excluded.number_text, number_num=excluded.number_num,\n           title=excluded.title, lang=excluded.lang, season=excluded.season,\n           published_at=excluded.published_at, updated_at=CURRENT_TIMESTAMP",
    )
    .bind(&e.id)
    .bind(&e.series_id)
    .bind(&e.source_id)
    .bind(&e.external_id)
    .bind(&e.number_text)
    .bind(&e.number_num)
    .bind(&e.title)
    .bind(&e.lang)
    .bind(&e.season)
    .bind(&e.published_at)
    .bind(&e.upload_group)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_streams(
    pool: &AnyPool,
    episode_id: &str,
    streams: &[StreamInsert],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    for s in streams {
        sqlx::query(
            "INSERT INTO streams(episode_id, url, quality, mime) VALUES(?, ?, ?, ?)\n             ON CONFLICT DO NOTHING",
        )
        .bind(episode_id)
        .bind(&s.url)
        .bind(&s.quality)
        .bind(&s.mime)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

// New helpers for canonical identity
pub async fn find_series_id_by_source_external(
    pool: &AnyPool,
    source_id: &str,
    external_id: &str,
) -> Result<Option<String>> {
    let id = sqlx::query_scalar::<_, String>(
        "SELECT series_id FROM series_sources WHERE source_id = ? AND external_id = ? LIMIT 1",
    )
    .bind(source_id)
    .bind(external_id)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

pub async fn find_chapter_id_by_mapping(
    pool: &AnyPool,
    series_id: &str,
    source_id: &str,
    external_id: &str,
) -> Result<Option<String>> {
    let id = sqlx::query_scalar::<_, String>(
        "SELECT id FROM chapters WHERE series_id = ? AND source_id = ? AND external_id = ? LIMIT 1",
    )
    .bind(series_id)
    .bind(source_id)
    .bind(external_id)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

pub async fn find_episode_id_by_mapping(
    pool: &AnyPool,
    series_id: &str,
    source_id: &str,
    external_id: &str,
) -> Result<Option<String>> {
    let id = sqlx::query_scalar::<_, String>(
        "SELECT id FROM episodes WHERE series_id = ? AND source_id = ? AND external_id = ? LIMIT 1",
    )
    .bind(series_id)
    .bind(source_id)
    .bind(external_id)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

// New: Resolve episode id without requiring series_id (best-effort)
pub async fn find_episode_id_by_source_external(
    pool: &AnyPool,
    source_id: &str,
    external_id: &str,
) -> Result<Option<String>> {
    let id = sqlx::query_scalar::<_, String>(
        "SELECT id FROM episodes WHERE source_id = ? AND external_id = ? LIMIT 1",
    )
    .bind(source_id)
    .bind(external_id)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

pub async fn find_chapter_identity(
    pool: &AnyPool,
    chapter_id_or_external: &str,
) -> Result<Option<(String, String)>> {
    if let Some(row) = sqlx::query_as::<_, (String, String)>(
        "SELECT id, series_id FROM chapters WHERE id = ? LIMIT 1",
    )
    .bind(chapter_id_or_external)
    .fetch_optional(pool)
    .await?
    {
        return Ok(Some(row));
    }

    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT id, series_id FROM chapters WHERE external_id = ? LIMIT 1",
    )
    .bind(chapter_id_or_external)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn find_chapter_fetch_info(
    pool: &AnyPool,
    chapter_id_or_external: &str,
) -> Result<Option<(String, String, String)>> {
    if let Some(row) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, source_id, external_id FROM chapters WHERE id = ? LIMIT 1",
    )
    .bind(chapter_id_or_external)
    .fetch_optional(pool)
    .await?
    {
        return Ok(Some(row));
    }

    let row = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, source_id, external_id FROM chapters WHERE external_id = ? LIMIT 1",
    )
    .bind(chapter_id_or_external)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn upsert_chapter_progress(
    pool: &AnyPool,
    chapter_id: &str,
    series_id: &str,
    page_index: i64,
    total_pages: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO chapter_progress(chapter_id, series_id, page_index, total_pages, updated_at)
         VALUES(?, ?, ?, ?, unixepoch())
         ON CONFLICT(chapter_id) DO UPDATE SET
           series_id=excluded.series_id,
           page_index=excluded.page_index,
           total_pages=excluded.total_pages,
           updated_at=unixepoch()",
    )
    .bind(chapter_id)
    .bind(series_id)
    .bind(page_index)
    .bind(total_pages)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn clear_chapter_progress(pool: &AnyPool, chapter_id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM chapter_progress WHERE chapter_id = ?")
        .bind(chapter_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn get_chapter_progress(
    pool: &AnyPool,
    chapter_id: &str,
) -> Result<Option<ChapterProgress>> {
    let row = sqlx::query_as::<_, (String, String, i64, Option<i64>, i64)>(
        "SELECT chapter_id, series_id, page_index, total_pages, updated_at
         FROM chapter_progress WHERE chapter_id = ? LIMIT 1",
    )
    .bind(chapter_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(chapter_id, series_id, page_index, total_pages, updated_at)| ChapterProgress {
            chapter_id,
            series_id,
            page_index,
            total_pages,
            updated_at,
        },
    ))
}

pub async fn get_chapter_progress_for_series(
    pool: &AnyPool,
    series_id: &str,
) -> Result<Vec<ChapterProgress>> {
    let rows = sqlx::query_as::<_, (String, String, i64, Option<i64>, i64)>(
        "SELECT chapter_id, series_id, page_index, total_pages, updated_at
         FROM chapter_progress WHERE series_id = ?",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(chapter_id, series_id, page_index, total_pages, updated_at)| ChapterProgress {
                chapter_id,
                series_id,
                page_index,
                total_pages,
                updated_at,
            },
        )
        .collect())
}

// New: preferences
pub async fn get_series_pref(pool: &AnyPool, series_id: &str) -> Result<Option<SeriesPref>> {
    // Use COALESCE to avoid decoding NULL directly into Option<String> with the Any driver
    let opt: Option<String> = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(download_path, '') FROM series_prefs WHERE series_id = ?",
    )
    .bind(series_id)
    .fetch_optional(pool)
    .await?;

    match opt {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(Some(SeriesPref {
            series_id: series_id.to_string(),
            download_path: None,
        })),
        Some(s) => Ok(Some(SeriesPref {
            series_id: series_id.to_string(),
            download_path: Some(s),
        })),
    }
}

pub async fn set_series_download_path(
    pool: &AnyPool,
    series_id: &str,
    path: Option<&str>,
) -> Result<()> {
    // Ensure the series exists to avoid FK violations and provide a clearer error
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_optional(pool)
        .await?;
    if exists.is_none() {
        return Err(anyhow::anyhow!("Series not found: {}", series_id));
    }

    sqlx::query(
        "INSERT INTO series_prefs(series_id, download_path) VALUES(?, ?)\n         ON CONFLICT(series_id) DO UPDATE SET download_path=excluded.download_path, updated_at=CURRENT_TIMESTAMP",
    )
    .bind(series_id)
    .bind(path)
    .execute(pool)
    .await?;
    Ok(())
}

// Deletion helpers (cascade removes children where FK declared)
pub async fn delete_series(pool: &AnyPool, series_id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM series WHERE id = ?")
        .bind(series_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn delete_chapter(pool: &AnyPool, chapter_id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM chapters WHERE id = ?")
        .bind(chapter_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn delete_episode(pool: &AnyPool, episode_id: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM episodes WHERE id = ?")
        .bind(episode_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

// Lookups to drive downloads/selection
pub async fn list_series(pool: &AnyPool, kind: Option<&str>) -> Result<Vec<(String, String)>> {
    let rows = if let Some(k) = kind {
        sqlx::query_as::<_, (String, String)>(
            "SELECT id, title FROM series WHERE kind = ? ORDER BY title",
        )
        .bind(k)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, (String, String)>("SELECT id, title FROM series ORDER BY title")
            .fetch_all(pool)
            .await?
    };
    Ok(rows)
}

pub async fn list_chapters_for_series(
    pool: &AnyPool,
    series_id: &str,
) -> Result<Vec<(String, Option<f64>, Option<String>, Option<String>)>> {
    let rows = sqlx::query_as::<_, (String, Option<f64>, Option<String>, Option<String>)>(
        "SELECT id, number_num, number_text, upload_group FROM chapters WHERE series_id = ? ORDER BY number_num NULLS LAST, number_text",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_episodes_for_series(
    pool: &AnyPool,
    series_id: &str,
) -> Result<Vec<(String, Option<f64>, Option<String>, Option<String>)>> {
    let rows = sqlx::query_as::<_, (String, Option<f64>, Option<String>, Option<String>)>(
        "SELECT id, number_num, number_text, upload_group FROM episodes WHERE series_id = ? ORDER BY number_num NULLS LAST, number_text",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
