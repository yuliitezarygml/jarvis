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
    /// Нет поля в `config.json` — провайдер включён (запись, добавленная руками без `enabled`, не выключает его).
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
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
    /// Адрес сервера голоса: LAN (`https://192.168.50.38:8443`) или туннель Cloudflare (`https://….trycloudflare.com`).
    pub url: String,
    pub voice: String,
    /// CA для проверки TLS сервера в LAN (путь от корня проекта). Для публичных адресов — обычные корни Windows.
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
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// Микрофон, динамики и громкость голоса.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Имя микрофона, как его показывает Windows; `None` — микрофон Windows по умолчанию.
    pub input_device: Option<String>,
    /// Имя устройства вывода; `None` — динамики Windows по умолчанию.
    pub output_device: Option<String>,
    /// Громкость голоса Джарвиса: 0.0–1.5 (1.0 — как пришло с сервера TTS).
    pub volume: f32,
    /// Включать микрофон при запуске (иначе — кнопкой в HUD).
    pub listen_on_start: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { input_device: None, output_device: None, volume: 1.0, listen_on_start: true }
    }
}

impl AudioConfig {
    pub const MAX_VOLUME: f32 = 1.5;

    /// Громкость в допустимых пределах (NaN → 1.0).
    pub fn gain(&self) -> f32 {
        if self.volume.is_finite() { self.volume.clamp(0.0, Self::MAX_VOLUME) } else { 1.0 }
    }

    /// Пустое имя устройства = «по умолчанию».
    pub fn input(&self) -> Option<&str> {
        self.input_device.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

    pub fn output(&self) -> Option<&str> {
        self.output_device.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
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
    /// Живые обои: весь HUD на рабочем столе под иконками (как Wallpaper Engine). Работает вместе
    /// с виджетом и окном HUD; клики проходят к рабочему столу.
    pub wallpaper: bool,
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
    pub audio: AudioConfig,
    pub appearance: AppearanceConfig,
}

/// Размер виджета в углу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CornerSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl CornerSize {
    /// Сторона окна виджета, логические пиксели.
    pub fn px(self) -> f64 {
        match self {
            Self::Small => 160.0,
            Self::Medium => 220.0,
            Self::Large => 300.0,
        }
    }
}

/// Внешний вид: тема (акцентный цвет всех окон), свечение, анимация, виджет, панели HUD.
/// Сигнальные цвета (янтарь T3, красный — подтверждение/ошибка) темой не меняются.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    /// `arc-reactor` (циан, по умолчанию), `mark-iii`, `emerald`, `violet`, `white-ice`.
    pub theme: String,
    /// Свой акцентный цвет `#RRGGBB` поверх темы; `None` — цвет темы.
    pub accent: Option<String>,
    /// Яркость свечения, 0.3–1.5.
    pub glow: f32,
    /// Скорость анимации реактора, 0.25–2.
    pub animation_speed: f32,
    pub corner_size: CornerSize,
    /// Сканлинии и гекс-сетка на фоне HUD («как в фильме»).
    pub scanlines: bool,
    pub show_telemetry: bool,
    pub show_actions: bool,
    pub show_sources: bool,
    pub show_subtitles: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "arc-reactor".into(),
            accent: None,
            glow: 1.0,
            animation_speed: 1.0,
            corner_size: CornerSize::Medium,
            scanlines: true,
            show_telemetry: true,
            show_actions: true,
            show_sources: true,
            show_subtitles: true,
        }
    }
}

impl AppearanceConfig {
    /// Свой акцент, только если это корректный `#RRGGBB`.
    pub fn accent_hex(&self) -> Option<&str> {
        self.accent.as_deref().map(str::trim).filter(|s| s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit()))
    }
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
            audio: AudioConfig::default(),
            appearance: AppearanceConfig::default(),
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
    fn old_config_with_removed_persona_section_loads() {
        // Секция `persona` из прежних config.json (функция удалена) молча игнорируется.
        let c: Config = serde_json::from_str(
            r#"{"autonomy":"never","persona":{"mode":"buddy","tease":"often","swear_always":true,"banned_words":[]}}"#,
        )
        .unwrap();
        assert_eq!(c, Config { autonomy: Autonomy::Never, ..Config::default() });
        assert!(!serde_json::to_string(&c).unwrap().contains("persona"), "при сохранении секция пропадает");
    }

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
        let c = Config { autonomy: Autonomy::Never, ..Config::default() };
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
    fn missing_enabled_means_enabled_and_old_files_get_audio_defaults() {
        let c: Config = serde_json::from_str(
            r#"{"providers":[{"id":"groq","api_key":"k"},{"id":"zen","enabled":false}],"web_search":[{"id":"brave"}]}"#,
        )
        .unwrap();
        assert!(c.providers[0].enabled, "запись без enabled не выключает провайдера");
        assert!(!c.providers[1].enabled);
        assert!(c.web_search[0].enabled);
        assert_eq!(c.audio, AudioConfig::default());
        assert_eq!(c.appearance, AppearanceConfig::default());
        let ap: AppearanceConfig = serde_json::from_str(r##"{"theme":"mark-iii","accent":"#12ab9Z","corner_size":"large"}"##).unwrap();
        assert_eq!((ap.theme.as_str(), ap.accent_hex(), ap.corner_size.px(), ap.show_subtitles), ("mark-iii", None, 300.0, true));
        let ap: AppearanceConfig = serde_json::from_str(r##"{"accent":" #12AB9f "}"##).unwrap();
        assert_eq!(ap.accent_hex(), Some("#12AB9f"));
        assert_eq!(serde_json::to_value(Config::default()).unwrap()["appearance"]["corner_size"], "medium");
        let a: AudioConfig = serde_json::from_str(r#"{"input_device":"  ","volume":9}"#).unwrap();
        assert_eq!((a.input(), a.output(), a.gain(), a.listen_on_start), (None, None, AudioConfig::MAX_VOLUME, true));
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
