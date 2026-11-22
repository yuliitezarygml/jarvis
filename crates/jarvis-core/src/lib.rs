//! Общие контракты J.A.R.V.I.S. Все остальные крейты строятся поверх этих типов.
//! Менять этот крейт может только интегратор: модули его не редактируют.

pub mod config;
pub mod event;
pub mod llm;
pub mod risk;
pub mod stt_protocol;
pub mod tool;
pub mod untrusted;

pub use config::{Autonomy, Config, ConfigError, ModelRef, ProviderConfig, ProviderKind};
pub use event::{AssistantState, Event, EventSink, NullSink, CollectSink, Rect};
pub use llm::{ChatEvent, ChatMessage, ChatModel, ChatRequest, ChatResponse, ImageData, LlmError, Role, ToolCall, Usage};
pub use risk::{requires_confirmation, RiskTier};
pub use tool::{Attachment, Tool, ToolCtx, ToolError, ToolOutput, ToolRegistry, ToolSpec};
pub use untrusted::wrap_untrusted;

/// Токен отмены (Ctrl+Alt+K, «стоп», барж-ин) — общий для всех модулей.
pub use tokio_util::sync::CancellationToken;
