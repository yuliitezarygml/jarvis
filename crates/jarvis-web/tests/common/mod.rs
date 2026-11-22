//! Общие помощники интеграционных тестов: фикстуры, мок-модель, мок-поиск, мок-страницы, мок-браузер.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use jarvis_core::{CancellationToken, ChatMessage, ChatModel, ChatRequest, ChatResponse, LlmError};
use jarvis_web::search::{SearchError, SearchHit, SearchProvider};
use jarvis_web::{FetchError, FetchOptions, Fetcher, Opener, Page, PageFetcher};

pub fn fixture(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Fetcher для мок-сервера на 127.0.0.1 (внутренняя сеть разрешена только в тестах).
pub fn test_fetcher(opts: FetchOptions) -> Fetcher {
    Fetcher::new(FetchOptions {
        allow_private_hosts: true,
        ..opts
    })
}

/// Крошечный валидный PDF с одной строкой текста (Helvetica, правильная таблица xref).
pub fn tiny_pdf(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 18 Tf 72 720 Td ({text}) Tj ET");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

type Responder = Box<dyn Fn(&ChatRequest, usize) -> Result<ChatMessage, LlmError> + Send + Sync>;

/// Мок-модель: записывает все запросы и отвечает функцией (номер вызова → ответ).
pub struct RecordingModel {
    pub requests: Mutex<Vec<ChatRequest>>,
    responder: Responder,
}

impl RecordingModel {
    pub fn new(
        f: impl Fn(&ChatRequest, usize) -> Result<ChatMessage, LlmError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            requests: Mutex::new(vec![]),
            responder: Box::new(f),
        }
    }
    /// Первый вызов — план (JSON), второй и далее — синтез.
    pub fn planner_then(plan: &str, answer: ChatMessage) -> Self {
        let plan = plan.to_string();
        Self::new(move |_, i| {
            Ok(if i == 0 {
                ChatMessage::assistant(plan.clone())
            } else {
                answer.clone()
            })
        })
    }
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChatModel for RecordingModel {
    fn id(&self) -> String {
        "mock/recorder".into()
    }
    async fn chat(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, LlmError> {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        let i = {
            let mut r = self.requests.lock().unwrap();
            r.push(req.clone());
            r.len() - 1
        };
        let message = (self.responder)(&req, i)?;
        Ok(ChatResponse {
            message,
            model: self.id(),
            usage: None,
        })
    }
}

/// Мок-поиск: запрос → выдача; неизвестный запрос → пусто. Записывает запросы.
#[derive(Default)]
pub struct StaticSearch {
    pub id: String,
    pub map: HashMap<String, Vec<SearchHit>>,
    pub default: Vec<SearchHit>,
    pub queries: Mutex<Vec<String>>,
    pub fail: Option<SearchError>,
}

impl StaticSearch {
    pub fn with_default(hits: Vec<SearchHit>) -> Self {
        Self {
            id: "static".into(),
            default: hits,
            ..Default::default()
        }
    }
    pub fn failing(id: &str, e: SearchError) -> Self {
        Self {
            id: id.into(),
            fail: Some(e),
            ..Default::default()
        }
    }
}

#[async_trait]
impl SearchProvider for StaticSearch {
    fn id(&self) -> &str {
        &self.id
    }
    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        self.queries.lock().unwrap().push(query.to_string());
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        let hits = self.map.get(query).unwrap_or(&self.default);
        Ok(hits.iter().take(n).cloned().collect())
    }
}

pub fn hit(title: &str, url: &str, snippet: &str) -> SearchHit {
    SearchHit {
        title: title.into(),
        url: url.into(),
        snippet: snippet.into(),
    }
}

/// Мок-страницы: адрес → HTML (проходит через настоящее извлечение) или ошибка.
#[derive(Default)]
pub struct MapFetcher {
    pub pages: HashMap<String, Result<String, FetchError>>,
    pub fetched: Mutex<Vec<String>>,
}

impl MapFetcher {
    pub fn html(mut self, url: &str, html: &str) -> Self {
        self.pages.insert(url.into(), Ok(html.into()));
        self
    }
    pub fn error(mut self, url: &str, e: FetchError) -> Self {
        self.pages.insert(url.into(), Err(e));
        self
    }
}

#[async_trait]
impl PageFetcher for MapFetcher {
    async fn fetch_page(&self, url: &str) -> Result<Page, FetchError> {
        self.fetched.lock().unwrap().push(url.to_string());
        match self.pages.get(url) {
            Some(Ok(html)) => {
                let ex = jarvis_web::extract::html_to_markdown(html, Some(url));
                Ok(Page {
                    url: url.into(),
                    final_url: url.into(),
                    title: ex.title,
                    markdown: ex.markdown,
                    truncated: false,
                    content_type: "text/html".into(),
                })
            }
            Some(Err(e)) => Err(e.clone()),
            None => Err(FetchError::Http(404)),
        }
    }
}

/// Мок-браузер: запоминает, что «открыли».
#[derive(Default)]
pub struct RecordingOpener(pub Mutex<Vec<String>>);

impl Opener for RecordingOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(url.to_string());
        Ok(())
    }
}
