//! Память и самообучение J.A.R.V.I.S. (план, раздел 6; сигнатуры ошибок и привычки — 3б).
//!
//! * `data/memory.db` — SQLite (WAL) + FTS5 по стеммированным колонкам (русский + английский).
//! * Слои: `sessions`/`messages` (эпизоды), `facts`, `lessons` (память ошибок), `playbooks`, `actions` (аудит).
//! * [`Memory::retrieve_context`] → [`MemoryContext::render`] — блок «Память:» для системного промпта.
//! * [`error_signature`] + [`Memory::lesson_for_signature`] — повторная ошибка решается без LLM.
//! * [`Reflector`] — разбор задачи моделью роли `reflection` → факты/уроки/плейбуки с дедупликацией.
//! * [`memory_tools`] — `memory_remember`, `memory_recall`, `memory_forget`.
//!
//! Это накопление опыта в памяти, а не дообучение весов модели.

mod async_api;
mod consolidate;
mod db;
mod error;
mod facts;
mod journal;
mod lessons;
mod model;
mod playbooks;
mod reflect;
mod retrieve;
mod schema;
mod signature;
mod stubs;
pub mod text;
mod tools;

#[cfg(test)]
mod tests;

pub use consolidate::{ConsolidateReport, DECAY_AFTER_DAYS, DECAY_FACTOR, FORGET_BELOW};
pub use db::{DB_FILE, Memory};
pub use error::{MemoryError, Result};
pub use facts::FACT_DUP_THRESHOLD;
pub use lessons::{LESSON_DUP_THRESHOLD, REFINE_WINDOW_SECS};
pub use model::{
    Action, Fact, FactSource, Lesson, LessonMatch, LessonScope, MemoryKind, MemoryRef, MemoryStats, Message, NewAction,
    NewFact, NewLesson, NewPlaybook, Playbook, PlaybookStep, SaveStatus, Saved, Session, Timestamp, category,
};
pub use playbooks::{PLAYBOOK_DUP_THRESHOLD, is_tool_name};
pub use reflect::{
    ApplyReport, REFLECT_SYSTEM_PROMPT, ReflectError, ReflectionResult, Reflector, TaskTranscript, TranscriptStep,
    extract_json, parse_reflection,
};
pub use retrieve::{DEFAULT_BUDGET_TOKENS, MemoryContext, STYLE_FACTS_K, STYLE_HEADING, estimate_tokens};
pub use schema::SCHEMA_VERSION;
pub use signature::{ErrorSig, error_signature, normalize_error};
pub use stubs::{is_correction, lesson_stub_from_correction, lesson_stub_from_tool_error, lesson_user_habit};
pub use tools::{ForgetTool, RecallTool, RememberTool, memory_tools};
