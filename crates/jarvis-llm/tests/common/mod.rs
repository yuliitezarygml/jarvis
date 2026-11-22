//! Общее для интеграционных тестов: мок-сервер на axum на случайном порту, SSE с разрезанными чанками, запись запросов.
#![allow(dead_code)]

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use futures_util::StreamExt;
use jarvis_core::{ProviderConfig, ProviderKind};
use serde_json::Value;

/// Поднять сервер, вернуть `http://127.0.0.1:PORT`.
pub async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

pub fn json(status: u16, v: Value) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v.to_string()))
        .unwrap()
}

/// SSE-ответ: байты отдаются кусками `piece` с паузой, чтобы клиент получал их раздельно
/// (в том числе посреди JSON и посреди UTF-8).
pub fn sse_raw(raw: String, piece: usize, pause_ms: u64) -> Response {
    let bytes = raw.into_bytes();
    let parts: Vec<Vec<u8>> = bytes.chunks(piece.max(1)).map(|c| c.to_vec()).collect();
    let stream = futures_util::stream::iter(parts).then(move |p| async move {
        tokio::time::sleep(Duration::from_millis(pause_ms)).await;
        Ok::<_, Infallible>(Bytes::from(p))
    });
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// OpenAI-стиль: `data: {...}\n\n` для каждого чанка + `data: [DONE]`.
pub fn openai_sse(chunks: &[Value], piece: usize) -> Response {
    let mut raw = String::new();
    for c in chunks {
        raw.push_str("data: ");
        raw.push_str(&c.to_string());
        raw.push_str("\n\n");
    }
    raw.push_str("data: [DONE]\n\n");
    sse_raw(raw, piece, 2)
}

/// Anthropic-стиль: `event: <type>\ndata: {...}\n\n`.
pub fn anthropic_sse(events: &[Value], piece: usize) -> Response {
    let mut raw = String::new();
    for e in events {
        raw.push_str(&format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e));
    }
    sse_raw(raw, piece, 2)
}

/// Записанный запрос.
#[derive(Debug, Clone)]
pub struct Seen {
    pub path: String,
    pub headers: HeaderMap,
    pub body: Value,
}

#[derive(Clone, Default)]
pub struct Recorder {
    pub seen: Arc<Mutex<Vec<Seen>>>,
    pub hits: Arc<AtomicUsize>,
}

impl Recorder {
    pub fn record(&self, path: &str, headers: &HeaderMap, body: &Bytes) {
        self.hits.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(Seen {
            path: path.to_string(),
            headers: headers.clone(),
            body: serde_json::from_slice(body).unwrap_or(Value::Null),
        });
    }
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
    pub fn last(&self) -> Seen {
        self.seen.lock().unwrap().last().cloned().expect("запросов не было")
    }
    pub fn all(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

pub fn provider(id: &str, base: &str, key: &str) -> ProviderConfig {
    ProviderConfig { id: id.into(), kind: ProviderKind::OpenaiCompat, base_url: base.into(), api_key: key.into(), enabled: true }
}

pub fn anthropic_provider(id: &str, base: &str, key: &str) -> ProviderConfig {
    ProviderConfig { id: id.into(), kind: ProviderKind::Anthropic, base_url: base.into(), api_key: key.into(), enabled: true }
}

/// Временный каталог, удаляется при drop.
pub struct TempDir(pub std::path::PathBuf);
impl TempDir {
    pub fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "jarvis-llm-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Фикстура `/models` в стиле Groq.
pub fn groq_models() -> Value {
    serde_json::json!({
        "object": "list",
        "data": [
            {"id":"openai/gpt-oss-120b","object":"model","created":1754408224,"owned_by":"OpenAI","active":true,"context_window":131072,"public_apps":null,"max_completion_tokens":65536},
            {"id":"openai/gpt-oss-20b","object":"model","created":1754407957,"owned_by":"OpenAI","active":true,"context_window":131072,"public_apps":null},
            {"id":"llama-3.1-8b-instant","object":"model","created":1693721698,"owned_by":"Meta","active":true,"context_window":131072,"public_apps":null},
            {"id":"qwen/qwen3.8-27b","object":"model","created":1790000000,"owned_by":"Alibaba Cloud","active":true,"context_window":131072,"public_apps":null},
            {"id":"meta-llama/llama-4-scout-17b-16e-instruct","object":"model","created":1743874824,"owned_by":"Meta","active":false,"context_window":131072,"public_apps":null},
            {"id":"whisper-large-v3","object":"model","created":1693721698,"owned_by":"OpenAI","active":true,"context_window":448,"public_apps":null},
            {"id":"whisper-large-v3-turbo","object":"model","created":1728413088,"owned_by":"OpenAI","active":true,"context_window":448,"public_apps":null},
            {"id":"meta-llama/llama-prompt-guard-2-86m","object":"model","created":1748632165,"owned_by":"Meta","active":true,"context_window":512,"public_apps":null},
            {"id":"openai/gpt-oss-safeguard-20b","object":"model","created":1761708789,"owned_by":"OpenAI","active":true,"context_window":131072,"public_apps":null},
            {"id":"canopylabs/orpheus-v1-english","object":"model","created":1765000000,"owned_by":"Canopy Labs","active":true,"context_window":4000,"public_apps":null}
        ]
    })
}

/// Фикстура `/models` в стиле OpenRouter (записи взяты из живого ответа и сокращены).
pub fn openrouter_models() -> Value {
    serde_json::json!({
        "data": [
            {"id":"openai/gpt-oss-120b","name":"OpenAI: gpt-oss-120b","context_length":131072,
             "architecture":{"modality":"text->text","input_modalities":["text"],"output_modalities":["text"],"tokenizer":"GPT","instruct_type":null},
             "pricing":{"prompt":"0.00000009","completion":"0.00000036"},
             "top_provider":{"context_length":131072,"max_completion_tokens":117964,"is_moderated":false},
             "supported_parameters":["include_reasoning","max_tokens","reasoning","response_format","temperature","tool_choice","tools","top_p"]},
            {"id":"anthropic/claude-fable-5.2","name":"Anthropic: Claude Fable 5.2","context_length":1000000,
             "architecture":{"modality":"text+image+file->text","input_modalities":["text","image","file"],"output_modalities":["text"],"tokenizer":"Claude","instruct_type":null},
             "pricing":{"prompt":"0.000008","completion":"0.00004"},
             "top_provider":{"context_length":1000000,"max_completion_tokens":128000,"is_moderated":true},
             "supported_parameters":["include_reasoning","max_tokens","reasoning","stop","tool_choice","tools"]},
            {"id":"inclusionai/ling-3.2-flash:free","name":"inclusionAI: Ling 3.2 Flash (free)","context_length":1048576,
             "architecture":{"modality":"text+image->text","input_modalities":["text","image"],"output_modalities":["text"],"tokenizer":"Other","instruct_type":null},
             "pricing":{"prompt":"0","completion":"0"},
             "supported_parameters":["max_tokens","temperature","tool_choice","tools"]},
            {"id":"some/old-chat-model","name":"Old chat","context_length":8192,
             "architecture":{"modality":"text->text","input_modalities":["text"],"output_modalities":["text"]},
             "pricing":{"prompt":"0.000001","completion":"0.000002"},
             "supported_parameters":["max_tokens","temperature"]},
            {"id":"google/gemini-3-pro-image","name":"Google: Gemini 3 Pro Image","context_length":65536,
             "architecture":{"modality":"text+image->text+image","input_modalities":["text","image"],"output_modalities":["image","text"]},
             "pricing":{"prompt":"0.000002","completion":"0.000012"},"supported_parameters":["max_tokens"]},
            {"id":"openai/text-embedding-3-large","name":"OpenAI: Text Embedding 3 Large","context_length":8192,
             "architecture":{"modality":"text->embeddings","input_modalities":["text"],"output_modalities":["embeddings"]},
             "pricing":{"prompt":"0.00000013","completion":"0"},"supported_parameters":[]}
        ]
    })
}
