//! Клиент OpenAI-совместимого `/chat/completions` (Groq, OpenCode Zen, OpenRouter, OpenAI, Gemini compat, Mistral,
//! DeepSeek, Together, Fireworks, Cerebras, xAI, Z.AI, Ollama, LM Studio, свой URL).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jarvis_core::{
    CancellationToken, ChatEvent, ChatMessage, ChatModel, ChatRequest, ChatResponse, ImageData, LlmError, Role,
    ToolCall, ToolSpec, Usage,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::http::{self, SseReader};
use crate::presets::{AuthStyle, MaxTokensField, ProviderSpec};
use crate::think::{ThinkFilter, strip_think};

pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct OpenAiCompatClient {
    spec: Arc<ProviderSpec>,
    model: String,
    base_url: String,
    auth: AuthStyle,
    http: reqwest::Client,
    request_timeout: Duration,
    idle_timeout: Duration,
}

impl std::fmt::Debug for OpenAiCompatClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Ключ не печатаем.
        f.debug_struct("OpenAiCompatClient").field("id", &self.id()).field("base_url", &self.base_url).finish()
    }
}

impl OpenAiCompatClient {
    pub fn new(spec: ProviderSpec, model: impl Into<String>) -> Self {
        Self::from_arc(Arc::new(spec), model)
    }

    pub fn from_arc(spec: Arc<ProviderSpec>, model: impl Into<String>) -> Self {
        Self {
            base_url: spec.base_url.clone(),
            auth: spec.auth,
            spec,
            model: model.into(),
            http: http::shared_client(),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    /// Другой адрес для этой модели (маршрут Zen и т.п.).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    pub fn with_auth(mut self, auth: AuthStyle) -> Self {
        self.auth = auth;
        self
    }

    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// `request` — весь обычный ответ / заголовки стрима; `idle` — максимум тишины между чанками стрима.
    pub fn with_timeouts(mut self, request: Duration, idle: Duration) -> Self {
        self.request_timeout = request;
        self.idle_timeout = idle;
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn provider(&self) -> &str {
        &self.spec.id
    }

    /// Тело запроса `/chat/completions`.
    pub fn build_body(&self, req: &ChatRequest, stream: bool) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": to_openai_messages(&req.messages),
        });
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(req.tools.iter().map(tool_json).collect());
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = req.max_tokens {
            let field = match self.spec.max_tokens_field {
                MaxTokensField::MaxTokens => "max_tokens",
                MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
            };
            body[field] = json!(m);
        }
        if stream {
            body["stream"] = json!(true);
            if self.spec.stream_usage {
                body["stream_options"] = json!({ "include_usage": true });
            }
        }
        body
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let url = http::join_url(&self.base_url, "/chat/completions");
        let rb = self.http.post(url).json(body);
        http::apply_auth(rb, self.auth, &self.spec.api_key, &self.spec.extra_headers)
    }

    fn model_ref(&self) -> String {
        format!("{}/{}", self.spec.id, self.model)
    }
}

fn tool_json(t: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": { "name": t.name, "description": t.description, "parameters": t.parameters },
    })
}

fn data_url(img: &ImageData) -> String {
    format!("data:{};base64,{}", img.mime, img.base64)
}

fn user_with_images(text: &str, images: &[ImageData]) -> Value {
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(json!({ "type": "text", "text": text }));
    }
    for img in images {
        parts.push(json!({ "type": "image_url", "image_url": { "url": data_url(img) } }));
    }
    json!({ "role": "user", "content": parts })
}

fn args_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(), // сырые (невалидные) аргументы возвращаем как были
        Value::Null => "{}".into(),
        other => other.to_string(),
    }
}

/// `ChatMessage` → сообщения OpenAI. Картинки из результатов инструментов (tool-сообщения картинок не несут)
/// идут отдельным user-сообщением сразу после блока tool-ответов.
pub(crate) fn to_openai_messages(msgs: &[ChatMessage]) -> Vec<Value> {
    let mut out = Vec::with_capacity(msgs.len());
    let mut tool_images: Vec<ImageData> = Vec::new();
    for m in msgs {
        if m.role != Role::Tool && !tool_images.is_empty() {
            out.push(user_with_images("Изображения из результата инструмента:", &tool_images));
            tool_images.clear();
        }
        match m.role {
            Role::System => out.push(json!({ "role": "system", "content": m.content })),
            Role::User if m.images.is_empty() => out.push(json!({ "role": "user", "content": m.content })),
            Role::User => out.push(user_with_images(&m.content, &m.images)),
            Role::Assistant => {
                let mut v = json!({ "role": "assistant" });
                v["content"] =
                    if m.content.is_empty() && !m.tool_calls.is_empty() { Value::Null } else { json!(m.content) };
                if !m.tool_calls.is_empty() {
                    v["tool_calls"] = Value::Array(
                        m.tool_calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": { "name": c.name, "arguments": args_to_string(&c.arguments) },
                                })
                            })
                            .collect(),
                    );
                }
                out.push(v);
            }
            Role::Tool => {
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content,
                }));
                tool_images.extend(m.images.iter().cloned());
            }
        }
    }
    if !tool_images.is_empty() {
        out.push(user_with_images("Изображения из результата инструмента:", &tool_images));
    }
    out
}

/// Аргументы: JSON-строка провайдера → `Value`. Пусто → `{}`; невалидный JSON остаётся строкой (инструмент сообщит).
pub(crate) fn parse_args(raw: &Value) -> Value {
    match raw {
        Value::String(s) if s.trim().is_empty() => json!({}),
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|e| {
            tracing::warn!("аргументы инструмента — не JSON ({e})");
            Value::String(s.clone())
        }),
        Value::Null => json!({}),
        other => other.clone(),
    }
}

/// content бывает строкой, null или массивом частей `{type:"text", text}`.
fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect(),
        _ => String::new(),
    }
}

fn parse_usage(v: Option<&Value>) -> Option<Usage> {
    let u = v.filter(|u| u.is_object())?;
    let get = |k: &str| u.get(k).and_then(|x| x.as_u64()).map(|x| x as u32);
    Some(Usage {
        input_tokens: get("prompt_tokens").or_else(|| get("input_tokens")).unwrap_or(0),
        output_tokens: get("completion_tokens").or_else(|| get("output_tokens")).unwrap_or(0),
    })
}

pub(crate) fn parse_response(v: &Value, model_ref: &str, model: &str, key: &str) -> Result<ChatResponse, LlmError> {
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return Err(http::map_inline_error(err, model, key));
    }
    let choice = v
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| LlmError::BadResponse("в ответе нет choices".into()))?;
    let msg = &choice["message"];
    let tool_calls = msg["tool_calls"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, tc)| ToolCall {
                    id: tc["id"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(format!("call_{i}")),
                    name: tc["function"]["name"].as_str().unwrap_or_default().to_string(),
                    arguments: parse_args(&tc["function"]["arguments"]),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ChatResponse {
        message: ChatMessage {
            role: Role::Assistant,
            content: strip_think(&content_text(&msg["content"])),
            images: vec![],
            tool_calls,
            tool_call_id: None,
        },
        model: model_ref.to_string(),
        usage: parse_usage(v.get("usage")),
    })
}

#[derive(Debug, Default)]
struct PartialTool {
    id: String,
    name: String,
    args: String,
}

/// Сборка стрима: текст, tool_calls из дельт по `index`, usage.
#[derive(Debug, Default)]
pub(crate) struct StreamAcc {
    pub text: String,
    think: ThinkFilter,
    tools: BTreeMap<usize, PartialTool>,
    pub usage: Option<Usage>,
}

impl StreamAcc {
    /// Обработать один чанк; вернуть видимую дельту текста.
    pub(crate) fn on_chunk(&mut self, v: &Value, model: &str, key: &str) -> Result<String, LlmError> {
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            return Err(http::map_inline_error(err, model, key));
        }
        if let Some(u) = parse_usage(v.get("usage")).or_else(|| parse_usage(v.get("x_groq").and_then(|x| x.get("usage"))))
        {
            self.usage = Some(u);
        }
        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else { return Ok(String::new()) };
        let delta = &choice["delta"];
        if let Some(calls) = delta["tool_calls"].as_array() {
            for tc in calls {
                self.on_tool_delta(tc);
            }
        }
        let raw = content_text(&delta["content"]);
        if raw.is_empty() {
            return Ok(String::new());
        }
        let visible = self.think.push(&raw);
        self.text.push_str(&visible);
        Ok(visible)
    }

    fn on_tool_delta(&mut self, tc: &Value) {
        let id = tc["id"].as_str().filter(|s| !s.is_empty());
        let idx = match tc.get("index").and_then(|i| i.as_u64()) {
            Some(i) => i as usize,
            // Без index (некоторые совместимые серверы): новый id — новый вызов, иначе продолжение последнего.
            None => {
                let last = self.tools.keys().next_back().copied();
                match (id, last) {
                    (Some(id), Some(l)) if self.tools[&l].id != id => l + 1,
                    (_, Some(l)) => l,
                    (_, None) => 0,
                }
            }
        };
        let e = self.tools.entry(idx).or_default();
        if let Some(id) = id {
            e.id = id.to_string();
        }
        if let Some(n) = tc["function"]["name"].as_str().filter(|n| !n.is_empty()) {
            if e.name.is_empty() {
                e.name = n.to_string();
            } else if e.name != n {
                e.name.push_str(n);
            }
        }
        match &tc["function"]["arguments"] {
            Value::String(s) => e.args.push_str(s),
            Value::Null => {}
            other => e.args = other.to_string(),
        }
    }

    /// Конец стрима: хвост текста и готовые вызовы инструментов (по порядку index).
    pub(crate) fn finish(&mut self) -> (String, Vec<ToolCall>) {
        let tail = self.think.finish();
        self.text.push_str(&tail);
        let calls = std::mem::take(&mut self.tools)
            .into_values()
            .enumerate()
            .filter(|(_, t)| !t.name.is_empty())
            .map(|(i, t)| ToolCall {
                id: if t.id.is_empty() { format!("call_{i}") } else { t.id },
                name: t.name,
                arguments: parse_args(&Value::String(t.args)),
            })
            .collect();
        (tail, calls)
    }
}

fn is_json(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("application/json"))
}

#[async_trait]
impl ChatModel for OpenAiCompatClient {
    fn id(&self) -> String {
        self.model_ref()
    }

    async fn chat(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatResponse, LlmError> {
        let body = self.build_body(&req, false);
        let rb = self.request(&body).timeout(self.request_timeout);
        let resp = http::send(rb, &cancel, &self.model, &self.spec.api_key).await?;
        let v = http::read_json(resp, &cancel, &self.spec.api_key).await?;
        parse_response(&v, &self.model_ref(), &self.model, &self.spec.api_key)
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

        // Сервер проигнорировал stream=true и прислал обычный JSON.
        if is_json(&resp) {
            let v = http::read_json(resp, &cancel, key).await?;
            let r = parse_response(&v, &self.model_ref(), &self.model, key)?;
            if !r.message.content.is_empty() {
                http::emit(&tx, ChatEvent::TextDelta(r.message.content.clone()), &cancel).await?;
            }
            for c in &r.message.tool_calls {
                http::emit(&tx, ChatEvent::ToolCall(c.clone()), &cancel).await?;
            }
            return Ok(r);
        }

        let mut reader = SseReader::new(resp, self.idle_timeout);
        let mut acc = StreamAcc::default();
        while let Some(ev) = reader.next(&cancel).await? {
            let data = ev.data.trim();
            if data == "[DONE]" {
                break;
            }
            if data.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                tracing::debug!("пропускаю не-JSON чанк SSE");
                continue;
            };
            let delta = acc.on_chunk(&v, &self.model, key)?;
            if !delta.is_empty() {
                http::emit(&tx, ChatEvent::TextDelta(delta), &cancel).await?;
            }
        }
        let (tail, calls) = acc.finish();
        if !tail.is_empty() {
            http::emit(&tx, ChatEvent::TextDelta(tail), &cancel).await?;
        }
        for c in &calls {
            http::emit(&tx, ChatEvent::ToolCall(c.clone()), &cancel).await?;
        }
        Ok(ChatResponse {
            message: ChatMessage {
                role: Role::Assistant,
                content: acc.text,
                images: vec![],
                tool_calls: calls,
                tool_call_id: None,
            },
            model: self.model_ref(),
            usage: acc.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::{ProviderConfig, RiskTier};

    fn client(id: &str) -> OpenAiCompatClient {
        let pc = ProviderConfig { id: id.into(), base_url: "http://x/v1".into(), api_key: "k".into(), ..Default::default() };
        OpenAiCompatClient::new(ProviderSpec::from_config(&pc), "m")
    }

    #[test]
    fn body_shape() {
        let img = ImageData { mime: "image/png".into(), base64: "AAAA".into() };
        let mut user = ChatMessage::user("что на картинке?");
        user.images.push(img.clone());
        let mut asst = ChatMessage::assistant("");
        asst.tool_calls.push(ToolCall { id: "c1".into(), name: "get_time".into(), arguments: json!({"tz":"msk"}) });
        let mut tool = ChatMessage::tool_result("c1", "12:00");
        tool.images.push(img);
        let req = ChatRequest {
            messages: vec![ChatMessage::system("ты Джарвис"), user, asst, tool, ChatMessage::user("ещё")],
            tools: vec![ToolSpec {
                name: "get_time".into(),
                description: "время".into(),
                parameters: json!({"type":"object","properties":{}}),
                tier: RiskTier::T0,
            }],
            temperature: Some(0.2),
            max_tokens: Some(100),
        };
        let b = client("groq").build_body(&req, true);
        assert_eq!(b["model"], "m");
        assert_eq!(b["stream"], true);
        assert_eq!(b["max_completion_tokens"], 100, "у Groq max_completion_tokens");
        assert!(b.get("max_tokens").is_none());
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 6, "{m:#?}");
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[1]["content"][0]["type"], "text");
        assert_eq!(m[1]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        assert!(m[2]["content"].is_null());
        assert_eq!(m[2]["tool_calls"][0]["function"]["arguments"], r#"{"tz":"msk"}"#);
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["tool_call_id"], "c1");
        assert_eq!(m[4]["role"], "user", "картинка из инструмента — отдельным user");
        assert_eq!(m[4]["content"][1]["type"], "image_url");
        assert_eq!(m[5]["content"], "ещё");
        assert_eq!(b["tools"][0]["type"], "function");
        assert_eq!(b["tools"][0]["function"]["name"], "get_time");

        let b = client("custom-x").build_body(&req, false);
        assert_eq!(b["max_tokens"], 100);
        assert!(b.get("stream").is_none());
        assert!(b.get("stream_options").is_none());
        let b = client("openai").build_body(&req, true);
        assert_eq!(b["stream_options"]["include_usage"], true);
    }

    #[test]
    fn stream_tool_call_from_deltas() {
        let chunks = [
            json!({"choices":[{"delta":{"role":"assistant","content":null}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"get_time","arguments":""}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"tz\":"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"echo","arguments":"{}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Europe/Moscow\"}"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"x_groq":{"usage":{"prompt_tokens":10,"completion_tokens":7}}}),
        ];
        let mut acc = StreamAcc::default();
        for c in &chunks {
            assert_eq!(acc.on_chunk(c, "m", "").unwrap(), "");
        }
        let (_, calls) = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].arguments, json!({"tz":"Europe/Moscow"}));
        assert_eq!(calls[1].name, "echo");
        assert_eq!(acc.usage, Some(Usage { input_tokens: 10, output_tokens: 7 }));
    }

    #[test]
    fn stream_tool_calls_without_index() {
        let mut acc = StreamAcc::default();
        acc.on_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"id":"x1","function":{"name":"a","arguments":"{\"q\":1}"}}]}}]}), "m", "")
            .unwrap();
        acc.on_chunk(&json!({"choices":[{"delta":{"tool_calls":[{"id":"x2","function":{"name":"b","arguments":"{}"}}]}}]}), "m", "")
            .unwrap();
        let (_, calls) = acc.finish();
        assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(calls[0].arguments, json!({"q":1}));
    }

    #[test]
    fn parse_plain_response() {
        let v = json!({
            "choices":[{"message":{"role":"assistant","content":"<think>хм</think>Голубое","tool_calls":[
                {"id":"","type":"function","function":{"name":"get_time","arguments":""}}]}}],
            "usage":{"prompt_tokens":3,"completion_tokens":2}
        });
        let r = parse_response(&v, "p/m", "m", "").unwrap();
        assert_eq!(r.message.content, "Голубое");
        assert_eq!(r.message.tool_calls[0].id, "call_0");
        assert_eq!(r.message.tool_calls[0].arguments, json!({}));
        assert_eq!(r.usage.unwrap().output_tokens, 2);
        assert_eq!(r.model, "p/m");
        assert!(matches!(parse_response(&json!({"x":1}), "p/m", "m", ""), Err(LlmError::BadResponse(_))));
    }

    #[test]
    fn debug_hides_key() {
        let s = format!("{:?}", client("groq"));
        assert!(!s.contains("api_key"));
    }
}
