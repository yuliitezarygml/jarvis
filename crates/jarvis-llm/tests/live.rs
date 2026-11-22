//! Живые тесты с реальными ключами из `d:\jarvis\config.json`. По умолчанию выключены.
//!
//! Запуск:
//!   cargo test --test live -- --ignored --nocapture
//!   cargo test --test live live_groq -- --ignored --nocapture
//!
//! Ключ берётся из `providers[].api_key` записи с нужным `id` (`groq`, `opencode-zen`). Если записи или ключа нет,
//! тест печатает «нет ключа» и завершается успешно. Ключ никогда не печатается целиком (только `mask_key`).
//! Корень проекта можно переопределить переменной `JARVIS_ROOT`.

use std::path::PathBuf;
use std::time::Duration;

use jarvis_core::config::mask_key;
use jarvis_core::{Config, ProviderConfig};
use jarvis_llm::{
    CheckOptions, KeyStatus, ModelCache, ModelRole, ProbeOptions, Presets, check_provider_with, list_models, validate_key,
};

fn root() -> PathBuf {
    std::env::var_os("JARVIS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

/// Запись провайдера с ключом из config.json (base_url — из конфига или из пресета).
fn provider_with_key(id: &str) -> Option<(ProviderConfig, Config)> {
    let path = root().join("config.json");
    let cfg = match Config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            println!("{id}: не удалось прочитать {}: {e} — пропуск", path.display());
            return None;
        }
    };
    let Some(pc) = cfg.provider(id).filter(|p| !p.api_key.trim().is_empty()).cloned() else {
        println!("{id}: нет ключа в {} — тест пропущен", path.display());
        return None;
    };
    let mut pc = pc;
    if pc.base_url.trim().is_empty() {
        pc.base_url = Presets::builtin().get(id).map(|p| p.base_url.clone()).unwrap_or_default();
    }
    println!("{id}: ключ {} , base_url {}", mask_key(&pc.api_key), pc.base_url);
    Some((pc, cfg))
}

async fn full_check(id: &str) {
    let Some((pc, cfg)) = provider_with_key(id) else { return };
    let status = validate_key(&pc).await;
    println!("{id}: {}", status.describe_ru());
    assert_eq!(status, KeyStatus::Valid);

    let models = list_models(&pc).await.expect("список моделей");
    println!("{id}: {} чатовых моделей", models.len());
    for m in models.iter().take(40) {
        println!(
            "  {:<45} ctx={:<8} tools={} vision={} q={} src={:?}",
            m.id,
            m.ctx.map(|c| c.to_string()).unwrap_or("?".into()),
            m.tools,
            m.vision,
            m.quality,
            m.source
        );
    }
    assert!(!models.is_empty());

    let opts = CheckOptions {
        cache: Some(ModelCache::for_config(&root(), &cfg)),
        max_candidates: 4,
        vision_slots: 1,
        probe: ProbeOptions { timeout: Duration::from_secs(15), ..Default::default() },
        ..Default::default()
    };
    let rep = check_provider_with(&pc, &opts).await;
    for p in &rep.probes {
        println!(
            "  probe {:<50} ttft={:?}ms tok/s={:?} tools={} ru={} vision={:?} ответ={:?} ошибка={:?}",
            p.model, p.ttft_ms, p.tok_per_s, p.tools_ok, p.russian_ok, p.vision_ok, p.answer, p.error
        );
    }
    for role in ModelRole::ALL {
        for r in rep.recommendations_by_role.get(&role).into_iter().flatten() {
            println!("  {role:<10} {:<50} {:>7.2}  {}{}", r.model_ref, r.score, r.label, if r.recommended { "  ← Рекомендуем" } else { "" });
        }
    }
    assert!(rep.probes.iter().any(|p| p.answered()), "хоть одна модель должна ответить: {:?}", rep.error);
}

#[tokio::test]
#[ignore = "сеть и ключ Groq из config.json: cargo test --test live live_groq -- --ignored --nocapture"]
async fn live_groq() {
    full_check("groq").await;
}

#[tokio::test]
#[ignore = "сеть и ключ OpenCode Zen из config.json: cargo test --test live live_opencode_zen -- --ignored --nocapture"]
async fn live_opencode_zen() {
    full_check("opencode-zen").await;
}

/// Публичный список моделей Zen (без ключа): проверяет, что формат `/models` всё ещё понимается.
#[tokio::test]
#[ignore = "сеть: cargo test --test live live_zen_public_models -- --ignored --nocapture"]
async fn live_zen_public_models() {
    let preset = Presets::builtin().get("opencode-zen").unwrap();
    let pc = preset.to_config("");
    let models = list_models(&pc).await.expect("публичный /models Zen");
    println!("opencode-zen: {} чатовых моделей без ключа", models.len());
    let spec = jarvis_llm::ProviderSpec::from_config(&pc);
    for m in jarvis_llm::report::select_candidates(&models, &spec, 8, 2) {
        println!("  кандидат {:<28} q={} ctx={:?} vision={} api={:?}", m.id, m.quality, m.ctx, m.vision, m.api.as_ref().map(|a| a.kind));
    }
    assert!(models.iter().any(|m| m.id.starts_with("claude-") && m.api.as_ref().is_some_and(|a| a.kind == jarvis_core::ProviderKind::Anthropic)));
    assert!(!models.iter().any(|m| m.id.contains("embedding")));
}
