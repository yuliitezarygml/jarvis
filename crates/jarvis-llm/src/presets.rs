//! Пресеты провайдеров (`presets/providers.json`, встроены в бинарь; можно переопределить файлом без перекомпиляции).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use jarvis_core::{ProviderConfig, ProviderKind};
use serde::{Deserialize, Serialize};

use crate::glob_match;

const BUILTIN_JSON: &str = include_str!("../presets/providers.json");

/// Как передавать ключ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthStyle {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>` (Anthropic).
    XApiKey,
    /// Не отправлять ключ.
    None,
}

/// Имя поля лимита ответа в `/chat/completions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensField {
    #[default]
    MaxTokens,
    /// OpenAI (reasoning-модели не принимают `max_tokens`), Groq, xAI.
    MaxCompletionTokens,
}

/// Маршрут части моделей провайдера на другой API (OpenCode Zen: `claude-*` → `/anthropic/v1/messages`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRoute {
    /// glob-шаблоны id модели.
    pub models: Vec<String>,
    pub kind: ProviderKind,
    pub base_url: String,
    #[serde(default)]
    pub auth: Option<AuthStyle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// Где взять ключ.
    pub key_url: String,
    pub docs_url: String,
    pub needs_key: bool,
    /// Локальный сервер (Ollama, LM Studio): бесплатно, без ключа.
    pub local: bool,
    /// Можно начать бесплатно (бесплатный ключ или бесплатные модели).
    pub free_tier: bool,
    /// Одна строка для пользователя в настройках («очень быстрый, есть бесплатный ключ»).
    pub tagline: String,
    /// Заметки разработчика про API — пользователю не показываются (разве что в «подробнее»).
    pub notes: String,
    /// По умолчанию: Bearer для OpenAI-совместимых, x-api-key для Anthropic.
    pub auth: Option<AuthStyle>,
    /// Путь списка моделей относительно `base_url`.
    pub models_path: String,
    /// Путь проверки ключа (OpenRouter: `/key`, потому что `/models` публичный).
    pub key_check_path: String,
    pub max_tokens_field: MaxTokensField,
    /// Слать `stream_options.include_usage` (только где поддержка подтверждена).
    pub stream_usage: bool,
    /// Заголовок `anthropic-version` для kind=anthropic.
    pub anthropic_version: String,
    pub extra_headers: BTreeMap<String, String>,
    pub routes: Vec<ModelRoute>,
    /// Если `/models` недоступен (404) — этот список.
    pub fallback_models: Vec<String>,
    /// Не брать в живую проверку (например, бесплатные модели Zen, доступные только из клиента OpenCode).
    pub probe_skip: Vec<String>,
}

impl Default for ProviderPreset {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: ProviderKind::OpenaiCompat,
            base_url: String::new(),
            key_url: String::new(),
            docs_url: String::new(),
            needs_key: true,
            local: false,
            free_tier: false,
            tagline: String::new(),
            notes: String::new(),
            auth: None,
            models_path: "/models".into(),
            key_check_path: "/models".into(),
            max_tokens_field: MaxTokensField::MaxTokens,
            stream_usage: false,
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.into(),
            extra_headers: BTreeMap::new(),
            routes: vec![],
            fallback_models: vec![],
            probe_skip: vec![],
        }
    }
}

pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

impl ProviderPreset {
    /// Готовая запись для `config.json`.
    pub fn to_config(&self, api_key: impl Into<String>) -> ProviderConfig {
        ProviderConfig {
            id: self.id.clone(),
            kind: self.kind,
            base_url: self.base_url.clone(),
            api_key: api_key.into(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Presets {
    pub version: u32,
    pub verified_at: String,
    pub providers: Vec<ProviderPreset>,
}

impl Presets {
    /// Встроенные пресеты (парсятся один раз; файл проверяется тестом).
    pub fn builtin() -> &'static Presets {
        static P: OnceLock<Presets> = OnceLock::new();
        P.get_or_init(|| Presets::from_json(BUILTIN_JSON).expect("presets/providers.json повреждён"))
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Пресеты из файла (правятся без перекомпиляции), при отсутствии или ошибке — встроенные.
    pub fn load_or_builtin(path: impl AsRef<Path>) -> Presets {
        match std::fs::read_to_string(path.as_ref()) {
            Ok(s) => match Presets::from_json(&s) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("{}: {e}; беру встроенные пресеты", path.as_ref().display());
                    Presets::builtin().clone()
                }
            },
            Err(_) => Presets::builtin().clone(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&ProviderPreset> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Пресет для записи из конфига: по id, а если id свой (второй ключ: `groq-2`) — по адресу API.
    pub fn for_config(&self, pc: &ProviderConfig) -> Option<&ProviderPreset> {
        self.get(&pc.id).or_else(|| {
            let url = normalize_url(&pc.base_url);
            if url.is_empty() {
                return None;
            }
            self.providers.iter().find(|p| p.kind == pc.kind && normalize_url(&p.base_url) == url)
        })
    }
}

fn normalize_url(u: &str) -> String {
    u.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// Всё, что нужно клиенту: запись из конфига + особенности провайдера из пресета.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSpec {
    pub id: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub auth: AuthStyle,
    pub needs_key: bool,
    pub local: bool,
    pub models_path: String,
    pub key_check_path: String,
    pub max_tokens_field: MaxTokensField,
    pub stream_usage: bool,
    pub anthropic_version: String,
    pub extra_headers: BTreeMap<String, String>,
    pub routes: Vec<ModelRoute>,
    pub fallback_models: Vec<String>,
    pub probe_skip: Vec<String>,
}

impl ProviderSpec {
    /// По встроенным пресетам (поиск по `id`). Неизвестный провайдер — как «custom».
    pub fn from_config(pc: &ProviderConfig) -> Self {
        Self::with_presets(pc, Presets::builtin())
    }

    pub fn with_presets(pc: &ProviderConfig, presets: &Presets) -> Self {
        let preset = presets.for_config(pc).cloned().unwrap_or_else(|| ProviderPreset {
            id: pc.id.clone(),
            kind: pc.kind,
            needs_key: false,
            ..Default::default()
        });
        // Пресет другого типа (пользователь сменил kind) — берём только общие поля.
        let same_kind = preset.kind == pc.kind;
        let default_auth = match pc.kind {
            ProviderKind::OpenaiCompat => AuthStyle::Bearer,
            ProviderKind::Anthropic => AuthStyle::XApiKey,
        };
        let base_url = if pc.base_url.trim().is_empty() { preset.base_url.clone() } else { pc.base_url.clone() };
        Self {
            id: pc.id.clone(),
            kind: pc.kind,
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            api_key: pc.api_key.trim().to_string(),
            auth: if same_kind { preset.auth.unwrap_or(default_auth) } else { default_auth },
            needs_key: preset.needs_key,
            local: preset.local,
            models_path: preset.models_path,
            key_check_path: preset.key_check_path,
            max_tokens_field: if same_kind { preset.max_tokens_field } else { MaxTokensField::MaxTokens },
            stream_usage: same_kind && preset.stream_usage,
            anthropic_version: if preset.anthropic_version.is_empty() {
                DEFAULT_ANTHROPIC_VERSION.into()
            } else {
                preset.anthropic_version
            },
            extra_headers: preset.extra_headers,
            routes: preset.routes,
            fallback_models: preset.fallback_models,
            probe_skip: preset.probe_skip,
        }
    }

    /// Маршрут из пресета для модели, если есть.
    pub fn route_for(&self, model: &str) -> Option<&ModelRoute> {
        self.routes.iter().find(|r| r.models.iter().any(|p| glob_match(p, model)))
    }

    pub fn skip_probe(&self, model: &str) -> bool {
        self.probe_skip.iter().any(|p| glob_match(p, model))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_parses_and_has_all_providers() {
        let p = Presets::builtin();
        let ids: Vec<_> = p.providers.iter().map(|p| p.id.as_str()).collect();
        for want in [
            "groq", "opencode-zen", "openrouter", "openai", "anthropic", "gemini", "mistral", "deepseek", "together",
            "fireworks", "cerebras", "xai", "zai", "ollama", "lmstudio", "custom",
        ] {
            assert!(ids.contains(&want), "нет пресета {want}");
        }
        for pr in &p.providers {
            if pr.id != "custom" {
                assert!(pr.base_url.starts_with("http"), "{}: base_url", pr.id);
                assert!(!pr.base_url.ends_with('/'), "{}: base_url без / в конце", pr.id);
            }
            if pr.needs_key {
                assert!(pr.key_url.starts_with("https://"), "{}: key_url", pr.id);
            }
            assert!(!pr.notes.is_empty(), "{}: notes", pr.id);
        }
        assert_eq!(p.get("anthropic").unwrap().kind, ProviderKind::Anthropic);
        assert!(!p.get("ollama").unwrap().needs_key);
    }

    #[test]
    fn every_preset_has_short_user_tagline() {
        for p in &Presets::builtin().providers {
            let t = p.tagline.trim();
            assert!(!t.is_empty(), "{}: нет tagline", p.id);
            assert!(t.chars().count() <= 70, "{}: tagline длинный для карточки", p.id);
            assert!(!t.contains("/models") && !t.contains("GET "), "{}: в tagline заметки разработчика", p.id);
            assert!(!p.local || p.free_tier, "{}: локальный — значит бесплатный", p.id);
        }
    }

    #[test]
    fn second_key_with_own_id_gets_preset_by_url() {
        let groq = Presets::builtin().get("groq").unwrap().clone();
        let pc = ProviderConfig {
            id: "groq-2".into(),
            kind: ProviderKind::OpenaiCompat,
            base_url: format!("{}/", groq.base_url.to_uppercase()),
            api_key: "k".into(),
            enabled: true,
        };
        let s = ProviderSpec::from_config(&pc);
        assert_eq!(s.id, "groq-2", "id остаётся своим: у второго ключа свой circuit breaker");
        assert!(s.needs_key);
        assert_eq!(s.max_tokens_field, groq.max_tokens_field);
        // Свой адрес без пресета — пустой пресет, как раньше.
        let other = ProviderSpec::from_config(&ProviderConfig { base_url: "http://my.host/v1".into(), ..pc });
        assert!(!other.needs_key);
    }

    #[test]
    fn spec_from_config_merges_preset() {
        let pc = ProviderConfig {
            id: "opencode-zen".into(),
            kind: ProviderKind::OpenaiCompat,
            base_url: "https://opencode.ai/inference/openai/v1/".into(),
            api_key: " k ".into(),
            enabled: true,
        };
        let s = ProviderSpec::from_config(&pc);
        assert_eq!(s.base_url, "https://opencode.ai/inference/openai/v1");
        assert_eq!(s.api_key, "k");
        assert_eq!(s.auth, AuthStyle::Bearer);
        let r = s.route_for("claude-sonnet-5-5").unwrap();
        assert_eq!(r.kind, ProviderKind::Anthropic);
        assert!(s.route_for("glm-5.3").is_none());
        assert!(s.skip_probe("beam-free"));

        let a = ProviderSpec::from_config(&ProviderConfig {
            id: "anthropic".into(),
            kind: ProviderKind::Anthropic,
            ..Default::default()
        });
        assert_eq!(a.auth, AuthStyle::XApiKey);
        assert_eq!(a.base_url, "https://api.anthropic.com/v1");
        assert_eq!(a.anthropic_version, "2023-06-01");

        let c = ProviderSpec::from_config(&ProviderConfig { id: "my-vllm".into(), ..Default::default() });
        assert!(!c.needs_key);
        assert_eq!(c.models_path, "/models");
    }
}
