//! Интеграционные тесты ядра без GUI: настоящие модули (sys, pc, web, kb, memory, ConfirmGate, агент),
//! вместо LLM — мок-модель со сценарием, события — в `CollectSink`.
//! Ничего не двигает мышь, не играет звук и не включает микрофон; T3-команда безвредна и не выполняется.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use jarvis_agent::{DENIED_TEXT, StepStatus, StopReason};
use jarvis_core::{
    ActionStatus, AudioSource, CancellationToken, ChatMessage, ChatModel, ChatRequest, ChatResponse, CollectSink, Config,
    Event, LlmError, RiskTier, Role, Speaker, ToolCall,
};

use super::*;
use crate::hooks::UiHooks;

/// Модель со сценарием: отдаёт заготовленные ответы по очереди и запоминает запросы.
struct Script {
    replies: Mutex<VecDeque<ChatMessage>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl Script {
    fn new(replies: Vec<ChatMessage>) -> Arc<Self> {
        Arc::new(Self { replies: Mutex::new(replies.into()), seen: Mutex::new(vec![]) })
    }
    fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChatModel for Script {
    fn id(&self) -> String {
        "mock/script".into()
    }
    async fn chat(&self, req: ChatRequest, _c: CancellationToken) -> Result<ChatResponse, LlmError> {
        self.seen.lock().unwrap().push(req);
        let msg = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| ChatMessage::assistant("Готово, сэр."));
        Ok(ChatResponse { message: msg, model: self.id(), usage: None })
    }
}

fn call(name: &str, args: serde_json::Value) -> ChatMessage {
    ChatMessage {
        tool_calls: vec![ToolCall { id: format!("call-{name}"), name: name.into(), arguments: args }],
        ..ChatMessage::assistant("")
    }
}

fn real_kb() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../data/kb.sqlite");
    p.is_file().then_some(p)
}

struct Harness {
    core: Arc<Core>,
    sink: Arc<CollectSink>,
    model: Arc<Script>,
    _dir: tempfile::TempDir,
}

fn harness(replies: Vec<ChatMessage>, kb: Option<PathBuf>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(CollectSink::default());
    let model = Script::new(replies);
    let opts = CoreOptions {
        brain_override: Some(model.clone()),
        kb_path: kb,
        confirm_timeout: Duration::from_secs(10),
        ..CoreOptions::headless(dir.path().to_path_buf())
    };
    let core = Core::start(Config::default(), sink.clone(), opts, tokio::runtime::Handle::current());
    Harness { core, sink, model, _dir: dir }
}

fn events(sink: &CollectSink) -> Vec<Event> {
    sink.0.lock().unwrap().clone()
}

/// Дождаться `ConfirmRequest` и вернуть его id.
async fn wait_confirm(sink: &CollectSink) -> String {
    for _ in 0..500 {
        if let Some(id) = events(sink).iter().find_map(|e| match e {
            Event::ConfirmRequest { id, .. } => Some(id.clone()),
            _ => None,
        }) {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("ConfirmRequest не пришёл: {:?}", events(sink));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_has_all_modules_without_duplicates() {
    let h = harness(vec![], None);
    let names: Vec<String> = h.core.agent.tools().specs().into_iter().map(|s| s.name).collect();
    assert!(h.core.tool_duplicates.is_empty(), "дубли: {:?}", h.core.tool_duplicates);
    for n in ["powershell_run", "sys_snapshot", "open_target", "mouse_click", "ui_snapshot", "terminal_context", "web_search", "web_open", "url_open", "memory_remember"] {
        assert!(names.contains(&n.to_string()), "нет {n}");
    }
    assert_eq!(h.core.tool_owners["sys_snapshot"], "jarvis-sys");
    assert_eq!(h.core.tool_owners["web_open"], "jarvis-web");
    // kb в этом тесте не открыт (временный каталог) — это видно в issues, а не паника
    assert!(h.core.issues.iter().any(|i| i.module == "kb"));
    assert!(h.core.startup_problems().iter().any(|m| m.contains("База знаний")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_command_runs_real_t0_tool_and_answers() {
    let h = harness(vec![call("sys_snapshot", serde_json::json!({})), ChatMessage::assistant("Процессор свободен, сэр.")], None);
    h.core.submit("сколько памяти занято?", false).unwrap();
    // submit работает в фоне — ждём, пока задача завершится
    let mut done = false;
    for _ in 0..1000 {
        if !h.core.is_busy() && h.model.requests().len() >= 2 && h.core.task.lock().unwrap().is_none() {
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(done, "задача не завершилась: {:?}", events(&h.sink));
    let ev = events(&h.sink);
    assert!(
        ev.contains(&Event::Subtitle { speaker: Speaker::User, text: "сколько памяти занято?".into(), is_final: true }),
        "субтитр пользователя"
    );
    let statuses: Vec<ActionStatus> = ev
        .iter()
        .filter_map(|e| match e {
            Event::Action { tool, status, .. } if tool == "sys_snapshot" => Some(*status),
            _ => None,
        })
        .collect();
    assert_eq!(statuses, [ActionStatus::Running, ActionStatus::Done], "{ev:?}");
    assert!(ev.contains(&Event::Subtitle { speaker: Speaker::Jarvis, text: "Процессор свободен, сэр.".into(), is_final: true }));
    assert_eq!(ev.last(), Some(&Event::State { state: jarvis_core::AssistantState::Idle }));
    // T0 не светится
    assert!(!ev.iter().any(|e| matches!(e, Event::ControlStart { .. })));
    // Второй запрос к модели содержит настоящий вывод sys_snapshot.
    let second = &h.model.requests()[1];
    let tool_msg = second.messages.iter().find(|m| m.role == Role::Tool).expect("ответ инструмента");
    assert!(!tool_msg.content.trim().is_empty() && !tool_msg.content.contains("Ошибка"), "{}", tool_msg.content);
    // Инструменты всех модулей ушли модели.
    assert!(second.tools.iter().any(|t| t.name == "web_search") && second.tools.iter().any(|t| t.name == "mouse_click"));
    // Журнал диалога записан в память (экран «Журнал»).
    let mem = h.core.memory.clone().unwrap();
    let mut items = vec![];
    for _ in 0..200 {
        items = services::history_list(&mem).unwrap().items;
        if items.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(items.iter().map(|i| i.speaker.as_str()).collect::<Vec<_>>(), ["user", "jarvis"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_word_alone_gets_spoken_reply() {
    let h = harness(vec![], None);
    on_intent(&h.core.me, jarvis_voice::Intent::WakeOnly);
    assert!(
        events(&h.sink).contains(&Event::Subtitle { speaker: Speaker::Jarvis, text: "Слушаю, сэр.".into(), is_final: true }),
        "{:?}",
        events(&h.sink)
    );
    // Модель не вызывается: это только отклик.
    assert!(h.model.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "замер размера запроса"]
async fn measure_request_size() {
    let h = harness(vec![ChatMessage::assistant("ok")], None);
    h.core.submit("привет", false).unwrap();
    for _ in 0..500 {
        if !h.model.requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let r = &h.model.requests()[0];
    let tools = serde_json::to_string(&r.tools).unwrap();
    let msgs: usize = r.messages.iter().map(|m| m.content.chars().count()).sum();
    let mut per: Vec<(usize, String)> = r.tools.iter().map(|t| (serde_json::to_string(t).unwrap().chars().count(), t.name.clone())).collect();
    per.sort_by(|a, b| b.0.cmp(&a.0));
    println!("TOOLS n={} chars={} | MSG chars={} max_tokens={:?}", r.tools.len(), tools.chars().count(), msgs, r.max_tokens);
    println!("TOP {:?}", &per[..12]);
    println!("ALL {}", per.iter().map(|(n, s)| format!("{s}:{n}")).collect::<Vec<_>>().join(" "));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kb_search_tool_on_real_base_finds_powercfg() {
    let Some(kb) = real_kb() else {
        eprintln!("data/kb.sqlite нет — пропуск");
        return;
    };
    let h = harness(
        vec![call("kb_search", serde_json::json!({"queries": ["powercfg power plan"]})), ChatMessage::assistant("Нашёл, сэр.")],
        Some(kb),
    );
    let out = h.core.run_command("как сменить схему питания?").await.expect("задача выполнилась");
    assert_eq!(out.stop_reason, StopReason::Completed);
    assert_eq!(out.transcript.steps[0].status, StepStatus::Done, "{:?}", out.transcript.steps[0]);
    let tool_msg = h.model.requests()[1].messages.iter().find(|m| m.role == Role::Tool).unwrap().content.clone();
    assert!(tool_msg.to_lowercase().contains("powercfg"), "{tool_msg}");
    assert!(tool_msg.contains("<untrusted"), "документация — недоверенные данные");
    // И команда экрана «База знаний».
    let r = services::kb_search(h.core.kb.as_ref().unwrap(), "powercfg").unwrap();
    assert!(!r.demo && !r.hits.is_empty());
    assert!(
        r.hits.iter().any(|x| format!("{} {} {}", x.title, x.url, x.snippet).to_lowercase().contains("powercfg")),
        "{:?}",
        r.hits
    );
}

const T3_COMMAND: &str = "Stop-Service -Name JarvisNoSuchService";

async fn t3_denied(deny: impl Fn(&Core, &str)) {
    let h = harness(
        vec![call("powershell_run", serde_json::json!({"command": T3_COMMAND})), ChatMessage::assistant("Хорошо, сэр, не буду.")],
        None,
    );
    let core = h.core.clone();
    let task = tokio::spawn(async move { core.run_command("останови службу").await });
    let id = wait_confirm(&h.sink).await;
    assert!(h.core.gate.awaiting());
    deny(&h.core, &id);
    let out = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap();

    let step = &out.transcript.steps[0];
    assert_eq!(step.tool, "powershell_run");
    assert_eq!(step.tier, Some(RiskTier::T3));
    assert_eq!(step.confirmed, Some(false));
    assert_eq!(step.status, StepStatus::Denied);
    assert_eq!(out.final_text, "Хорошо, сэр, не буду.");
    let ev = events(&h.sink);
    assert!(ev.iter().any(|e| matches!(e, Event::ConfirmRequest { tier: RiskTier::T3, timeout_ms: 10_000, .. })));
    assert!(ev.contains(&Event::ConfirmResolved { id: id.clone(), approved: false }));
    assert!(ev.iter().any(|e| matches!(e, Event::Action { tool, status: ActionStatus::Cancelled, .. } if tool == "powershell_run")));
    assert!(!ev.iter().any(|e| matches!(e, Event::Action { tool, status: ActionStatus::Running, .. } if tool == "powershell_run")), "не выполнялась");
    // Джарвис озвучил вопрос (субтитр) и модель получила отказ, а не вывод команды.
    assert!(ev.iter().any(|e| matches!(e, Event::Subtitle { speaker: Speaker::Jarvis, text, .. } if text.contains("подтверждение"))));
    let tool_msg = h.model.requests()[1].messages.iter().find(|m| m.role == Role::Tool).unwrap().content.clone();
    assert_eq!(tool_msg, DENIED_TEXT);
    assert!(!h.core.gate.awaiting());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_denied_by_hud_button() {
    // кнопка «Нет» в HUD → confirm_respond → UiHooks::confirm
    t3_denied(|core, id| CoreHooks(core.me.clone()).confirm(id, false)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_denied_by_voice() {
    // «нет» с микрофона → Intent::Deny → ConfirmGate
    t3_denied(|core, _| on_intent(&core.me, jarvis_voice::Intent::Deny)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tts_voice_cannot_confirm_and_stop_cancels() {
    let h = harness(vec![call("powershell_run", serde_json::json!({"command": T3_COMMAND}))], None);
    let core = h.core.clone();
    let task = tokio::spawn(async move { core.run_command("останови службу").await });
    let _id = wait_confirm(&h.sink).await;
    // эхо собственной речи не подтверждает
    let v = h.core.gate.on_transcript("да, сэр", AudioSource::Tts, h.core.speech.tts_finished_at());
    assert!(matches!(v, jarvis_sys::TranscriptVerdict::Ignored(_)));
    assert!(h.core.gate.awaiting());
    // «стоп» (Ctrl+Alt+K тоже сюда) — отмена задачи и подтверждения
    CoreHooks(h.core.me.clone()).emergency_stop("test");
    let out = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap();
    assert_eq!(out.stop_reason, StopReason::Cancelled);
    assert!(!h.core.gate.awaiting());
    assert_eq!(h.model.requests().len(), 1, "после отмены к модели не ходили");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_llm_key_app_works_and_explains() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(CollectSink::default());
    let core = Core::start(Config::default(), sink.clone(), CoreOptions::headless(dir.path().to_path_buf()), tokio::runtime::Handle::current());
    assert_eq!(core.startup_problems().first().map(String::as_str), Some(models::NO_KEY_TEXT));
    assert!(core.run_command("привет").await.is_none());
    core.submit("привет", false).unwrap();
    let ev = events(&sink);
    assert!(ev.contains(&Event::Error { message: models::NO_KEY_TEXT.into() }), "{ev:?}");
    assert!(ev.iter().any(|e| matches!(e, Event::Subtitle { speaker: Speaker::Jarvis, text, .. } if text.contains("нет ключа"))));
    assert!(core.submit("  ", false).is_err());

    // Ключ появился в настройках → модель собирается на лету, без перезапуска.
    let mut cfg = Config::default();
    cfg.providers[0].api_key = format!("{}{}", "gsk_", "live0000000000000000");
    let restart = core.apply_config(&cfg);
    assert!(restart.is_empty());
    assert!(core.models.brain.problem().is_none());
    // а смена папок поиска — только после перезапуска
    cfg.search_roots = vec!["C:\\".into()];
    assert_eq!(core.apply_config(&cfg).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_command_cancels_previous() {
    let h = harness(vec![call("powershell_run", serde_json::json!({"command": T3_COMMAND})), ChatMessage::assistant("Привет, сэр.")], None);
    let core = h.core.clone();
    let first = tokio::spawn(async move { core.run_command("останови службу").await });
    wait_confirm(&h.sink).await;
    let second = h.core.run_command("привет").await.expect("вторая задача");
    let first = first.await.unwrap().unwrap();
    assert_eq!(first.stop_reason, StopReason::Cancelled);
    assert_eq!(second.final_text, "Привет, сэр.");
}

/// Живая проверка серверов Xeon (только `GET /ready` и `GET /health`: звук не играет, микрофон не нужен).
/// `cargo test -p jarvis live_voice_check -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "сеть: серверы TTS/STT на Xeon"]
async fn live_voice_check_tts_and_stt() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let cfg = Config::load(root.join("config.json")).unwrap_or_default();
    let tts = services::voice_check(&cfg, &root, "tts", &cfg.tts.url).await;
    let stt = services::voice_check(&cfg, &root, "stt", &cfg.stt.url).await;
    eprintln!("TTS: {} ({:?} мс)\nSTT: {} ({:?} мс)", tts.message, tts.latency_ms, stt.message, stt.latency_ms);
    assert!(tts.ok, "{}", tts.message);
    assert!(stt.ok, "{}", stt.message);
    assert!(!tts.demo && !stt.demo);
}
