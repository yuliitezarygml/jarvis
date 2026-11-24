//! Типы записей памяти. Все сериализуются в JSON (экран «Память», экспорт, Tauri-команды).

use jarvis_core::event::ActionStatus;
use jarvis_core::{RiskTier, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Время — секунды Unix.
pub type Timestamp = i64;

/// Категории фактов, которые понимает рефлектор и экран «Память». Любая другая строка тоже допустима.
pub mod category {
    pub const PREFERENCE: &str = "preference";
    pub const PERSONAL: &str = "personal";
    pub const ALIAS: &str = "alias";
    pub const HABIT: &str = "habit";
    pub const CONTACT: &str = "contact";
    pub const GENERAL: &str = "general";
    /// Как пользователь общается: мат, юмор, что злит, любимые словечки. Всегда в контексте отдельным
    /// блоком «Как с ним общаться» (не среди важных фактов).
    pub const STYLE: &str = "style";
    pub const ALL: &[&str] = &[PREFERENCE, PERSONAL, ALIAS, HABIT, CONTACT, GENERAL, STYLE];
}

/// Источник факта: пользователь сказал явно («запомни…») или выведено рефлектором.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactSource {
    Explicit,
    #[default]
    Inferred,
}

impl FactSource {
    pub fn as_str(self) -> &'static str {
        match self {
            FactSource::Explicit => "explicit",
            FactSource::Inferred => "inferred",
        }
    }
    pub fn parse(s: &str) -> Self {
        if s == "explicit" { FactSource::Explicit } else { FactSource::Inferred }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: i64,
    pub text: String,
    pub category: String,
    /// 0..1
    pub confidence: f64,
    pub source: FactSource,
    /// Всегда попадает в контекст (вместе с категорией `preference`).
    pub important: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub last_used: Option<Timestamp>,
    pub use_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewFact {
    pub text: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub source: FactSource,
    #[serde(default)]
    pub important: bool,
}

fn default_category() -> String {
    category::GENERAL.into()
}
fn default_confidence() -> f64 {
    0.6
}

impl NewFact {
    /// Факт, который пользователь явно попросил запомнить: confidence 1.0.
    pub fn explicit(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            category: default_category(),
            confidence: 1.0,
            source: FactSource::Explicit,
            important: false,
        }
    }
    /// Факт, выведенный рефлектором.
    pub fn inferred(text: impl Into<String>, confidence: f64) -> Self {
        Self { text: text.into(), category: default_category(), confidence, source: FactSource::Inferred, important: false }
    }
    pub fn with_category(mut self, c: impl Into<String>) -> Self {
        self.category = c.into();
        self
    }
    pub fn with_important(mut self, important: bool) -> Self {
        self.important = important;
        self
    }
}

/// Область урока (план, раздел 6 и 3б).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LessonScope {
    /// Ошибка/особенность инструмента.
    Tool,
    /// Особенность приложения.
    App,
    /// Типичная ошибка самого пользователя (опечатки в командах и т.п.) — для «поправляет меня».
    UserHabits,
    #[default]
    General,
}

impl LessonScope {
    pub fn as_str(self) -> &'static str {
        match self {
            LessonScope::Tool => "tool",
            LessonScope::App => "app",
            LessonScope::UserHabits => "user_habits",
            LessonScope::General => "general",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "tool" | "tools" => LessonScope::Tool,
            "app" | "application" => LessonScope::App,
            "user_habits" | "user_habit" | "user" | "habits" => LessonScope::UserHabits,
            "general" => LessonScope::General,
            _ => return None,
        })
    }
}

/// Урок — память ошибок.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub id: i64,
    /// Общая сигнатура ошибки (16 hex), если урок порождён текстом ошибки.
    pub signature: Option<String>,
    /// Точная сигнатура (с именами в кавычках).
    pub exact_signature: Option<String>,
    /// Нормализованный текст ошибки.
    pub error_text: Option<String>,
    pub scope: LessonScope,
    /// Ситуация: что делали, когда случилось.
    pub trigger: String,
    /// Что пошло не так.
    pub mistake: String,
    pub cause: String,
    /// Рецепт исправления.
    pub fix: String,
    pub tool: Option<String>,
    pub app: Option<String>,
    pub seen: i64,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
    pub last_used: Option<Timestamp>,
    pub confidence: f64,
    pub success_after_fix: i64,
    pub fail_after_fix: i64,
    /// Заготовка без LLM (ещё не доработана рефлектором).
    pub is_stub: bool,
}

impl Lesson {
    pub fn has_fix(&self) -> bool {
        !self.fix.trim().is_empty()
    }

    /// Предупреждение перед вызовом инструмента: «В прошлый раз…».
    pub fn warning(&self) -> String {
        let mut s = String::from("В прошлый раз");
        if let Some(t) = &self.tool {
            s.push_str(&format!(" с {t}"));
        }
        if let Some(a) = &self.app {
            s.push_str(&format!(" ({a})"));
        }
        let what = if !self.mistake.is_empty() { &self.mistake } else { &self.trigger };
        s.push_str(&format!(": {}", what.trim_end_matches('.')));
        if !self.cause.is_empty() {
            s.push_str(&format!(". Причина: {}", self.cause.trim_end_matches('.')));
        }
        if self.has_fix() {
            s.push_str(&format!(". Как правильно: {}", self.fix.trim_end_matches('.')));
        }
        s.push('.');
        s
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewLesson {
    #[serde(default)]
    pub scope: LessonScope,
    #[serde(default)]
    pub trigger: String,
    #[serde(default)]
    pub mistake: String,
    #[serde(default)]
    pub cause: String,
    #[serde(default)]
    pub fix: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    /// Сырой текст ошибки: из него автоматически считаются сигнатуры и нормализованный текст.
    #[serde(default)]
    pub error_text: Option<String>,
    /// Явная сигнатура (если текста ошибки нет).
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub is_stub: bool,
}

impl Default for NewLesson {
    fn default() -> Self {
        Self {
            scope: LessonScope::General,
            trigger: String::new(),
            mistake: String::new(),
            cause: String::new(),
            fix: String::new(),
            tool: None,
            app: None,
            error_text: None,
            signature: None,
            confidence: default_confidence(),
            is_stub: false,
        }
    }
}

/// Шаг плейбука — вызов инструмента с параметрами. В `args` допускаются подстановки `{{param}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybookStep {
    pub tool: String,
    #[serde(default = "empty_object")]
    pub args: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

impl PlaybookStep {
    pub fn new(tool: impl Into<String>, args: Value) -> Self {
        Self { tool: tool.into(), args, note: None }
    }
}

/// Плейбук — удачная последовательность действий как именованный рецепт.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playbook {
    pub id: i64,
    pub name: String,
    /// Условия применения («когда пользователь просит …»).
    pub when: String,
    pub steps: Vec<PlaybookStep>,
    /// JSON Schema параметров (`{{param}}` в шагах).
    pub params_schema: Value,
    pub success_count: i64,
    pub fail_count: i64,
    pub version: i64,
    /// Повышен консолидацией (часто и успешно используется).
    pub promoted: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub last_used: Option<Timestamp>,
}

impl Playbook {
    /// Доля успехов со сглаживанием Лапласа: новый плейбук = 0.5.
    pub fn success_rate(&self) -> f64 {
        (self.success_count as f64 + 1.0) / ((self.success_count + self.fail_count) as f64 + 2.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPlaybook {
    pub name: String,
    #[serde(default)]
    pub when: String,
    pub steps: Vec<PlaybookStep>,
    #[serde(default = "empty_object")]
    pub params_schema: Value,
}

/// Запись аудита действий.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: i64,
    pub ts: Timestamp,
    pub session_id: Option<i64>,
    pub tool: String,
    pub args: Value,
    pub tier: RiskTier,
    pub status: ActionStatus,
    pub result_summary: String,
    /// Кто подтвердил T3: `voice`, `hud_button`, `auto` …
    pub approved_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewAction {
    #[serde(default)]
    pub session_id: Option<i64>,
    pub tool: String,
    #[serde(default = "empty_object")]
    pub args: Value,
    pub tier: RiskTier,
    pub status: ActionStatus,
    #[serde(default)]
    pub result_summary: String,
    #[serde(default)]
    pub approved_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: i64,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub title: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub session_id: i64,
    pub ts: Timestamp,
    pub role: Role,
    pub content: String,
    pub tool_name: Option<String>,
}

/// Вид записи памяти (экран «Память», `mark_used`, `delete`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Lesson,
    Playbook,
}

impl MemoryKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "fact" | "facts" | "факт" => MemoryKind::Fact,
            "lesson" | "lessons" | "урок" => MemoryKind::Lesson,
            "playbook" | "playbooks" | "плейбук" => MemoryKind::Playbook,
            _ => return None,
        })
    }
    pub(crate) fn table(self) -> &'static str {
        match self {
            MemoryKind::Fact => "facts",
            MemoryKind::Lesson => "lessons",
            MemoryKind::Playbook => "playbooks",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryRef {
    pub kind: MemoryKind,
    pub id: i64,
}

/// Что произошло при записи с дедупликацией.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveStatus {
    /// Новая запись.
    Inserted,
    /// Слито с существующей (`seen += 1`, обновлены поля).
    Merged,
    /// Плейбук: шаги изменились, версия увеличена.
    NewVersion,
    /// Плейбук: то же самое, ничего не изменилось.
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub id: i64,
    pub status: SaveStatus,
}

impl Saved {
    pub fn is_new(&self) -> bool {
        self.status == SaveStatus::Inserted
    }
}

/// Урок, найденный по тексту ошибки. `exact = true` — совпали и имена в кавычках, рецепт можно
/// применять сразу; `false` — та же ошибка, но с другим объектом (подсказка для LLM).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LessonMatch {
    pub lesson: Lesson,
    pub exact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MemoryStats {
    pub facts: i64,
    pub lessons: i64,
    pub playbooks: i64,
    pub actions: i64,
    pub sessions: i64,
    pub messages: i64,
}
