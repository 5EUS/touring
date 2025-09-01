use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::AnyPool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInsert {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesInsert {
    pub id: String,
    pub kind: String,                // "manga" | "anime"
    pub title: String,
    pub alt_titles: Option<String>,  // JSON array string
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub tags: Option<String>,        // JSON array string
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
    pub volume: Option<String>,
    pub published_at: Option<String>, // ISO string
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInsert {
    pub episode_id: String,
    pub url: String,
    pub quality: Option<String>,
    pub mime: Option<String>,
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
        "INSERT INTO chapters(\n            id, series_id, source_id, external_id, number_text, number_num, title, lang, volume, published_at\n         ) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)\n         ON CONFLICT(series_id, source_id, external_id) DO UPDATE SET\n           id=excluded.id, number_text=excluded.number_text, number_num=excluded.number_num,\n           title=excluded.title, lang=excluded.lang, volume=excluded.volume,\n           published_at=excluded.published_at, updated_at=CURRENT_TIMESTAMP",
    )
    .bind(&c.id)
    .bind(&c.series_id)
    .bind(&c.source_id)
    .bind(&c.external_id)
    .bind(&c.number_text)
    .bind(&c.number_num)
    .bind(&c.title)
    .bind(&c.lang)
    .bind(&c.volume)
    .bind(&c.published_at)
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
        "INSERT INTO episodes(\n            id, series_id, source_id, external_id, number_text, number_num, title, lang, season, published_at\n         ) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)\n         ON CONFLICT(series_id, source_id, external_id) DO UPDATE SET\n           id=excluded.id, number_text=excluded.number_text, number_num=excluded.number_num,\n           title=excluded.title, lang=excluded.lang, season=excluded.season,\n           published_at=excluded.published_at, updated_at=CURRENT_TIMESTAMP",
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
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_streams(pool: &AnyPool, episode_id: &str, streams: &[StreamInsert]) -> Result<()> {
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
pub async fn find_series_id_by_source_external(pool: &AnyPool, source_id: &str, external_id: &str) -> Result<Option<String>> {
    let id = sqlx::query_scalar::<_, String>(
        "SELECT series_id FROM series_sources WHERE source_id = ? AND external_id = ? LIMIT 1",
    )
    .bind(source_id)
    .bind(external_id)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

pub async fn find_chapter_id_by_mapping(pool: &AnyPool, series_id: &str, source_id: &str, external_id: &str) -> Result<Option<String>> {
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
