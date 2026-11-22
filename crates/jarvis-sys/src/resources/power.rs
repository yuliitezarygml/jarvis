//! Схемы питания: `power_plan_get` (T0), `power_plan_set` (T2, с откатом).

use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::open::sim;
use crate::undo::{UndoAction, UndoJournal};
use crate::util::{req_str, spec};
use crate::win;

/// Стандартные схемы и их русские названия.
const KNOWN: &[(&str, &[&str])] = &[
    ("381b4222-f694-41f0-9685-ff5bb260df2e", &["сбалансированная", "balanced", "обычная"]),
    ("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c", &["высокая производительность", "high performance", "производительность", "максимальная"]),
    ("a1841308-3541-4fab-bc81-f71556f20b4a", &["экономия энергии", "power saver", "экономия", "энергосбережение"]),
    ("e9a42b02-d5df-448d-aa00-03f14749eb61", &["максимальная производительность", "ultimate performance"]),
];

/// Найти схему по имени/GUID среди доступных.
pub fn pick_scheme(schemes: &[(String, String)], query: &str) -> Option<(String, String)> {
    let q = query.trim().to_lowercase();
    if let Some(s) = schemes.iter().find(|(g, _)| *g == q.trim_matches(['{', '}'])) {
        return Some(s.clone());
    }
    let mut best: Option<(f32, &(String, String))> = None;
    for s in schemes {
        let mut sc = sim(&q, &s.1);
        if let Some((_, names)) = KNOWN.iter().find(|(g, _)| *g == s.0) {
            sc = names.iter().map(|n| sim(&q, n)).fold(sc, f32::max);
        }
        if sc >= 0.7 && best.is_none_or(|(b, _)| sc > b) {
            best = Some((sc, s));
        }
    }
    best.map(|(_, s)| s.clone())
}

pub struct PowerPlanGet;

#[async_trait]
impl Tool for PowerPlanGet {
    fn spec(&self) -> ToolSpec {
        spec("power_plan_get", "Схемы электропитания и активная схема.", json!({"type":"object","properties":{}}), RiskTier::T0)
    }
    async fn call(&self, _a: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let (all, active) = tokio::task::spawn_blocking(|| (win::power_schemes(), win::active_power_scheme()))
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let all = all.map_err(|e| ToolError::Failed(e.to_string()))?;
        let active = active.map_err(|e| ToolError::Failed(e.to_string()))?;
        let lines: Vec<String> = all.iter().map(|(g, n)| format!("{}{n} ({g})", if *g == active { "* " } else { "  " })).collect();
        Ok(ToolOutput { content: lines.join("\n"), data: Some(json!({"active": active, "schemes": all})), ..Default::default() })
    }
}

pub struct PowerPlanSet(pub Arc<UndoJournal>);

#[async_trait]
impl Tool for PowerPlanSet {
    fn spec(&self) -> ToolSpec {
        spec(
            "power_plan_set",
            "Включить схему электропитания по названию («высокая производительность», «сбалансированная», «экономия энергии») или GUID. Откат — undo_last.",
            json!({"type":"object","properties":{"plan":{"type":"string"}},"required":["plan"]}),
            RiskTier::T2,
        )
    }
    async fn call(&self, args: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let q = req_str(&args, "plan")?;
        let undo = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let all = win::power_schemes().map_err(|e| ToolError::Failed(e.to_string()))?;
            let (guid, name) = pick_scheme(&all, &q).ok_or_else(|| ToolError::Failed(format!("схема «{q}» не найдена")))?;
            let prev = win::active_power_scheme().map_err(|e| ToolError::Failed(e.to_string()))?;
            if prev == guid {
                return Ok(ToolOutput::text(format!("Схема «{name}» уже активна")));
            }
            win::set_power_scheme(&guid).map_err(|e| ToolError::Failed(e.to_string()))?;
            undo.record("power_plan_set", &format!("схема питания: {name}"), vec![UndoAction::PowerScheme { previous: prev }])
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            Ok(ToolOutput::text(format!("Включена схема «{name}»")))
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_is_read_only() {
        let all = win::power_schemes().unwrap();
        assert!(!all.is_empty());
        let active = win::active_power_scheme().unwrap();
        assert!(all.iter().any(|(g, _)| *g == active));
    }

    #[test]
    fn pick_by_russian_name() {
        let schemes = vec![
            ("381b4222-f694-41f0-9685-ff5bb260df2e".to_string(), "Сбалансированная".to_string()),
            ("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".to_string(), "High performance".to_string()),
        ];
        assert_eq!(pick_scheme(&schemes, "высокая производительность").unwrap().0, schemes[1].0);
        assert_eq!(pick_scheme(&schemes, "сбалансированная").unwrap().0, schemes[0].0);
        assert_eq!(pick_scheme(&schemes, "381b4222-f694-41f0-9685-ff5bb260df2e").unwrap().0, schemes[0].0);
        assert!(pick_scheme(&schemes, "квантовая").is_none());
    }
}
