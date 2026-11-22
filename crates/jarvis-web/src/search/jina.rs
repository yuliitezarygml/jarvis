//! Jina Search: `GET https://s.jina.ai/<запрос>` с `Accept: application/json` и
//! `X-Respond-With: no-content` (только выдача, без чтения страниц); ключ `Authorization: Bearer` необязателен.
//! Ответ `{code, status, data: [{title, url, description, content?}]}`.

use async_trait::async_trait;
use serde::Deserialize;

use super::{SearchError, SearchHit, SearchProvider, send_json};

pub const DEFAULT_BASE: &str = "https://s.jina.ai";

pub struct JinaSearch {
    client: reqwest::Client,
    key: Option<String>,
    base_url: String,
}

impl JinaSearch {
    pub fn new(client: reqwest::Client, key: Option<String>) -> Self {
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
    data: Option<Vec<Item>>,
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
impl SearchProvider for JinaSearch {
    fn id(&self) -> &str {
        "jina"
    }

    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        let mut url = url::Url::parse(&format!("{}/", self.base_url.trim_end_matches('/')))
            .map_err(|e| SearchError::BadResponse(format!("адрес Jina: {e}")))?;
        // Запрос — один сегмент пути, с процентным кодированием («/», «?», «#» внутри запроса не ломают адрес).
        url.path_segments_mut()
            .map_err(|_| SearchError::BadResponse("адрес Jina без пути".into()))?
            .pop_if_empty()
            .push(query);
        let mut req = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header("X-Respond-With", "no-content");
        if let Some(k) = &self.key {
            req = req.bearer_auth(k);
        }
        let resp: Resp = send_json(req).await?;
        Ok(resp
            .data
            .unwrap_or_default()
            .iter()
            .filter_map(|i| SearchHit::cleaned(&i.title, &i.url, &i.description))
            .take(n)
            .collect())
    }
}
