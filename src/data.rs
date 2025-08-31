pub mod data
{
    use serde::Serialize;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize)]
    pub enum MediaType
    {
        Image,
        Video,
        Text,
        Other(String),
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ContentItem 
    {
        pub id: String,
        pub title: String,
        pub description: Option<String>,
        pub media_type: MediaType,
        pub url: String,
        pub thumbnail_url: Option<String>,
        pub metadata: serde_json::Value, // Arbitrary extra info
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct SourceConfig 
    {
        pub name: String,
        pub base_url: String,
        pub list_endpoint: String,
        pub detail_endpoint: Option<String>,
        pub parsing_rules: serde_json::Value, // Will store selectors, paths, etc.
    } 
}