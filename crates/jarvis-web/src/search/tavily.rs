//! Tavily: `POST https://api.tavily.com/search`, `Authorization: Bearer tvly-…`,
//! тело `{query, max_results (0–20), search_depth}`; ответ `results[] {title, url, content, score}`.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{SearchError, SearchHit, SearchProvider, send_json};

pub const DEFAULT_BASE: &str = "https://api.tavily.com";

pub struct Tavily {
    client: reqwest::Client,
    key: String,
    base_url: String,
}

impl Tavily {
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
    results: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

#[async_trait]
impl SearchProvider for Tavily {
    fn id(&self) -> &str {
        "tavily"
    }

    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        let req = self
            .client
            .post(format!("{}/search", self.base_url.trim_end_matches('/')))
            .bearer_auth(&self.key)
            .json(&json!({ "query": query, "max_results": n.clamp(1, 20), "search_depth": "basic" }));
        let resp: Resp = send_json(req).await?;
        Ok(resp.results.iter().filter_map(|i| SearchHit::cleaned(&i.title, &i.url, &i.content)).take(n).collect())
    }
}
