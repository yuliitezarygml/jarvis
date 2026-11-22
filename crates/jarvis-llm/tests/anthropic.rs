//! AnthropicClient на мок-сервере + маршрут OpenCode Zen `claude-*` → `/anthropic/v1/messages`.

mod common;

use axum::Router;
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::routing::post;
use common::*;
use jarvis_core::{CancellationToken, ChatEvent, ChatMessage, ChatModel, ChatRequest, LlmError, ProviderKind, RiskTier, ToolSpec};
use jarvis_llm::{AnthropicClient, ModelRoute, ProviderSpec, client_for};
use serde_json::json;
use tokio::sync::mpsc;

fn tool() -> ToolSpec {
    ToolSpec {
        name: "get_time".into(),
        description: "время".into(),
        parameters: json!({"type":"object","properties":{"timezone":{"type":"string"}}}),
        tier: RiskTier::T0,
    }
}

/// Последовательность из документации (streaming, tool_use), адаптированная под русский.
fn stream_events() -> Vec<serde_json::Value> {
    vec![
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[],"stop_reason":null,"usage":{"input_tokens":472,"output_tokens":2}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"ping"}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Сейчас "}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"узнаю время."}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"get_time","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"timezone\":"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"Europe/Mos"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"cow\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":89}}),
        json!({"type":"message_stop"}),
    ]
}

fn app(rec: Recorder, prefix: &'static str) -> Router {
    Router::new().route(
        &format!("{prefix}/messages"),
        post(move |h: HeaderMap, b: Bytes| {
            let rec = rec.clone();
            async move {
                rec.record("/messages", &h, &b);
                let body: serde_json::Value = serde_json::from_slice(&b).unwrap();
                match body["model"].as_str().unwrap() {
                    "missing" => json(404, json!({"type":"error","error":{"type":"not_found_error","message":"model: missing"}})),
                    "busy" => json(529, json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}})),
                    _ if body["stream"] == true => anthropic_sse(&stream_events(), 9),
                    _ => json(200, json!({
                        "id":"msg_2","type":"message","role":"assistant","model":"claude-haiku-5-5",
                        "content":[{"type":"text","text":"Голубое"},{"type":"tool_use","id":"toolu_9","name":"get_time","input":{"timezone":"UTC"}}],
                        "stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":5}
                    })),
                }
            }
        }),
    )
}

#[tokio::test]
async fn messages_plain_headers_and_body() {
    let rec = Recorder::default();
    let base = serve(app(rec.clone(), "/v1")).await;
    // base без /v1 — клиент допишет.
    let spec = ProviderSpec::from_config(&anthropic_provider("anthropic", &base, "sk-ant-test-0000000000"));
    let c = AnthropicClient::new(spec, "claude-haiku-5-5");
    let req = ChatRequest {
        messages: vec![ChatMessage::system("Ты Джарвис."), ChatMessage::user("Небо какого цвета?")],
        tools: vec![tool()],
        max_tokens: Some(100),
        ..Default::default()
    };
    let r = c.chat(req, CancellationToken::new()).await.unwrap();
    assert_eq!(r.message.content, "Голубое");
    assert_eq!(r.message.tool_calls[0].arguments, json!({"timezone":"UTC"}));
    assert_eq!(r.model, "anthropic/claude-haiku-5-5");
    let s = rec.last();
    assert_eq!(s.headers["x-api-key"], "sk-ant-test-0000000000");
    assert_eq!(s.headers["anthropic-version"], "2023-06-01");
    assert!(s.headers.get("authorization").is_none());
    assert_eq!(s.body["system"], "Ты Джарвис.");
    assert_eq!(s.body["messages"][0]["role"], "user");
    assert_eq!(s.body["max_tokens"], 100);
    assert_eq!(s.body["tools"][0]["input_schema"]["type"], "object");
}

#[tokio::test]
async fn messages_stream_with_tool_use() {
    let rec = Recorder::default();
    let base = serve(app(rec.clone(), "/v1")).await;
    let spec = ProviderSpec::from_config(&anthropic_provider("anthropic", &format!("{base}/v1"), "k-0000000000"));
    let c = AnthropicClient::new(spec, "claude-haiku-5-5");
    let (tx, mut rx) = mpsc::channel(64);
    let req = ChatRequest { messages: vec![ChatMessage::user("который час?")], tools: vec![tool()], ..Default::default() };
    let fut = c.chat_stream(req, tx, CancellationToken::new());
    let recv = async {
        let mut v = Vec::new();
        while let Some(e) = rx.recv().await {
            v.push(e);
        }
        v
    };
    let (res, events) = tokio::join!(fut, recv);
    let res = res.unwrap();
    assert_eq!(res.message.content, "Сейчас узнаю время.");
    assert_eq!(res.message.tool_calls.len(), 1);
    assert_eq!(res.message.tool_calls[0].arguments, json!({"timezone":"Europe/Moscow"}));
    assert_eq!(res.usage.unwrap().output_tokens, 89);
    assert_eq!(events.first(), Some(&ChatEvent::TextDelta("Сейчас ".into())));
    assert!(matches!(events.last(), Some(ChatEvent::ToolCall(c)) if c.id == "toolu_01"));
    assert_eq!(rec.last().body["stream"], true);
}

#[tokio::test]
async fn anthropic_errors() {
    let base = serve(app(Recorder::default(), "/v1")).await;
    let spec = ProviderSpec::from_config(&anthropic_provider("anthropic", &format!("{base}/v1"), "k-0000000000"));
    let req = ChatRequest { messages: vec![ChatMessage::user("hi")], ..Default::default() };
    let e = AnthropicClient::new(spec.clone(), "missing").chat(req.clone(), CancellationToken::new()).await.unwrap_err();
    assert_eq!(e, LlmError::ModelNotFound("missing".into()));
    let e = AnthropicClient::new(spec, "busy").chat(req, CancellationToken::new()).await.unwrap_err();
    assert_eq!(e, LlmError::Server(529));
    assert!(e.should_failover());
}

#[tokio::test]
async fn zen_claude_goes_to_messages_with_bearer() {
    let rec = Recorder::default();
    let base = serve(app(rec.clone(), "/inference/anthropic/v1")).await;
    // Как пресет opencode-zen, но адреса — мок-сервера.
    let mut spec = ProviderSpec::from_config(&provider("opencode-zen", &format!("{base}/inference/openai/v1"), "zen-key-0000000000"));
    spec.routes = vec![ModelRoute {
        models: vec!["claude-*".into()],
        kind: ProviderKind::Anthropic,
        base_url: format!("{base}/inference/anthropic/v1"),
        auth: Some(jarvis_llm::AuthStyle::Bearer),
    }];
    let c = client_for(&spec, "claude-sonnet-5-5", None);
    assert_eq!(c.id(), "opencode-zen/claude-sonnet-5-5");
    let r = c.chat(ChatRequest { messages: vec![ChatMessage::user("hi")], ..Default::default() }, CancellationToken::new()).await.unwrap();
    assert_eq!(r.model, "opencode-zen/claude-sonnet-5-5");
    let s = rec.last();
    assert_eq!(s.headers["authorization"], "Bearer zen-key-0000000000");
    assert!(s.headers.get("x-api-key").is_none());
    assert_eq!(s.headers["anthropic-version"], "2023-06-01");
    assert_eq!(s.body["model"], "claude-sonnet-5-5");
}
