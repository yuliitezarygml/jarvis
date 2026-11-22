//! Google Programmable Search (Custom Search JSON API):
//! `GET https://customsearch.googleapis.com/customsearch/v1?key=…&cx=…&q=…&num=…` (`num` 1–10);
//! ответ `items[] {title, link, snippet}` (поля `items` нет, если ничего не найдено).

use async_trait::async_trait;
use serde::Deserialize;

use super::{SearchError, SearchHit, SearchProvider, send_json};

pub const DEFAULT_BASE: &str = "https://customsearch.googleapis.com";

pub struct GoogleCse {
    client: reqwest::Client,
    key: String,
    cx: String,
    base_url: String,
}

impl GoogleCse {
    pub fn new(client: reqwest::Client, key: String, cx: String) -> Self {
        Self {
            client,
            key,
            cx,
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
    items: Vec<Item>,
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
impl SearchProvider for GoogleCse {
    fn id(&self) -> &str {
        "google_cse"
    }

    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        let num = n.clamp(1, 10).to_string();
        let req = self
            .client
            .get(format!(
                "{}/customsearch/v1",
                self.base_url.trim_end_matches('/')
            ))
            .query(&[
                ("key", self.key.as_str()),
                ("cx", self.cx.as_str()),
                ("q", query),
                ("num", num.as_str()),
            ]);
        let resp: Resp = send_json(req).await?;
        Ok(resp
            .items
            .iter()
            .filter_map(|i| SearchHit::cleaned(&i.title, &i.link, &i.snippet))
            .take(n)
            .collect())
    }
}
