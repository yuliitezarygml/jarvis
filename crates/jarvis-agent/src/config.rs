//! Настройки цикла агента.

use std::collections::HashMap;
use std::time::Duration;

use jarvis_core::{Autonomy, Config};

use crate::prompt::PERSONA_PROMPT;

/// Параметры агента. `Default` — значения из плана (раздел 3).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentConfig {
    /// Максимум обращений к модели за одну задачу (план: 25).
    pub max_steps: usize,
    /// Таймаут инструмента по умолчанию (30 с).
    pub tool_timeout: Duration,
    /// Переопределения таймаута по имени инструмента.
    pub tool_timeouts: HashMap<String, Duration>,
    /// Когда спрашивать подтверждение (по умолчанию — только T3).
    pub autonomy: Autonomy,
    /// Разрешено ли отправлять скриншоты модели.
    pub vision_enabled: bool,
    /// Бюджет сообщений запроса в токенах (≈ символы/4): системный промпт + история + текущая задача.
    /// Описания инструментов (`tools`) сюда не входят.
    pub history_budget_tokens: usize,
    /// Сколько прошлых ходов хранить в памяти диалога (сверх бюджета всё равно обрежется).
    pub max_history_turns: usize,
    /// Длинный вывод инструмента обрезается до этого числа символов (начало + конец).
    pub max_tool_output_chars: usize,
    /// Сколько одинаковых вызовов подряд считается зацикливанием (план: 3).
    pub loop_repeat_limit: usize,
    /// Промпт персоны (по умолчанию — [`PERSONA_PROMPT`]).
    pub persona: String,
    pub temperature: Option<f32>,
    /// Бюджет ответа модели. Groq считает запрошенный бюджет в лимит токенов в минуту, поэтому не `None`;
    /// у reasoning-моделей (gpt-oss) сюда входят и рассуждения — меньше ~1500 рискует пустым ответом.
    pub max_tokens: Option<u32>,
    /// Ленивая загрузка инструментов: модели уходят `core` + группы по словам запроса и прошлого хода,
    /// остальное — через мета-инструмент `tools_load`. `false` — всегда весь реестр.
    pub lazy_tools: bool,
}

/// Бюджет ответа по умолчанию.
pub const DEFAULT_MAX_TOKENS: u32 = 1536;

impl Default for AgentConfig {
    fn default() -> Self {
        let tool_timeouts = [
            ("powershell_run", 60),
            ("cmd_run", 60),
            ("terminal_run", 60),
            ("web_research", 90),
        ]
        .into_iter()
        .map(|(n, s)| (n.to_string(), Duration::from_secs(s)))
        .collect();
        Self {
            max_steps: 25,
            tool_timeout: Duration::from_secs(30),
            tool_timeouts,
            autonomy: Autonomy::AskDangerous,
            vision_enabled: true,
            // Оценка «символы/4» для кириллицы занижает реальные токены в 1,5–2 раза, а бесплатный Groq даёт
            // 8000 токенов в минуту на модель (вход + бюджет ответа). Поэтому запрос держим ≈5–6 тыс. реальных
            // токенов: история ≈2,5k оценочных, вывод инструмента ≤3500 символов (страницы режутся).
            history_budget_tokens: 2_500,
            max_history_turns: 40,
            max_tool_output_chars: 3_500,
            loop_repeat_limit: 3,
            persona: PERSONA_PROMPT.to_string(),
            temperature: None,
            max_tokens: Some(DEFAULT_MAX_TOKENS),
            lazy_tools: true,
        }
    }
}

impl AgentConfig {
    /// Значения по умолчанию + `autonomy` и `vision_enabled` из общего `config.json`.
    pub fn from_core(cfg: &Config) -> Self {
        Self { autonomy: cfg.autonomy, vision_enabled: cfg.vision_enabled, ..Self::default() }
    }

    /// Таймаут для конкретного инструмента.
    pub fn timeout_for(&self, tool: &str) -> Duration {
        self.tool_timeouts.get(tool).copied().unwrap_or(self.tool_timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_from_plan() {
        let c = AgentConfig::default();
        assert_eq!(c.max_steps, 25);
        assert_eq!(c.timeout_for("ui_click"), Duration::from_secs(30));
        assert_eq!(c.timeout_for("powershell_run"), Duration::from_secs(60));
        assert_eq!(c.timeout_for("cmd_run"), Duration::from_secs(60));
        assert_eq!(c.timeout_for("terminal_run"), Duration::from_secs(60));
        assert_eq!(c.timeout_for("web_research"), Duration::from_secs(90));
        assert_eq!(c.autonomy, Autonomy::AskDangerous);
        assert_eq!(c.max_tokens, Some(DEFAULT_MAX_TOKENS));
        assert!(c.lazy_tools);
    }

    #[test]
    fn from_core_takes_user_settings() {
        let core = Config { autonomy: Autonomy::Never, vision_enabled: false, ..Config::default() };
        let c = AgentConfig::from_core(&core);
        assert_eq!(c.autonomy, Autonomy::Never);
        assert!(!c.vision_enabled);
        assert_eq!(c.max_steps, 25);
    }
}
