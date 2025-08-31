// Shared between host and plugin
pub mod api
{
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    pub struct Chapter 
    {
        pub id: String,
        pub title: String,
        pub images: Vec<String>,
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    pub struct Manga 
    {
        pub id: String,
        pub title: String,
        pub description: Option<String>,
        pub cover_url: Option<String>,
    }

    pub trait Source 
    {
        fn fetch_manga_list(&self, query: &str) -> Vec<Manga>;
        fn fetch_chapter_images(&self, chapter_id: &str) -> Vec<String>;
    }
}
