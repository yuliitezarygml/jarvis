//! Клиент Anthropic Messages API (`POST {base}/messages`): Claude напрямую и модели OpenCode Zen с родным `/messages`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jarvis_core::{
    CancellationToken, ChatEvent, ChatMessage, ChatModel, ChatRequest, ChatResponse, ImageData, LlmError, Role,
    ToolCall, Usage,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::http::{self, SseReader};
use crate::openai::{DEFAULT_IDLE_TIMEOUT, DEFAULT_REQUEST_TIMEOUT, parse_args};
use crate::presets::{AuthStyle, ProviderSpec};
use crate::think::ThinkFilter;

/// `max_tokens` обязателен в Messages API.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
/// С этой версии `temperature`/`top_p`/`top_k` убраны (400 на любой модели).
const VERSION_WITHOUT_TEMPERATURE: &str = "2026-10-21";

#[derive(Clone)]
pub struct AnthropicClient {
    spec: Arc<ProviderSpec>,
    model: String,
    base_url: String,
    auth: AuthStyle,
    version: String,
    http: reqwest::Client,
    request_timeout: Duration,
    idle_timeout: Duration,
    default_max_tokens: u32,
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient").field("id", &self.id()).field("base_url", &self.base_url).finish()
    }
}

/// `https://api.anthropic.com` → `https://api.anthropic.com/v1` (если путь не указан).
pub(crate) fn normalize_base(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    match reqwest::Url::parse(url) {
        Ok(u) if u.path() == "/" || u.path().is_empty() => format!("{url}/v1"),
        _ => url.to_string(),
    }
}

/// Заголовки Anthropic: ключ + `anthropic-version`.
pub(crate) fn anthropic_request(
    rb: reqwest::RequestBuilder,
    spec: &ProviderSpec,
    auth: AuthStyle,
    version: &str,
) -> reqwest::RequestBuilder {
    http::apply_auth(rb, auth, &spec.api_key, &spec.extra_headers).header("anthropic-version", version)
}

impl AnthropicClient {
    pub fn new(spec: ProviderSpec, model: impl Into<String>) -> Self {
        Self::from_arc(Arc::new(spec), model)
    }

    pub fn from_arc(spec: Arc<ProviderSpec>, model: impl Into<String>) -> Self {
        Self {
            base_url: normalize_base(&spec.base_url),
            auth: spec.auth,
            version: spec.anthropic_version.clone(),
            spec,
            model: model.into(),
            http: http::shared_client(),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            default_max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = normalize_base(&base_url.into());
        self
    }

    pub fn with_auth(mut self, auth: AuthStyle) -> Self {
        self.auth = auth;
        self
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    pub fn with_timeouts(mut self, request: Duration, idle: Duration) -> Self {
        self.request_timeout = request;
        self.idle_timeout = idle;
        self
    }

    pub fn with_default_max_tokens(mut self, n: u32) -> Self {
        self.default_max_tokens = n;
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn build_body(&self, req: &ChatRequest, stream: bool) -> Value {
        let (system, messages) = to_anthropic_messages(&req.messages);
        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens.unwrap_or(self.default_max_tokens),
            "messages": messages,
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(
                req.tools
                    .iter()
                    .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.parameters }))
                    .collect(),
            );
        }
        if let Some(t) = req.temperature
            && self.version.as_str() < VERSION_WITHOUT_TEMPERATURE
        {
            body["temperature"] = json!(t);
        }
        if stream {
            body["stream"] = json!(true);
        }
        body
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let url = http::join_url(&self.base_url, "/messages");
        anthropic_request(self.http.post(url).json(body), &self.spec, self.auth, &self.version)
    }

    fn model_ref(&self) -> String {
        format!("{}/{}", self.spec.id, self.model)
    }
}

fn image_block(img: &ImageData) -> Value {
    json!({ "type": "image", "source": { "type": "base64", "media_type": img.mime, "data": img.base64 } })
}

fn text_block(s: &str) -> Value {
    json!({ "type": "text", "text": s })
}

/// `ChatMessage` → (system, messages). System-сообщения уходят в отдельное поле; соседние сообщения одной роли
/// склеиваются (tool_result-ы — user-сообщение); пустые текстовые блоки не отправляются (API их отклоняет).
pub(crate) fn to_anthropic_messages(msgs: &[ChatMessage]) -> (String, Vec<Value>) {
    let mut system: Vec<&str> = Vec::new();
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for m in msgs {
        let (role, blocks): (&'static str, Vec<Value>) = match m.role {
            Role::System => {
                if !m.content.trim().is_empty() {
                    system.push(&m.content);
                }
                continue;
            }
            Role::User => {
                // Картинки перед текстом — так рекомендует документация.
                let mut b: Vec<Value> = m.images.iter().map(image_block).collect();
                if !m.content.trim().is_empty() {
                    b.push(text_block(&m.content));
                }
                ("user", b)
            }
            Role::Assistant => {
                let mut b = Vec::new();
                if !m.content.trim().is_empty() {
                    b.push(text_block(&m.content));
                }
                for c in &m.tool_calls {
                    let input = match &c.arguments {
                        Value::Object(_) => c.arguments.clone(),
                        Value::Null => json!({}),
                        other => json!({ "value": other }),
                    };
                    b.push(json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": input }));
                }
                ("assistant", b)
            }
            Role::Tool => {
                let content = if m.images.is_empty() {
                    json!(if m.content.is_empty() { "(пустой результат)" } else { m.content.as_str() })
                } else {
                    let mut parts = Vec::new();
                    if !m.content.is_empty() {
                        parts.push(text_block(&m.content));
                    }
                    parts.extend(m.images.iter().map(image_block));
                    Value::Array(parts)
                };
                (
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                        "content": content,
                    })],
                )
            }
        };
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((r, b)) if *r == role => b.extend(blocks),
            _ => out.push((role, blocks)),
        }
    }
    let messages = out.into_iter().map(|(role, content)| json!({ "role": role, "content": content })).collect();
    (system.join("\n\n"), messages)
}

/// Тип ошибки Anthropic (`error.type`) → `LlmError` (для `event: error` в стриме и тел ответов).
pub(crate) fn map_error_type(err: &Value, model: &str, key: &str) -> LlmError {
    let t = err.get("type").and_then(|t| t.as_str()).unwrap_or_default();
    let msg = http::redact(err.get("message").and_then(|m| m.as_str()).unwrap_or(t), key);
    match t {
        "authentication_error" | "permission_error" => LlmError::Auth,
        "not_found_error" => LlmError::ModelNotFound(model.to_string()),
        "rate_limit_error" => LlmError::RateLimited { retry_after_ms: None },
        "overloaded_error" => LlmError::Server(529),
        "api_error" => LlmError::Server(500),
        "timeout_error" => LlmError::Timeout,
        _ => LlmError::Other(format!("{t}: {msg}")),
    }
}

pub(crate) fn parse_response(v: &Value, model_ref: &str, model: &str, key: &str) -> Result<ChatResponse, LlmError> {
    if v.get("type").and_then(|t| t.as_str()) == Some("error") {
        return Err(map_error_type(&v["error"], model, key));
    }
    let blocks = v
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| LlmError::BadResponse("в ответе нет content".into()))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for b in blocks {
        match b["type"].as_str() {
            Some("text") => text.push_str(b["text"].as_str().unwrap_or_default()),
            Some("tool_use") => calls.push(ToolCall {
                id: b["id"].as_str().unwrap_or_default().to_string(),
                name: b["name"].as_str().unwrap_or_default().to_string(),
                arguments: parse_args(&b["input"]),
            }),
            _ => {} // thinking, redacted_thinking, server_tool_use…
        }
    }
    let usage = v.get("usage").map(|u| Usage {
        input_tokens: u["input_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: u["output_tokens"].as_u64().unwrap_or(0) as u32,
    });
    Ok(ChatResponse {
        message: ChatMessage {
            role: Role::Assistant,
            content: crate::think::strip_think(&text),
            images: vec![],
            tool_calls: calls,
            tool_call_id: None,
        },
        model: model_ref.to_string(),
        usage,
    })
}

#[derive(Debug, Default)]
struct ToolBlock {
    id: String,
    name: String,
    json: String,
}

/// Сборка SSE-стрима Messages API.
#[derive(Debug, Default)]
pub(crate) struct AnthropicStreamAcc {
    pub text: String,
    think: ThinkFilter,
    tools: BTreeMap<u64, ToolBlock>,
    pub calls: Vec<ToolCall>,
    pub usage: Usage,
    pub done: bool,
}

/// Результат обработки одного события.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct StepOut {
    pub text: String,
    pub call: Option<ToolCall>,
}

impl AnthropicStreamAcc {
    pub(crate) fn on_event(&mut self, v: &Value, model: &str, key: &str) -> Result<StepOut, LlmError> {
        let mut out = StepOut::default();
        match v["type"].as_str().unwrap_or_default() {
            "message_start" => {
                let u = &v["message"]["usage"];
                self.usage.input_tokens = u["input_tokens"].as_u64().unwrap_or(0) as u32;
                self.usage.output_tokens = u["output_tokens"].as_u64().unwrap_or(0) as u32;
            }
            "content_block_start" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                let b = &v["content_block"];
                match b["type"].as_str() {
                    Some("tool_use") => {
                        self.tools.insert(
                            idx,
                            ToolBlock {
                                id: b["id"].as_str().unwrap_or_default().into(),
                                name: b["name"].as_str().unwrap_or_default().into(),
                                json: String::new(),
                            },
                        );
                    }
                    Some("text") => out.text = self.push_text(b["text"].as_str().unwrap_or_default()),
                    _ => {}
                }
            }
            "content_block_delta" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                let d = &v["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => out.text = self.push_text(d["text"].as_str().unwrap_or_default()),
                    Some("input_json_delta") => {
                        if let Some(t) = self.tools.get_mut(&idx) {
                            t.json.push_str(d["partial_json"].as_str().unwrap_or_default());
                        }
                    }
                    _ => {} // thinking_delta, signature_delta, citations_delta…
                }
            }
            "content_block_stop" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                if let Some(t) = self.tools.remove(&idx) {
                    let call = Self::finish_tool(t);
                    self.calls.push(call.clone());
                    out.call = Some(call);
                }
            }
            "message_delta" => {
                if let Some(n) = v["usage"]["output_tokens"].as_u64() {
                    self.usage.output_tokens = n as u32;
                }
            }
            "message_stop" => self.done = true,
            "error" => return Err(map_error_type(&v["error"], model, key)),
            _ => {} // ping и новые типы событий — игнорируем (так требует политика версий API)
        }
        Ok(out)
    }

    fn push_text(&mut self, s: &str) -> String {
        if s.is_empty() {
            return String::new();
        }
        let vis = self.think.push(s);
        self.text.push_str(&vis);
        vis
    }

    fn finish_tool(t: ToolBlock) -> ToolCall {
        ToolCall { id: t.id, name: t.name, arguments: parse_args(&Value::String(t.json)) }
    }

    /// Конец стрима: хвост текста и незакрытые tool-блоки (если поток оборвался).
    pub(crate) fn finish(&mut self) -> (String, Vec<ToolCall>) {
        let tail = self.think.finish();
        self.text.push_str(&tail);
        let rest: Vec<ToolCall> = std::mem::take(&mut self.tools).into_values().map(Self::finish_tool).collect();
        self.calls.extend(rest.iter().cloned());
        (tail, rest)
    }
}

#[async_trait]
impl ChatModel for AnthropicClient {
    fn id(&self) -> String {
        self.model_ref()
    }

    async fn chat(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatResponse, LlmError> {
        let key = self.spec.api_key.as_str();
        let body = self.build_body(&req, false);
        let resp = http::send(self.request(&body).timeout(self.request_timeout), &cancel, &self.model, key).await?;
        let v = http::read_json(resp, &cancel, key).await?;
        parse_response(&v, &self.model_ref(), &self.model, key)
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<ChatEvent>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, LlmError> {
        let key = self.spec.api_key.as_str();
        let body = self.build_body(&req, true);
        let resp = tokio::time::timeout(self.request_timeout, http::send(self.request(&body), &cancel, &self.model, key))
            .await
            .map_err(|_| LlmError::Timeout)??;
        let mut reader = SseReader::new(resp, self.idle_timeout);
        let mut acc = AnthropicStreamAcc::default();
        while let Some(ev) = reader.next(&cancel).await? {
            let Ok(v) = serde_json::from_str::<Value>(ev.data.trim()) else { continue };
            let step = acc.on_event(&v, &self.model, key)?;
            if !step.text.is_empty() {
                http::emit(&tx, ChatEvent::TextDelta(step.text), &cancel).await?;
            }
            if let Some(c) = step.call {
                http::emit(&tx, ChatEvent::ToolCall(c), &cancel).await?;
            }
            if acc.done {
                break;
            }
        }
        let (tail, rest) = acc.finish();
        if !tail.is_empty() {
            http::emit(&tx, ChatEvent::TextDelta(tail), &cancel).await?;
        }
        for c in rest {
            http::emit(&tx, ChatEvent::ToolCall(c), &cancel).await?;
        }
        Ok(ChatResponse {
            message: ChatMessage {
                role: Role::Assistant,
                content: acc.text,
                images: vec![],
                tool_calls: acc.calls,
                tool_call_id: None,
            },
            model: self.model_ref(),
            usage: Some(acc.usage),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::{ProviderConfig, ProviderKind, RiskTier, ToolSpec};

    fn client() -> AnthropicClient {
        let pc = ProviderConfig {
            id: "anthropic".into(),
            kind: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".into(),
            api_key: "k".into(),
            enabled: true,
        };
        AnthropicClient::new(ProviderSpec::from_config(&pc), "claude-haiku-5-5")
    }

    #[test]
    fn base_normalization() {
        assert_eq!(normalize_base("https://api.anthropic.com/"), "https://api.anthropic.com/v1");
        assert_eq!(normalize_base("https://opencode.ai/inference/anthropic/v1"), "https://opencode.ai/inference/anthropic/v1");
        assert_eq!(client().base_url, "https://api.anthropic.com/v1");
    }

    #[test]
    fn body_shape() {
        let mut user = ChatMessage::user("что тут?");
        user.images.push(ImageData { mime: "image/png".into(), base64: "AAAA".into() });
        let mut asst = ChatMessage::assistant("Смотрю время.");
        asst.tool_calls.push(ToolCall { id: "toolu_1".into(), name: "get_time".into(), arguments: json!({}) });
        let req = ChatRequest {
            messages: vec![
                ChatMessage::system("ты Джарвис"),
                ChatMessage::system("отвечай кратко"),
                user,
                asst,
                ChatMessage::tool_result("toolu_1", "12:00"),
                ChatMessage::user("спасибо"),
                ChatMessage::assistant(""),
            ],
            tools: vec![ToolSpec {
                name: "get_time".into(),
                description: "время".into(),
                parameters: json!({"type":"object","properties":{}}),
                tier: RiskTier::T0,
            }],
            temperature: Some(0.3),
            max_tokens: None,
        };
        let b = client().build_body(&req, true);
        assert_eq!(b["system"], "ты Джарвис\n\nотвечай кратко");
        assert_eq!(b["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(b["stream"], true);
        assert!((b["temperature"].as_f64().unwrap() - 0.3).abs() < 1e-6);
        assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3, "{m:#?}");
        assert_eq!(m[0]["content"][0]["type"], "image");
        assert_eq!(m[0]["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(m[0]["content"][1]["text"], "что тут?");
        assert_eq!(m[1]["role"], "assistant");
        assert_eq!(m[1]["content"][1]["type"], "tool_use");
        assert_eq!(m[2]["role"], "user");
        assert_eq!(m[2]["content"][0]["type"], "tool_result");
        assert_eq!(m[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(m[2]["content"][1]["text"], "спасибо", "tool_result и текст склеены в одно user-сообщение");

        let b = client().with_version("2026-10-21").build_body(&req, false);
        assert!(b.get("temperature").is_none(), "в 2026-10-21 temperature убран");
    }

    #[test]
    fn stream_events() {
        let evs = [
            json!({"type":"message_start","message":{"usage":{"input_tokens":472,"output_tokens":2}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"ping"}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Сейчас "}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"узнаю."}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_time","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"tz\": \"Eur"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"ope/Moscow\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":89}}),
            json!({"type":"some_future_event"}),
            json!({"type":"message_stop"}),
        ];
        let mut acc = AnthropicStreamAcc::default();
        let mut text = String::new();
        let mut calls = Vec::new();
        for e in &evs {
            let s = acc.on_event(e, "m", "").unwrap();
            text.push_str(&s.text);
            calls.extend(s.call);
        }
        assert!(acc.done);
        assert_eq!(text, "Сейчас узнаю.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, json!({"tz":"Europe/Moscow"}));
        assert_eq!(acc.usage, Usage { input_tokens: 472, output_tokens: 89 });
    }

    #[test]
    fn stream_error_event() {
        let mut acc = AnthropicStreamAcc::default();
        let e = json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}});
        assert_eq!(acc.on_event(&e, "m", ""), Err(LlmError::Server(529)));
        let e = json!({"type":"error","error":{"type":"rate_limit_error","message":"slow"}});
        assert_eq!(acc.on_event(&e, "m", ""), Err(LlmError::RateLimited { retry_after_ms: None }));
    }

    #[test]
    fn parse_plain() {
        let v = json!({"type":"message","content":[
            {"type":"thinking","thinking":"..."},
            {"type":"text","text":"Голубое"},
            {"type":"tool_use","id":"t1","name":"get_time","input":{"a":1}}],
            "usage":{"input_tokens":5,"output_tokens":3}});
        let r = parse_response(&v, "anthropic/m", "m", "").unwrap();
        assert_eq!(r.message.content, "Голубое");
        assert_eq!(r.message.tool_calls[0].arguments, json!({"a":1}));
        let err = json!({"type":"error","error":{"type":"not_found_error","message":"model: x"}});
        assert_eq!(parse_response(&err, "a/x", "x", ""), Err(LlmError::ModelNotFound("x".into())));
    }
}
