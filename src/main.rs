use reqwest::Client;
use scraper::{Html, Selector};

pub mod data;
pub mod api;
pub mod wasmsource;

async fn fetch_page(url: &str) -> reqwest::Result<String>
{
    let client = Client::new();
    let res = client.get(url).send().await?;
    res.text().await
}

fn parse_items(html: &str) -> Vec<String>
{
    let document = Html::parse_document(html);
    let selector = Selector::parse("div.w-full").unwrap();
    document
        .select(&selector)
        .map(|elem| elem.text().collect::<String>())
        .collect()
}

#[tokio::main]
async fn main() 
{

    let html = fetch_page("https://mangadex.org/title/9e954c6b-7a02-4fd7-986d-6331c0ba95d4/the-summer-hikaru-died").await.unwrap();
    let items = parse_items(&html);

    println!("Items: {:#?}", items);

}
