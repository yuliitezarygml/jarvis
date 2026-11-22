//! `d:\jarvis\config.json` — единственный файл конфигурации и секретов (в .gitignore).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    /// Спрашивать только про T3 (решение пользователя).
    #[default]
    AskDangerous,
    AskAllChanges,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI-совместимый `/chat/completions` + `/models` (Groq, OpenCode Zen, OpenRouter, OpenAI, Gemini compat, …).
    #[default]
    OpenaiCompat,
    /// Anthropic Messages API.
    Anthropic,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Короткий id: `groq`, `opencode-zen`, `openrouter`…
    pub id: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
}

/// Цепочки моделей по ролям, каждая строка — «provider_id/model_id».
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roles {
    pub brain: Vec<String>,
    pub fast: Vec<String>,
    pub vision: Vec<String>,
    pub reflection: Vec<String>,
}

impl Default for Roles {
    fn default() -> Self {
        Self { brain: vec!["groq/openai/gpt-oss-120b".into()], fast: vec![], vision: vec![], reflection: vec![] }
    }
}

/// «provider_id/model_id»; id модели сам может содержать «/» (`openai/gpt-oss-120b`), делим по первому.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn parse(s: &str) -> Option<Self> {
        let (p, m) = s.split_once('/')?;
        (!p.is_empty() && !m.is_empty()).then(|| Self { provider: p.into(), model: m.into() })
    }
}

impl std::fmt::Display for ModelRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    pub url: String,
    pub voice: String,
    /// CA для проверки TLS сервера (путь от корня проекта).
    pub ca_path: String,
}
impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            url: "https://192.168.50.38:8443".into(),
            voice: "jarvis-ru".into(),
            ca_path: "src-tauri/config/xeon-tts-ca.pem".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// `server` (Xeon) или `groq_whisper` (резерв).
    pub engine: String,
    pub url: String,
    pub ca_path: String,
}
impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: "server".into(),
            url: crate::stt_protocol::DEFAULT_URL.into(),
            ca_path: "src-tauri/config/xeon-tts-ca.pem".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchProviderConfig {
    /// `brave`, `tavily`, `serper`, `google_cse`, `jina`, `duckduckgo`.
    pub id: String,
    pub api_key: String,
    /// Для Google Programmable Search — cx.
    pub extra: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HudMode {
    Fullscreen,
    #[default]
    Corner,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HudConfig {
    pub mode: HudMode,
    pub corner_x: Option<i32>,
    pub corner_y: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Каталог данных (memory.db, kb.sqlite, undo/, logs/), относительно корня проекта.
    pub data_dir: String,
    pub tts: TtsConfig,
    pub stt: SttConfig,
    pub providers: Vec<ProviderConfig>,
    pub roles: Roles,
    pub web_search: Vec<SearchProviderConfig>,
    pub autonomy: Autonomy,
    pub run_as_admin: bool,
    /// Разрешить отправку скриншотов vision-модели.
    pub vision_enabled: bool,
    /// Авто-ввод исправленной команды в терминал (T0–T2).
    pub terminal_autorun: bool,
    pub resource_guard: bool,
    /// Корни для `file_search`.
    pub search_roots: Vec<String>,
    pub hud: HudConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: "data".into(),
            tts: TtsConfig::default(),
            stt: SttConfig::default(),
            providers: vec![ProviderConfig {
                id: "groq".into(),
                kind: ProviderKind::OpenaiCompat,
                base_url: "https://api.groq.com/openai/v1".into(),
                api_key: String::new(),
                enabled: true,
            }],
            roles: Roles::default(),
            web_search: vec![SearchProviderConfig { id: "duckduckgo".into(), enabled: true, ..Default::default() }],
            autonomy: Autonomy::AskDangerous,
            run_as_admin: true,
            vision_enabled: true,
            terminal_autorun: true,
            resource_guard: true,
            search_roots: vec!["%USERPROFILE%".into(), "D:\\".into()],
            hud: HudConfig::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("не удалось прочитать/записать конфиг: {0}")]
    Io(#[from] std::io::Error),
    #[error("ошибка в config.json: {0}")]
    Parse(#[from] serde_json::Error),
}

impl Config {
    /// Нет файла — конфиг по умолчанию. Неизвестные поля игнорируются, недостающие берутся по умолчанию.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path.as_ref()) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Атомарная запись: во временный файл рядом, затем переименование.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)? + "\n")?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }
}

/// Замаскировать ключ для UI/логов: `gsk_…abcd`.
pub fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "•".repeat(chars.len());
    }
    let head: String = chars.iter().take_while(|c| **c != '_' && **c != '-').take(4).collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{}…{}", if head.len() < chars.len() / 2 { head } else { String::new() }, tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ref_splits_on_first_slash() {
        let r = ModelRef::parse("groq/openai/gpt-oss-120b").unwrap();
        assert_eq!((r.provider.as_str(), r.model.as_str()), ("groq", "openai/gpt-oss-120b"));
        assert_eq!(r.to_string(), "groq/openai/gpt-oss-120b");
        assert!(ModelRef::parse("nomodel").is_none());
        assert!(ModelRef::parse("/x").is_none());
    }

    #[test]
    fn roundtrip_and_partial() {
        let dir = std::env::temp_dir().join(format!("jarvis-core-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");
        let mut c = Config::default();
        c.autonomy = Autonomy::Never;
        c.save(&p).unwrap();
        assert_eq!(Config::load(&p).unwrap(), c);

        std::fs::write(&p, r#"{"tts":{"voice":"x"},"unknown":1}"#).unwrap();
        let c2 = Config::load(&p).unwrap();
        assert_eq!(c2.tts.voice, "x");
        assert_eq!(c2.tts.url, TtsConfig::default().url);
        assert_eq!(c2.autonomy, Autonomy::AskDangerous);
        assert!(Config::load(dir.join("missing.json")).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_names() {
        let j = serde_json::to_value(Config::default()).unwrap();
        assert_eq!(j["autonomy"], "ask_dangerous");
        assert_eq!(j["providers"][0]["kind"], "openai_compat");
        assert_eq!(j["hud"]["mode"], "corner");
    }

    #[test]
    fn masking() {
        assert_eq!(mask_key("gsk_abcdefghijklmnop1234"), "gsk…1234");
        assert_eq!(mask_key("short"), "•••••");
        assert!(!mask_key("sk-proj-aaaaaaaaaaaaaaaaaaaaZZZZ").contains("aaaa"));
    }
}
