//! Рейтинг моделей по ролям на основе метаданных и живой проверки. Детерминированно: при равном счёте — по id.

use serde::{Deserialize, Serialize};

use crate::ModelRole;
use crate::discovery::ModelInfo;
use crate::probe::ProbeResult;

/// Минимальный контекст для роли brain.
pub const MIN_BRAIN_CTX: u32 = 32_768;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    /// «provider/model».
    pub model_ref: String,
    pub score: f64,
    /// Почему (по-русски, для подсказки в UI).
    pub reasons: Vec<String>,
    /// Короткая подпись: «tools ✓ · русский ✓ · 0,4 с · 128k».
    pub label: String,
    /// Первое место — метка «Рекомендуем».
    pub recommended: bool,
}

fn mark(b: bool) -> &'static str {
    if b { "✓" } else { "✗" }
}

/// Число с одной цифрой после запятой, по-русски: 0.4 → «0,4».
fn ru1(x: f64) -> String {
    format!("{x:.1}").replace('.', ",")
}

/// 400 → «0,4 с»; 1234 → «1,2 с».
pub fn fmt_secs(ms: u64) -> String {
    let s = (ms as f64 / 1000.0).max(0.1);
    format!("{} с", ru1(s))
}

/// 131072 → «128k», 200000 → «200k», 1048576 → «1M».
pub fn fmt_ctx(ctx: u32) -> String {
    let pow2 = ctx % 1024 == 0;
    let k = if pow2 { ctx / 1024 } else { (ctx as f64 / 1000.0).round() as u32 };
    if k < 1000 {
        return format!("{k}k");
    }
    let m = if pow2 { ctx as f64 / 1_048_576.0 } else { ctx as f64 / 1_000_000.0 };
    let r = (m * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{}M", r as u64) } else { format!("{}M", ru1(r)) }
}

/// Цена за 1M токенов: «бесплатно» или «$0,09/$0,36 за 1M».
pub fn fmt_price(price_in: Option<f64>, price_out: Option<f64>) -> Option<String> {
    match (price_in, price_out) {
        (Some(i), Some(o)) if i == 0.0 && o == 0.0 => Some("бесплатно".into()),
        (Some(i), Some(o)) => Some(format!("${}/${} за 1M", ru_price(i), ru_price(o))),
        _ => None,
    }
}

fn ru_price(p: f64) -> String {
    let s = if p >= 10.0 { format!("{p:.0}") } else if p >= 1.0 { format!("{p:.1}") } else { format!("{p:.2}") };
    s.replace('.', ",")
}

fn ctx_label(ctx: Option<u32>) -> String {
    ctx.map(fmt_ctx).unwrap_or_else(|| "контекст ?".into())
}

fn ctx_bonus(ctx: Option<u32>, per_doubling: f64, cap: f64) -> f64 {
    ctx.map(|c| ((c as f64 / MIN_BRAIN_CTX as f64).log2().max(0.0) * per_doubling).min(cap)).unwrap_or(0.0)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn base_label(m: &ModelInfo, p: &ProbeResult) -> String {
    format!(
        "tools {} · русский {} · {} · {}",
        mark(p.tools_ok),
        mark(p.russian_ok),
        p.ttft_ms.map(fmt_secs).unwrap_or_else(|| "? с".into()),
        ctx_label(m.ctx)
    )
}

fn score(m: &ModelInfo, p: &ProbeResult, role: ModelRole) -> Option<Recommendation> {
    if !p.answered() {
        return None;
    }
    let q = m.quality as f64;
    let ttft = p.ttft_ms.unwrap_or(0) as f64 / 1000.0;
    let tps = p.tok_per_s.unwrap_or(0.0) as f64;
    let mut reasons = Vec::new();
    let s;
    let label;
    match role {
        ModelRole::Brain => {
            if !p.tools_ok || !p.russian_ok || m.ctx.is_none_or(|c| c < MIN_BRAIN_CTX) {
                return None;
            }
            s = q * 20.0 + ctx_bonus(m.ctx, 2.0, 10.0) + if m.reasoning { 3.0 } else { 0.0 } - (ttft * 8.0).min(40.0)
                + (tps / 100.0).min(5.0);
            reasons.push("инструменты вызываются".into());
            reasons.push("отвечает по-русски".into());
            reasons.push(format!("качество {}/5", m.quality));
            reasons.push(format!("контекст {}", ctx_label(m.ctx)));
            reasons.push(format!("первый токен {}", fmt_secs(p.ttft_ms.unwrap_or(0))));
            if m.reasoning {
                reasons.push("умеет рассуждать".into());
            }
            label = base_label(m, p);
        }
        ModelRole::Fast => {
            if !p.russian_ok {
                return None;
            }
            s = 60.0 - (ttft * 30.0).min(60.0) + tps.min(1000.0) / 20.0 + q * 4.0 + if p.tools_ok { 4.0 } else { 0.0 };
            reasons.push(format!("первый токен {}", fmt_secs(p.ttft_ms.unwrap_or(0))));
            if let Some(t) = p.tok_per_s {
                reasons.push(format!("{} ток/с", t.round()));
            }
            reasons.push("отвечает по-русски".into());
            reasons.push(format!("качество {}/5", m.quality));
            label = match p.tok_per_s {
                Some(t) => format!("{} · {} ток/с", base_label(m, p), t.round()),
                None => base_label(m, p),
            };
        }
        ModelRole::Vision => {
            if p.vision_ok != Some(true) {
                return None;
            }
            s = q * 20.0
                + if p.tools_ok { 5.0 } else { 0.0 }
                + if p.russian_ok { 5.0 } else { 0.0 }
                - (ttft * 8.0).min(40.0);
            reasons.push("распознала цифру на картинке".into());
            reasons.push(format!("качество {}/5", m.quality));
            if p.tools_ok {
                reasons.push("инструменты вызываются".into());
            }
            reasons.push(format!("первый токен {}", fmt_secs(p.ttft_ms.unwrap_or(0))));
            label = format!("зрение ✓ · {}", base_label(m, p));
        }
        ModelRole::Reflection => {
            if !p.russian_ok {
                return None;
            }
            let blended = match (m.price_in, m.price_out) {
                (Some(i), Some(o)) => Some(i * 0.6 + o * 0.4),
                _ => None,
            };
            // Главное — цена и контекст; качество вторично (рефлектор пишет короткий JSON).
            let cheap = match blended {
                Some(b) if b == 0.0 => 40.0,
                Some(b) => (40.0 - 10.0 * (1.0 + b * 4.0).log10()).max(0.0),
                None => 15.0,
            };
            s = cheap + ctx_bonus(m.ctx, 5.0, 25.0) + q * 5.0 - (ttft * 2.0).min(10.0);
            match fmt_price(m.price_in, m.price_out) {
                Some(pr) => reasons.push(format!("цена {pr}")),
                None => reasons.push("цена неизвестна".into()),
            }
            reasons.push(format!("контекст {}", ctx_label(m.ctx)));
            reasons.push(format!("качество {}/5", m.quality));
            label = match fmt_price(m.price_in, m.price_out) {
                Some(pr) => format!("{} · {pr}", base_label(m, p)),
                None => base_label(m, p),
            };
        }
    }
    Some(Recommendation { model_ref: m.model_ref(), score: round2(s), reasons, label, recommended: false })
}

/// Все подходящие для роли модели, лучшие первыми. Модели без успешной живой проверки не рекомендуются.
pub fn rank(models: &[ModelInfo], probes: &[ProbeResult], role: ModelRole) -> Vec<Recommendation> {
    let mut out: Vec<Recommendation> = models
        .iter()
        .filter_map(|m| {
            let mr = m.model_ref();
            let p = probes.iter().find(|p| p.model == mr)?;
            score(m, p, role)
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.model_ref.cmp(&b.model_ref)));
    out.dedup_by(|a, b| a.model_ref == b.model_ref);
    if let Some(first) = out.first_mut() {
        first.recommended = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(fmt_secs(400), "0,4 с");
        assert_eq!(fmt_secs(1234), "1,2 с");
        assert_eq!(fmt_secs(20), "0,1 с");
        assert_eq!(fmt_ctx(131_072), "128k");
        assert_eq!(fmt_ctx(200_000), "200k");
        assert_eq!(fmt_ctx(32_768), "32k");
        assert_eq!(fmt_ctx(1_048_576), "1M");
        assert_eq!(fmt_ctx(1_000_000), "1M");
        assert_eq!(fmt_ctx(1_500_000), "1,5M");
        assert_eq!(fmt_price(Some(0.0), Some(0.0)).unwrap(), "бесплатно");
        assert_eq!(fmt_price(Some(0.09), Some(0.36)).unwrap(), "$0,09/$0,36 за 1M");
        assert_eq!(fmt_price(Some(3.0), Some(15.0)).unwrap(), "$3,0/$15 за 1M");
        assert_eq!(fmt_price(None, Some(1.0)), None);
    }
}
