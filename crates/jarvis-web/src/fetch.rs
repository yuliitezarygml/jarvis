//! `web_fetch`: страница → очищенный Markdown для модели.
//! reqwest (rustls), UA `JarvisBot/1.0`, таймаут 15 с, тело ≤ 2 МБ, кодировка, readability, HTML → Markdown,
//! PDF → текст, лимит ~12k токенов (48 000 символов) с пометкой обрезки; для JS-страниц — Jina Reader (опционально).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use jarvis_core::Config;
use serde::{Deserialize, Serialize};

use crate::extract;
use crate::http::{self, check_url, private_address_error, read_limited};
use crate::text::{tidy, truncate_markdown};

/// Прочитанная страница.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    /// Запрошенный адрес (нормализованный).
    pub url: String,
    /// Адрес после редиректов.
    pub final_url: String,
    pub title: String,
    /// Основное содержимое в Markdown (для PDF и text/* — текст).
    pub markdown: String,
    /// Текст или тело страницы обрезаны по лимиту.
    pub truncated: bool,
    /// MIME без параметров: `text/html`, `application/pdf`, `text/plain`; `text/markdown` — через Jina Reader.
    pub content_type: String,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FetchError {
    #[error("некорректный адрес: {0}")]
    InvalidUrl(String),
    #[error("адрес запрещён: {0}")]
    Forbidden(String),
    #[error("сайт ответил HTTP {0}")]
    Http(u16),
    #[error("сеть недоступна: {0}")]
    Network(String),
    #[error("таймаут")]
    Timeout,
    #[error("неподдерживаемое содержимое: {0}")]
    Unsupported(String),
    #[error("не удалось прочитать PDF: {0}")]
    Pdf(String),
    #[error("на странице нет текста (возможно, она строится JavaScript'ом)")]
    Empty,
}

impl FetchError {
    fn from_reqwest(e: reqwest::Error) -> Self {
        if let Some(msg) = private_address_error(&e) {
            FetchError::Forbidden(msg)
        } else if e.is_timeout() {
            FetchError::Timeout
        } else {
            FetchError::Network(e.without_url().to_string())
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub timeout: Duration,
    /// Лимит тела HTML/текста (2 МБ). Больше — читается начало, страница помечается обрезанной.
    pub max_bytes: usize,
    /// Лимит PDF (обрезанный PDF не разобрать, поэтому отдельный и побольше).
    pub max_pdf_bytes: usize,
    /// Лимит текста для модели: ~12k токенов ≈ 48 000 символов.
    pub max_chars: usize,
    /// Если из HTML извлечено меньше — страница, вероятно, строится JS (тогда пробуем Jina Reader).
    pub min_chars: usize,
    /// Разрешить запасной Jina Reader (`r.jina.ai/<url>`): адрес страницы уходит в Jina.
    pub jina_reader: bool,
    pub jina_key: Option<String>,
    pub jina_reader_base: String,
    /// Разрешить localhost/LAN (только для тестов и явной настройки).
    pub allow_private_hosts: bool,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            timeout: http::DEFAULT_TIMEOUT,
            max_bytes: 2 * 1024 * 1024,
            max_pdf_bytes: 10 * 1024 * 1024,
            max_chars: 48_000,
            min_chars: 250,
            jina_reader: false,
            jina_key: None,
            jina_reader_base: "https://r.jina.ai".into(),
            allow_private_hosts: false,
        }
    }
}

impl FetchOptions {
    /// Jina Reader включается, если пользователь сам включил Jina в списке поисковиков (ключ берётся оттуда).
    pub fn from_config(cfg: &Config) -> Self {
        let jina = cfg.web_search.iter().find(|s| s.id == "jina" && s.enabled);
        Self {
            jina_reader: jina.is_some(),
            jina_key: jina.map(|j| j.api_key.trim().to_string()).filter(|k| !k.is_empty()),
            ..Self::default()
        }
    }
}

/// Источник страниц для исследования (реальный — [`Fetcher`]; в тестах — мок).
#[async_trait]
pub trait PageFetcher: Send + Sync {
    async fn fetch_page(&self, url: &str) -> Result<Page, FetchError>;
}

pub struct Fetcher {
    client: reqwest::Client,
    opts: FetchOptions,
}

/// Прочитать страницу с настройками по умолчанию.
pub async fn fetch(url: &str) -> Result<Page, FetchError> {
    static DEFAULT: OnceLock<Arc<Fetcher>> = OnceLock::new();
    DEFAULT.get_or_init(|| Arc::new(Fetcher::new(FetchOptions::default()))).fetch(url).await
}

enum Kind {
    Html,
    Pdf,
    Text,
    Other,
}

impl Fetcher {
    /// Паникует, только если не удалось инициализировать TLS (как `reqwest::Client::new`).
    pub fn new(opts: FetchOptions) -> Self {
        let client = http::build_page_client(opts.timeout, opts.allow_private_hosts)
            .expect("не удалось создать HTTP-клиент (TLS)");
        Self { client, opts }
    }

    pub fn from_config(cfg: &Config) -> Self {
        Self::new(FetchOptions::from_config(cfg))
    }

    pub fn options(&self) -> &FetchOptions {
        &self.opts
    }

    pub async fn fetch(&self, raw_url: &str) -> Result<Page, FetchError> {
        let url = normalize_url(raw_url)?;
        check_url(&url, self.opts.allow_private_hosts).map_err(FetchError::Forbidden)?;

        let mut resp = self
            .client
            .get(url.clone())
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,application/pdf;q=0.9,text/plain;q=0.8,*/*;q=0.5",
            )
            .header(reqwest::header::ACCEPT_LANGUAGE, "ru-RU,ru;q=0.9,en;q=0.8")
            .send()
            .await
            .map_err(FetchError::from_reqwest)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(FetchError::Http(status.as_u16()));
        }
        let final_url = resp.url().to_string();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mime = content_type
            .as_deref()
            .and_then(|c| c.split(';').next())
            .map(|m| m.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let limit = if mime == "application/pdf" { self.opts.max_pdf_bytes } else { self.opts.max_bytes };
        let (body, body_truncated) = read_limited(&mut resp, limit).await.map_err(FetchError::from_reqwest)?;

        let (title, markdown, page_type) = match classify(&mime, &body) {
            Kind::Pdf => {
                if body_truncated || body.len() >= self.opts.max_pdf_bytes {
                    return Err(FetchError::Unsupported(format!(
                        "PDF больше {} МБ",
                        self.opts.max_pdf_bytes / (1024 * 1024)
                    )));
                }
                let text = tokio::task::spawn_blocking(move || extract::pdf_to_text(&body))
                    .await
                    .map_err(|_| FetchError::Pdf("сбой разбора PDF".into()))?
                    .map_err(FetchError::Pdf)?;
                let title = text
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .map(|l| crate::text::truncate_plain(l, 120))
                    .unwrap_or_else(|| file_name(&final_url));
                (title, text, "application/pdf".to_string())
            }
            Kind::Html => {
                let ct = content_type.clone();
                let base = final_url.clone();
                let ex = tokio::task::spawn_blocking(move || {
                    let html = extract::decode_text(&body, ct.as_deref(), true);
                    extract::html_to_markdown(&html, Some(&base))
                })
                .await
                .map_err(|_| FetchError::Unsupported("сбой разбора HTML".into()))?;
                let mut out = (ex.title, ex.markdown, "text/html".to_string());
                if self.opts.jina_reader
                    && out.1.chars().count() < self.opts.min_chars
                    && let Ok((title, md)) = self.jina_read(&final_url).await
                    && md.chars().count() > out.1.chars().count()
                {
                    out = (if title.is_empty() { out.0 } else { title }, md, "text/markdown".into());
                }
                out
            }
            Kind::Text => {
                let text = tidy(&extract::decode_text(&body, content_type.as_deref(), false));
                (file_name(&final_url), text, if mime.is_empty() { "text/plain".into() } else { mime.clone() })
            }
            Kind::Other => return Err(FetchError::Unsupported(if mime.is_empty() { "двоичные данные".into() } else { mime })),
        };

        if markdown.trim().is_empty() {
            return Err(FetchError::Empty);
        }
        let (mut markdown, text_truncated) = truncate_markdown(&markdown, self.opts.max_chars);
        if body_truncated && !text_truncated {
            markdown.push_str(&format!(
                "\n\n[…страница больше {} МБ, прочитано только начало]",
                self.opts.max_bytes / (1024 * 1024)
            ));
        }
        Ok(Page {
            url: url.to_string(),
            final_url,
            title: if title.is_empty() { file_name(url.as_str()) } else { title },
            markdown,
            truncated: body_truncated || text_truncated,
            content_type: page_type,
        })
    }

    /// Jina Reader: `GET https://r.jina.ai/<url>` с `Accept: application/json` → `{data: {title, content}}`.
    async fn jina_read(&self, url: &str) -> Result<(String, String), FetchError> {
        let endpoint = format!("{}/{}", self.opts.jina_reader_base.trim_end_matches('/'), url);
        let mut req = self.client.get(endpoint).header(reqwest::header::ACCEPT, "application/json");
        if let Some(k) = &self.opts.jina_key {
            req = req.bearer_auth(k);
        }
        let mut resp = req.send().await.map_err(FetchError::from_reqwest)?;
        if !resp.status().is_success() {
            return Err(FetchError::Http(resp.status().as_u16()));
        }
        let (body, _) = read_limited(&mut resp, self.opts.max_bytes).await.map_err(FetchError::from_reqwest)?;
        let v: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| FetchError::Unsupported(format!("ответ Jina Reader: {e}")))?;
        let s = |p: &str| v.pointer(p).and_then(|x| x.as_str()).unwrap_or_default().trim().to_string();
        Ok((s("/data/title"), tidy(&s("/data/content"))))
    }
}

#[async_trait]
impl PageFetcher for Fetcher {
    async fn fetch_page(&self, url: &str) -> Result<Page, FetchError> {
        self.fetch(url).await
    }
}

/// `example.com/x` → `https://example.com/x`; фрагмент `#…` отбрасывается.
pub(crate) fn normalize_url(raw: &str) -> Result<url::Url, FetchError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(FetchError::InvalidUrl("пустой адрес".into()));
    }
    let with_scheme = if raw.contains("://") || raw.starts_with("mailto:") || raw.starts_with("file:") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let mut url = url::Url::parse(&with_scheme).map_err(|e| FetchError::InvalidUrl(format!("{raw}: {e}")))?;
    url.set_fragment(None);
    Ok(url)
}

fn classify(mime: &str, body: &[u8]) -> Kind {
    if mime == "application/pdf" || body.starts_with(b"%PDF-") {
        return Kind::Pdf;
    }
    match mime {
        "text/html" | "application/xhtml+xml" => return Kind::Html,
        m if m.starts_with("text/")
            || m.ends_with("json")
            || m.ends_with("+xml")
            || m == "application/xml"
            || m == "application/javascript" =>
        {
            return Kind::Text;
        }
        "" | "application/octet-stream" | "binary/octet-stream" => {}
        _ => return Kind::Other,
    }
    // Тип не указан — смотрим на содержимое.
    let head = String::from_utf8_lossy(&body[..body.len().min(512)]).trim_start().to_ascii_lowercase();
    if ["<!doctype html", "<html", "<head", "<body", "<!--"].iter().any(|p| head.starts_with(p)) {
        Kind::Html
    } else if std::str::from_utf8(&body[..body.len().min(4096)]).is_ok() {
        Kind::Text
    } else {
        Kind::Other
    }
}

/// Последний сегмент пути или хост — заголовок для PDF/текста без заголовка.
fn file_name(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            let seg = u.path_segments()?.rev().find(|s| !s.is_empty())?.to_string();
            Some(percent_decode(&seg))
        })
        .or_else(|| url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)))
        .unwrap_or_else(|| url.to_string())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalization() {
        assert_eq!(normalize_url("rust-lang.org").unwrap().as_str(), "https://rust-lang.org/");
        assert_eq!(normalize_url(" https://a.b/x#frag ").unwrap().as_str(), "https://a.b/x");
        assert!(normalize_url("").is_err());
        assert!(normalize_url("http://").is_err());
    }

    #[test]
    fn names() {
        assert_eq!(file_name("https://a.b/docs/%D0%BE%D1%82%D1%87%D1%91%D1%82.pdf"), "отчёт.pdf");
        assert_eq!(file_name("https://a.b/"), "a.b");
    }

    #[test]
    fn classification() {
        assert!(matches!(classify("", b"%PDF-1.4 ..."), Kind::Pdf));
        assert!(matches!(classify("", b"  <!DOCTYPE html><html>"), Kind::Html));
        assert!(matches!(classify("application/json", b"{}"), Kind::Text));
        assert!(matches!(classify("image/png", b"\x89PNG"), Kind::Other));
        assert!(matches!(classify("", &[0xff, 0xfe, 0x00, 0x81]), Kind::Other));
    }
}
