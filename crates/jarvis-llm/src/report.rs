//! «Настройки → Провайдеры → Проверить»: ключ → модели → живая проверка кандидатов → рекомендации по ролям.

use std::collections::BTreeMap;
use std::sync::Arc;

use jarvis_core::{ChatModel, Config, ProviderConfig};
use serde::{Deserialize, Serialize};

use crate::ModelRole;
use crate::chain::client_for;
use crate::discovery::{self, KeyStatus, ModelCache, ModelInfo};
use crate::http;
use crate::presets::{Presets, ProviderSpec};
use crate::probe::{ProbeOptions, ProbeResult, probe_many};
use crate::rank::{Recommendation, rank};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderReport {
    pub provider: String,
    pub key_status: KeyStatus,
    /// Чатовые модели провайдера.
    pub models: Vec<ModelInfo>,
    /// Живая проверка топ-кандидатов.
    pub probes: Vec<ProbeResult>,
    /// Топ-3 на роль (первый — «Рекомендуем»).
    pub recommendations_by_role: BTreeMap<ModelRole, Vec<Recommendation>>,
    /// Что пошло не так (человеческим текстом), если отчёт неполный.
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CheckOptions {
    /// Кэш списка моделей; `None` — всегда из сети.
    pub cache: Option<ModelCache>,
    /// Игнорировать свежий кэш (кнопка «Обновить»).
    pub force_refresh: bool,
    /// Сколько моделей проверять вживую (план: 6–8).
    pub max_candidates: usize,
    /// Из них зарезервировать под vision-модели.
    pub vision_slots: usize,
    pub probe: ProbeOptions,
    /// Пресеты (по умолчанию встроенные).
    pub presets: Option<Presets>,
    /// HTTP-клиент для проверки ключа и списка моделей (по умолчанию общий).
    pub http: Option<reqwest::Client>,
    pub top_per_role: usize,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            cache: None,
            force_refresh: false,
            max_candidates: 8,
            vision_slots: 2,
            probe: ProbeOptions::default(),
            presets: None,
            http: None,
            top_per_role: 3,
        }
    }
}

/// Предварительный счёт кандидата для живой проверки (только по метаданным).
fn pre_score(m: &ModelInfo) -> i64 {
    let ctx_ok = m.ctx.is_some_and(|c| c >= crate::rank::MIN_BRAIN_CTX);
    let speed = match m.speed {
        crate::SpeedHint::Fast => 2,
        crate::SpeedHint::Medium => 1,
        crate::SpeedHint::Slow => 0,
    };
    m.quality as i64 * 10 + if m.tools { 6 } else { 0 } + if ctx_ok { 3 } else { 0 } + speed + i64::from(m.vision)
}

/// Кого проверять: лучшие по метаданным + несколько vision-моделей; детерминированно.
pub fn select_candidates<'a>(models: &'a [ModelInfo], spec: &ProviderSpec, max: usize, vision_slots: usize) -> Vec<&'a ModelInfo> {
    let mut pool: Vec<&ModelInfo> = models.iter().filter(|m| !spec.skip_probe(&m.id)).collect();
    pool.sort_by(|a, b| pre_score(b).cmp(&pre_score(a)).then_with(|| a.id.cmp(&b.id)));
    let vision_slots = vision_slots.min(max);
    let mut picked: Vec<&ModelInfo> = pool.iter().copied().take(max - vision_slots).collect();
    // Зарезервированные места — лучшим vision-моделям, ещё не попавшим в список.
    let extra: Vec<&ModelInfo> =
        pool.iter().copied().filter(|m| m.vision && !picked.iter().any(|p| p.id == m.id)).take(vision_slots).collect();
    picked.extend(extra);
    for m in &pool {
        if picked.len() >= max {
            break;
        }
        if !picked.iter().any(|p| p.id == m.id) {
            picked.push(m);
        }
    }
    picked.truncate(max);
    picked
}

/// Полная проверка провайдера с настройками по умолчанию (без кэша).
pub async fn check_provider(pc: &ProviderConfig) -> ProviderReport {
    check_provider_with(pc, &CheckOptions::default()).await
}

pub async fn check_provider_with(pc: &ProviderConfig, opts: &CheckOptions) -> ProviderReport {
    let spec = match &opts.presets {
        Some(p) => ProviderSpec::with_presets(pc, p),
        None => ProviderSpec::from_config(pc),
    };
    let http = opts.http.clone().unwrap_or_else(http::shared_client);
    let cancel = &opts.probe.cancel;
    let mut report = ProviderReport {
        provider: pc.id.clone(),
        key_status: KeyStatus::Unreachable("не проверено".into()),
        models: vec![],
        probes: vec![],
        recommendations_by_role: BTreeMap::new(),
        error: None,
    };

    report.key_status = discovery::validate_key_spec(&spec, &http, cancel).await;
    if !report.key_status.is_valid() {
        report.error = Some(report.key_status.describe_ru());
        return report;
    }

    let models = match &opts.cache {
        Some(c) => discovery::list_models_cached_spec(&spec, c, opts.force_refresh, &http, cancel).await,
        None => discovery::fetch_models(&spec, &http, crate::Catalog::builtin(), cancel).await,
    };
    report.models = match models {
        Ok(m) => m,
        Err(e) => {
            report.error = Some(format!("не удалось получить список моделей: {e}"));
            return report;
        }
    };

    let candidates: Vec<(Arc<dyn ChatModel>, bool)> = select_candidates(&report.models, &spec, opts.max_candidates, opts.vision_slots)
        .into_iter()
        .map(|m| (client_for(&spec, &m.id, m.api.as_ref()), m.vision))
        .collect();
    report.probes = probe_many(candidates, &opts.probe).await;

    for role in ModelRole::ALL {
        let mut recs = rank(&report.models, &report.probes, role);
        recs.truncate(opts.top_per_role);
        report.recommendations_by_role.insert(role, recs);
    }
    if report.probes.iter().all(|p| !p.answered()) && !report.probes.is_empty() {
        report.error = Some("ни одна модель не прошла живую проверку".into());
    }
    report
}

/// «Выбрать лучшее автоматически»: собрать цепочки ролей из отчётов (по убыванию счёта, до `per_role` моделей).
/// Существующие цепочки заменяются, только если для роли есть рекомендации.
pub fn apply_best(cfg: &mut Config, reports: &[ProviderReport], per_role: usize) {
    for role in ModelRole::ALL {
        let mut all: Vec<&Recommendation> =
            reports.iter().filter_map(|r| r.recommendations_by_role.get(&role)).flatten().collect();
        all.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.model_ref.cmp(&b.model_ref)));
        let chain: Vec<String> = all.iter().take(per_role).map(|r| r.model_ref.clone()).collect();
        if !chain.is_empty() {
            *role.chain_mut(&mut cfg.roles) = chain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn mk(id: &str) -> ModelInfo {
        ModelInfo::from_catalog("zen", id, Catalog::builtin())
    }

    #[test]
    fn candidates_are_deterministic_and_include_vision() {
        let models: Vec<ModelInfo> = [
            "openai/gpt-oss-120b",
            "openai/gpt-oss-20b",
            "llama-3.1-8b-instant",
            "deepseek-v4-pro",
            "glm-5.3",
            "kimi-k2-instruct",
            "minimax-m2.7",
            "qwen3-235b",
            "meta-llama/llama-4-scout-17b-16e-instruct",
            "beam-free",
        ]
        .iter()
        .map(|i| mk(i))
        .collect();
        let spec = ProviderSpec::from_config(&ProviderConfig { id: "opencode-zen".into(), ..Default::default() });
        let a: Vec<_> = select_candidates(&models, &spec, 6, 2).iter().map(|m| m.id.clone()).collect();
        let b: Vec<_> = select_candidates(&models, &spec, 6, 2).iter().map(|m| m.id.clone()).collect();
        assert_eq!(a, b);
        assert_eq!(a.len(), 6);
        assert_eq!(a[0], "deepseek-v4-pro");
        assert!(a.iter().any(|i| i.contains("llama-4-scout")), "vision-модель попала в кандидаты: {a:?}");
        assert!(!a.iter().any(|i| i == "beam-free"), "probe_skip");
    }
}
