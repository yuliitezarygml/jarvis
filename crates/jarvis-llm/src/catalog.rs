//! Кураторский каталог семейств моделей (`presets/model_catalog.json`) и фильтр нечатовых моделей.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::glob_match;

const BUILTIN_JSON: &str = include_str!("../presets/model_catalog.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedHint {
    Fast,
    #[default]
    Medium,
    Slow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyInfo {
    pub family: String,
    #[serde(rename = "match")]
    pub patterns: Vec<String>,
    /// 1–5.
    pub quality: u8,
    pub tools: bool,
    pub vision: bool,
    pub reasoning: bool,
    /// Контекст по умолчанию (если провайдер его не отдаёт).
    pub ctx: Option<u32>,
    #[serde(default)]
    pub speed: SpeedHint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    /// glob-шаблоны нечатовых моделей (embedding, whisper, tts, guard, image, rerank, audio…).
    pub non_chat: Vec<String>,
    pub families: Vec<FamilyInfo>,
    pub default: FamilyInfo,
}

impl Catalog {
    pub fn builtin() -> &'static Catalog {
        static C: OnceLock<Catalog> = OnceLock::new();
        C.get_or_init(|| Catalog::from_json(BUILTIN_JSON).expect("presets/model_catalog.json повреждён"))
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Семейство модели по id (первое совпадение) или `default`.
    pub fn lookup(&self, model_id: &str) -> &FamilyInfo {
        self.find(model_id).unwrap_or(&self.default)
    }

    /// Семейство, если модель известна каталогу.
    pub fn find(&self, model_id: &str) -> Option<&FamilyInfo> {
        self.families.iter().find(|f| f.patterns.iter().any(|p| glob_match(p, model_id)))
    }

    /// Чатовая ли модель по id (без учёта метаданных провайдера).
    pub fn is_chat_model(&self, model_id: &str) -> bool {
        !self.non_chat.iter().any(|p| glob_match(p, model_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_is_valid() {
        let c = Catalog::builtin();
        assert!(c.families.len() > 30);
        for f in &c.families {
            assert!((1..=5).contains(&f.quality), "{}", f.family);
            assert!(!f.patterns.is_empty(), "{}", f.family);
        }
    }

    #[test]
    fn non_chat_filter() {
        let c = Catalog::builtin();
        for id in [
            "whisper-large-v3",
            "whisper-large-v3-turbo",
            "distil-whisper-large-v3-en",
            "text-embedding-3-small",
            "mistral-embed",
            "nomic-embed-text",
            "bge-m3",
            "voyage-4",
            "meta-llama/llama-guard-4-12b",
            "meta-llama/llama-prompt-guard-2-86m",
            "openai/gpt-oss-safeguard-20b",
            "omni-moderation-latest",
            "gpt-image-2.5-sunburst",
            "dall-e-3",
            "grok-imagine-image-2.0",
            "gemini-nano-banana-2.1",
            "playai-tts",
            "canopylabs/orpheus-v1-english",
            "gpt-4o-mini-tts",
            "gpt-4o-transcribe",
            "rerank-v3.5",
            "gpt-4o-audio-preview",
            "gpt-realtime",
            "gpt-live-1",
            "jev-1.13",
            "gpt-6-luna-decision",
            "mistral-ocr-latest",
        ] {
            assert!(!c.is_chat_model(id), "{id} должен быть отфильтрован");
        }
        for id in [
            "openai/gpt-oss-120b",
            "llama-3.3-70b-versatile",
            "meta-llama/llama-4-scout-17b-16e-instruct",
            "qwen/qwen3.8-27b",
            "moonshotai/kimi-k2-instruct",
            "glm-5.3",
            "claude-sonnet-5-5",
            "gemini-3.8-flash",
            "codestral-latest",
            "grok-4.7",
            "deepseek-v4-pro",
            "gpt-5.4-mini",
            "qwen3-vl-32b",
        ] {
            assert!(c.is_chat_model(id), "{id} — чатовая модель");
        }
    }

    #[test]
    fn family_lookup_order() {
        let c = Catalog::builtin();
        assert_eq!(c.lookup("openai/gpt-oss-120b").family, "gpt-oss-120b");
        assert_eq!(c.lookup("openai/gpt-oss-20b").quality, 3);
        assert_eq!(c.lookup("gpt-5.4-mini").family, "gpt-5-mini");
        assert_eq!(c.lookup("gpt-5.5").family, "gpt-5");
        assert_eq!(c.lookup("gpt-6-luna").family, "gpt-luna");
        assert_eq!(c.lookup("claude-haiku-4-5").family, "claude-haiku");
        assert_eq!(c.lookup("claude-3-5-sonnet-latest").family, "claude-sonnet");
        assert_eq!(c.lookup("gemini-3.5-flash-lite").family, "gemini-flash-lite");
        assert_eq!(c.lookup("meta-llama/llama-4-maverick-17b-128e-instruct").family, "llama-4-maverick");
        assert!(c.lookup("meta-llama/llama-4-scout-17b-16e-instruct").vision);
        assert_eq!(c.lookup("moonshotai/kimi-k2-instruct-0905").family, "kimi-k2");
        assert_eq!(c.lookup("qwen/qwen3-32b").family, "qwen3");
        assert_eq!(c.lookup("deepseek-v4.1-flash").family, "deepseek-flash");
        assert_eq!(c.lookup("mistral-large-4").family, "mistral-large");
        assert_eq!(c.lookup("codestral-latest").family, "codestral");
        assert_eq!(c.lookup("x-ai/grok-4.7").family, "grok-4");
        assert_eq!(c.lookup("totally-unknown-model").family, "unknown");
        assert!(c.find("totally-unknown-model").is_none());
    }
}
