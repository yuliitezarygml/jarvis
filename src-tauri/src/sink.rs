//! Мост событий: `jarvis_core::Event` → Tauri event `jarvis://event` (JSON с полем `type`) во все окна.
//!
//! Кроме пересылки `TauriSink` ведёт `EventCache` — снимок «что сейчас на экране» (состояние, свечение,
//! открытый запрос подтверждения, источники, телеметрия, последние действия). Окно, открытое позже
//! (настройки, перезагруженный HUD), вызывает `events_replay` и получает этот снимок как список событий.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use jarvis_core::event::{ActionStatus, SourceLink};
use jarvis_core::{AssistantState, Event, EventSink, RiskTier};
use tauri::{AppHandle, Emitter, Runtime};

/// Имя Tauri-события для всех `jarvis_core::Event`.
pub const EVENT_NAME: &str = "jarvis://event";
/// Позиция курсора для оверлея (только окну `overlay`, пока идёт управление): `{x, y}` в пикселях экрана.
pub const CURSOR_EVENT: &str = "jarvis://cursor";
/// Смена режима HUD: `"fullscreen" | "corner"`.
pub const HUD_MODE_EVENT: &str = "jarvis://hud_mode";
/// Аварийная остановка (Ctrl+Alt+K / кнопка): payload `{source}`.
pub const EMERGENCY_EVENT: &str = "jarvis://emergency_stop";
/// Открыть вкладку в окне настроек: payload — id вкладки.
pub const SETTINGS_TAB_EVENT: &str = "jarvis://settings_tab";
/// Настроение Джарвиса (`Event::PersonaMood`): payload `{mood: "calm"|"irritated"|"offended", reason}`
/// (`PERSONA_MOOD_EVENT` в `src/lib/types.ts`).
pub const PERSONA_MOOD_EVENT: &str = "persona:mood";

const MAX_ACTIONS: usize = 50;

/// Последнее известное состояние интерфейса, восстанавливаемое из потока событий.
#[derive(Debug, Default)]
pub struct EventCache {
    state: Option<AssistantState>,
    control: Option<RiskTier>,
    target: Option<Event>,
    confirm: Option<Event>,
    sources: Option<Vec<SourceLink>>,
    telemetry: Option<Event>,
    /// id → последнее событие Action (порядок — по первому появлению).
    actions: VecDeque<(String, Event)>,
}

impl EventCache {
    pub fn record(&mut self, e: &Event) {
        match e {
            Event::State { state } => self.state = Some(*state),
            Event::ControlStart { tier } => {
                self.control = Some(*tier);
                self.target = None;
            }
            Event::ControlTarget { .. } => self.target = Some(e.clone()),
            Event::ControlStop => {
                self.control = None;
                self.target = None;
            }
            Event::ConfirmRequest { .. } => self.confirm = Some(e.clone()),
            Event::ConfirmResolved { id, .. } => {
                if matches!(&self.confirm, Some(Event::ConfirmRequest { id: cur, .. }) if cur == id) {
                    self.confirm = None;
                }
            }
            Event::Sources { items } => self.sources = Some(items.clone()),
            Event::Telemetry { .. } => self.telemetry = Some(e.clone()),
            Event::Action { id, .. } => {
                if let Some(slot) = self.actions.iter_mut().find(|(k, _)| k == id) {
                    slot.1 = e.clone();
                } else {
                    self.actions.push_back((id.clone(), e.clone()));
                    while self.actions.len() > MAX_ACTIONS {
                        self.actions.pop_front();
                    }
                }
            }
            // Субтитры, уровни звука, ошибки и настроение (своё событие) в снимок не входят.
            Event::Subtitle { .. } | Event::AudioLevel { .. } | Event::Error { .. } | Event::PersonaMood { .. } => {}
        }
    }

    /// Снимок как последовательность событий: фронтенд прогоняет их через тот же редьюсер.
    pub fn replay(&self) -> Vec<Event> {
        let mut out = Vec::new();
        out.extend(self.actions.iter().map(|(_, e)| e.clone()));
        if let Some(t) = &self.telemetry {
            out.push(t.clone());
        }
        if let Some(items) = &self.sources {
            out.push(Event::Sources { items: items.clone() });
        }
        if let Some(tier) = self.control {
            out.push(Event::ControlStart { tier });
            out.extend(self.target.clone());
        }
        out.extend(self.confirm.clone());
        if let Some(state) = self.state {
            out.push(Event::State { state });
        }
        out
    }

    pub fn is_controlling(&self) -> bool {
        self.control.is_some()
    }

    /// Открытые (не завершённые) действия — их помечаем отменёнными при аварийной остановке.
    pub fn running_actions(&self) -> Vec<Event> {
        self.actions
            .iter()
            .filter_map(|(_, e)| match e {
                Event::Action { status: ActionStatus::Running | ActionStatus::AwaitingConfirm, .. } => Some(e.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn pending_confirm_id(&self) -> Option<String> {
        match &self.confirm {
            Some(Event::ConfirmRequest { id, .. }) => Some(id.clone()),
            _ => None,
        }
    }
}

/// Побочные эффекты событий в оконном слое (след курсора и т.п.). Реализует `windows_manager`.
pub trait SinkEffects<R: Runtime>: Send + Sync {
    fn on_event(&self, app: &AppHandle<R>, event: &Event);
}

/// `EventSink` для Tauri: кэширует, выполняет эффекты окон и рассылает событие во все окна.
pub struct TauriSink<R: Runtime> {
    app: AppHandle<R>,
    cache: Arc<Mutex<EventCache>>,
    effects: Vec<Arc<dyn SinkEffects<R>>>,
}

impl<R: Runtime> Clone for TauriSink<R> {
    fn clone(&self) -> Self {
        Self { app: self.app.clone(), cache: self.cache.clone(), effects: self.effects.clone() }
    }
}

impl<R: Runtime> TauriSink<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app, cache: Arc::new(Mutex::new(EventCache::default())), effects: Vec::new() }
    }

    /// Добавить обработчик побочных эффектов (вызываются по порядку добавления).
    pub fn with_effects(mut self, effects: Arc<dyn SinkEffects<R>>) -> Self {
        self.effects.push(effects);
        self
    }

    pub fn cache(&self) -> Arc<Mutex<EventCache>> {
        self.cache.clone()
    }

    pub fn replay(&self) -> Vec<Event> {
        self.cache.lock().unwrap().replay()
    }
}

impl<R: Runtime> EventSink for TauriSink<R> {
    fn emit(&self, event: Event) {
        self.cache.lock().unwrap().record(&event);
        for fx in &self.effects {
            fx.on_event(&self.app, &event);
        }
        if let Event::PersonaMood { mood, reason } = &event {
            let payload = serde_json::json!({ "mood": mood, "reason": reason });
            if let Err(e) = self.app.emit(PERSONA_MOOD_EVENT, payload) {
                eprintln!("[jarvis] не удалось отправить настроение во фронтенд: {e}");
            }
            return;
        }
        if let Err(e) = self.app.emit(EVENT_NAME, &event) {
            eprintln!("[jarvis] не удалось отправить событие во фронтенд: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::Rect;
    use jarvis_core::event::{AudioSource, Speaker};
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::Listener;

    fn all_events() -> Vec<Event> {
        vec![
            Event::State { state: AssistantState::Listening },
            Event::Subtitle { speaker: Speaker::User, text: "Джарвис, привет".into(), is_final: true },
            Event::AudioLevel { source: AudioSource::Mic, rms: 0.5 },
            Event::ControlStart { tier: RiskTier::T3 },
            Event::ControlTarget { rect: Rect { x: 10, y: 20, w: 30, h: 40 }, label: "Нажимаю: Сохранить".into() },
            Event::ControlStop,
            Event::Action { id: "a1".into(), tool: "ui_click".into(), summary: "Клик".into(), status: ActionStatus::AwaitingConfirm },
            Event::ConfirmRequest { id: "c1".into(), summary: "Остановить службу".into(), tier: RiskTier::T3, timeout_ms: 20000 },
            Event::ConfirmResolved { id: "c1".into(), approved: false },
            Event::Telemetry { cpu: 12.5, ram_used_mb: 8000, ram_total_mb: 16000, gpu: None },
            Event::Sources { items: vec![SourceLink { title: "Rust".into(), url: "https://www.rust-lang.org".into() }] },
            Event::Error { message: "сбой".into() },
        ]
    }

    #[test]
    fn json_contract_matches_frontend() {
        let j: Vec<serde_json::Value> = all_events().iter().map(|e| serde_json::to_value(e).unwrap()).collect();
        let types: Vec<&str> = j.iter().map(|v| v["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            [
                "state", "subtitle", "audio_level", "control_start", "control_target", "control_stop", "action",
                "confirm_request", "confirm_resolved", "telemetry", "sources", "error"
            ]
        );
        assert_eq!(j[0]["state"], "listening");
        assert_eq!(j[1]["speaker"], "user");
        assert_eq!(j[1]["is_final"], true);
        assert_eq!(j[2]["source"], "mic");
        assert_eq!(j[3]["tier"], "T3");
        assert_eq!(j[4]["rect"], serde_json::json!({"x":10,"y":20,"w":30,"h":40}));
        assert_eq!(j[5], serde_json::json!({"type":"control_stop"}));
        assert_eq!(j[6]["status"], "awaiting_confirm");
        assert_eq!(j[7]["timeout_ms"], 20000);
        assert!(j[9]["gpu"].is_null());
        assert_eq!(j[10]["items"][0]["url"], "https://www.rust-lang.org");
    }

    #[test]
    fn tauri_sink_emits_json_to_frontend() {
        let app = tauri::test::mock_app();
        let (tx, rx) = mpsc::channel::<String>();
        app.listen_any(EVENT_NAME, move |e| {
            tx.send(e.payload().to_string()).unwrap();
        });
        let sink = TauriSink::new(app.handle().clone());
        for e in all_events() {
            sink.emit(e);
        }
        let got: Vec<serde_json::Value> = (0..all_events().len())
            .map(|_| serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap())
            .collect();
        let want: Vec<serde_json::Value> = all_events().iter().map(|e| serde_json::to_value(e).unwrap()).collect();
        assert_eq!(got, want);
        // и обратно: фронтенд/интегратор может прислать то же JSON
        let back: Event = serde_json::from_value(got[4].clone()).unwrap();
        assert_eq!(back, all_events()[4]);
    }

    #[test]
    fn persona_mood_goes_to_its_own_event() {
        let app = tauri::test::mock_app();
        let (tx, rx) = mpsc::channel::<(String, String)>();
        let t2 = tx.clone();
        app.listen_any(PERSONA_MOOD_EVENT, move |e| {
            tx.send(("mood".into(), e.payload().to_string())).unwrap();
        });
        app.listen_any(EVENT_NAME, move |e| {
            t2.send(("event".into(), e.payload().to_string())).unwrap();
        });
        let sink = TauriSink::new(app.handle().clone());
        sink.emit(Event::PersonaMood { mood: jarvis_core::MoodLabel::Offended, reason: Some("его дважды послали".into()) });
        let (name, payload) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(name, "mood");
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v, serde_json::json!({"mood": "offended", "reason": "его дважды послали"}));
        sink.emit(Event::PersonaMood { mood: jarvis_core::MoodLabel::Calm, reason: None });
        let (_, payload) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v, serde_json::json!({"mood": "calm", "reason": null}));
        // В общий поток `jarvis://event` настроение не дублируется и в снимок не входит.
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(sink.replay().is_empty());
    }

    #[test]
    fn cache_replays_current_screen() {
        let mut c = EventCache::default();
        for e in all_events() {
            c.record(&e);
        }
        let r = c.replay();
        // control остановлен, подтверждение закрыто → их нет; состояние, телеметрия, источники, действие — есть
        assert!(!r.iter().any(|e| matches!(e, Event::ControlStart { .. } | Event::ConfirmRequest { .. })));
        assert!(r.iter().any(|e| matches!(e, Event::Telemetry { .. })));
        assert!(r.iter().any(|e| matches!(e, Event::Sources { .. })));
        assert_eq!(r.last(), Some(&Event::State { state: AssistantState::Listening }));
        assert_eq!(c.running_actions().len(), 1);

        c.record(&Event::ControlStart { tier: RiskTier::T1 });
        c.record(&Event::ControlTarget { rect: Rect::default(), label: "x".into() });
        c.record(&Event::ConfirmRequest { id: "c2".into(), summary: "s".into(), tier: RiskTier::T3, timeout_ms: 1 });
        let r = c.replay();
        assert!(r.contains(&Event::ControlStart { tier: RiskTier::T1 }));
        assert!(r.iter().any(|e| matches!(e, Event::ControlTarget { .. })));
        assert_eq!(c.pending_confirm_id().as_deref(), Some("c2"));
        // чужой id не закрывает текущий запрос
        c.record(&Event::ConfirmResolved { id: "other".into(), approved: true });
        assert_eq!(c.pending_confirm_id().as_deref(), Some("c2"));
    }

    #[test]
    fn cache_updates_action_in_place_and_caps() {
        let mut c = EventCache::default();
        let mk = |id: &str, st| Event::Action { id: id.into(), tool: "t".into(), summary: "s".into(), status: st };
        c.record(&mk("a", ActionStatus::Running));
        c.record(&mk("a", ActionStatus::Done));
        assert_eq!(c.replay(), vec![mk("a", ActionStatus::Done)]);
        for i in 0..80 {
            c.record(&mk(&format!("n{i}"), ActionStatus::Done));
        }
        assert_eq!(c.replay().len(), MAX_ACTIONS);
    }
}
