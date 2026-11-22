//! LLM-провайдеры J.A.R.V.I.S. (план, раздел 2).
//!
//! - [`OpenAiCompatClient`] и [`AnthropicClient`] реализуют [`jarvis_core::ChatModel`] (обычный ответ и SSE-стрим,
//!   tool calling, картинки).
//! - [`presets`] — встроенные пресеты провайдеров (`presets/providers.json`), [`catalog`] — кураторский каталог семейств.
//! - [`discovery`] — проверка ключа, список моделей с метаданными, кэш на 24 ч.
//! - [`probe`] — живая проверка моделей, [`rank`] — рейтинг по ролям.
//! - [`chain`] — цепочка с failover и circuit breaker, сборка модели роли из конфига.
//! - [`report`] — `check_provider` для экрана настроек.

pub mod anthropic;
pub mod catalog;
pub mod chain;
pub mod discovery;
mod http;
pub mod openai;
pub mod png;
pub mod presets;
pub mod probe;
pub mod rank;
pub mod report;
mod think;

use jarvis_core::config::Roles;
use serde::{Deserialize, Serialize};

pub use anthropic::AnthropicClient;
pub use catalog::{Catalog, FamilyInfo, SpeedHint};
pub use chain::{
    BuildOptions, CircuitBreaker, FailoverEvent, ModelChain, build_role_model, build_role_model_with, client_for,
    model_from_ref,
};
pub use discovery::{
    ApiRoute, KeyStatus, MetaSource, ModelCache, ModelInfo, list_models, list_models_cached, validate_key,
};
pub use openai::OpenAiCompatClient;
pub use presets::{AuthStyle, MaxTokensField, ModelRoute, Presets, ProviderPreset, ProviderSpec};
pub use probe::{ProbeOptions, ProbeResult, probe, probe_many};
pub use rank::{Recommendation, rank};
pub use report::{CheckOptions, ProviderReport, apply_best, check_provider, check_provider_with};

/// Роль модели в приложении (у каждой в `config.json` своя цепочка «provider/model»).
///
/// В `jarvis_core::Role` — роли сообщений чата (system/user/…), поэтому здесь отдельный тип.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Основной мозг агента: tools + русский, большой контекст, качество.
    Brain,
    /// Быстрые короткие ответы: время до первого токена и скорость.
    Fast,
    /// Зрение: скриншоты, set-of-marks.
    Vision,
    /// Рефлектор памяти: дёшево и большой контекст.
    Reflection,
}

impl ModelRole {
    pub const ALL: [ModelRole; 4] = [ModelRole::Brain, ModelRole::Fast, ModelRole::Vision, ModelRole::Reflection];

    /// Цепочка этой роли из конфига.
    pub fn chain(self, roles: &Roles) -> &[String] {
        match self {
            ModelRole::Brain => &roles.brain,
            ModelRole::Fast => &roles.fast,
            ModelRole::Vision => &roles.vision,
            ModelRole::Reflection => &roles.reflection,
        }
    }

    pub fn chain_mut(self, roles: &mut Roles) -> &mut Vec<String> {
        match self {
            ModelRole::Brain => &mut roles.brain,
            ModelRole::Fast => &mut roles.fast,
            ModelRole::Vision => &mut roles.vision,
            ModelRole::Reflection => &mut roles.reflection,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ModelRole::Brain => "brain",
            ModelRole::Fast => "fast",
            ModelRole::Vision => "vision",
            ModelRole::Reflection => "reflection",
        }
    }

    /// Название для UI.
    pub fn title_ru(self) -> &'static str {
        match self {
            ModelRole::Brain => "Мозг",
            ModelRole::Fast => "Быстрые ответы",
            ModelRole::Vision => "Зрение",
            ModelRole::Reflection => "Рефлексия",
        }
    }
}

impl std::fmt::Display for ModelRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ModelRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ModelRole::ALL.into_iter().find(|r| r.as_str() == s).ok_or_else(|| format!("неизвестная роль: {s}"))
    }
}

/// Сопоставление id модели с glob-шаблоном: `*` — любая подстрока, регистр не важен.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob() {
        assert!(glob_match("*gpt-oss-120b*", "openai/gpt-oss-120b"));
        assert!(glob_match("claude-*", "Claude-Opus-5"));
        assert!(!glob_match("claude-*", "anthropic/claude-opus"));
        assert!(glob_match("*-free", "beam-free"));
        assert!(!glob_match("*-free", "beam-free-x"));
        assert!(glob_match("qwen3.5-plus", "qwen3.5-plus"));
        assert!(!glob_match("qwen3.5-plus", "qwen3.5-plus2"));
        assert!(glob_match("*", ""));
        assert!(glob_match("*gpt-5*-mini*", "gpt-5.4-mini"));
    }

    #[test]
    fn role_roundtrip() {
        for r in ModelRole::ALL {
            assert_eq!(r.as_str().parse::<ModelRole>().unwrap(), r);
            assert_eq!(serde_json::to_value(r).unwrap(), r.as_str());
        }
        let mut roles = Roles::default();
        ModelRole::Fast.chain_mut(&mut roles).push("groq/x".into());
        assert_eq!(ModelRole::Fast.chain(&roles), ["groq/x"]);
    }
}
