//! DuckDuckGo HTML (`html.duckduckgo.com/html/`) — без ключа, запасной поисковик по умолчанию.
//! Форма: POST `q`, `b`, `kl`. Ссылки бывают прямые или через редирект `//duckduckgo.com/l/?uddg=<url>`.

use async_trait::async_trait;
use dom_query::Document;

use super::{SearchError, SearchHit, SearchProvider};
use crate::http::BROWSER_USER_AGENT;

pub const DEFAULT_BASE: &str = "https://html.duckduckgo.com";

pub struct DuckDuckGo {
    client: reqwest::Client,
    base_url: String,
    /// Регион `kl` (`ru-ru`, `us-en`, `wt-wt`); пусто — DuckDuckGo решает сам.
    region: String,
}

impl DuckDuckGo {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client, base_url: DEFAULT_BASE.into(), region: String::new() }
    }
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }
    pub fn with_region(mut self, kl: impl Into<String>) -> Self {
        self.region = kl.into();
        self
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGo {
    fn id(&self) -> &str {
        "duckduckgo"
    }

    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        let url = format!("{}/html/", self.base_url.trim_end_matches('/'));
        let mut resp = self
            .client
            .post(&url)
            .header(reqwest::header::USER_AGENT, BROWSER_USER_AGENT)
            .header(reqwest::header::REFERER, "https://html.duckduckgo.com/")
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
            .header(reqwest::header::ACCEPT_LANGUAGE, "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7")
            .form(&[("q", query), ("b", ""), ("kl", self.region.as_str())])
            .send()
            .await
            .map_err(SearchError::from_reqwest)?;
        let status = resp.status();
        let (body, _) = crate::http::read_limited(&mut resp, 2 * 1024 * 1024).await.map_err(SearchError::from_reqwest)?;
        let html = String::from_utf8_lossy(&body);
        if !status.is_success() && status.as_u16() != 202 {
            return Err(SearchError::from_status(status.as_u16(), &html));
        }
        parse_results(&html, n)
    }
}

/// Разобрать страницу выдачи. Капча (`anomaly-modal`, HTTP 202) → [`SearchError::Blocked`].
pub fn parse_results(html: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
    let doc = Document::from(html);
    if doc.select(".anomaly-modal__modal, #challenge-form").exists() {
        return Err(SearchError::Blocked);
    }
    let mut hits: Vec<SearchHit> = Vec::new();
    for res in doc.select("div.result").iter() {
        if res.has_class("result--ad") || res.has_class("result--ad--small") {
            continue;
        }
        let link = res.select("a.result__a").first();
        let Some(href) = link.attr("href") else { continue };
        let Some(url) = decode_href(&href) else { continue };
        if is_ad_url(&url) {
            continue;
        }
        let title = link.text();
        let snippet = res.select(".result__snippet").first().text();
        if let Some(hit) = SearchHit::cleaned(&title, &url, &snippet)
            && !hits.iter().any(|h| h.url == hit.url)
        {
            hits.push(hit);
        }
        if hits.len() >= n {
            break;
        }
    }
    if hits.is_empty()
        && !doc.select(".no-results").exists()
        && !doc.select("#links, .serp__results").exists()
    {
        // Не страница выдачи вообще — DuckDuckGo поменял разметку или вернул что-то другое.
        return Err(SearchError::BadResponse("не похоже на страницу выдачи DuckDuckGo".into()));
    }
    Ok(hits)
}

/// Прямой адрес из href выдачи: `//duckduckgo.com/l/?uddg=https%3A%2F%2F…&rut=…` → `https://…`.
pub fn decode_href(href: &str) -> Option<String> {
    let href = href.trim();
    let absolute = if href.starts_with("//") {
        format!("https:{href}")
    } else if href.starts_with('/') {
        format!("https://duckduckgo.com{href}")
    } else {
        href.to_string()
    };
    let url = url::Url::parse(&absolute).ok()?;
    let host = url.host_str().unwrap_or_default();
    if host.ends_with("duckduckgo.com") && url.path().starts_with("/l/") {
        let target = url.query_pairs().find(|(k, _)| k == "uddg").map(|(_, v)| v.into_owned())?;
        return url::Url::parse(&target).ok().map(String::from);
    }
    Some(url.into())
}

fn is_ad_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.host_str().is_some_and(|h| h.ends_with("duckduckgo.com")) && u.path().starts_with("/y.js")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_uddg_redirects() {
        assert_eq!(
            decode_href("//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2Flearn%3Fa%3D1%26b%3D2&rut=abc").as_deref(),
            Some("https://www.rust-lang.org/learn?a=1&b=2")
        );
        assert_eq!(
            decode_href("/l/?kh=-1&uddg=https%3A%2F%2Fru.wikipedia.org%2Fwiki%2FRust").as_deref(),
            Some("https://ru.wikipedia.org/wiki/Rust")
        );
        assert_eq!(decode_href("https://rust-lang.org/").as_deref(), Some("https://rust-lang.org/"));
        assert!(decode_href("//duckduckgo.com/l/?rut=only").is_none());
    }
}
