//! ModelChain: failover, circuit breaker, стрим до первого токена, сборка модели роли из конфига.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::post;
use common::*;
use jarvis_core::{CancellationToken, ChatEvent, ChatMessage, ChatModel, ChatRequest, Config, LlmError, ProviderKind};
use jarvis_llm::{
    ApiRoute, BuildOptions, CircuitBreaker, MetaSource, ModelCache, ModelChain, ModelInfo, ModelRole, ProviderSpec,
    SpeedHint, build_role_model_with, client_for,
};
use serde_json::json;
use tokio::sync::mpsc;

/// Сервер с тремя «провайдерами»: /a — ошибка `a_status`, /b — нормальный ответ, /c — 400 (не failover).
async fn three_providers(a_status: u16) -> (String, Recorder, Recorder) {
    let (ra, rb) = (Recorder::default(), Recorder::default());
    let (ra2, rb2) = (ra.clone(), rb.clone());
    let app = Router::new()
        .route(
            "/a/chat/completions",
            post(move |h: HeaderMap, b: Bytes| async move {
                ra2.record("a", &h, &b);
                let mut r: Response = json(a_status, json!({"error":{"message":"fail"}}));
                if a_status == 429 {
                    r.headers_mut().insert("retry-after", "1".parse().unwrap());
                }
                r
            }),
        )
        .route(
            "/b/chat/completions",
            post(move |h: HeaderMap, b: Bytes| async move {
                rb2.record("b", &h, &b);
                let body: serde_json::Value = serde_json::from_slice(&b).unwrap();
                if body["stream"] == true {
                    openai_sse(&[json!({"choices":[{"delta":{"content":"Ответ "}}]}), json!({"choices":[{"delta":{"content":"от B"}}]})], 6)
                } else {
                    json(200, json!({"choices":[{"message":{"content":"Ответ от B"}}]}))
                }
            }),
        )
        .route("/c/chat/completions", post(|| async { json(400, json!({"error":{"message":"bad request"}})) }));
    (serve(app).await, ra, rb)
}

fn model(base: &str, provider: &str, path: &str) -> Arc<dyn ChatModel> {
    let spec = ProviderSpec::from_config(&common::provider(provider, &format!("{base}/{path}"), "k-0000000000"));
    client_for(&spec, "m", None)
}

fn req() -> ChatRequest {
    ChatRequest { messages: vec![ChatMessage::user("привет")], ..Default::default() }
}

#[tokio::test]
async fn failover_on_429_and_shows_who_answered() {
    let (base, ra, rb) = three_providers(429).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev2 = events.clone();
    let chain = ModelChain::new(
        vec![model(&base, "pa", "a"), model(&base, "pb", "b")],
        Arc::new(CircuitBreaker::new(3, Duration::from_secs(60))),
    )
    .with_observer(move |e| ev2.lock().unwrap().push(e.clone()));
    assert_eq!(chain.id(), "pa/m");
    let r = chain.chat(req(), CancellationToken::new()).await.unwrap();
    assert_eq!(r.model, "pb/m", "в ответе видно, кто ответил");
    assert_eq!(r.message.content, "Ответ от B");
    assert_eq!((ra.hits(), rb.hits()), (1, 1));
    let ev = events.lock().unwrap().clone();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].model, "pa/m");
    assert_eq!(ev[0].error, LlmError::RateLimited { retry_after_ms: Some(1000) });
    assert_eq!(ev[0].next.as_deref(), Some("pb/m"));
}

#[tokio::test]
async fn circuit_breaker_skips_provider_then_retries() {
    let (base, ra, rb) = three_providers(500).await;
    let breaker = Arc::new(CircuitBreaker::new(3, Duration::from_millis(400)));
    let chain = ModelChain::new(vec![model(&base, "pa", "a"), model(&base, "pb", "b")], breaker.clone());
    for _ in 0..3 {
        assert_eq!(chain.chat(req(), CancellationToken::new()).await.unwrap().model, "pb/m");
    }
    assert_eq!(ra.hits(), 3);
    assert!(breaker.is_open("pa"));
    // 4-й и 5-й вызовы — A пропускается.
    chain.chat(req(), CancellationToken::new()).await.unwrap();
    chain.chat(req(), CancellationToken::new()).await.unwrap();
    assert_eq!(ra.hits(), 3, "открытый breaker — без запросов к A");
    assert_eq!(rb.hits(), 5);
    // После паузы — одна пробная попытка к A.
    tokio::time::sleep(Duration::from_millis(500)).await;
    chain.chat(req(), CancellationToken::new()).await.unwrap();
    assert_eq!(ra.hits(), 4);
    assert!(breaker.is_open("pa"), "пробная попытка не удалась — снова открыт");
}

#[tokio::test]
async fn all_open_still_tries_and_non_failover_error_stops() {
    let (base, ra, rb) = three_providers(500).await;
    let breaker = Arc::new(CircuitBreaker::new(1, Duration::from_secs(60)));
    let only_a = ModelChain::new(vec![model(&base, "pa", "a")], breaker.clone());
    assert_eq!(only_a.chat(req(), CancellationToken::new()).await.unwrap_err(), LlmError::Server(500));
    assert!(breaker.is_open("pa"));
    // Все провайдеры цепочки «открыты» — всё равно пробуем, а не молчим.
    assert_eq!(only_a.chat(req(), CancellationToken::new()).await.unwrap_err(), LlmError::Server(500));
    assert_eq!(ra.hits(), 2);

    // 400 (не failover) — сразу ошибка, B не вызывается.
    let chain = ModelChain::new(vec![model(&base, "pc", "c"), model(&base, "pb", "b")], Arc::new(CircuitBreaker::new(3, Duration::from_secs(60))));
    match chain.chat(req(), CancellationToken::new()).await.unwrap_err() {
        LlmError::Other(s) => assert!(s.contains("400")),
        e => panic!("{e:?}"),
    }
    assert_eq!(rb.hits(), 0);

    // Отмена — без попыток.
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(chain.chat(req(), cancel).await.unwrap_err(), LlmError::Cancelled);
}

#[tokio::test]
async fn stream_failover_before_first_token() {
    let (base, _ra, _rb) = three_providers(503).await;
    let chain = ModelChain::new(
        vec![model(&base, "pa", "a"), model(&base, "pb", "b")],
        Arc::new(CircuitBreaker::new(3, Duration::from_secs(60))),
    );
    let (tx, mut rx) = mpsc::channel(16);
    let fut = chain.chat_stream(req(), tx, CancellationToken::new());
    let recv = async {
        let mut v = Vec::new();
        while let Some(e) = rx.recv().await {
            v.push(e);
        }
        v
    };
    let (res, events) = tokio::join!(fut, recv);
    let res = res.unwrap();
    assert_eq!(res.model, "pb/m");
    assert_eq!(res.message.content, "Ответ от B");
    let text: String = events.iter().map(|e| match e { ChatEvent::TextDelta(t) => t.clone(), _ => String::new() }).collect();
    assert_eq!(text, "Ответ от B", "события только от ответившей модели");
}

#[tokio::test]
async fn build_role_model_from_config_with_zen_route_from_cache() {
    let rec = Recorder::default();
    let r1 = rec.clone();
    let r2 = rec.clone();
    let app = Router::new()
        .route("/zen/openai/v1/chat/completions", post(move |h: HeaderMap, b: Bytes| async move {
            r1.record("chat", &h, &b);
            json(500, json!({}))
        }))
        .route("/zen/anthropic/v1/messages", post(move |h: HeaderMap, b: Bytes| async move {
            r2.record("messages", &h, &b);
            json(200, json!({"type":"message","content":[{"type":"text","text":"Да, сэр."}],"usage":{"input_tokens":1,"output_tokens":2}}))
        }));
    let base = serve(app).await;
    let zen_base = format!("{base}/zen/openai/v1");

    let mut cfg = Config {
        providers: vec![
            provider("opencode-zen", &zen_base, "zen-key-0000000000"),
            // Выключенный провайдер пропускается.
            jarvis_core::ProviderConfig { enabled: false, ..provider("groq", &zen_base, "k") },
        ],
        ..Default::default()
    };
    cfg.roles.brain = vec!["groq/openai/gpt-oss-120b".into(), "opencode-zen/my-claude-alias".into()];

    // Маршрут модели берётся из кэша списка моделей (как его отдаёт Zen в поле api).
    let root = TempDir::new("chain");
    let cache = ModelCache::for_config(&root.0, &cfg);
    let info = ModelInfo {
        provider: "opencode-zen".into(),
        id: "my-claude-alias".into(),
        name: None,
        ctx: Some(200_000),
        tools: true,
        vision: true,
        reasoning: true,
        quality: 5,
        speed: SpeedHint::Medium,
        price_in: None,
        price_out: None,
        source: MetaSource::Provider,
        family: None,
        api: Some(ApiRoute { kind: ProviderKind::Anthropic, base_url: Some(format!("{base}/zen/anthropic/v1")), native_api: None }),
    };
    cache.store("opencode-zen", &zen_base, &[info]).unwrap();

    let opts = BuildOptions { root: Some(root.0.clone()), breaker: Arc::new(CircuitBreaker::new(3, Duration::from_secs(60))) };
    let m = build_role_model_with(&cfg, ModelRole::Brain, &opts).unwrap();
    assert_eq!(m.id(), "opencode-zen/my-claude-alias");
    let r = m.chat(req(), CancellationToken::new()).await.unwrap();
    assert_eq!(r.message.content, "Да, сэр.");
    assert_eq!(r.model, "opencode-zen/my-claude-alias");
    let seen = rec.all();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "messages");
    assert_eq!(seen[0].headers["authorization"], "Bearer zen-key-0000000000", "для Zen /messages — Bearer");
}
