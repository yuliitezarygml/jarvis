//! validate_key / list_models / кэш на мок-сервере с фикстурами в стиле Groq, OpenRouter, Anthropic, Z.AI.

mod common;

use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::RawQuery;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use common::*;
use jarvis_core::Config;
use jarvis_llm::{KeyStatus, MetaSource, ModelCache, list_models, list_models_cached, validate_key};
use serde_json::json;

fn auth_ok(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer good-key-0000000000")
        || h.get("x-api-key").and_then(|v| v.to_str().ok()) == Some("good-key-0000000000")
}

#[tokio::test]
async fn groq_style_models() {
    let rec = Recorder::default();
    let r = rec.clone();
    let app = Router::new().route(
        "/openai/v1/models",
        get(move |h: HeaderMap| async move {
            r.record("/models", &h, &Bytes::new());
            if auth_ok(&h) { json(200, groq_models()) } else { json(401, json!({"error":{"message":"Invalid API Key"}})) }
        }),
    );
    let base = serve(app).await;
    let pc = provider("groq", &format!("{base}/openai/v1"), "good-key-0000000000");
    let models = list_models(&pc).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["llama-3.1-8b-instant", "openai/gpt-oss-120b", "openai/gpt-oss-20b", "qwen/qwen3.8-27b"],
        "whisper, guard, safeguard, tts и неактивные отфильтрованы");
    let oss = models.iter().find(|m| m.id == "openai/gpt-oss-120b").unwrap();
    assert_eq!(oss.ctx, Some(131072));
    assert!(oss.tools && oss.reasoning && !oss.vision);
    assert_eq!(oss.quality, 4);
    assert_eq!(oss.source, MetaSource::Mixed, "ctx от провайдера, остальное из каталога");
    assert_eq!(oss.model_ref(), "groq/openai/gpt-oss-120b");
    assert!(models.iter().find(|m| m.id == "qwen/qwen3.8-27b").unwrap().vision);

    assert_eq!(validate_key(&pc).await, KeyStatus::Valid);
    let bad = provider("groq", &format!("{base}/openai/v1"), "bad");
    assert_eq!(validate_key(&bad).await, KeyStatus::Invalid);
    let empty = provider("groq", &format!("{base}/openai/v1"), "");
    let before = rec.hits();
    assert_eq!(validate_key(&empty).await, KeyStatus::Invalid);
    assert_eq!(rec.hits(), before, "пустой ключ — без запроса");
    assert_eq!(list_models(&bad).await.unwrap_err(), jarvis_core::LlmError::Auth);
}

#[tokio::test]
async fn openrouter_style_models_and_key_endpoint() {
    let app = Router::new()
        // /models публичный — отвечает и без ключа.
        .route("/api/v1/models", get(|| async { json(200, openrouter_models()) }))
        .route(
            "/api/v1/key",
            get(|h: HeaderMap| async move {
                if auth_ok(&h) {
                    json(200, json!({"data":{"label":"sk-or-v1-abc...","limit":null,"usage":0.5,"is_free_tier":false}}))
                } else {
                    json(401, json!({"error":{"message":"Missing Authentication header","code":401}}))
                }
            }),
        );
    let base = serve(app).await;
    let pc = provider("openrouter", &format!("{base}/api/v1"), "good-key-0000000000");
    assert_eq!(validate_key(&pc).await, KeyStatus::Valid);
    assert_eq!(validate_key(&provider("openrouter", &format!("{base}/api/v1"), "wrong")).await, KeyStatus::Invalid,
        "ключ проверяется через /key, а не публичный /models");

    let models = list_models(&pc).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["anthropic/claude-fable-5.2", "inclusionai/ling-3.2-flash:free", "openai/gpt-oss-120b", "some/old-chat-model"],
        "embedding и генерация картинок отфильтрованы");
    let claude = &models[0];
    assert_eq!(claude.ctx, Some(1_000_000));
    assert!(claude.tools && claude.vision && claude.reasoning);
    assert_eq!((claude.price_in, claude.price_out), (Some(8.0), Some(40.0)));
    assert_eq!(claude.source, MetaSource::Provider);
    let oss = models.iter().find(|m| m.id == "openai/gpt-oss-120b").unwrap();
    assert_eq!((oss.price_in, oss.price_out), (Some(0.09), Some(0.36)));
    let free = &models[1];
    assert_eq!(free.price_in, Some(0.0));
    let old = &models[3];
    assert!(!old.tools, "supported_parameters без tools");
    assert_eq!(old.ctx, Some(8192));
}

#[tokio::test]
async fn anthropic_models_pagination() {
    let app = Router::new().route(
        "/v1/models",
        get(|h: HeaderMap, RawQuery(q): RawQuery| async move {
            assert_eq!(h["anthropic-version"], "2023-06-01");
            if !auth_ok(&h) {
                return json(401, json!({"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}));
            }
            // Проверка ключа ходит без параметров, список моделей — с limit=1000.
            let q = q.unwrap_or_default();
            assert!(q.is_empty() || q.contains("limit=1000"), "{q}");
            if q.contains("after_id=claude-b") {
                json(200, json!({"data":[{"type":"model","id":"claude-haiku-5-5","display_name":"Claude Haiku 5.5","max_input_tokens":1000000,
                    "capabilities":{"image_input":{"supported":true}}}],"has_more":false,"first_id":"claude-haiku-5-5","last_id":"claude-haiku-5-5"}))
            } else {
                json(200, json!({"data":[
                    {"type":"model","id":"claude-opus-5-5","display_name":"Claude Opus 5.5","max_input_tokens":1000000},
                    {"type":"model","id":"claude-b","display_name":"B"}],"has_more":true,"first_id":"claude-opus-5-5","last_id":"claude-b"}))
            }
        }),
    );
    let base = serve(app).await;
    let pc = anthropic_provider("anthropic", &format!("{base}/v1"), "good-key-0000000000");
    assert_eq!(validate_key(&pc).await, KeyStatus::Valid);
    assert_eq!(validate_key(&anthropic_provider("anthropic", &format!("{base}/v1"), "nope-0000000000")).await, KeyStatus::Invalid);
    let models = list_models(&pc).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["claude-b", "claude-haiku-5-5", "claude-opus-5-5"]);
    assert!(models[1].vision);
    assert_eq!(models[2].ctx, Some(1_000_000));
}

#[tokio::test]
async fn zai_fallback_models_when_no_models_endpoint() {
    let app = Router::new().route(
        "/api/paas/v4/chat/completions",
        post(|h: HeaderMap| async move {
            if auth_ok(&h) {
                json(200, json!({"choices":[{"message":{"content":"ok"}}]}))
            } else {
                json(401, json!({"error":{"code":"401","message":"token expired or incorrect"}}))
            }
        }),
    );
    let base = serve(app).await;
    let pc = provider("zai", &format!("{base}/api/paas/v4"), "good-key-0000000000");
    let models = list_models(&pc).await.unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["glm-5.3", "glm-5.3-flash"]);
    assert_eq!(models[0].source, MetaSource::Catalog);
    // /models → 404, ключ проверяется минимальным запросом к модели.
    assert_eq!(validate_key(&pc).await, KeyStatus::Valid);
    assert_eq!(validate_key(&provider("zai", &format!("{base}/api/paas/v4"), "bad-0000000000")).await, KeyStatus::Invalid);
}

#[tokio::test]
async fn validate_key_unreachable_cases() {
    let app = Router::new()
        .route("/down/models", get(|| async { json(503, json!({})) }))
        .route("/gem/models", get(|| async {
            json(400, json!({"error":{"code":400,"message":"Please pass a valid API key","status":"INVALID_ARGUMENT"}}))
        }));
    let base = serve(app).await;
    match validate_key(&provider("custom", &format!("{base}/down"), "k")).await {
        KeyStatus::Unreachable(s) => assert!(s.contains("503"), "{s}"),
        s => panic!("{s:?}"),
    }
    match validate_key(&provider("custom", &format!("{base}/missing"), "k")).await {
        KeyStatus::Unreachable(s) => assert!(s.contains("404") && s.contains("base_url"), "{s}"),
        s => panic!("{s:?}"),
    }
    // Gemini отвечает 400 на неверный ключ.
    assert_eq!(validate_key(&provider("gemini", &format!("{base}/gem"), "AIza-bad-key")).await, KeyStatus::Invalid);

    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    match validate_key(&provider("custom", &format!("http://127.0.0.1:{port}/v1"), "k")).await {
        KeyStatus::Unreachable(s) => assert!(s.contains("соединения"), "{s}"),
        s => panic!("{s:?}"),
    }
    assert!(matches!(validate_key(&provider("custom", "", "k")).await, KeyStatus::Unreachable(_)));
}

#[tokio::test]
async fn cache_24h_refresh_and_stale_fallback() {
    let rec = Recorder::default();
    let r = rec.clone();
    let app = Router::new().route(
        "/v1/models",
        get(move |h: HeaderMap| async move {
            r.record("/models", &h, &Bytes::new());
            json(200, groq_models())
        }),
    );
    let base = serve(app).await;
    let root = TempDir::new("cache");
    let cfg = Config::default(); // data_dir = "data"
    let cache = ModelCache::for_config(&root.0, &cfg);
    assert_eq!(cache.path_for("groq"), root.0.join("data").join("cache").join("models-groq.json"));
    let pc = provider("groq", &format!("{base}/v1"), "good-key-0000000000");

    let a = list_models_cached(&pc, &cache, false).await.unwrap();
    assert_eq!(rec.hits(), 1);
    let b = list_models_cached(&pc, &cache, false).await.unwrap();
    assert_eq!(rec.hits(), 1, "второй раз — из кэша");
    assert_eq!(a, b);
    list_models_cached(&pc, &cache, true).await.unwrap();
    assert_eq!(rec.hits(), 2, "кнопка «Обновить»");

    // Файл кэша без ключа.
    let text = std::fs::read_to_string(cache.path_for("groq")).unwrap();
    assert!(!text.contains("good-key"));
    assert!(text.contains("openai/gpt-oss-120b"));

    // Устаревший кэш → новая загрузка.
    let expired = cache.clone().with_ttl(Duration::ZERO);
    list_models_cached(&pc, &expired, false).await.unwrap();
    assert_eq!(rec.hits(), 3);

    // Сменился base_url — кэш не подходит.
    assert!(cache.load_fresh("groq", "https://other/v1").is_none());

    // Провайдер недоступен → устаревший кэш вместо ошибки.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let dead_base = format!("http://127.0.0.1:{port}/v1");
    cache.store("groq", &dead_base, &a).unwrap();
    let dead = provider("groq", &dead_base, "good-key-0000000000");
    let stale = list_models_cached(&dead, &expired, false).await.unwrap();
    assert_eq!(stale, a);
    cache.invalidate("groq");
    assert!(list_models_cached(&dead, &cache, false).await.is_err());
}
