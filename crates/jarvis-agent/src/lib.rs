//! «Мозг» J.A.R.V.I.S.: цикл агента «наблюдай → действуй → проверь» (план, раздел 3).
//!
//! Модель (`ChatModel`) и инструменты (`ToolRegistry`) — из `jarvis-core`; всё остальное внешнее
//! (подтверждение голосом/HUD, долговременная память, озвучка, редактор секретов, защищённые пути)
//! подключается через трейты и типы из [`hooks`] — реализации подставляет интегратор.

pub mod agent;
pub mod config;
mod history;
pub mod hooks;
pub mod prompt;
pub mod text;
pub mod transcript;

pub use agent::{
    Agent, AgentBuilder, CANCELLED_TEXT, DENIED_TEXT, KNOWN_FIX_PREFIX, TaskOutcome, VISION_OFF_MARK,
    humanize_llm_error,
};
pub use config::AgentConfig;
pub use history::{IMAGE_TOKENS, estimate_tokens};
pub use hooks::{
    Confirmer, FixedConfirmer, MemoryHooks, NoopMemory, NullSpeech, ProtectedPathCheck, Redactor, SpeechSink,
    identity_redactor, no_protected_paths,
};
pub use prompt::{PERSONA_PROMPT, build_system_prompt};
pub use transcript::{StepStatus, StopReason, Transcript, TranscriptStep};
