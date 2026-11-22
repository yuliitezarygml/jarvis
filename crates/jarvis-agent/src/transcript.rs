//! Протокол задачи для рефлектора памяти (сериализуемый JSON).

use jarvis_core::RiskTier;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Чем закончилась задача.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Модель дала финальный ответ.
    #[default]
    Completed,
    /// Ctrl+Alt+K / «стоп» / барж-ин.
    Cancelled,
    /// Исчерпан лимит шагов.
    MaxSteps,
    /// Модель недоступна или вернула ошибку.
    ModelError,
}

/// Чем закончился один вызов инструмента.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Done,
    Failed,
    TimedOut,
    /// Пользователь отказал в подтверждении — инструмент не вызывался.
    Denied,
    Cancelled,
    /// Модель вызвала несуществующий инструмент.
    UnknownTool,
    /// Повтор того же вызова — не выполнялся (детектор зацикливания).
    LoopBlocked,
}

/// Один вызов инструмента.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptStep {
    /// Номер обращения к модели (с 1), в ответ на которое сделан вызов.
    pub step: usize,
    pub tool: String,
    /// Аргументы (строки пропущены через `Redactor`).
    pub args: Value,
    /// `None` — инструмент не найден.
    pub tier: Option<RiskTier>,
    /// `None` — подтверждение не требовалось; `Some(true/false)` — ответ пользователя.
    pub confirmed: Option<bool>,
    pub duration_ms: u64,
    pub status: StepStatus,
    /// Начало результата (≤300 символов, без секретов).
    pub result: String,
    pub error: Option<String>,
    /// Решение из памяти, подсказанное модели после ошибки.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_fix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Transcript {
    pub user_text: String,
    /// Unix-время начала, мс.
    pub started_at_ms: u64,
    pub duration_ms: u64,
    /// Сколько раз обращались к модели.
    pub model_calls: usize,
    /// Какая модель ответила последней («provider/model», важно при failover).
    pub model: Option<String>,
    pub steps: Vec<TranscriptStep>,
    /// Служебные заметки: зацикливание, лимит шагов и т.п.
    pub notes: Vec<String>,
    pub final_text: String,
    pub stop_reason: StopReason,
    pub error: Option<String>,
}

impl Transcript {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Transcript сериализуется всегда")
    }
    /// Использовались ли инструменты (рефлектору интересны в первую очередь такие задачи).
    pub fn used_tools(&self) -> bool {
        !self.steps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_and_names() {
        let t = Transcript {
            user_text: "открой блокнот".into(),
            steps: vec![TranscriptStep {
                step: 1,
                tool: "open_target".into(),
                args: serde_json::json!({"name": "notepad"}),
                tier: Some(RiskTier::T1),
                confirmed: None,
                duration_ms: 12,
                status: StepStatus::Done,
                result: "ok".into(),
                error: None,
                known_fix: None,
            }],
            final_text: "Готово, сэр.".into(),
            stop_reason: StopReason::Completed,
            ..Default::default()
        };
        let j: Value = serde_json::from_str(&t.to_json()).unwrap();
        assert_eq!(j["stop_reason"], "completed");
        assert_eq!(j["steps"][0]["status"], "done");
        assert_eq!(j["steps"][0]["tier"], "T1");
        assert!(j["steps"][0].get("known_fix").is_none());
        let back: Transcript = serde_json::from_value(j).unwrap();
        assert_eq!(back, t);
        assert!(back.used_tools());
    }
}
