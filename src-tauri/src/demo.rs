//! Демо-данные для экранов, у которых ещё нет бэкенда: проверка провайдеров/серверов/поиска,
//! аудиоустройства, память, база знаний, история. Все ответы помечены `demo: true` — UI показывает
//! плашку «демо». Интегратор заменяет тела команд в `commands.rs` на настоящие модули,
//! сохраняя эти типы ответа (их стоит перенести в `jarvis-core`).

use std::sync::Mutex;

use jarvis_core::ProviderKind;
use serde::{Deserialize, Serialize};

// ───────────────────────────── провайдеры LLM ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// Где получить ключ.
    pub key_url: String,
    pub needs_key: bool,
    pub note: String,
}

fn preset(id: &str, name: &str, kind: ProviderKind, base_url: &str, key_url: &str, needs_key: bool, note: &str) -> ProviderPreset {
    ProviderPreset {
        id: id.into(),
        name: name.into(),
        kind,
        base_url: base_url.into(),
        key_url: key_url.into(),
        needs_key,
        note: note.into(),
    }
}

/// Пресеты (демо-копия; настоящий список — `providers.json` модуля LLM).
pub fn provider_presets() -> Vec<ProviderPreset> {
    use ProviderKind::*;
    vec![
        preset("groq", "Groq", OpenaiCompat, "https://api.groq.com/openai/v1", "https://console.groq.com/keys", true, "Быстрые открытые модели, бесплатный лимит"),
        preset("opencode-zen", "OpenCode Zen", OpenaiCompat, "https://opencode.ai/zen/v1", "https://opencode.ai/auth", true, "Кураторский набор моделей для агентов"),
        preset("openrouter", "OpenRouter", OpenaiCompat, "https://openrouter.ai/api/v1", "https://openrouter.ai/keys", true, "Сотни моделей через один ключ"),
        preset("openai", "OpenAI", OpenaiCompat, "https://api.openai.com/v1", "https://platform.openai.com/api-keys", true, ""),
        preset("anthropic", "Anthropic (Claude)", Anthropic, "https://api.anthropic.com/v1", "https://console.anthropic.com/settings/keys", true, "Messages API"),
        preset("gemini", "Google Gemini", OpenaiCompat, "https://generativelanguage.googleapis.com/v1beta/openai", "https://aistudio.google.com/apikey", true, "OpenAI-совместимый режим"),
        preset("mistral", "Mistral", OpenaiCompat, "https://api.mistral.ai/v1", "https://console.mistral.ai/api-keys", true, ""),
        preset("deepseek", "DeepSeek", OpenaiCompat, "https://api.deepseek.com/v1", "https://platform.deepseek.com/api_keys", true, ""),
        preset("cerebras", "Cerebras", OpenaiCompat, "https://api.cerebras.ai/v1", "https://cloud.cerebras.ai/", true, ""),
        preset("ollama", "Ollama (локально)", OpenaiCompat, "http://127.0.0.1:11434/v1", "", false, "Без ключа, модели на этом ПК"),
        preset("lmstudio", "LM Studio (локально)", OpenaiCompat, "http://127.0.0.1:1234/v1", "", false, "Без ключа"),
        preset("custom", "Свой адрес", OpenaiCompat, "", "", true, "Любой OpenAI-совместимый сервер"),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelView {
    pub id: String,
    pub ctx: u32,
    pub tools: bool,
    pub vision: bool,
    pub reasoning: bool,
    /// Время до первого токена по живой проверке, мс.
    pub latency_ms: Option<u32>,
    pub tokens_per_s: Option<f32>,
    pub tools_ok: Option<bool>,
    pub vision_ok: Option<bool>,
    /// Качество 1–5 из каталога.
    pub quality: u8,
    /// Роли, для которых модель в топе: `brain`, `fast`, `vision`, `reflection`.
    pub recommended_for: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedModel {
    /// «provider_id/model_id».
    pub model: String,
    /// «tools ✓ · русский ✓ · 0,4 с · 128k».
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleRanking {
    pub role: String,
    pub top: Vec<RankedModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    BadKey,
    Network,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderCheck {
    pub demo: bool,
    pub status: CheckStatus,
    /// Человеческий текст: «Ключ рабочий, 6 моделей».
    pub message: String,
    pub models: Vec<ModelView>,
    pub ranking: Vec<RoleRanking>,
}

fn model(id: &str, ctx: u32, tools: bool, vision: bool, reasoning: bool, lat: u32, tps: f32, q: u8) -> ModelView {
    ModelView {
        id: id.into(),
        ctx,
        tools,
        vision,
        reasoning,
        latency_ms: Some(lat),
        tokens_per_s: Some(tps),
        tools_ok: Some(tools),
        vision_ok: if vision { Some(true) } else { None },
        quality: q,
        recommended_for: vec![],
    }
}

fn demo_models(provider_id: &str) -> Vec<ModelView> {
    match provider_id {
        "groq" => vec![
            model("openai/gpt-oss-120b", 131_072, true, false, true, 380, 480.0, 4),
            model("openai/gpt-oss-20b", 131_072, true, false, true, 210, 900.0, 3),
            model("meta-llama/llama-4-maverick-17b-128e-instruct", 131_072, true, true, false, 420, 520.0, 3),
            model("meta-llama/llama-4-scout-17b-16e-instruct", 131_072, true, true, false, 300, 700.0, 3),
            model("qwen/qwen3-32b", 131_072, true, false, true, 450, 400.0, 4),
            model("moonshotai/kimi-k2-instruct", 131_072, true, false, false, 600, 220.0, 4),
        ],
        "anthropic" => vec![
            model("claude-sonnet", 200_000, true, true, true, 900, 80.0, 5),
            model("claude-haiku", 200_000, true, true, false, 500, 140.0, 4),
        ],
        _ => vec![
            model("demo/large-instruct", 128_000, true, true, true, 800, 90.0, 4),
            model("demo/small-instruct", 32_768, true, false, false, 250, 300.0, 3),
            model("demo/vision-lite", 64_000, false, true, false, 400, 150.0, 3),
        ],
    }
}

fn human_ctx(ctx: u32) -> String {
    if ctx >= 1000 { format!("{}k", ctx / 1024) } else { ctx.to_string() }
}

fn reason(m: &ModelView) -> String {
    let lat = m.latency_ms.map(|l| format!("{:.1} с", l as f32 / 1000.0).replace('.', ",")).unwrap_or_default();
    let mut parts = vec![];
    if m.tools_ok == Some(true) {
        parts.push("tools ✓".to_string());
    }
    if m.vision_ok == Some(true) {
        parts.push("vision ✓".to_string());
    }
    parts.push("русский ✓".to_string());
    parts.push(lat);
    parts.push(human_ctx(m.ctx));
    parts.join(" · ")
}

/// Простая ранжировка по ролям (демо; настоящая — в модуле LLM, план п.2.4).
pub fn rank(provider_id: &str, models: &mut [ModelView]) -> Vec<RoleRanking> {
    let score = |role: &str, m: &ModelView| -> Option<f32> {
        let lat = m.latency_ms.unwrap_or(2000) as f32 / 1000.0;
        match role {
            "brain" => (m.tools_ok == Some(true) && m.ctx >= 32_000).then(|| m.quality as f32 * 2.0 - lat),
            "fast" => Some(m.tokens_per_s.unwrap_or(0.0) / 100.0 - lat * 2.0),
            "vision" => (m.vision_ok == Some(true)).then(|| m.quality as f32 - lat),
            "reflection" => Some(m.ctx as f32 / 65_536.0 + m.quality as f32 * 0.5),
            _ => None,
        }
    };
    let roles = ["brain", "fast", "vision", "reflection"];
    let mut out = vec![];
    for role in roles {
        let mut scored: Vec<(f32, usize)> =
            models.iter().enumerate().filter_map(|(i, m)| score(role, m).map(|s| (s, i))).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let top: Vec<RankedModel> = scored
            .iter()
            .take(3)
            .map(|(_, i)| RankedModel { model: format!("{provider_id}/{}", models[*i].id), reason: reason(&models[*i]) })
            .collect();
        if let Some((_, best)) = scored.first() {
            models[*best].recommended_for.push(role.to_string());
        }
        out.push(RoleRanking { role: role.into(), top });
    }
    out
}

/// Демо-проверка ключа: пусто → «не указан», короткий → «не принят», иначе список моделей.
pub fn provider_check(provider_id: &str, base_url: &str, api_key: &str, needs_key: bool) -> ProviderCheck {
    if base_url.trim().is_empty() || !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return ProviderCheck {
            demo: true,
            status: CheckStatus::Network,
            message: "Адрес не похож на URL: нужен http:// или https://".into(),
            models: vec![],
            ranking: vec![],
        };
    }
    if needs_key && api_key.trim().is_empty() {
        return ProviderCheck {
            demo: true,
            status: CheckStatus::BadKey,
            message: "Вставьте ключ — без него провайдер не ответит".into(),
            models: vec![],
            ranking: vec![],
        };
    }
    if needs_key && api_key.trim().chars().count() < 12 {
        return ProviderCheck {
            demo: true,
            status: CheckStatus::BadKey,
            message: "Ключ не принят провайдером (HTTP 401). Проверьте, что скопировали его целиком".into(),
            models: vec![],
            ranking: vec![],
        };
    }
    let mut models = demo_models(provider_id);
    let ranking = rank(provider_id, &mut models);
    ProviderCheck {
        demo: true,
        status: CheckStatus::Ok,
        message: format!("Ключ рабочий · моделей: {} · проверено {}", models.len(), models.len().min(8)),
        models,
        ranking,
    }
}

// ───────────────────────────── серверы и поиск ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceCheck {
    pub demo: bool,
    pub ok: bool,
    pub message: String,
    pub latency_ms: Option<u32>,
}

/// `kind`: `tts` | `stt`.
pub fn voice_check(kind: &str, url: &str) -> ServiceCheck {
    let ok = url.starts_with("https://") || url.starts_with("http://");
    ServiceCheck {
        demo: true,
        ok,
        message: match (ok, kind) {
            (false, _) => "Адрес должен начинаться с https://".into(),
            (true, "tts") => "Сервер речи отвечает: /ready → 200, голос jarvis-ru".into(),
            (true, _) => "Сервер распознавания отвечает: /health → ok".into(),
        },
        latency_ms: ok.then_some(if kind == "tts" { 42 } else { 37 }),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchPreset {
    pub id: String,
    pub name: String,
    pub needs_key: bool,
    /// Подпись второго поля (Google: «cx»), пусто — поля нет.
    pub extra_label: String,
    pub key_url: String,
}

pub fn search_presets() -> Vec<SearchPreset> {
    let p = |id: &str, name: &str, needs_key: bool, extra: &str, url: &str| SearchPreset {
        id: id.into(),
        name: name.into(),
        needs_key,
        extra_label: extra.into(),
        key_url: url.into(),
    };
    vec![
        p("brave", "Brave Search", true, "", "https://api-dashboard.search.brave.com/"),
        p("tavily", "Tavily", true, "", "https://app.tavily.com/"),
        p("serper", "Serper (Google)", true, "", "https://serper.dev/"),
        p("google_cse", "Google Programmable Search", true, "Идентификатор поисковой системы (cx)", "https://programmablesearchengine.google.com/"),
        p("jina", "Jina Search", true, "", "https://jina.ai/"),
        p("duckduckgo", "DuckDuckGo (без ключа, запасной)", false, "", ""),
    ]
}

pub fn search_check(id: &str, api_key: &str, extra: &str) -> ServiceCheck {
    let preset = search_presets().into_iter().find(|p| p.id == id);
    let Some(p) = preset else {
        return ServiceCheck { demo: true, ok: false, message: format!("Неизвестный поисковик «{id}»"), latency_ms: None };
    };
    if p.needs_key && api_key.trim().is_empty() {
        return ServiceCheck { demo: true, ok: false, message: "Вставьте ключ поиска".into(), latency_ms: None };
    }
    if !p.extra_label.is_empty() && extra.trim().is_empty() {
        return ServiceCheck { demo: true, ok: false, message: format!("Заполните поле «{}»", p.extra_label), latency_ms: None };
    }
    ServiceCheck { demo: true, ok: true, message: "Работает ✓ (8 результатов, 0,6 с)".into(), latency_ms: Some(600) }
}

// ───────────────────────────── аудио ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioDevices {
    pub demo: bool,
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
}

pub fn audio_devices() -> AudioDevices {
    let d = |id: &str, name: &str, default: bool| AudioDevice { id: id.into(), name: name.into(), default };
    AudioDevices {
        demo: true,
        inputs: vec![d("default-in", "Микрофон (по умолчанию)", true), d("headset-in", "Гарнитура — микрофон", false)],
        outputs: vec![d("default-out", "Динамики (по умолчанию)", true), d("headset-out", "Гарнитура — наушники", false)],
    }
}

// ───────────────────────────── память ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Fact {
    pub id: String,
    pub text: String,
    pub confidence: f32,
    pub source: String,
    pub updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Lesson {
    pub id: String,
    pub trigger: String,
    pub what_went_wrong: String,
    pub fix: String,
    pub tool: String,
    pub seen: u32,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Playbook {
    pub id: String,
    pub name: String,
    pub when: String,
    pub steps: Vec<String>,
    pub successes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MemoryData {
    pub demo: bool,
    pub facts: Vec<Fact>,
    pub lessons: Vec<Lesson>,
    pub playbooks: Vec<Playbook>,
}

fn seed_memory() -> MemoryData {
    MemoryData {
        demo: true,
        facts: vec![
            Fact { id: "f1".into(), text: "Пользователь пьёт чёрный кофе".into(), confidence: 0.95, source: "разговор".into(), updated: "2026-11-20".into() },
            Fact { id: "f2".into(), text: "Рабочая папка проектов — D:\\projects".into(), confidence: 0.8, source: "file_search".into(), updated: "2026-11-18".into() },
            Fact { id: "f3".into(), text: "Предпочитает короткие ответы голосом".into(), confidence: 0.7, source: "рефлексия".into(), updated: "2026-11-15".into() },
        ],
        lessons: vec![
            Lesson {
                id: "l1".into(),
                trigger: "git: 'comit' is not a git command".into(),
                what_went_wrong: "Опечатка в подкоманде git".into(),
                fix: "Заменить comit на commit".into(),
                tool: "terminal".into(),
                seen: 3,
                last_seen: "2026-11-21".into(),
            },
            Lesson {
                id: "l2".into(),
                trigger: "Блокнот: кнопка «Сохранить» не найдена через UIA".into(),
                what_went_wrong: "В новом Блокноте диалог сохранения открывается по Ctrl+S".into(),
                fix: "Использовать хоткей Ctrl+S вместо поиска кнопки".into(),
                tool: "ui_click".into(),
                seen: 1,
                last_seen: "2026-11-19".into(),
            },
        ],
        playbooks: vec![Playbook {
            id: "p1".into(),
            name: "Открыть настройки Bluetooth".into(),
            when: "«открой блютуз», «настройки Bluetooth»".into(),
            steps: vec!["open_target ms-settings:bluetooth".into(), "проверить заголовок окна «Bluetooth»".into()],
            successes: 5,
        }],
    }
}

trait HasId {
    fn id_mut(&mut self) -> &mut String;
}
impl HasId for Fact {
    fn id_mut(&mut self) -> &mut String {
        &mut self.id
    }
}
impl HasId for Lesson {
    fn id_mut(&mut self) -> &mut String {
        &mut self.id
    }
}
impl HasId for Playbook {
    fn id_mut(&mut self) -> &mut String {
        &mut self.id
    }
}

/// Вставить или заменить по id; пустой id — новая запись с id `{prefix}{n}`.
fn upsert_by_id<T: HasId>(v: &mut Vec<T>, mut item: T, prefix: &str, n: u64) {
    if item.id_mut().is_empty() {
        *item.id_mut() = format!("{prefix}{n}");
    }
    let id = item.id_mut().clone();
    match v.iter_mut().position(|x| *x.id_mut() == id) {
        Some(i) => v[i] = item,
        None => v.push(item),
    }
}

/// Память в оперативке на время сессии (демо): правки и удаления видны до перезапуска.
pub struct MemoryStore {
    data: Mutex<MemoryData>,
    next_id: std::sync::atomic::AtomicU64,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self { data: Mutex::new(seed_memory()), next_id: std::sync::atomic::AtomicU64::new(100) }
    }
}

impl MemoryStore {
    pub fn list(&self) -> MemoryData {
        self.data.lock().unwrap().clone()
    }

    /// `kind`: `facts` | `lessons` | `playbooks`. Пустой `id` — новая запись.
    pub fn upsert(&self, kind: &str, item: serde_json::Value) -> Result<MemoryData, String> {
        let n = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let bad = |e: serde_json::Error| format!("неверная запись: {e}");
        let mut m = self.data.lock().unwrap();
        match kind {
            "facts" => upsert_by_id(&mut m.facts, serde_json::from_value::<Fact>(item).map_err(bad)?, "f", n),
            "lessons" => upsert_by_id(&mut m.lessons, serde_json::from_value::<Lesson>(item).map_err(bad)?, "l", n),
            "playbooks" => upsert_by_id(&mut m.playbooks, serde_json::from_value::<Playbook>(item).map_err(bad)?, "p", n),
            _ => return Err(format!("неизвестный раздел памяти «{kind}»")),
        }
        Ok(m.clone())
    }

    pub fn delete(&self, kind: &str, id: &str) -> Result<MemoryData, String> {
        let mut m = self.data.lock().unwrap();
        match kind {
            "facts" => m.facts.retain(|x| x.id != id),
            "lessons" => m.lessons.retain(|x| x.id != id),
            "playbooks" => m.playbooks.retain(|x| x.id != id),
            _ => return Err(format!("неизвестный раздел памяти «{kind}»")),
        }
        Ok(m.clone())
    }
}

// ───────────────────────────── база знаний и история ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KbHit {
    pub title: String,
    /// Путь заголовков: «Службы › Остановка службы».
    pub section: String,
    pub url: String,
    pub snippet: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KbResult {
    pub demo: bool,
    pub hits: Vec<KbHit>,
}

pub fn kb_search(query: &str) -> KbResult {
    let all = [
        ("Stop-Service", "PowerShell › Microsoft.PowerShell.Management", "https://learn.microsoft.com/powershell/module/microsoft.powershell.management/stop-service", "Останавливает одну или несколько запущенных служб. Stop-Service -Name Spooler"),
        ("sc.exe config", "Windows Commands › sc config", "https://learn.microsoft.com/windows-server/administration/windows-commands/sc-config", "Изменяет параметры службы в реестре и в базе диспетчера служб: start= disabled"),
        ("Автозагрузка приложений", "Windows › Параметры › Приложения", "https://support.microsoft.com/windows/configure-startup-applications-in-windows", "Параметры → Приложения → Автозагрузка: выключите переключатель у приложения"),
        ("winget install", "Windows Package Manager › winget", "https://learn.microsoft.com/windows/package-manager/winget/install", "Устанавливает указанное приложение: winget install --id Git.Git -e"),
        ("ipconfig /flushdns", "Windows Commands › ipconfig", "https://learn.microsoft.com/windows-server/administration/windows-commands/ipconfig", "Очищает и сбрасывает содержимое кэша DNS-сопоставителя клиента"),
    ];
    let q = query.trim().to_lowercase();
    let hits = all
        .iter()
        .enumerate()
        .filter(|(_, (t, s, _, snip))| {
            q.is_empty()
                || q.split_whitespace().any(|w| {
                    t.to_lowercase().contains(w) || s.to_lowercase().contains(w) || snip.to_lowercase().contains(w)
                })
        })
        .map(|(i, (t, s, u, snip))| KbHit {
            title: (*t).into(),
            section: (*s).into(),
            url: (*u).into(),
            snippet: (*snip).into(),
            score: 1.0 - i as f32 * 0.12,
        })
        .collect();
    KbResult { demo: true, hits }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryItem {
    pub id: String,
    /// ISO 8601.
    pub at: String,
    /// `user` | `jarvis`.
    pub speaker: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryResult {
    pub demo: bool,
    pub items: Vec<HistoryItem>,
}

pub fn history() -> HistoryResult {
    let h = |id: &str, at: &str, sp: &str, text: &str| HistoryItem { id: id.into(), at: at.into(), speaker: sp.into(), text: text.into() };
    HistoryResult {
        demo: true,
        items: vec![
            h("h1", "2026-11-22T09:14:03", "user", "Джарвис, что сейчас жрёт память?"),
            h("h2", "2026-11-22T09:14:05", "jarvis", "Сэр, память занята на 71 %, больше всего берёт браузер — 3,2 ГБ."),
            h("h3", "2026-11-22T09:20:41", "user", "Джарвис, посмотри ошибку"),
            h("h4", "2026-11-22T09:20:44", "jarvis", "Опечатка: comit вместо commit. Исправил и запустил, сэр."),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_check_paths() {
        assert_eq!(provider_check("groq", "https://api.groq.com/openai/v1", "", true).status, CheckStatus::BadKey);
        assert_eq!(provider_check("groq", "https://api.groq.com/openai/v1", "short", true).status, CheckStatus::BadKey);
        assert_eq!(provider_check("groq", "ftp://x", "gsk_aaaaaaaaaaaaaaaa", true).status, CheckStatus::Network);
        let ok = provider_check("groq", "https://api.groq.com/openai/v1", "gsk_aaaaaaaaaaaaaaaa", true);
        assert_eq!(ok.status, CheckStatus::Ok);
        assert!(ok.demo);
        assert_eq!(ok.ranking.len(), 4);
        let brain = &ok.ranking[0];
        assert_eq!(brain.role, "brain");
        assert!(brain.top[0].model.starts_with("groq/"));
        assert!(brain.top[0].reason.contains("tools ✓"));
        assert!(ok.models.iter().any(|m| m.recommended_for.contains(&"brain".to_string())));
        // локальный провайдер без ключа
        assert_eq!(provider_check("ollama", "http://127.0.0.1:11434/v1", "", false).status, CheckStatus::Ok);
    }

    #[test]
    fn memory_crud() {
        let s = MemoryStore::default();
        let n = s.list().facts.len();
        let m = s.upsert("facts", serde_json::json!({"text": "Любит тишину"})).unwrap();
        assert_eq!(m.facts.len(), n + 1);
        let new_id = m.facts.last().unwrap().id.clone();
        assert!(!new_id.is_empty());
        let m = s.upsert("facts", serde_json::json!({"id": new_id, "text": "Любит музыку"})).unwrap();
        assert_eq!(m.facts.len(), n + 1);
        assert_eq!(m.facts.last().unwrap().text, "Любит музыку");
        let m = s.delete("facts", &new_id).unwrap();
        assert_eq!(m.facts.len(), n);
        assert!(s.upsert("nope", serde_json::json!({})).is_err());
        assert!(s.delete("lessons", "l1").unwrap().lessons.iter().all(|l| l.id != "l1"));
    }

    #[test]
    fn kb_and_checks() {
        assert!(kb_search("служб").hits.iter().any(|h| h.title == "Stop-Service"));
        assert_eq!(kb_search("").hits.len(), 5);
        assert!(kb_search("zzzz").hits.is_empty());
        assert!(voice_check("tts", "https://192.168.50.38:8443").ok);
        assert!(!voice_check("stt", "192.168.50.38").ok);
        assert!(search_check("duckduckgo", "", "").ok);
        assert!(!search_check("brave", "", "").ok);
        assert!(!search_check("google_cse", "key", "").ok);
        assert!(search_check("google_cse", "key", "cx").ok);
    }
}
