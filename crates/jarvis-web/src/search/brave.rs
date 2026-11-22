//! Brave Search API: `GET https://api.search.brave.com/res/v1/web/search?q=…&count=…`,
//! заголовок `X-Subscription-Token`; результаты в `web.results[] {title, url, description}`; `count` ≤ 20.

use async_trait::async_trait;
use serde::Deserialize;

use super::{SearchError, SearchHit, SearchProvider, send_json};

pub const DEFAULT_BASE: &str = "https://api.search.brave.com";

pub struct Brave {
    client: reqwest::Client,
    key: String,
    base_url: String,
}

impl Brave {
    pub fn new(client: reqwest::Client, key: String) -> Self {
        Self { client, key, base_url: DEFAULT_BASE.into() }
    }
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    web: Option<Web>,
}
#[derive(Deserialize)]
struct Web {
    #[serde(default)]
    results: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
}

#[async_trait]
impl SearchProvider for Brave {
    fn id(&self) -> &str {
        "brave"
    }

    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        let count = n.clamp(1, 20).to_string();
        let req = self
            .client
            .get(format!("{}/res/v1/web/search", self.base_url.trim_end_matches('/')))
            .header("X-Subscription-Token", &self.key)
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[("q", query), ("count", count.as_str())]);
        let resp: Resp = send_json(req).await?;
        Ok(resp
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .iter()
            .filter_map(|i| SearchHit::cleaned(&i.title, &i.url, &i.description))
            .take(n)
            .collect())
    }
}
