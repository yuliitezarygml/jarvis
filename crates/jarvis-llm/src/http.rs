//! Общее для HTTP-клиентов: reqwest, авторизация, маппинг ошибок в `LlmError`, разбор SSE, отмена.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use jarvis_core::{CancellationToken, LlmError};
use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::presets::AuthStyle;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Один общий клиент на процесс (пул соединений).
pub(crate) fn shared_client() -> reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(concat!("jarvis-llm/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    })
    .clone()
}

pub(crate) fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path.is_empty() {
        return base.to_string();
    }
    if path.starts_with('/') { format!("{base}{path}") } else { format!("{base}/{path}") }
}

pub(crate) fn apply_auth(
    rb: reqwest::RequestBuilder,
    auth: AuthStyle,
    key: &str,
    extra: &std::collections::BTreeMap<String, String>,
) -> reqwest::RequestBuilder {
    let mut rb = rb;
    for (k, v) in extra {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if key.is_empty() {
        return rb;
    }
    match auth {
        AuthStyle::Bearer => rb.bearer_auth(key),
        AuthStyle::XApiKey => rb.header("x-api-key", key),
        AuthStyle::None => rb,
    }
}

/// Убрать ключ из текста ошибки провайдера (OpenAI, например, повторяет присланный ключ).
pub(crate) fn redact(text: &str, key: &str) -> String {
    if key.len() < 4 {
        return text.to_string();
    }
    text.replace(key, &jarvis_core::config::mask_key(key))
}

/// Retry-After: секунды (в т.ч. дробные) или HTTP-дата; также `retry-after-ms`.
pub(crate) fn retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    if let Some(v) = headers.get("retry-after-ms").and_then(|v| v.to_str().ok())
        && let Ok(ms) = v.trim().parse::<f64>()
        && ms >= 0.0
    {
        return Some(ms.round() as u64);
    }
    let v = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<f64>() {
        return (secs >= 0.0).then(|| (secs * 1000.0).round() as u64);
    }
    let when = httpdate::parse_http_date(v).ok()?;
    Some(when.duration_since(std::time::SystemTime::now()).map(|d| d.as_millis() as u64).unwrap_or(0))
}

/// Человеческий текст ошибки из тела ответа (OpenAI/Groq/OpenRouter/Anthropic/Mistral/xAI форматы).
pub(crate) fn error_message(body: &str) -> String {
    let fallback = || body.chars().take(300).collect::<String>();
    let Ok(v) = serde_json::from_str::<Value>(body) else { return fallback() };
    let pick = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o.get("message").and_then(|m| m.as_str()).map(str::to_string),
            _ => None,
        }
    };
    v.get("error")
        .and_then(pick)
        .or_else(|| v.get("message").and_then(|m| m.as_str()).map(str::to_string))
        .or_else(|| v.get("detail").and_then(pick))
        .unwrap_or_else(fallback)
}

fn looks_like_bad_key(lower: &str) -> bool {
    let about_key = lower.contains("api key") || lower.contains("api_key") || lower.contains("apikey");
    about_key
        && ["invalid", "incorrect", "not valid", "valid api key", "wrong", "expired", "missing"]
            .iter()
            .any(|w| lower.contains(w))
}

fn looks_like_missing_model(lower: &str) -> bool {
    lower.contains("model")
        && [
            "not found",
            "not_found",
            "does not exist",
            "doesn't exist",
            "not exist",
            "unknown model",
            "invalid model",
            "no such model",
            "is unavailable",
            "model unavailable",
            "decommissioned",
            "no endpoints found",
        ]
        .iter()
        .any(|w| lower.contains(w))
}

/// Маппинг HTTP-статуса и тела в `LlmError`.
pub(crate) fn map_status(status: u16, retry_after: Option<u64>, body: &str, model: &str, key: &str) -> LlmError {
    let lower = body.to_lowercase();
    let msg = || redact(&error_message(body), key);
    match status {
        401 | 403 => LlmError::Auth,
        400 if looks_like_bad_key(&lower) => LlmError::Auth,
        404 => LlmError::ModelNotFound(model.to_string()),
        400 | 422 if looks_like_missing_model(&lower) => LlmError::ModelNotFound(model.to_string()),
        // OpenRouter: кончились кредиты/лимит ключа — переключаемся на следующего, как при лимите.
        402 => LlmError::RateLimited { retry_after_ms: retry_after },
        408 => LlmError::Timeout,
        429 => LlmError::RateLimited { retry_after_ms: retry_after },
        500..=599 => LlmError::Server(status),
        _ => LlmError::Other(format!("HTTP {status}: {}", msg())),
    }
}

/// Ошибка внутри тела 200 (OpenRouter может прислать `{"error":{...}}` в т.ч. посреди стрима).
pub(crate) fn map_inline_error(err: &Value, model: &str, key: &str) -> LlmError {
    let code = err.get("code").and_then(|c| c.as_u64().or_else(|| c.as_str().and_then(|s| s.parse().ok())));
    let text = err.to_string();
    match code {
        Some(c @ 400..=599) => map_status(c as u16, None, &serde_json::json!({ "error": err }).to_string(), model, key),
        _ => {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or(&text);
            LlmError::BadResponse(redact(msg, key))
        }
    }
}

pub(crate) fn map_reqwest(e: &reqwest::Error) -> LlmError {
    if e.is_timeout() {
        return LlmError::Timeout;
    }
    // Текст reqwest содержит URL, но не заголовки, — ключ в него не попадает.
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    if e.is_decode() || e.is_body() {
        return LlmError::BadResponse(msg);
    }
    LlmError::Network(msg)
}

/// Отправить запрос с отменой; не-2xx превращается в `LlmError`.
pub(crate) async fn send(
    rb: reqwest::RequestBuilder,
    cancel: &CancellationToken,
    model: &str,
    key: &str,
) -> Result<reqwest::Response, LlmError> {
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(LlmError::Cancelled),
        r = rb.send() => r.map_err(|e| map_reqwest(&e))?,
    };
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(resp);
    }
    let retry = retry_after_ms(resp.headers());
    let body = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(LlmError::Cancelled),
        b = resp.text() => b.unwrap_or_default(),
    };
    let err = map_status(status, retry, &body, model, key);
    tracing::debug!(status, model, error = %err, "ответ провайдера с ошибкой");
    Err(err)
}

/// Прочитать JSON-тело с отменой.
pub(crate) async fn read_json(resp: reqwest::Response, cancel: &CancellationToken, key: &str) -> Result<Value, LlmError> {
    let text = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(LlmError::Cancelled),
        t = resp.text() => t.map_err(|e| map_reqwest(&e))?,
    };
    serde_json::from_str(&text)
        .map_err(|e| LlmError::BadResponse(format!("не JSON ({e}): {}", redact(&text.chars().take(200).collect::<String>(), key))))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Инкрементальный разбор text/event-stream: байты могут резаться где угодно (в т.ч. посреди UTF-8).
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line).into_owned();
            self.line(&line, &mut out);
        }
        out
    }

    pub(crate) fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let line = String::from_utf8_lossy(&rest).trim_end_matches('\r').to_string();
            self.line(&line, &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        if line.starts_with(':') {
            return; // комментарий / keep-alive
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "data" => self.data.push(value.to_string()),
            "event" => self.event = Some(value.to_string()),
            _ => {} // id, retry — не нужны
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if self.data.is_empty() {
            self.event = None;
            return;
        }
        out.push(SseEvent { event: self.event.take(), data: std::mem::take(&mut self.data).join("\n") });
    }
}

type ByteStream = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;

/// Чтение SSE из ответа с отменой и таймаутом простоя между чанками.
pub(crate) struct SseReader {
    stream: ByteStream,
    parser: SseParser,
    queue: VecDeque<SseEvent>,
    done: bool,
    idle: Duration,
}

impl SseReader {
    pub(crate) fn new(resp: reqwest::Response, idle: Duration) -> Self {
        Self {
            stream: Box::pin(resp.bytes_stream()),
            parser: SseParser::default(),
            queue: VecDeque::new(),
            done: false,
            idle,
        }
    }

    pub(crate) async fn next(&mut self, cancel: &CancellationToken) -> Result<Option<SseEvent>, LlmError> {
        loop {
            if let Some(ev) = self.queue.pop_front() {
                return Ok(Some(ev));
            }
            if self.done {
                return Ok(None);
            }
            let chunk = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(LlmError::Cancelled),
                r = tokio::time::timeout(self.idle, self.stream.next()) => match r {
                    Err(_) => return Err(LlmError::Timeout),
                    Ok(c) => c,
                },
            };
            match chunk {
                Some(Ok(bytes)) => self.queue.extend(self.parser.push(&bytes)),
                Some(Err(e)) => return Err(map_reqwest(&e)),
                None => {
                    self.done = true;
                    self.queue.extend(self.parser.finish());
                }
            }
        }
    }
}

/// Хост из URL (для проверки, что ключ не уходит на чужой домен).
pub(crate) fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url).ok()?.host_str().map(|h| h.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderValue, RETRY_AFTER};

    #[test]
    fn sse_split_anywhere() {
        let raw = "event: a\ndata: {\"x\":\"при\u{0432}ет\"}\n\n: ping\n\ndata: line1\ndata: line2\r\n\r\ndata: [DONE]\n\n";
        let bytes = raw.as_bytes();
        // Разрезаем на куски по 1..7 байт, в том числе посреди кириллицы.
        for step in 1..8 {
            let mut p = SseParser::default();
            let mut evs = Vec::new();
            for ch in bytes.chunks(step) {
                evs.extend(p.push(ch));
            }
            evs.extend(p.finish());
            assert_eq!(evs.len(), 3, "step {step}");
            assert_eq!(evs[0], SseEvent { event: Some("a".into()), data: "{\"x\":\"привет\"}".into() });
            assert_eq!(evs[1].data, "line1\nline2");
            assert_eq!(evs[2].data, "[DONE]");
        }
    }

    #[test]
    fn sse_last_event_without_blank_line() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: tail").is_empty());
        assert_eq!(p.finish(), vec![SseEvent { event: None, data: "tail".into() }]);
    }

    #[test]
    fn status_mapping() {
        assert_eq!(map_status(401, None, "", "m", ""), LlmError::Auth);
        assert_eq!(map_status(403, None, "", "m", ""), LlmError::Auth);
        assert_eq!(map_status(404, None, "", "m", ""), LlmError::ModelNotFound("m".into()));
        let groq = r#"{"error":{"message":"The model `x` does not exist or you do not have access to it.","type":"invalid_request_error","code":"model_not_found"}}"#;
        assert_eq!(map_status(400, None, groq, "x", ""), LlmError::ModelNotFound("x".into()));
        let zen = r#"{"error":{"type":"server_error","message":"Upstream request failed: Model is unavailable."}}"#;
        assert_eq!(map_status(400, None, zen, "x", ""), LlmError::ModelNotFound("x".into()));
        let gemini = r#"{"error":{"code":400,"message":"Please pass a valid API key","status":"INVALID_ARGUMENT"}}"#;
        assert_eq!(map_status(400, None, gemini, "m", ""), LlmError::Auth);
        let xai = r#"{"code":"invalid-argument","error":"Incorrect API key provided."}"#;
        assert_eq!(map_status(400, None, xai, "m", ""), LlmError::Auth);
        assert_eq!(map_status(429, Some(1500), "", "m", ""), LlmError::RateLimited { retry_after_ms: Some(1500) });
        assert_eq!(map_status(503, None, "", "m", ""), LlmError::Server(503));
        assert_eq!(map_status(529, None, "", "m", ""), LlmError::Server(529));
        match map_status(400, None, r#"{"error":{"message":"bad temperature"}}"#, "m", "") {
            LlmError::Other(s) => assert!(s.contains("bad temperature")),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn key_is_redacted_from_errors() {
        let key = "sk-proj-SECRETSECRETSECRET1234";
        let body = format!(r#"{{"error":{{"message":"Incorrect thing: {key}"}}}}"#);
        let e = map_status(418, None, &body, "m", key);
        let s = e.to_string();
        assert!(!s.contains("SECRETSECRET"), "{s}");
        assert!(s.contains("1234"));
    }

    #[test]
    fn retry_after_parsing() {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_static("2"));
        assert_eq!(retry_after_ms(&h), Some(2000));
        h.insert(RETRY_AFTER, HeaderValue::from_static("0.5"));
        assert_eq!(retry_after_ms(&h), Some(500));
        h.insert("retry-after-ms", HeaderValue::from_static("120"));
        assert_eq!(retry_after_ms(&h), Some(120));
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
        assert_eq!(retry_after_ms(&h), Some(0));
        assert_eq!(retry_after_ms(&HeaderMap::new()), None);
    }

    #[test]
    fn inline_error() {
        let e = serde_json::json!({"code": 429, "message": "slow down"});
        assert_eq!(map_inline_error(&e, "m", ""), LlmError::RateLimited { retry_after_ms: None });
        let e = serde_json::json!({"message": "weird"});
        assert_eq!(map_inline_error(&e, "m", ""), LlmError::BadResponse("weird".into()));
    }

    #[test]
    fn urls() {
        assert_eq!(join_url("https://a/v1/", "/models"), "https://a/v1/models");
        assert_eq!(join_url("https://a/v1", "key"), "https://a/v1/key");
        assert_eq!(host_of("https://OpenCode.ai/inference/anthropic/v1").as_deref(), Some("opencode.ai"));
    }
}
