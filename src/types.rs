use serde::{Serialize, Deserialize};
use crate::plugins::{Media, MediaType};

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct MediaCache {
    pub id: String,
    pub mediatype: String,
    pub title: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub cover_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SearchEntry { pub source_id: String, pub media: MediaCache }

pub(crate) fn media_to_cache(m: &Media) -> MediaCache {
    let mediatype = match &m.mediatype { MediaType::Manga => "manga".to_string(), MediaType::Anime => "anime".to_string(), MediaType::Other(s) => format!("other:{}", s) };
    MediaCache { id: m.id.clone(), mediatype, title: m.title.clone(), description: m.description.clone(), url: m.url.clone(), cover_url: m.cover_url.clone() }
}

pub(crate) fn media_from_cache(mc: MediaCache) -> Media {
    let mediatype = match mc.mediatype.as_str() { "manga" => MediaType::Manga, "anime" => MediaType::Anime, s if s.starts_with("other:") => MediaType::Other(s[6..].to_string()), _ => MediaType::Other(mc.mediatype.clone()) };
    Media { id: mc.id, mediatype, title: mc.title, description: mc.description, url: mc.url, cover_url: mc.cover_url }
}
