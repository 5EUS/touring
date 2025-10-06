use crate::dao::{ChapterInsert, SeriesInsert, SeriesSourceInsert};
use crate::plugins::{Media, MediaType, Unit, UnitKind};

fn kind_str(mt: &MediaType) -> &'static str {
    match mt {
        MediaType::Manga => "manga",
        MediaType::Anime => "anime",
        MediaType::Other(_) => "other",
    }
}

pub fn series_id_from(source_id: &str, media: &Media) -> String {
    format!(
        "series:{}:{}:{}",
        source_id,
        kind_str(&media.mediatype),
        media.id
    )
}

pub fn chapter_id_from(source_id: &str, unit: &Unit) -> String {
    let kind = match unit.kind {
        UnitKind::Chapter => "chapter",
        UnitKind::Episode => "episode",
        UnitKind::Section => "section",
        UnitKind::Other(_) => "unit",
    };
    format!("{}:{}:{}", source_id, kind, unit.id)
}

pub fn series_insert_from_media(id: String, media: &Media) -> SeriesInsert {
    SeriesInsert {
        id,
        kind: kind_str(&media.mediatype).to_string(),
        title: media.title.clone(),
        alt_titles: None,
        description: media.description.clone(),
        cover_url: media.cover_url.clone(),
        tags: None,
        status: None,
    }
}

pub fn series_source_from(
    series_id: String,
    source_id: String,
    external_id: String,
) -> SeriesSourceInsert {
    SeriesSourceInsert {
        series_id,
        source_id,
        external_id,
    }
}

pub fn chapter_insert_from_unit(
    id: String,
    series_id: String,
    source_id: String,
    u: &Unit,
) -> ChapterInsert {
    ChapterInsert {
        id,
        series_id,
        source_id,
        external_id: u.id.clone(),
        number_text: u.number_text.clone(),
        number_num: u.number.map(|n| n as f64),
        title: Some(u.title.clone()).filter(|s| !s.is_empty()),
        lang: u.lang.clone(),
        group: u.group.clone(),
        published_at: u.published_at.clone(),
        upload_group: u.upload_group.clone()
    }
}
