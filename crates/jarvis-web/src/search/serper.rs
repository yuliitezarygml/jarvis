//! Serper (Google через API): `POST https://google.serper.dev/search`, заголовок `X-API-KEY`,
//! тело `{q, num}`; ответ `organic[] {title, link, snippet, position}`.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{SearchError, SearchHit, SearchProvider, send_json};

pub const DEFAULT_BASE: &str = "https://google.serper.dev";

pub struct Serper {
    client: reqwest::Client,
    key: String,
    base_url: String,
}

impl Serper {
    pub fn new(client: reqwest::Client, key: String) -> Self {
        Self {
            client,
            key,
            base_url: DEFAULT_BASE.into(),
        }
    }
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    organic: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    snippet: String,
}

#[async_trait]
impl SearchProvider for Serper {
    fn id(&self) -> &str {
        "serper"
    }

    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        let req = self
            .client
            .post(format!("{}/search", self.base_url.trim_end_matches('/')))
            .header("X-API-KEY", &self.key)
            .json(&json!({ "q": query, "num": n.clamp(1, 100) }));
        let resp: Resp = send_json(req).await?;
        Ok(resp
            .organic
            .iter()
            .filter_map(|i| SearchHit::cleaned(&i.title, &i.link, &i.snippet))
            .take(n)
            .collect())
    }
}
