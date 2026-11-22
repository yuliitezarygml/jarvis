//! probe/probe_many на фейковых моделях, rank на фикстурах, check_provider целиком на мок-сервере.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Bytes;
use axum::routing::{get, post};
use common::*;
use jarvis_core::{CancellationToken, ChatMessage, ChatModel, ChatRequest, ChatResponse, Config, LlmError, ToolCall, Usage};
use jarvis_llm::{
    Catalog, CheckOptions, KeyStatus, ModelInfo, ModelRole, ProbeOptions, ProbeResult, apply_best, check_provider_with,
    probe, probe_many, rank,
};
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    id: String,
    delay_ms: u64,
    text: String,
    calls_tool: bool,
    vision_answer: String,
    rate_limit_once: AtomicBool,
    error: Option<LlmError>,
    inflight: Arc<AtomicUsize>,
    max_inflight: Arc<AtomicUsize>,
}

#[async_trait]
impl ChatModel for Fake {
    fn id(&self) -> String {
        self.id.clone()
    }
    async fn chat(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatResponse, LlmError> {
        let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_inflight.fetch_max(now, Ordering::SeqCst);
        let _guard = scopeguard(self.inflight.clone());
        tokio::select! {
            _ = cancel.cancelled() => return Err(LlmError::Cancelled),
            _ = tokio::time::sleep(Duration::from_millis(self.delay_ms)) => {}
        }
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if self.rate_limit_once.swap(false, Ordering::SeqCst) {
            return Err(LlmError::RateLimited { retry_after_ms: Some(20) });
        }
        let mut msg = ChatMessage::assistant("");
        if !req.tools.is_empty() {
            if self.calls_tool {
                msg.tool_calls.push(ToolCall { id: "c1".into(), name: "get_time".into(), arguments: json!({}) });
            } else {
                msg.content = "Сейчас примерно полдень.".into();
            }
        } else if !req.messages[0].images.is_empty() {
            msg.content = self.vision_answer.clone();
        } else {
            msg.content = self.text.clone();
        }
        Ok(ChatResponse { message: msg, model: self.id.clone(), usage: Some(Usage { input_tokens: 10, output_tokens: 20 }) })
    }
}

struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
fn scopeguard(a: Arc<AtomicUsize>) -> Guard {
    Guard(a)
}

fn opts() -> ProbeOptions {
    ProbeOptions { timeout: Duration::from_millis(500), ..Default::default() }
}

#[tokio::test]
async fn probe_measures_everything() {
    let m = Fake {
        id: "p/good".into(),
        delay_ms: 30,
        text: "Голубое".into(),
        calls_tool: true,
        vision_answer: "7".into(),
        ..Default::default()
    };
    let r = probe(&m, &opts()).await;
    assert_eq!(r.model, "p/good");
    assert!(r.ttft_ms.unwrap() >= 30);
    assert!(r.tok_per_s.unwrap() > 0.0);
    assert!(r.tools_ok && r.russian_ok);
    assert_eq!(r.vision_ok, Some(true));
    assert_eq!(r.answer, "Голубое");
    assert_eq!(r.error, None);

    let no_vision = ProbeOptions { check_vision: false, ..opts() };
    assert_eq!(probe(&m, &no_vision).await.vision_ok, None);

    let english = Fake { id: "p/en".into(), text: "Blue".into(), vision_answer: "It is 1".into(), ..Default::default() };
    let r = probe(&english, &opts()).await;
    assert!(r.answered() && !r.russian_ok && !r.tools_ok);
    assert_eq!(r.vision_ok, Some(false));
}

#[tokio::test]
async fn probe_errors_timeouts_and_429_retry() {
    let broken = Fake { id: "p/broken".into(), error: Some(LlmError::ModelNotFound("broken".into())), ..Default::default() };
    let r = probe(&broken, &opts()).await;
    assert!(!r.answered());
    assert!(r.error.as_deref().unwrap().contains("стрим"), "{:?}", r.error);

    let slow = Fake { id: "p/slow".into(), delay_ms: 2000, text: "Голубое".into(), ..Default::default() };
    let r = probe(&slow, &opts()).await;
    assert!(r.error.as_deref().unwrap().contains("таймаут"), "{:?}", r.error);

    let limited = Fake { id: "p/429".into(), text: "Синее".into(), calls_tool: true, rate_limit_once: AtomicBool::new(true), ..Default::default() };
    let r = probe(&limited, &opts()).await;
    assert!(r.answered() && r.russian_ok && r.tools_ok, "после 429 — повтор по Retry-After: {r:?}");
}

#[tokio::test]
async fn probe_many_limits_parallelism_and_keeps_order() {
    let inflight = Arc::new(AtomicUsize::new(0));
    let max = Arc::new(AtomicUsize::new(0));
    let models: Vec<(Arc<dyn ChatModel>, bool)> = (0..7)
        .map(|i| {
            let m: Arc<dyn ChatModel> = Arc::new(Fake {
                id: format!("p/m{i}"),
                delay_ms: 40,
                text: "Голубое".into(),
                calls_tool: true,
                vision_answer: "7".into(),
                inflight: inflight.clone(),
                max_inflight: max.clone(),
                ..Default::default()
            });
            (m, i % 2 == 0)
        })
        .collect();
    let res = probe_many(models, &opts()).await;
    assert_eq!(res.iter().map(|r| r.model.clone()).collect::<Vec<_>>(), (0..7).map(|i| format!("p/m{i}")).collect::<Vec<_>>());
    assert!(max.load(Ordering::SeqCst) <= 3, "параллельно не больше 3, было {}", max.load(Ordering::SeqCst));
    assert!(max.load(Ordering::SeqCst) >= 2, "но параллельно");
    assert_eq!(res[0].vision_ok, Some(true));
    assert_eq!(res[1].vision_ok, None);
}

fn info(id: &str, ctx: u32, price: Option<(f64, f64)>) -> ModelInfo {
    let mut m = ModelInfo::from_catalog("groq", id, Catalog::builtin());
    m.ctx = Some(ctx);
    if let Some((i, o)) = price {
        m.price_in = Some(i);
        m.price_out = Some(o);
    }
    m
}

fn pr(model: &str, ttft: u64, tps: f32, tools: bool, ru: bool, vision: Option<bool>) -> ProbeResult {
    ProbeResult {
        model: format!("groq/{model}"),
        ttft_ms: Some(ttft),
        tok_per_s: Some(tps),
        tools_ok: tools,
        russian_ok: ru,
        vision_ok: vision,
        answer: String::new(),
        error: None,
    }
}

#[test]
fn rank_by_role_on_fixtures() {
    let models = vec![
        info("openai/gpt-oss-120b", 131_072, Some((0.15, 0.6))), // q4
        info("openai/gpt-oss-20b", 131_072, Some((0.075, 0.3))), // q3
        info("llama-3.1-8b-instant", 131_072, Some((0.05, 0.08))), // q2, без tools в пробе
        info("qwen/qwen3.8-27b", 131_072, Some((0.3, 0.6))),     // vision
        info("kimi-k2-small-ctx", 16_384, Some((1.0, 3.0))),     // ctx < 32k
        info("free-model-x", 1_048_576, Some((0.0, 0.0))),       // бесплатная, большой контекст
        info("not-probed", 131_072, None),
    ];
    let probes = vec![
        pr("openai/gpt-oss-120b", 400, 500.0, true, true, None),
        pr("openai/gpt-oss-20b", 250, 1000.0, true, true, None),
        pr("llama-3.1-8b-instant", 150, 800.0, false, true, None),
        pr("qwen/qwen3.8-27b", 900, 200.0, true, true, Some(true)),
        pr("kimi-k2-small-ctx", 300, 100.0, true, true, None),
        pr("free-model-x", 2500, 50.0, false, true, Some(false)),
        ProbeResult { error: Some("стрим: таймаут".into()), ttft_ms: None, ..pr("broken", 0, 0.0, false, false, None) },
    ];

    let brain = rank(&models, &probes, ModelRole::Brain);
    let ids: Vec<&str> = brain.iter().map(|r| r.model_ref.as_str()).collect();
    // 120b (качество 4) впереди; 20b и qwen — оба 3/5, быстрее 20b. Без tools (llama) и с ctx < 32k (kimi) — нет.
    assert_eq!(ids, ["groq/openai/gpt-oss-120b", "groq/openai/gpt-oss-20b", "groq/qwen/qwen3.8-27b"]);
    assert!(brain[0].recommended && !brain[1].recommended);
    assert_eq!(brain[0].label, "tools ✓ · русский ✓ · 0,4 с · 128k");
    assert!(brain[0].reasons.iter().any(|r| r == "качество 4/5"));

    let fast = rank(&models, &probes, ModelRole::Fast);
    assert_eq!(fast[0].model_ref, "groq/openai/gpt-oss-20b", "{fast:#?}");
    assert!(fast[0].label.ends_with("1000 ток/с"), "{}", fast[0].label);
    assert!(fast.iter().any(|r| r.model_ref == "groq/llama-3.1-8b-instant"), "fast не требует tools");

    let vision = rank(&models, &probes, ModelRole::Vision);
    assert_eq!(vision.len(), 1);
    assert_eq!(vision[0].model_ref, "groq/qwen/qwen3.8-27b");
    assert_eq!(vision[0].label, "зрение ✓ · tools ✓ · русский ✓ · 0,9 с · 128k");

    let refl = rank(&models, &probes, ModelRole::Reflection);
    assert_eq!(refl[0].model_ref, "groq/free-model-x", "{refl:#?}");
    assert!(refl[0].label.ends_with("бесплатно"), "{}", refl[0].label);

    // Детерминированно: порядок входа не влияет.
    let mut rev_models = models.clone();
    rev_models.reverse();
    let mut rev_probes = probes.clone();
    rev_probes.reverse();
    for role in ModelRole::ALL {
        assert_eq!(rank(&models, &probes, role), rank(&rev_models, &rev_probes, role), "{role}");
    }
}

/// Мок-провайдер: /models как у Groq, /chat/completions отвечает в зависимости от модели и запроса.
fn behaviour(model: &str, body: &Value) -> Value {
    let has_tools = body.get("tools").is_some();
    let has_image = body["messages"][0]["content"].is_array();
    let text = match (model, has_tools, has_image) {
        ("openai/gpt-oss-120b", true, _) => return json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"c","type":"function","function":{"name":"get_time","arguments":"{}"}}]}}]}),
        ("qwen/qwen3.8-27b", true, _) => return json!({"choices":[{"message":{"tool_calls":[{"id":"c","type":"function","function":{"name":"get_time","arguments":"{\"timezone\":\"UTC\"}"}}]}}]}),
        (_, true, _) => "Не знаю, который час.",
        ("qwen/qwen3.8-27b", _, true) => "7",
        (_, _, true) => "Я не вижу картинку",
        ("llama-3.1-8b-instant", _, _) => "Blue",
        _ => "Голубое",
    };
    json!({"choices":[{"message":{"content":text}}],"usage":{"prompt_tokens":5,"completion_tokens":3}})
}

#[tokio::test]
async fn check_provider_end_to_end() {
    let app = Router::new()
        .route("/v1/models", get(|| async { json(200, groq_models()) }))
        .route(
            "/v1/chat/completions",
            post(|b: Bytes| async move {
                let body: Value = serde_json::from_slice(&b).unwrap();
                let model = body["model"].as_str().unwrap().to_string();
                let resp = behaviour(&model, &body);
                if body["stream"] == true {
                    let text = resp["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string();
                    let (a, b) = text.split_at(text.char_indices().nth(text.chars().count() / 2).map(|(i, _)| i).unwrap_or(0));
                    openai_sse(&[json!({"choices":[{"delta":{"content":a}}]}), json!({"choices":[{"delta":{"content":b}}]})], 16)
                } else {
                    json(200, resp)
                }
            }),
        );
    let base = serve(app).await;
    let pc = provider("groq", &format!("{base}/v1"), "gsk_good_key_000000");
    let root = TempDir::new("report");
    let mut cfg = Config::default();
    let opts = CheckOptions {
        cache: Some(jarvis_llm::ModelCache::for_config(&root.0, &cfg)),
        probe: ProbeOptions { timeout: Duration::from_secs(5), ..Default::default() },
        ..Default::default()
    };
    let rep = check_provider_with(&pc, &opts).await;
    assert_eq!(rep.key_status, KeyStatus::Valid);
    assert_eq!(rep.models.len(), 4);
    assert_eq!(rep.probes.len(), 4);
    assert!(rep.error.is_none(), "{:?}", rep.error);
    assert!(root.0.join("data/cache/models-groq.json").is_file(), "список моделей закэширован");

    let brain = &rep.recommendations_by_role[&ModelRole::Brain];
    assert_eq!(brain[0].model_ref, "groq/openai/gpt-oss-120b", "{brain:#?}");
    assert!(brain[0].recommended);
    assert_eq!(brain.len(), 2, "только модели с tools и русским: {brain:#?}");
    let vision = &rep.recommendations_by_role[&ModelRole::Vision];
    assert_eq!(vision.iter().map(|r| r.model_ref.as_str()).collect::<Vec<_>>(), ["groq/qwen/qwen3.8-27b"]);
    let fast = &rep.recommendations_by_role[&ModelRole::Fast];
    assert!(!fast.iter().any(|r| r.model_ref.contains("llama")), "llama ответила по-английски");
    assert!(fast.len() <= 3);
    let llama = rep.probes.iter().find(|p| p.model.contains("llama")).unwrap();
    assert!(llama.answered() && !llama.russian_ok && !llama.tools_ok);

    // «Выбрать лучшее автоматически».
    apply_best(&mut cfg, std::slice::from_ref(&rep), 3);
    assert_eq!(cfg.roles.brain[0], "groq/openai/gpt-oss-120b");
    assert_eq!(cfg.roles.vision, ["groq/qwen/qwen3.8-27b"]);

    // Неверный ключ — понятная ошибка и никаких проверок.
    let app = Router::new().route("/v1/models", get(|| async { json(401, json!({"error":{"message":"Invalid API Key"}})) }));
    let base = serve(app).await;
    let rep = check_provider_with(&provider("groq", &format!("{base}/v1"), "bad"), &CheckOptions::default()).await;
    assert_eq!(rep.key_status, KeyStatus::Invalid);
    assert!(rep.models.is_empty() && rep.probes.is_empty());
    assert!(rep.error.unwrap().contains("Ключ не принят"));
}
