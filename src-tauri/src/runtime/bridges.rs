//! Реализации трейтов агента поверх модулей: подтверждения (`ConfirmGate`), память (`jarvis-memory`),
//! свечение на всю задачу (`GlowControl::hold`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jarvis_agent::{Confirmer, MemoryHooks, StepStatus, StopReason, Transcript};
use jarvis_core::{ActionStatus, CancellationToken, Event, EventSink, RiskTier, Role, Speaker};
use jarvis_memory::{
    DEFAULT_BUDGET_TOKENS, Memory, NewAction, Reflector, TaskTranscript, TranscriptStep, lesson_stub_from_tool_error,
};
use jarvis_pc::{GlowControl, GlowHold};
use jarvis_sys::ConfirmGate;
use serde_json::Value;

use super::models::{Models, SHARED_REFLECT_DELAY, SHARED_REFLECT_MAX_TOKENS, shared_reflection_allowed};
use super::persona::{PersonaRuntime, is_model_only_note};
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
    /// Обёртка вопроса — по характеру («Сэр, …? Скажите «да» или «нет».» / «…, точно? Да или нет?»).
    pub persona: Arc<PersonaRuntime>,
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

/// Вопрос подтверждения дворецкого (как было до характеров; в приложении обёртку даёт [`PersonaRuntime`]).
pub fn spoken_confirm(summary: &str) -> String {
    format!("Сэр, {}? Скажите «да» или «нет».", confirm_action(summary))
}

/// Что сказать голосом при подтверждении. Сырой вызов («powershell_run: command=$text=…») вслух не читаем —
/// только действие по-русски и короткое имя объекта; подробности видны в окне подтверждения.
pub fn confirm_action(summary: &str) -> String {
    let tool = summary.split(':').next().unwrap_or(summary).trim();
    let arg = |key: &str| -> Option<String> {
        let rest = summary.split_once(':')?.1;
        rest.split(", ").find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            let v = v.trim();
            (k.trim() == key
                && !v.is_empty()
                && v.chars().count() <= 30
                && v.chars().all(|c| c.is_alphanumeric() || " .-_".contains(c)))
            .then(|| v.to_string())
        })
    };
    let named = |what: &str, key: &str| match arg(key) {
        Some(n) => format!("{what} {n}"),
        None => what.to_string(),
    };
    match tool {
        "powershell_run" => "выполнить команду PowerShell".to_string(),
        "cmd_run" => "выполнить команду в командной строке".to_string(),
        "terminal_run" | "terminal_type" => "выполнить команду в терминале".to_string(),
        "service_stop" => named("остановить службу", "name"),
        "service_start" => named("запустить службу", "name"),
        "service_set_startup" => named("изменить запуск службы", "name"),
        "process_kill" => named("завершить процесс", "name"),
        "process_suspend" => named("приостановить процесс", "name"),
        "file_delete" => "удалить файл".to_string(),
        "file_move" => "переместить файл".to_string(),
        "file_write" => "записать файл".to_string(),
        "winget_install" => named("установить программу", "id"),
        "winget_uninstall" => named("удалить программу", "id"),
        "winget_upgrade" => "обновить программы".to_string(),
        "startup_disable" => named("убрать из автозагрузки", "name"),
        "power_plan_set" => "сменить план питания".to_string(),
        "temp_cleanup" => "очистить временные файлы".to_string(),
        "shell_hook_install" => "подключить терминал".to_string(),
        "undo_last" => "отменить последнее действие".to_string(),
        _ => "выполнить это действие".to_string(),
    }
}

#[async_trait]
impl Confirmer for GateConfirmer {
    async fn confirm(&self, summary: &str, tier: RiskTier, protected_path: bool) -> bool {
        let action = confirm_action(summary);
        let first = self.ask(summary.to_string(), tier, self.persona.confirm_phrase(&action)).await;
        if !first || !protected_path {
            return first;
        }
        self.ask(
            format!("Повторно (защищённая системная папка): {summary}"),
            RiskTier::T3,
            self.persona.protected_phrase(&action),
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
    /// Обычные разговоры без инструментов (копятся для рефлектора: так он узнаёт характер пользователя).
    pub chat: std::sync::Mutex<Vec<String>>,
    /// Любимые словечки из рефлексии → `config/persona/lexicon/user.json`.
    pub persona: Arc<PersonaRuntime>,
}

/// Сколько реплик разговора копить, прежде чем отдать рефлектору.
pub const CHAT_REFLECT_EVERY: usize = 5;

/// Реплика с «эмоцией» (крик, мат, посылание, повтор) — рефлектор разбирает разговор сразу.
pub fn is_emotional(notes: &[String]) -> bool {
    notes.iter().any(|n| ["голос:", "речь:", "повторяет", "настроение:"].iter().any(|k| n.contains(k)))
}

/// Строка разговора для рефлектора: реплика пользователя с пометками и ответ Джарвиса.
pub fn chat_line(t: &Transcript) -> String {
    let notes =
        t.notes.iter().filter(|n| n.starts_with('[') && !is_model_only_note(n)).cloned().collect::<Vec<_>>().join(" ");
    let user = if notes.is_empty() { t.user_text.clone() } else { format!("{} {notes}", t.user_text) };
    format!("Пользователь: {user}\nДжарвис: {}", t.final_text.trim())
}

/// Протокол «просто разговор» для рефлектора (несколько реплик сразу).
pub fn chat_transcript(lines: &[String]) -> TaskTranscript {
    TaskTranscript {
        user_request: format!("Разговор без задач (реплики подряд):\n{}", lines.join("\n")),
        steps: Vec::new(),
        final_answer: None,
        success: None,
        user_feedback: None,
        app: None,
    }
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

        if !self.reflect || t.stop_reason == StopReason::Cancelled {
            return;
        }
        let task = if t.used_tools() || t.error.is_some() {
            to_task_transcript(t)
        } else {
            // Обычный разговор: копим и отдаём пачкой (каждые 5 реплик или сразу после «эмоции»).
            let mut chat = self.chat.lock().unwrap_or_else(|e| e.into_inner());
            chat.push(chat_line(t));
            if chat.len() < CHAT_REFLECT_EVERY && !is_emotional(&t.notes) {
                return;
            }
            let lines = std::mem::take(&mut *chat);
            chat_transcript(&lines)
        };
        let Some(choice) = self.models.reflection_choice() else { return };
        let mem = self.mem.clone();
        let brain = self.models.brain.clone();
        let persona = self.persona.clone();
        let task_end = Instant::now();
        self.handle.spawn(async move {
            // Без своей цепочки рефлексия идёт в модель «мозга» и съедает тот же лимит (Groq: 8000 токенов
            // в минуту). Поэтому — позже, когда окно лимита задачи прошло, только если пользователь за это время
            // ничего не просил и 429 давно не было, и с меньшим бюджетом ответа.
            if choice.shared_with_brain {
                tokio::time::sleep(SHARED_REFLECT_DELAY).await;
                if !shared_reflection_allowed(brain.activity(), task_end, Instant::now()) {
                    eprintln!("[jarvis] рефлектор: пропуск — модель «мозга» занята или недавно упёрлась в лимит");
                    return;
                }
            }
            let cancel = CancellationToken::new();
            let mut reflector = Reflector::new(choice.model);
            if choice.shared_with_brain {
                reflector.max_tokens = SHARED_REFLECT_MAX_TOKENS;
            }
            let run = reflector.reflect_and_apply(&mem, &task, cancel.clone());
            match tokio::time::timeout(Duration::from_secs(90), run).await {
                Ok(Ok((result, _))) if !result.slang.is_empty() => {
                    // Словечки пользователя — в его словарь, Джарвис начинает их понимать сразу.
                    let slang = result.slang;
                    match tokio::task::spawn_blocking(move || persona.add_slang(&slang)).await {
                        Ok(Err(e)) => eprintln!("[jarvis] рефлектор: словечки не записаны: {e}"),
                        Err(e) => eprintln!("[jarvis] рефлектор: словечки не записаны: {e}"),
                        Ok(Ok(_)) => {}
                    }
                }
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
    // Пометки хода ([голос: …], [речь: …]) — замеры, рефлектору они полезны для фактов о характере.
    let marks: Vec<&str> =
        t.notes.iter().map(String::as_str).filter(|n| n.starts_with('[') && !is_model_only_note(n)).collect();
    TaskTranscript {
        user_request: if marks.is_empty() { t.user_text.clone() } else { format!("{} {}", t.user_text, marks.join(" ")) },
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
    fn chat_turns_go_to_reflector_with_marks() {
        let t = Transcript {
            user_text: "нахуй ты это открыл".into(),
            final_text: "Сам просил.".into(),
            notes: vec!["[голос: кричит (+14 дБ)]".into(), "цикл: служебное".into()],
            ..Default::default()
        };
        assert!(is_emotional(&t.notes));
        assert!(!is_emotional(&["цикл: служебное".to_string()]));
        let line = chat_line(&t);
        assert_eq!(line, "Пользователь: нахуй ты это открыл [голос: кричит (+14 дБ)]\nДжарвис: Сам просил.");
        let task = chat_transcript(&[line]);
        assert!(task.user_request.starts_with("Разговор без задач"));
        assert!(task.steps.is_empty() && task.success.is_none());
        // В протоколе задачи пометки тоже видны рефлектору.
        assert!(to_task_transcript(&t).user_request.ends_with("[голос: кричит (+14 дБ)]"));
    }

    #[test]
    fn model_only_notes_skip_reflector() {
        let t = Transcript {
            user_text: "пошёл нахуй".into(),
            final_text: "Сам иди.".into(),
            notes: vec![
                "[речь: мат (нахуй), посылание]".into(),
                "[можно материться: да]".into(),
                "[примеры твоих реплик в такой ситуации (это примеры манеры, не повторяй дословно): «Сам иди.»]".into(),
            ],
            ..Default::default()
        };
        assert_eq!(chat_line(&t), "Пользователь: пошёл нахуй [речь: мат (нахуй), посылание]\nДжарвис: Сам иди.");
        assert_eq!(to_task_transcript(&t).user_request, "пошёл нахуй [речь: мат (нахуй), посылание]");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn confirm_wrapper_follows_persona_and_keeps_yes_no() {
        use jarvis_core::config::{PersonaConfig, PersonaMode};
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        for mode in PersonaMode::ALL {
            let cfg = PersonaConfig { mode, ..Default::default() };
            let persona = Arc::new(PersonaRuntime::new(&cfg, Arc::new(jarvis_core::NullSink)));
            persona.load(&root.join("config/persona"), None).unwrap();
            let sink = Arc::new(CollectSink::default());
            let c = GateConfirmer {
                gate: ConfirmGate::new(sink.clone()),
                speech: SpeechQueue::new(&tokio::runtime::Handle::current()),
                events: sink.clone(),
                timeout: Duration::from_millis(50),
                persona,
            };
            // Таймаут — отказ; вопрос уже прозвучал.
            assert!(!c.confirm("service_stop: name=Spooler", RiskTier::T3, false).await);
            let asked: Vec<String> = sink
                .take()
                .into_iter()
                .filter_map(|e| match e {
                    Event::Subtitle { speaker: Speaker::Jarvis, text, .. } => Some(text),
                    _ => None,
                })
                .collect();
            assert_eq!(asked.len(), 1, "{asked:?}");
            let q = asked[0].to_lowercase();
            assert!(q.contains("службу spooler"), "{mode:?}: {q}");
            assert!(q.contains("да") && q.contains("нет"), "{mode:?}: {q}");
            if mode != PersonaMode::Butler {
                assert!(!q.contains("сэр"), "{mode:?}: {q}");
            }
        }
    }

    #[test]
    fn spoken_confirm_is_human_not_raw_call() {
        // Как в журнале: вслух звучало «пауэршелл ран, команд равно доллар текст равно…».
        let s = spoken_confirm("powershell_run: command=$text = 'Бельцы…'; Set-Content $env:USERPROFILE\\info.txt $text");
        assert_eq!(s, "Сэр, выполнить команду PowerShell? Скажите «да» или «нет».");
        assert!(!s.contains('$') && !s.contains('='));
        assert_eq!(spoken_confirm("service_stop: name=Spooler"), "Сэр, остановить службу Spooler? Скажите «да» или «нет».");
        // Длинное или странное имя не читаем.
        assert!(!spoken_confirm("process_kill: name=C:\\Windows\\x.exe").contains("Windows"));
        assert_eq!(spoken_confirm("что-то_новое"), "Сэр, выполнить это действие? Скажите «да» или «нет».");
    }

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
