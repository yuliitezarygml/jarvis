//! OpenAiCompatClient на мок-сервере: обычный ответ, SSE с tool_call по кускам, картинки, ошибки, отмена.

mod common;

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::post;
use common::*;
use futures_util::StreamExt;
use jarvis_core::{CancellationToken, ChatEvent, ChatMessage, ChatModel, ChatRequest, ImageData, LlmError, RiskTier, ToolSpec};
use jarvis_llm::{OpenAiCompatClient, ProviderSpec};
use serde_json::{Value, json};
use tokio::sync::mpsc;

fn client(base: &str, model: &str) -> OpenAiCompatClient {
    OpenAiCompatClient::new(ProviderSpec::from_config(&provider("groq", base, "gsk_test_key_123456789")), model)
}

fn tool() -> ToolSpec {
    ToolSpec {
        name: "get_time".into(),
        description: "время".into(),
        parameters: json!({"type":"object","properties":{"timezone":{"type":"string"}}}),
        tier: RiskTier::T0,
    }
}

async fn collect(model: &dyn ChatModel, req: ChatRequest) -> (Result<jarvis_core::ChatResponse, LlmError>, Vec<ChatEvent>) {
    let (tx, mut rx) = mpsc::channel(256);
    let fut = model.chat_stream(req, tx, CancellationToken::new());
    let recv = async {
        let mut v = Vec::new();
        while let Some(e) = rx.recv().await {
            v.push(e);
        }
        v
    };
    tokio::join!(fut, recv)
}

#[tokio::test]
async fn chat_non_stream_text_usage_and_headers() {
    let rec = Recorder::default();
    let r = rec.clone();
    let app = Router::new().route(
        "/openai/v1/chat/completions",
        post(move |h: HeaderMap, b: Bytes| async move {
            r.record("/chat/completions", &h, &b);
            json(200, json!({
                "id":"x","object":"chat.completion","model":"openai/gpt-oss-120b",
                "choices":[{"index":0,"message":{"role":"assistant","content":"Голубое","reasoning":"думаю…"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":20,"completion_tokens":3,"total_tokens":23}
            }))
        }),
    );
    let base = serve(app).await;
    let c = client(&format!("{base}/openai/v1"), "openai/gpt-oss-120b");
    let req = ChatRequest {
        messages: vec![ChatMessage::system("кратко"), ChatMessage::user("небо?")],
        max_tokens: Some(50),
        temperature: Some(0.1),
        ..Default::default()
    };
    let r = c.chat(req, CancellationToken::new()).await.unwrap();
    assert_eq!(r.message.content, "Голубое");
    assert_eq!(r.model, "groq/openai/gpt-oss-120b");
    assert_eq!(r.usage.unwrap().output_tokens, 3);
    let seen = rec.last();
    assert_eq!(seen.headers["authorization"], "Bearer gsk_test_key_123456789");
    assert_eq!(seen.body["model"], "openai/gpt-oss-120b");
    assert_eq!(seen.body["max_completion_tokens"], 50);
    assert_eq!(seen.body["messages"][0]["role"], "system");
    assert!(seen.body.get("stream").is_none());
}

#[tokio::test]
async fn stream_assembles_tool_call_split_across_chunks() {
    let chunks = vec![
        json!({"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}),
        json!({"choices":[{"index":0,"delta":{"content":"Сейчас "}}]}),
        json!({"choices":[{"index":0,"delta":{"content":"проверю."}}]}),
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_time","arguments":""}}]}}]}),
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"time"}}]}}]}),
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"zone\": \"Europe/"}}]}}]}),
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"Moscow\"}"}}]}}]}),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"x_groq":{"usage":{"prompt_tokens":40,"completion_tokens":12}}}),
    ];
    let rec = Recorder::default();
    let r = rec.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |h: HeaderMap, b: Bytes| {
            let chunks = chunks.clone();
            async move {
                r.record("/chat/completions", &h, &b);
                // Куски по 7 байт: режутся строки, JSON и кириллица.
                openai_sse(&chunks, 7)
            }
        }),
    );
    let base = serve(app).await;
    let c = client(&format!("{base}/v1"), "openai/gpt-oss-120b");
    let req = ChatRequest { messages: vec![ChatMessage::user("который час?")], tools: vec![tool()], ..Default::default() };
    let (res, events) = collect(&c, req).await;
    let res = res.unwrap();
    assert_eq!(res.message.content, "Сейчас проверю.");
    assert_eq!(res.message.tool_calls.len(), 1);
    let call = &res.message.tool_calls[0];
    assert_eq!((call.id.as_str(), call.name.as_str()), ("call_1", "get_time"));
    assert_eq!(call.arguments, json!({"timezone":"Europe/Moscow"}));
    assert_eq!(res.usage.unwrap().input_tokens, 40);
    assert_eq!(res.model, "groq/openai/gpt-oss-120b");

    let text: String = events
        .iter()
        .filter_map(|e| if let ChatEvent::TextDelta(t) = e { Some(t.as_str()) } else { None })
        .collect();
    assert_eq!(text, "Сейчас проверю.");
    assert!(events.iter().filter(|e| matches!(e, ChatEvent::TextDelta(_))).count() >= 2, "дельты идут по мере прихода");
    assert_eq!(events.last(), Some(&ChatEvent::ToolCall(call.clone())));

    let body = rec.last().body;
    assert_eq!(body["stream"], true);
    assert_eq!(body["tools"][0]["function"]["name"], "get_time");
}

#[tokio::test]
async fn stream_strips_think_and_handles_plain_json_fallback() {
    let chunks = vec![
        json!({"choices":[{"delta":{"content":"<think>"}}]}),
        json!({"choices":[{"delta":{"content":"пользователь спрашивает про небо"}}]}),
        json!({"choices":[{"delta":{"content":"</think>\n\nГолубое"}}]}),
    ];
    let app = Router::new()
        .route("/a/chat/completions", post(move || { let c = chunks.clone(); async move { openai_sse(&c, 5) } }))
        // Сервер игнорирует stream=true и отвечает обычным JSON.
        .route(
            "/b/chat/completions",
            post(|| async { json(200, json!({"choices":[{"message":{"role":"assistant","content":"Синее"}}]})) }),
        );
    let base = serve(app).await;
    let req = ChatRequest { messages: vec![ChatMessage::user("небо?")], ..Default::default() };

    let (res, events) = collect(&client(&format!("{base}/a"), "qwen/qwen3-32b"), req.clone()).await;
    assert_eq!(res.unwrap().message.content, "Голубое");
    assert_eq!(events, vec![ChatEvent::TextDelta("Голубое".into())]);

    let (res, events) = collect(&client(&format!("{base}/b"), "m"), req).await;
    assert_eq!(res.unwrap().message.content, "Синее");
    assert_eq!(events, vec![ChatEvent::TextDelta("Синее".into())]);
}

#[tokio::test]
async fn images_and_tool_messages_in_request() {
    let rec = Recorder::default();
    let r = rec.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |h: HeaderMap, b: Bytes| async move {
            r.record("/chat/completions", &h, &b);
            json(200, json!({"choices":[{"message":{"content":"7"}}]}))
        }),
    );
    let base = serve(app).await;
    let mut user = ChatMessage::user("Какая цифра?");
    user.images.push(ImageData { mime: "image/png".into(), base64: "iVBORw0KGgo=".into() });
    let mut asst = ChatMessage::assistant("");
    asst.tool_calls.push(jarvis_core::ToolCall { id: "c1".into(), name: "get_time".into(), arguments: json!({}) });
    let req = ChatRequest {
        messages: vec![user, asst, ChatMessage::tool_result("c1", "12:00")],
        tools: vec![tool()],
        ..Default::default()
    };
    client(&format!("{base}/v1"), "m").chat(req, CancellationToken::new()).await.unwrap();
    let m = rec.last().body["messages"].clone();
    assert_eq!(m[0]["content"][0], json!({"type":"text","text":"Какая цифра?"}));
    assert_eq!(m[0]["content"][1]["type"], "image_url");
    assert_eq!(m[0]["content"][1]["image_url"]["url"], "data:image/png;base64,iVBORw0KGgo=");
    assert_eq!(m[1]["tool_calls"][0]["function"]["arguments"], "{}");
    assert_eq!(m[2], json!({"role":"tool","tool_call_id":"c1","content":"12:00"}));
}

fn status_router() -> Router {
    fn route(status: u16, body: Value, retry: Option<&'static str>) -> axum::routing::MethodRouter {
        post(move || {
            let body = body.clone();
            async move {
                let mut r: Response = json(status, body);
                if let Some(v) = retry {
                    r.headers_mut().insert("retry-after", v.parse().unwrap());
                }
                r
            }
        })
    }
    Router::new()
        .route("/s401/chat/completions", route(401, json!({"error":{"message":"Invalid API Key","type":"invalid_request_error","code":"invalid_api_key"}}), None))
        .route("/s403/chat/completions", route(403, json!({"error":{"message":"forbidden"}}), None))
        .route("/s404/chat/completions", route(404, json!({"error":{"message":"not found"}}), None))
        .route("/s400m/chat/completions", route(400, json!({"error":{"message":"The model `nope` does not exist or you do not have access to it.","type":"invalid_request_error","code":"model_not_found"}}), None))
        .route("/s400k/chat/completions", route(400, json!({"error":{"code":400,"message":"Please pass a valid API key","status":"INVALID_ARGUMENT"}}), None))
        .route("/s400/chat/completions", route(400, json!({"error":{"message":"temperature must be 1 for this model"}}), None))
        .route("/s429/chat/completions", route(429, json!({"error":{"message":"Rate limit reached"}}), Some("3")))
        .route("/s500/chat/completions", route(500, json!({"error":{"message":"oops"}}), None))
        .route("/s503/chat/completions", route(503, json!({}), None))
        .route("/inline/chat/completions", post(|| async {
            openai_sse(&[json!({"choices":[{"delta":{"content":"Го"}}]}), json!({"error":{"code":502,"message":"upstream died"},"choices":[{"delta":{},"finish_reason":"error"}]})], 11)
        }))
}

#[tokio::test]
async fn error_mapping() {
    let base = serve(status_router()).await;
    let req = ChatRequest { messages: vec![ChatMessage::user("hi")], ..Default::default() };
    let cases: Vec<(&str, LlmError)> = vec![
        ("s401", LlmError::Auth),
        ("s403", LlmError::Auth),
        ("s404", LlmError::ModelNotFound("nope".into())),
        ("s400m", LlmError::ModelNotFound("nope".into())),
        ("s400k", LlmError::Auth),
        ("s429", LlmError::RateLimited { retry_after_ms: Some(3000) }),
        ("s500", LlmError::Server(500)),
        ("s503", LlmError::Server(503)),
    ];
    for (path, want) in cases {
        let c = client(&format!("{base}/{path}"), "nope");
        assert_eq!(c.chat(req.clone(), CancellationToken::new()).await.unwrap_err(), want, "chat {path}");
        let (res, _) = collect(&c, req.clone()).await;
        assert_eq!(res.unwrap_err(), want, "stream {path}");
    }
    match client(&format!("{base}/s400"), "m").chat(req.clone(), CancellationToken::new()).await.unwrap_err() {
        LlmError::Other(s) => assert!(s.contains("HTTP 400") && s.contains("temperature"), "{s}"),
        e => panic!("{e:?}"),
    }
    // Ошибка посреди стрима (OpenRouter).
    let (res, events) = collect(&client(&format!("{base}/inline"), "m"), req.clone()).await;
    assert_eq!(res.unwrap_err(), LlmError::Server(502));
    assert_eq!(events, vec![ChatEvent::TextDelta("Го".into())]);
}

#[tokio::test]
async fn network_error_and_key_not_leaked() {
    // Порт без сервера.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let c = client(&format!("http://127.0.0.1:{port}/v1"), "m");
    let err = c.chat(ChatRequest { messages: vec![ChatMessage::user("hi")], ..Default::default() }, CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, LlmError::Network(_)), "{err:?}");
    assert!(!err.to_string().contains("gsk_test_key"), "ключ не должен попадать в текст ошибки");
    assert!(!format!("{c:?}").contains("gsk_test_key"));
}

#[tokio::test]
async fn cancel_and_idle_timeout() {
    let app = Router::new().route(
        "/slow/chat/completions",
        post(|| async {
            // Первый чанк сразу, дальше тишина 30 с.
            let raw = format!("data: {}\n\n", json!({"choices":[{"delta":{"content":"Го"}}]}));
            let s = futures_util::stream::iter(vec![raw]).chain(futures_util::stream::once(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                "data: [DONE]\n\n".to_string()
            }));
            let s = s.map(|x| Ok::<_, std::convert::Infallible>(Bytes::from(x)));
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(axum::body::Body::from_stream(s))
                .unwrap()
        }),
    );
    let base = serve(app).await;
    let req = ChatRequest { messages: vec![ChatMessage::user("hi")], ..Default::default() };

    let c = client(&format!("{base}/slow"), "m");
    let cancel = CancellationToken::new();
    let (tx, _rx) = mpsc::channel(16);
    let t0 = Instant::now();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c2.cancel();
    });
    assert_eq!(c.chat_stream(req.clone(), tx, cancel).await.unwrap_err(), LlmError::Cancelled);
    assert!(t0.elapsed() < Duration::from_secs(5));

    let c = client(&format!("{base}/slow"), "m").with_timeouts(Duration::from_secs(5), Duration::from_millis(300));
    let (tx, _rx) = mpsc::channel(16);
    assert_eq!(c.chat_stream(req, tx, CancellationToken::new()).await.unwrap_err(), LlmError::Timeout);
}
