use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::risk::RiskTier;

/// Состояние ассистента — по нему HUD выбирает анимацию реактора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    Controlling,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    User,
    Jarvis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSource {
    Mic,
    Tts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Running,
    Done,
    Failed,
    Cancelled,
    AwaitingConfirm,
}

/// Прямоугольник в пикселях виртуального рабочего стола.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceLink {
    pub title: String,
    pub url: String,
}

/// Событие из Rust-ядра во фронтенд (Tauri event `jarvis://event`, JSON с полем `type`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    State { state: AssistantState },
    Subtitle { speaker: Speaker, text: String, is_final: bool },
    AudioLevel { source: AudioSource, rms: f32 },
    /// Начало управления ПК: включить свечение рамки (цвет зависит от tier).
    ControlStart { tier: RiskTier },
    /// Подсветить элемент-цель: «Нажимаю: Сохранить».
    ControlTarget { rect: Rect, label: String },
    ControlStop,
    /// Строка ленты действий.
    Action { id: String, tool: String, summary: String, status: ActionStatus },
    ConfirmRequest { id: String, summary: String, tier: RiskTier, timeout_ms: u64 },
    ConfirmResolved { id: String, approved: bool },
    /// `net_kbps` — принято + отправлено всеми сетевыми адаптерами, кбит/с (`None` — ещё не измерено).
    Telemetry {
        cpu: f32,
        ram_used_mb: u64,
        ram_total_mb: u64,
        gpu: Option<f32>,
        #[serde(default)]
        net_kbps: Option<f32>,
    },
    Sources { items: Vec<SourceLink> },
    Error { message: String },
}

/// Куда модули отправляют события. Реализует Tauri-слой; в тестах — `CollectSink`.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

pub struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _event: Event) {}
}

/// Собирает события в память — для тестов.
#[derive(Default)]
pub struct CollectSink(pub Mutex<Vec<Event>>);
impl EventSink for CollectSink {
    fn emit(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}
impl CollectSink {
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_json_shape() {
        let e = Event::ControlTarget { rect: Rect { x: 1, y: 2, w: 3, h: 4 }, label: "Сохранить".into() };
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["type"], "control_target");
        assert_eq!(j["rect"]["w"], 3);
        let s = serde_json::to_string(&Event::State { state: AssistantState::Thinking }).unwrap();
        assert_eq!(s, r#"{"type":"state","state":"thinking"}"#);
    }

    #[test]
    fn collect_sink() {
        let s = CollectSink::default();
        s.emit(Event::ControlStop);
        assert_eq!(s.take(), vec![Event::ControlStop]);
        assert!(s.take().is_empty());
    }
}
