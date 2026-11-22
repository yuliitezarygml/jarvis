use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::event::EventSink;
use crate::llm::ImageData;
use crate::risk::RiskTier;

/// Описание инструмента для LLM (function calling) и для политики рисков.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// snake_case, уникально: `ui_click`, `web_search`, `kb_search`…
    pub name: String,
    /// По-русски или по-английски, коротко: что делает и когда звать.
    pub description: String,
    /// JSON Schema аргументов (object).
    pub parameters: serde_json::Value,
    /// Базовый уровень риска. Инструмент может повысить его для конкретных аргументов через `Tool::tier_for`.
    pub tier: RiskTier,
}

/// Контекст вызова: отмена и канал событий для HUD (свечение, лента действий).
#[derive(Clone)]
pub struct ToolCtx {
    pub cancel: CancellationToken,
    pub events: Arc<dyn EventSink>,
}

impl ToolCtx {
    pub fn for_tests() -> Self {
        Self { cancel: CancellationToken::new(), events: Arc::new(crate::event::NullSink) }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Attachment {
    Image(ImageData),
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Текст для модели (кратко; большие данные — сокращать).
    pub content: String,
    /// Структурированные данные для UI/тестов.
    pub data: Option<serde_json::Value>,
    /// true, если в `content` чужой текст (сайт, терминал, файл, экран) — агент обернёт его в `<untrusted>`.
    pub untrusted: bool,
    pub attachments: Vec<Attachment>,
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self {
        Self { content: s.into(), ..Default::default() }
    }
    pub fn untrusted(s: impl Into<String>) -> Self {
        Self { content: s.into(), untrusted: true, ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ToolError {
    #[error("неверные аргументы: {0}")]
    InvalidArgs(String),
    #[error("запрещено политикой: {0}")]
    Denied(String),
    #[error("отменено")]
    Cancelled,
    #[error("таймаут")]
    Timeout,
    #[error("не поддерживается на этой системе: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Failed(String),
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    /// Уровень риска для конкретных аргументов (например, `powershell_run` классифицирует команду).
    /// По умолчанию — базовый из `spec()`. Неизвестное должно давать T3 (fail-closed).
    fn tier_for(&self, _args: &serde_json::Value) -> RiskTier {
        self.spec().tier
    }

    /// Выполнить. Подтверждение T3 уже получено агентом до вызова — инструмент сам не спрашивает.
    async fn call(&self, args: serde_json::Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError>;
}

/// Реестр инструментов, отсортированный по имени (стабильный порядок для промпта).
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    /// Паникует при дубликате имени — это ошибка сборки реестра, а не рантайма.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.spec().name;
        assert!(self.tools.insert(name.clone(), tool).is_none(), "дубликат инструмента {name}");
    }
    pub fn extend(&mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) {
        for t in tools {
            self.register(t);
        }
    }
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }
    pub fn len(&self) -> usize {
        self.tools.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy(&'static str);
    #[async_trait]
    impl Tool for Dummy {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.0.into(),
                description: "test".into(),
                parameters: serde_json::json!({"type":"object","properties":{}}),
                tier: RiskTier::T0,
            }
        }
        async fn call(&self, _a: serde_json::Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text("ok"))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn registry_sorted_and_callable() {
        let mut r = ToolRegistry::new();
        r.extend([Arc::new(Dummy("b")) as Arc<dyn Tool>, Arc::new(Dummy("a"))]);
        let names: Vec<_> = r.specs().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["a", "b"]);
        let out = r.get("a").unwrap().call(serde_json::json!({}), &ToolCtx::for_tests()).await.unwrap();
        assert_eq!(out.content, "ok");
    }

    #[test]
    #[should_panic]
    fn duplicate_panics() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(Dummy("x")));
        r.register(Arc::new(Dummy("x")));
    }
}
