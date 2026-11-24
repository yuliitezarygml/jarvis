//! Внешние зависимости агента. Реализации подставляет интегратор
//! (голос/HUD — `Confirmer` и `SpeechSink`, `jarvis-memory` — `MemoryHooks`, `jarvis-sys` — `Redactor`).

use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{RiskTier, Situation, Vars};
use serde_json::Value;

use crate::transcript::Transcript;

/// Подтверждение опасного действия (голос «да, сэр» / кнопка в HUD).
///
/// Реализация сама показывает `Event::ConfirmRequest`/`ConfirmResolved`, держит таймаут (план: 20 с → отказ)
/// и принимает «да» только с микрофона после окончания TTS. Агент вызывает `confirm` после того,
/// как отправил `Event::Action{AwaitingConfirm}` и сбросил речь (`SpeechSink::flush`).
#[async_trait]
pub trait Confirmer: Send + Sync {
    /// `protected_path` — действие затрагивает защищённый путь (`C:\Windows`, `Program Files`):
    /// нужно повторное/усиленное подтверждение. `true` — пользователь согласился.
    async fn confirm(&self, summary: &str, tier: RiskTier, protected_path: bool) -> bool;
}

/// Всегда один и тот же ответ. `FixedConfirmer(false)` — безопасное значение по умолчанию (fail-closed).
#[derive(Debug, Clone, Copy, Default)]
pub struct FixedConfirmer(pub bool);

#[async_trait]
impl Confirmer for FixedConfirmer {
    async fn confirm(&self, _summary: &str, _tier: RiskTier, _protected_path: bool) -> bool {
        self.0
    }
}

/// Долговременная память (`jarvis-memory`). Все методы должны быть быстрыми: тяжёлую работу
/// (рефлектор) реализация уводит в фон.
#[async_trait]
pub trait MemoryHooks: Send + Sync {
    /// Текст блока «Память» системного промпта: факты, уроки, плейбуки (≤1500 токенов).
    /// `tool_hint` — последний использованный инструмент (для уроков «в прошлый раз…»), `None` в начале задачи.
    async fn context(&self, query: &str, tool_hint: Option<&str>) -> String;
    /// Известное решение для текста ошибки (по сигнатуре ошибки).
    async fn known_fix(&self, error_text: &str) -> Option<String>;
    /// Ошибка/таймаут инструмента — заготовка урока.
    async fn on_tool_error(&self, tool: &str, args: &Value, error: &str);
    /// Задача завершена (любым образом) — протокол для рефлектора.
    async fn on_task_done(&self, transcript: &Transcript);
}

/// Память выключена.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopMemory;

#[async_trait]
impl MemoryHooks for NoopMemory {
    async fn context(&self, _query: &str, _tool_hint: Option<&str>) -> String {
        String::new()
    }
    async fn known_fix(&self, _error_text: &str) -> Option<String> {
        None
    }
    async fn on_tool_error(&self, _tool: &str, _args: &Value, _error: &str) {}
    async fn on_task_done(&self, _transcript: &Transcript) {}
}

/// Куда идёт текст ответа для озвучки (нормализатор речи → TTS).
pub trait SpeechSink: Send + Sync {
    /// Очередная дельта текста ответа модели.
    fn delta(&self, text: &str);
    /// Сообщение модели закончилось: озвучить накопленный хвост фразы.
    fn flush(&self);
    /// Задача отменена: выбросить неозвученный буфер. По умолчанию ничего не делает.
    fn cancel(&self) {}
}

/// Без озвучки.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullSpeech;

impl SpeechSink for NullSpeech {
    fn delta(&self, _text: &str) {}
    fn flush(&self) {}
}

/// Готовые фразы агента вне модели («Остановлено», «Готово», ошибка модели, лимит шагов) — по характеру.
/// Интегратор берёт их из банка реплик (`jarvis_core::persona::Replies`) с обращением и «зеркалом» мата.
/// `None` — фразы нет: агент говорит свой текст по умолчанию (дворецкий).
pub trait Phrases: Send + Sync {
    /// `vars` — подстановки ситуации (`err`, `n`); обращение и разрешение мата интегратор добавляет сам.
    fn phrase(&self, situation: Situation, vars: &Vars) -> Option<String>;
}

/// Вырезание секретов из недоверенного текста перед отправкой в облачную модель
/// (интегратор подставит `jarvis-sys::redact_secrets`).
pub type Redactor = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Затрагивает ли вызов `(имя инструмента, аргументы)` защищённый путь.
pub type ProtectedPathCheck = Arc<dyn Fn(&str, &Value) -> bool + Send + Sync>;

/// Тождественный редактор (по умолчанию).
pub fn identity_redactor() -> Redactor {
    Arc::new(|s: &str| s.to_string())
}

/// Ничего не защищено (по умолчанию).
pub fn no_protected_paths() -> ProtectedPathCheck {
    Arc::new(|_: &str, _: &Value| false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn defaults_are_safe() {
        assert!(!FixedConfirmer::default().confirm("x", RiskTier::T3, false).await);
        assert!(FixedConfirmer(true).confirm("x", RiskTier::T3, true).await);
        assert_eq!(NoopMemory.context("q", None).await, "");
        assert_eq!(NoopMemory.known_fix("e").await, None);
        assert_eq!(identity_redactor()("gsk_x"), "gsk_x");
        assert!(!no_protected_paths()("fs_delete", &serde_json::json!({"path": "C:\\Windows"})));
    }
}
