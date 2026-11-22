//! Поиск за trait [`SearchProvider`], цепочка [`SearchChain`] с failover, проверка ключа [`validate`].

pub mod brave;
pub mod duckduckgo;
pub mod google;
pub mod jina;
pub mod serper;
pub mod tavily;

pub use brave::Brave;
pub use duckduckgo::DuckDuckGo;
pub use google::GoogleCse;
pub use jina::JinaSearch;
pub use serper::Serper;
pub use tavily::Tavily;

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use jarvis_core::config::SearchProviderConfig;
use serde::{Deserialize, Serialize};

use crate::text::{html_fragment_text, plural_ru, truncate_plain};

/// Один результат поиска.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

impl SearchHit {
    /// Нормализовать: снять HTML из заголовка/сниппета, ограничить длину сниппета.
    pub(crate) fn cleaned(title: &str, url: &str, snippet: &str) -> Option<Self> {
        let url = url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return None;
        }
        let title = html_fragment_text(title);
        Some(Self {
            title: if title.is_empty() { url.to_string() } else { title },
            url: url.to_string(),
            snippet: truncate_plain(&html_fragment_text(snippet), 400),
        })
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SearchError {
    #[error("для поисковика «{0}» нужен API-ключ")]
    MissingKey(String),
    #[error("ключ не принят: {0}")]
    Auth(String),
    #[error("лимит запросов исчерпан: {0}")]
    RateLimited(String),
    #[error("поисковик заблокировал автоматический запрос (капча), попробуйте позже")]
    Blocked,
    #[error("ошибка поисковика: HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("сеть недоступна: {0}")]
    Network(String),
    #[error("таймаут")]
    Timeout,
    #[error("некорректный ответ поисковика: {0}")]
    BadResponse(String),
    #[error("неизвестный поисковик: {0}")]
    UnknownProvider(String),
    #[error("нет ни одного включённого поисковика")]
    NoProviders,
    #[error("все поисковики недоступны: {}", describe_failures(.0))]
    AllFailed(Vec<(String, SearchError)>),
}

fn describe_failures(f: &[(String, SearchError)]) -> String {
    f.iter().map(|(id, e)| format!("{id}: {e}")).collect::<Vec<_>>().join("; ")
}

impl SearchError {
    pub(crate) fn from_reqwest(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            SearchError::Timeout
        } else if e.is_decode() {
            SearchError::BadResponse(e.to_string())
        } else {
            SearchError::Network(e.without_url().to_string())
        }
    }

    pub(crate) fn from_status(status: u16, body: &str) -> Self {
        let message = crate::http::api_error_message(body);
        match status {
            401 | 403 => SearchError::Auth(message),
            // 432/433 — лимиты тарифа Tavily.
            429 | 432 | 433 => SearchError::RateLimited(message),
            400 if message.to_ascii_lowercase().contains("api key") => SearchError::Auth(message),
            _ => SearchError::Http { status, message },
        }
    }
}

/// Поисковик. `n` — желаемое число результатов (провайдер может вернуть меньше).
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// `brave`, `tavily`, `serper`, `google_cse`, `jina`, `duckduckgo` (как в `config.json`).
    fn id(&self) -> &str;
    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError>;
}

/// Отправить запрос и разобрать JSON-ответ; не-2xx → [`SearchError`] с текстом ошибки API.
pub(crate) async fn send_json<T: serde::de::DeserializeOwned>(req: reqwest::RequestBuilder) -> Result<T, SearchError> {
    let mut resp = req.send().await.map_err(SearchError::from_reqwest)?;
    let status = resp.status();
    let (body, _) = crate::http::read_limited(&mut resp, 4 * 1024 * 1024).await.map_err(SearchError::from_reqwest)?;
    let body = String::from_utf8_lossy(&body);
    if !status.is_success() {
        return Err(SearchError::from_status(status.as_u16(), &body));
    }
    serde_json::from_str(&body).map_err(|e| SearchError::BadResponse(e.to_string()))
}

/// Результат цепочки: кто ответил и кто упал по дороге.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainResult {
    pub provider: String,
    pub hits: Vec<SearchHit>,
    pub failures: Vec<(String, SearchError)>,
}

/// Цепочка поисковиков в порядке из `Config.web_search`: первый непустой ответ выигрывает,
/// ошибка или пустая выдача → следующий.
#[derive(Clone, Default)]
pub struct SearchChain {
    providers: Vec<Arc<dyn SearchProvider>>,
    skipped: Vec<(String, String)>,
}

impl SearchChain {
    pub fn new(providers: Vec<Arc<dyn SearchProvider>>) -> Self {
        Self { providers, skipped: vec![] }
    }

    /// Собрать из конфига: выключенные и без обязательного ключа пропускаются (см. [`Self::skipped`]).
    /// Если не осталось ни одного — добавляется DuckDuckGo (работает без ключа).
    pub fn from_config(cfgs: &[SearchProviderConfig], client: &reqwest::Client) -> Self {
        let mut chain = Self::default();
        for c in cfgs {
            if !c.enabled {
                chain.skipped.push((c.id.clone(), "выключен".into()));
                continue;
            }
            match provider_from_config(c, client) {
                Ok(p) => chain.providers.push(p),
                Err(e) => chain.skipped.push((c.id.clone(), e.to_string())),
            }
        }
        if chain.providers.is_empty() {
            chain.providers.push(Arc::new(DuckDuckGo::new(client.clone())));
        }
        chain
    }

    pub fn provider_ids(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.id().to_string()).collect()
    }

    /// Какие записи конфига не вошли в цепочку и почему.
    pub fn skipped(&self) -> &[(String, String)] {
        &self.skipped
    }

    pub async fn search_detailed(&self, query: &str, n: usize) -> Result<ChainResult, SearchError> {
        if self.providers.is_empty() {
            return Err(SearchError::NoProviders);
        }
        let mut failures = Vec::new();
        let mut empty_from: Option<String> = None;
        for p in &self.providers {
            match p.search(query, n).await {
                Ok(hits) if !hits.is_empty() => {
                    return Ok(ChainResult { provider: p.id().to_string(), hits, failures });
                }
                Ok(_) => {
                    empty_from.get_or_insert_with(|| p.id().to_string());
                }
                Err(e) => failures.push((p.id().to_string(), e)),
            }
        }
        match empty_from {
            // Кто-то честно ответил «ничего не найдено» — это не ошибка.
            Some(provider) => Ok(ChainResult { provider, hits: vec![], failures }),
            None => Err(SearchError::AllFailed(failures)),
        }
    }
}

#[async_trait]
impl SearchProvider for SearchChain {
    fn id(&self) -> &str {
        "chain"
    }
    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        self.search_detailed(query, n).await.map(|r| r.hits)
    }
}

/// Создать поисковик по записи конфига (без учёта `enabled` — для кнопки «Проверить»).
pub fn provider_from_config(
    c: &SearchProviderConfig,
    client: &reqwest::Client,
) -> Result<Arc<dyn SearchProvider>, SearchError> {
    let key = c.api_key.trim();
    let need_key = |id: &str| {
        if key.is_empty() { Err(SearchError::MissingKey(id.to_string())) } else { Ok(key.to_string()) }
    };
    let client = client.clone();
    Ok(match c.id.trim() {
        "duckduckgo" | "ddg" => Arc::new(DuckDuckGo::new(client)),
        "brave" => Arc::new(Brave::new(client, need_key("brave")?)),
        "tavily" => Arc::new(Tavily::new(client, need_key("tavily")?)),
        "serper" => Arc::new(Serper::new(client, need_key("serper")?)),
        "google_cse" | "google" => {
            let key = need_key("google_cse")?;
            let cx = c.extra.trim();
            if cx.is_empty() {
                return Err(SearchError::MissingKey("google_cse (cx в поле extra)".into()));
            }
            Arc::new(GoogleCse::new(client, key, cx.to_string()))
        }
        // У Jina ключ необязателен (без ключа — меньше лимиты).
        "jina" => Arc::new(JinaSearch::new(client, (!key.is_empty()).then(|| key.to_string()))),
        other => return Err(SearchError::UnknownProvider(other.to_string())),
    })
}

/// Тестовый запрос кнопки «Проверить».
pub const VALIDATE_QUERY: &str = "Rust programming language";

/// Итог «Проверить»: «работает ✓ (8 результатов, 0,6 с)».
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidateReport {
    pub ok: bool,
    pub results: usize,
    pub latency_ms: u64,
    pub error: Option<String>,
}

impl ValidateReport {
    /// Строка для UI настроек.
    pub fn summary_ru(&self) -> String {
        let secs = format!("{:.1}", self.latency_ms as f64 / 1000.0).replace('.', ",");
        if self.ok {
            let word = plural_ru(self.results, "результат", "результата", "результатов");
            format!("работает ✓ ({} {word}, {secs} с)", self.results)
        } else {
            format!("не работает ✗: {}", self.error.as_deref().unwrap_or("неизвестная ошибка"))
        }
    }
}

/// Проверить поисковик тестовым запросом.
pub async fn validate(provider: &dyn SearchProvider) -> ValidateReport {
    let start = Instant::now();
    let res = provider.search(VALIDATE_QUERY, 8).await;
    let latency_ms = start.elapsed().as_millis() as u64;
    match res {
        Ok(hits) if !hits.is_empty() => ValidateReport { ok: true, results: hits.len(), latency_ms, error: None },
        Ok(_) => ValidateReport {
            ok: false,
            results: 0,
            latency_ms,
            error: Some("поиск вернул 0 результатов".into()),
        },
        Err(e) => ValidateReport { ok: false, results: 0, latency_ms, error: Some(e.to_string()) },
    }
}

/// Проверить запись конфига (ключ, cx) — то, что вызывает кнопка «Проверить» в настройках.
pub async fn validate_config(c: &SearchProviderConfig, client: &reqwest::Client) -> ValidateReport {
    match provider_from_config(c, client) {
        Ok(p) => validate(p.as_ref()).await,
        Err(e) => ValidateReport { ok: false, results: 0, latency_ms: 0, error: Some(e.to_string()) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_cleanup() {
        let h = SearchHit::cleaned("<b>Rust</b> &amp; Cargo", " https://rust-lang.org/ ", "a <strong>fast</strong>\n lang").unwrap();
        assert_eq!(h.title, "Rust & Cargo");
        assert_eq!(h.url, "https://rust-lang.org/");
        assert_eq!(h.snippet, "a fast lang");
        assert!(SearchHit::cleaned("x", "javascript:alert(1)", "").is_none());
        assert_eq!(SearchHit::cleaned("", "https://a.b/", "").unwrap().title, "https://a.b/");
    }

    #[test]
    fn status_mapping() {
        assert!(matches!(SearchError::from_status(401, "{}"), SearchError::Auth(_)));
        assert!(matches!(SearchError::from_status(432, "{}"), SearchError::RateLimited(_)));
        assert!(matches!(
            SearchError::from_status(400, r#"{"error":{"message":"API key not valid. Please pass a valid API key."}}"#),
            SearchError::Auth(_)
        ));
        assert!(matches!(SearchError::from_status(500, "oops"), SearchError::Http { status: 500, .. }));
    }

    #[test]
    fn report_summary() {
        let r = ValidateReport { ok: true, results: 8, latency_ms: 612, error: None };
        assert_eq!(r.summary_ru(), "работает ✓ (8 результатов, 0,6 с)");
        let r = ValidateReport { ok: false, results: 0, latency_ms: 10, error: Some("ключ не принят: x".into()) };
        assert_eq!(r.summary_ru(), "не работает ✗: ключ не принят: x");
    }

    #[test]
    fn config_building() {
        let client = crate::http::default_client();
        let cfg = |id: &str, key: &str, extra: &str, enabled: bool| SearchProviderConfig {
            id: id.into(),
            api_key: key.into(),
            extra: extra.into(),
            enabled,
        };
        let chain = SearchChain::from_config(
            &[
                cfg("brave", "", "", true),
                cfg("tavily", "tvly-x", "", true),
                cfg("google_cse", "k", "", true),
                cfg("serper", "s", "", false),
                cfg("jina", "", "", true),
                cfg("bing", "x", "", true),
                cfg("duckduckgo", "", "", true),
            ],
            &client,
        );
        assert_eq!(chain.provider_ids(), ["tavily", "jina", "duckduckgo"]);
        let skipped: Vec<_> = chain.skipped().iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(skipped, ["brave", "google_cse", "serper", "bing"]);

        // Ничего не настроено — запасной DuckDuckGo.
        let chain = SearchChain::from_config(&[cfg("brave", "", "", true)], &client);
        assert_eq!(chain.provider_ids(), ["duckduckgo"]);
    }
}
