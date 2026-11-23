use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::tool::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Картинка для vision-модели (скриншот), PNG или JPEG в base64.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    pub mime: String,
    pub base64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Аргументы уже распарсенные из JSON-строки провайдера.
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Для role=Tool: id вызова, на который это ответ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self::plain(Role::System, text)
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self::plain(Role::User, text)
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::plain(Role::Assistant, text)
    }
    pub fn tool_result(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self { tool_call_id: Some(call_id.into()), ..Self::plain(Role::Tool, text) }
    }
    fn plain(role: Role, text: impl Into<String>) -> Self {
        Self { role, content: text.into(), images: vec![], tool_calls: vec![], tool_call_id: None }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    /// Итоговое сообщение ассистента (текст и/или tool_calls).
    pub message: ChatMessage,
    /// Какая модель реально ответила, «provider_id/model_id» (важно при failover).
    pub model: String,
    pub usage: Option<Usage>,
}

/// Поток ответа: дельты текста идут в субтитры и TTS сразу по мере прихода.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    TextDelta(String),
    ToolCall(ToolCall),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LlmError {
    #[error("ключ не принят провайдером")]
    Auth,
    #[error("лимит запросов{}", retry_suffix(.retry_after_ms))]
    RateLimited { retry_after_ms: Option<u64> },
    #[error("модель не найдена: {0}")]
    ModelNotFound(String),
    #[error("ошибка сервера провайдера: HTTP {0}")]
    Server(u16),
    #[error("сеть недоступна: {0}")]
    Network(String),
    #[error("таймаут")]
    Timeout,
    #[error("отменено")]
    Cancelled,
    #[error("некорректный ответ: {0}")]
    BadResponse(String),
    #[error("{0}")]
    Other(String),
}

/// « (повторю через 33 с)» — секунды с округлением вверх; без `retry_after` — пусто.
fn retry_suffix(retry_after_ms: &Option<u64>) -> String {
    match retry_after_ms {
        Some(ms) => format!(" (повторю через {} с)", ms.div_ceil(1000).max(1)),
        None => String::new(),
    }
}

impl LlmError {
    /// Можно ли переключиться на следующую модель в цепочке.
    pub fn should_failover(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimited { .. }
                | LlmError::ModelNotFound(_)
                | LlmError::Server(_)
                | LlmError::Network(_)
                | LlmError::Timeout
                | LlmError::Auth
                // Модель выдала битый ответ (например, Groq: «Failed to parse tool call arguments as JSON») —
                // другая модель цепочки обычно справляется.
                | LlmError::BadResponse(_)
        )
    }
}

/// Любая модель чата: конкретный провайдер или цепочка с failover.
#[async_trait]
pub trait ChatModel: Send + Sync {
    /// «provider_id/model_id» (для цепочки — первой модели).
    fn id(&self) -> String;

    async fn chat(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatResponse, LlmError>;

    /// Потоковый вариант: дельты в `tx`, в конце — полный ответ.
    /// Реализация по умолчанию: обычный запрос + одна дельта.
    async fn chat_stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<ChatEvent>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, LlmError> {
        let resp = self.chat(req, cancel).await?;
        if !resp.message.content.is_empty() {
            let _ = tx.send(ChatEvent::TextDelta(resp.message.content.clone())).await;
        }
        for c in &resp.message.tool_calls {
            let _ = tx.send(ChatEvent::ToolCall(c.clone())).await;
        }
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;
    #[async_trait]
    impl ChatModel for Echo {
        fn id(&self) -> String {
            "test/echo".into()
        }
        async fn chat(&self, req: ChatRequest, _c: CancellationToken) -> Result<ChatResponse, LlmError> {
            let last = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();
            Ok(ChatResponse { message: ChatMessage::assistant(last), model: self.id(), usage: None })
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_stream_forwards_text() {
        let (tx, mut rx) = mpsc::channel(4);
        let req = ChatRequest { messages: vec![ChatMessage::user("привет")], ..Default::default() };
        let r = Echo.chat_stream(req, tx, CancellationToken::new()).await.unwrap();
        assert_eq!(r.message.content, "привет");
        assert_eq!(rx.recv().await, Some(ChatEvent::TextDelta("привет".into())));
    }

    #[test]
    fn failover_policy() {
        assert!(LlmError::RateLimited { retry_after_ms: None }.should_failover());
        assert!(!LlmError::Cancelled.should_failover());
        assert!(LlmError::BadResponse("Failed to parse tool call arguments as JSON".into()).should_failover());
    }

    #[test]
    fn rate_limit_text_is_human() {
        assert_eq!(LlmError::RateLimited { retry_after_ms: Some(33_000) }.to_string(), "лимит запросов (повторю через 33 с)");
        assert_eq!(LlmError::RateLimited { retry_after_ms: Some(1_200) }.to_string(), "лимит запросов (повторю через 2 с)");
        assert_eq!(LlmError::RateLimited { retry_after_ms: Some(0) }.to_string(), "лимит запросов (повторю через 1 с)");
        assert_eq!(LlmError::RateLimited { retry_after_ms: None }.to_string(), "лимит запросов");
    }
}
