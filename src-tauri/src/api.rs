//! Типы ответов Tauri-команд для экранов настроек, памяти, базы знаний и журнала (зеркало `src/lib/types.ts`).
//! Раньше их заполняли демо-заглушки; теперь — настоящие модули (`runtime::services`), поле `demo` всегда `false`
//! и оставлено ради совместимости с фронтендом (плашка «демо-данные» больше не показывается).

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
    pub docs_url: String,
    pub needs_key: bool,
    /// Локальный сервер (Ollama, LM Studio).
    pub local: bool,
    /// Можно начать бесплатно.
    pub free_tier: bool,
    /// Короткое описание для карточки провайдера.
    pub tagline: String,
    /// Технические заметки (только в свёрнутом «Подробнее для опытных»).
    pub note: String,
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
    /// Роли, для которых модель на первом месте: `brain`, `fast`, `vision`, `reflection`.
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
    /// Человеческий текст: «Ключ рабочий · моделей: 12 · проверено вживую: 6».
    pub message: String,
    pub models: Vec<ModelView>,
    pub ranking: Vec<RoleRanking>,
}

// ───────────────────────────── серверы и поиск ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceCheck {
    pub demo: bool,
    pub ok: bool,
    pub message: String,
    pub latency_ms: Option<u32>,
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

// ───────────────────────────── память ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Fact {
    pub id: String,
    pub text: String,
    pub confidence: f32,
    pub source: String,
    pub updated: String,
    /// Категория памяти: preference|personal|alias|habit|contact|general|style (пусто = не менять при правке).
    pub category: String,
    /// Всегда учитывать (важный факт).
    pub important: bool,
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
    /// Шаги строками: `open_target {"name":"блокнот"} — примечание`.
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

// ───────────────────────────── база знаний и история ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KbHit {
    pub title: String,
    /// Источник и раздел: «windows-commands › Examples».
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryItem {
    pub id: String,
    /// ISO 8601 (UTC).
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
