//! Реализации трейтов агента поверх модулей: подтверждения (`ConfirmGate`), память (`jarvis-memory`),
//! свечение на всю задачу (`GlowControl::hold`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use jarvis_agent::{Confirmer, MemoryHooks, StepStatus, StopReason, Transcript};
use jarvis_core::{ActionStatus, CancellationToken, Event, EventSink, RiskTier, Role, Speaker};
use jarvis_memory::{
    DEFAULT_BUDGET_TOKENS, Memory, NewAction, Reflector, TaskTranscript, TranscriptStep, lesson_stub_from_tool_error,
};
use jarvis_pc::{GlowControl, GlowHold};
use jarvis_sys::ConfirmGate;
use serde_json::Value;

use super::models::Models;
use super::speech::SpeechQueue;

// ───────────────────────────── свечение ─────────────────────────────

/// События агента → HUD. Пока идёт задача, свечение удерживается одним `GlowHold` с первого действия T1+
/// до конца задачи: `ControlStop` агента между шагами проглатывается (рамка не мигает), более опасный tier
/// перекрашивает рамку, конец задачи гасит её один раз. Вне задачи события проходят как есть.
pub struct GlowSink {
    inner: Arc<dyn EventSink>,
    glow: Arc<GlowControl>,
    active: AtomicBool,
    hold: Mutex<Option<(GlowHold, RiskTier)>>,
}

impl GlowSink {
    pub fn new(inner: Arc<dyn EventSink>, glow: Arc<GlowControl>) -> Self {
        Self { inner, glow, active: AtomicBool::new(false), hold: Mutex::new(None) }
    }

    pub fn begin_task(&self) {
        self.active.store(true, Ordering::Release);
    }

    /// Конец задачи: отпустить свечение (последний hold шлёт `ControlStop`).
    pub fn end_task(&self) {
        self.active.store(false, Ordering::Release);
        let hold = self.hold.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(hold);
    }
}

impl EventSink for GlowSink {
    fn emit(&self, event: Event) {
        if self.active.load(Ordering::Acquire) {
            match &event {
                Event::ControlStart { tier } => {
                    let mut h = self.hold.lock().unwrap_or_else(|e| e.into_inner());
                    match h.as_mut() {
                        None => *h = Some((self.glow.hold(self.inner.clone(), *tier), *tier)),
                        Some((_, cur)) if *tier > *cur => {
                            *cur = *tier;
                            self.inner.emit(event);
                        }
                        Some(_) => {}
                    }
                    return;
                }
                Event::ControlStop if self.hold.lock().unwrap_or_else(|e| e.into_inner()).is_some() => return,
                _ => {}
            }
        }
        self.inner.emit(event);
    }
}

// ───────────────────────────── подтверждения ─────────────────────────────

/// Таймаут подтверждения (план: 20 с, затем отказ).
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(20);

/// `Confirmer` агента: Джарвис озвучивает, что сделает, и ждёт «да, сэр» с микрофона или кнопку в HUD.
/// Защищённый путь (`C:\Windows`, `Program Files`…) — повторное подтверждение.
pub struct GateConfirmer {
    pub gate: ConfirmGate,
    pub speech: Arc<SpeechQueue>,
    pub events: Arc<dyn EventSink>,
    pub timeout: Duration,
}

/// Если future подтверждения бросили (отмена задачи), запрос закрывается отказом, а голосовой фильтр
/// выходит из режима «жду да».
struct PendingGuard<'a> {
    gate: &'a ConfirmGate,
    speech: &'a SpeechQueue,
    id: String,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.gate.on_click(&self.id, false);
        self.speech.set_awaiting_confirm(false);
    }
}

impl GateConfirmer {
    async fn ask(&self, summary: String, tier: RiskTier, phrase: String) -> bool {
        self.events.emit(Event::Subtitle { speaker: Speaker::Jarvis, text: phrase.clone(), is_final: true });
        self.speech.say(&phrase);
        self.speech.set_awaiting_confirm(true);
        let ticket = self.gate.request(summary, tier, self.timeout);
        let _guard = PendingGuard { gate: &self.gate, speech: &self.speech, id: ticket.id.clone() };
        ticket.await.approved
    }
}

#[async_trait]
impl Confirmer for GateConfirmer {
    async fn confirm(&self, summary: &str, tier: RiskTier, protected_path: bool) -> bool {
        let first = self
            .ask(summary.to_string(), tier, format!("Сэр, нужно ваше подтверждение: {summary}. Скажите «да, сэр» или «нет»."))
            .await;
        if !first || !protected_path {
            return first;
        }
        self.ask(
            format!("Повторно (защищённая системная папка): {summary}"),
            RiskTier::T3,
            "Сэр, это защищённая системная папка. Подтвердите ещё раз: да или нет?".into(),
        )
        .await
    }
}

// ───────────────────────────── память ─────────────────────────────

/// Подсказка из памяти для неточного совпадения: та же ошибка, но с другим объектом — применять вслепую нельзя.
pub const INEXACT_FIX_NOTE: &str = "похожая ошибка уже встречалась, но с другим объектом — проверь, подходит ли рецепт: ";

/// `MemoryHooks` агента поверх `jarvis-memory`: блок «Память», известные исправления, заготовки уроков,
/// журнал диалога/действий и рефлектор в фоне после задачи.
pub struct MemoryBridge {
    pub mem: Memory,
    pub models: Arc<Models>,
    pub session: Option<i64>,
    pub handle: tokio::runtime::Handle,
    /// Запускать рефлектор (в тестах выключен, чтобы не ходить в модель повторно).
    pub reflect: bool,
}

#[async_trait]
impl MemoryHooks for MemoryBridge {
    async fn context(&self, query: &str, tool_hint: Option<&str>) -> String {
        match self.mem.retrieve_context_async(query.to_string(), tool_hint.map(str::to_string), None).await {
            Ok(ctx) => {
                let refs = ctx.refs();
                if !refs.is_empty() {
                    let mem = self.mem.clone();
                    self.handle.spawn(async move {
                        let _ = mem.mark_used_async(refs).await;
                    });
                }
                ctx.render(DEFAULT_BUDGET_TOKENS)
            }
            Err(e) => {
                eprintln!("[jarvis] память: контекст не получен: {e}");
                String::new()
            }
        }
    }

    async fn known_fix(&self, error_text: &str) -> Option<String> {
        let m = self.mem.lesson_for_error_async(error_text.to_string()).await.ok().flatten()?;
        if !m.lesson.has_fix() {
            return None;
        }
        // Рецепт применяется сразу только при точном совпадении сигнатуры (с именами в кавычках).
        Some(if m.exact { m.lesson.fix.clone() } else { format!("{INEXACT_FIX_NOTE}{}", m.lesson.fix) })
    }

    async fn on_tool_error(&self, tool: &str, _args: &Value, error: &str) {
        if let Err(e) = self.mem.add_lesson_async(lesson_stub_from_tool_error(tool, None, error)).await {
            eprintln!("[jarvis] память: заготовка урока не записана: {e}");
        }
    }

    async fn on_task_done(&self, t: &Transcript) {
        let mem = self.mem.clone();
        let session = self.session;
        let log = t.clone();
        // Журнал диалога и аудит действий — в фоне (spawn_blocking внутри `run`).
        let _ = mem
            .run(move |m| {
                if let Some(sid) = session {
                    m.log_message(sid, Role::User, &log.user_text, None)?;
                    if !log.final_text.trim().is_empty() {
                        m.log_message(sid, Role::Assistant, &log.final_text, None)?;
                    }
                }
                for s in &log.steps {
                    m.record_action(NewAction {
                        session_id: session,
                        tool: s.tool.clone(),
                        args: s.args.clone(),
                        tier: s.tier.unwrap_or(RiskTier::T0),
                        status: action_status(s.status),
                        result_summary: s.error.clone().unwrap_or_else(|| s.result.clone()),
                        approved_by: (s.confirmed == Some(true)).then(|| "user".to_string()),
                    })?;
                }
                Ok(())
            })
            .await;

        if !self.reflect || t.stop_reason == StopReason::Cancelled || !(t.used_tools() || t.error.is_some()) {
            return;
        }
        let Some(model) = self.models.reflection_model() else { return };
        let task = to_task_transcript(t);
        let mem = self.mem.clone();
        self.handle.spawn(async move {
            let cancel = CancellationToken::new();
            let run = Reflector::new(model).reflect_and_apply(&mem, &task, cancel.clone());
            match tokio::time::timeout(Duration::from_secs(90), run).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => eprintln!("[jarvis] рефлектор: {e}"),
                Err(_) => {
                    cancel.cancel();
                    eprintln!("[jarvis] рефлектор: таймаут");
                }
            }
        });
    }
}

fn action_status(s: StepStatus) -> ActionStatus {
    match s {
        StepStatus::Done => ActionStatus::Done,
        StepStatus::Denied | StepStatus::Cancelled => ActionStatus::Cancelled,
        StepStatus::Failed | StepStatus::TimedOut | StepStatus::UnknownTool | StepStatus::LoopBlocked => ActionStatus::Failed,
    }
}

/// Протокол агента → протокол рефлектора.
pub fn to_task_transcript(t: &Transcript) -> TaskTranscript {
    TaskTranscript {
        user_request: t.user_text.clone(),
        steps: t
            .steps
            .iter()
            .map(|s| TranscriptStep {
                tool: s.tool.clone(),
                args: s.args.clone(),
                ok: s.status == StepStatus::Done,
                output: s.result.clone(),
                error: s.error.clone().or_else(|| (s.status != StepStatus::Done).then(|| format!("{:?}", s.status))),
            })
            .collect(),
        final_answer: (!t.final_text.trim().is_empty()).then(|| t.final_text.clone()),
        success: Some(t.stop_reason == StopReason::Completed && t.error.is_none()),
        user_feedback: None,
        app: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::CollectSink;

    #[test]
    fn glow_is_held_for_whole_task() {
        let sink = Arc::new(CollectSink::default());
        let glow = GlowControl::new();
        let g = GlowSink::new(sink.clone(), glow.clone());
        // вне задачи — как есть
        g.emit(Event::ControlStart { tier: RiskTier::T1 });
        g.emit(Event::ControlStop);
        assert_eq!(sink.take(), vec![Event::ControlStart { tier: RiskTier::T1 }, Event::ControlStop]);

        g.begin_task();
        g.emit(Event::State { state: jarvis_core::AssistantState::Thinking });
        g.emit(Event::ControlStart { tier: RiskTier::T1 });
        assert!(glow.is_held());
        g.emit(Event::ControlStop);
        g.emit(Event::ControlStart { tier: RiskTier::T1 });
        g.emit(Event::ControlStop);
        g.emit(Event::ControlStart { tier: RiskTier::T3 });
        g.emit(Event::ControlStop);
        g.end_task();
        assert!(!glow.is_held());
        let ev = sink.take();
        assert_eq!(
            ev,
            vec![
                Event::State { state: jarvis_core::AssistantState::Thinking },
                Event::ControlStart { tier: RiskTier::T1 },
                Event::ControlStart { tier: RiskTier::T3 },
                Event::ControlStop,
            ]
        );
    }

    #[test]
    fn transcript_conversion() {
        use jarvis_agent::TranscriptStep as AStep;
        let t = Transcript {
            user_text: "останови службу".into(),
            steps: vec![AStep {
                step: 1,
                tool: "service_stop".into(),
                args: serde_json::json!({"name": "Spooler"}),
                tier: Some(RiskTier::T3),
                confirmed: Some(false),
                duration_ms: 1,
                status: StepStatus::Denied,
                result: String::new(),
                error: None,
                known_fix: None,
            }],
            final_text: "Хорошо, сэр.".into(),
            ..Default::default()
        };
        let tt = to_task_transcript(&t);
        assert_eq!(tt.steps.len(), 1);
        assert!(!tt.steps[0].ok);
        assert_eq!(tt.final_answer.as_deref(), Some("Хорошо, сэр."));
        assert_eq!(action_status(StepStatus::Denied), ActionStatus::Cancelled);
    }
}
