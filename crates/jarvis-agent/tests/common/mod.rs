//! Моки для тестов цикла агента.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use jarvis_agent::{Agent, AgentConfig, Confirmer, MemoryHooks, ProtectedPathCheck, Redactor, SpeechSink, Transcript};
use jarvis_core::{
    Attachment, CancellationToken, ChatEvent, ChatMessage, ChatModel, ChatRequest, ChatResponse, CollectSink, Event,
    ImageData, LlmError, RiskTier, Role, Tool, ToolCall, ToolCtx, ToolError, ToolOutput, ToolRegistry, ToolSpec,
};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

// ---------- модель ----------

#[derive(Debug, Clone)]
pub enum Reply {
    Text(String),
    /// Текст (может быть пустым) + вызовы (имя, аргументы).
    Calls(String, Vec<(String, Value)>),
    Error(LlmError),
    /// Висит до отмены.
    Hang,
}

pub fn text(s: &str) -> Reply {
    Reply::Text(s.into())
}

pub fn call(name: &str, args: Value) -> Reply {
    Reply::Calls(String::new(), vec![(name.into(), args)])
}

pub fn say_and_call(say: &str, name: &str, args: Value) -> Reply {
    Reply::Calls(say.into(), vec![(name.into(), args)])
}

type Endless = Box<dyn Fn(usize) -> Reply + Send + Sync>;

/// Модель-сценарий: отвечает по списку, записывает все запросы.
pub struct ScriptModel {
    script: Mutex<VecDeque<Reply>>,
    endless: Option<Endless>,
    counter: AtomicUsize,
    pub requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptModel {
    pub fn new(script: Vec<Reply>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            endless: None,
            counter: AtomicUsize::new(0),
            requests: Mutex::default(),
        })
    }
    pub fn endless(f: impl Fn(usize) -> Reply + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::default(),
            endless: Some(Box::new(f)),
            counter: AtomicUsize::new(0),
            requests: Mutex::default(),
        })
    }
    pub fn request(&self, i: usize) -> ChatRequest {
        self.requests.lock().unwrap()[i].clone()
    }
    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    fn next(&self) -> Reply {
        let i = self.counter.fetch_add(1, Ordering::SeqCst);
        if let Some(r) = self.script.lock().unwrap().pop_front() {
            return r;
        }
        match &self.endless {
            Some(f) => f(i),
            None => Reply::Text("сценарий закончился".into()),
        }
    }
}

fn to_message(text: &str, calls: &[(String, Value)], n: usize) -> ChatMessage {
    ChatMessage {
        tool_calls: calls
            .iter()
            .enumerate()
            .map(|(i, (name, args))| ToolCall { id: format!("c{n}_{i}"), name: name.clone(), arguments: args.clone() })
            .collect(),
        ..ChatMessage::assistant(text)
    }
}

#[async_trait]
impl ChatModel for ScriptModel {
    fn id(&self) -> String {
        "mock/script".into()
    }

    async fn chat(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatResponse, LlmError> {
        let (tx, _rx) = mpsc::channel(1024);
        self.chat_stream(req, tx, cancel).await
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<ChatEvent>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, LlmError> {
        let n = {
            let mut r = self.requests.lock().unwrap();
            r.push(req);
            r.len()
        };
        let (content, calls) = match self.next() {
            Reply::Error(e) => return Err(e),
            Reply::Hang => {
                cancel.cancelled().await;
                return Err(LlmError::Cancelled);
            }
            Reply::Text(t) => (t, vec![]),
            Reply::Calls(t, c) => (t, c),
        };
        // Стримим текст по словам, как настоящий провайдер.
        for w in content.split_inclusive(' ') {
            let _ = tx.send(ChatEvent::TextDelta(w.to_string())).await;
        }
        let message = to_message(&content, &calls, n);
        for c in &message.tool_calls {
            let _ = tx.send(ChatEvent::ToolCall(c.clone())).await;
        }
        Ok(ChatResponse { message, model: self.id(), usage: None })
    }
}

// ---------- инструменты ----------

#[derive(Clone)]
pub enum Behavior {
    Text(String),
    Untrusted(String),
    Image(String),
    Fail(String),
    Sleep(Duration),
    /// Сообщает о старте и ждёт отмены.
    WaitCancel(Arc<Notify>),
}

pub struct FakeTool {
    pub name: String,
    pub tier: RiskTier,
    pub behavior: Behavior,
    pub tier_fn: Option<fn(&Value) -> RiskTier>,
    pub calls: AtomicUsize,
    pub seen_args: Mutex<Vec<Value>>,
}

impl FakeTool {
    pub fn new(name: &str, tier: RiskTier, behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            tier,
            behavior,
            tier_fn: None,
            calls: AtomicUsize::new(0),
            seen_args: Mutex::default(),
        })
    }
    pub fn ok(name: &str, tier: RiskTier) -> Arc<Self> {
        Self::new(name, tier, Behavior::Text(format!("{name}-ok")))
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Tool for FakeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: format!("тестовый {}", self.name),
            parameters: json!({"type": "object", "properties": {}}),
            tier: self.tier,
        }
    }
    fn tier_for(&self, args: &Value) -> RiskTier {
        self.tier_fn.map(|f| f(args)).unwrap_or(self.tier)
    }
    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_args.lock().unwrap().push(args);
        match &self.behavior {
            Behavior::Text(t) => Ok(ToolOutput::text(t.clone())),
            Behavior::Untrusted(t) => Ok(ToolOutput::untrusted(t.clone())),
            Behavior::Image(t) => Ok(ToolOutput {
                content: t.clone(),
                attachments: vec![Attachment::Image(ImageData { mime: "image/png".into(), base64: "iVBORw0K".into() })],
                ..Default::default()
            }),
            Behavior::Fail(e) => Err(ToolError::Failed(e.clone())),
            Behavior::Sleep(d) => {
                tokio::time::sleep(*d).await;
                Ok(ToolOutput::text(format!("{}-slept", self.name)))
            }
            Behavior::WaitCancel(started) => {
                started.notify_one();
                ctx.cancel.cancelled().await;
                Err(ToolError::Cancelled)
            }
        }
    }
}

// ---------- хуки ----------

#[derive(Default)]
pub struct RecConfirmer {
    pub answer: bool,
    pub asked: Mutex<Vec<(String, RiskTier, bool)>>,
}

impl RecConfirmer {
    pub fn asked(&self) -> Vec<(String, RiskTier, bool)> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait]
impl Confirmer for RecConfirmer {
    async fn confirm(&self, summary: &str, tier: RiskTier, protected_path: bool) -> bool {
        self.asked.lock().unwrap().push((summary.into(), tier, protected_path));
        self.answer
    }
}

#[derive(Default)]
pub struct RecMemory {
    pub context: String,
    pub fix: Option<String>,
    pub context_calls: Mutex<Vec<(String, Option<String>)>>,
    pub errors: Mutex<Vec<(String, Value, String)>>,
    pub done: Mutex<Vec<Transcript>>,
}

#[async_trait]
impl MemoryHooks for RecMemory {
    async fn context(&self, query: &str, tool_hint: Option<&str>) -> String {
        self.context_calls.lock().unwrap().push((query.into(), tool_hint.map(String::from)));
        self.context.clone()
    }
    async fn known_fix(&self, _error_text: &str) -> Option<String> {
        self.fix.clone()
    }
    async fn on_tool_error(&self, tool: &str, args: &Value, error: &str) {
        self.errors.lock().unwrap().push((tool.into(), args.clone(), error.into()));
    }
    async fn on_task_done(&self, transcript: &Transcript) {
        self.done.lock().unwrap().push(transcript.clone());
    }
}

#[derive(Default)]
pub struct RecSpeech {
    pub text: Mutex<String>,
    pub flushes: AtomicUsize,
    pub cancels: AtomicUsize,
}

impl RecSpeech {
    pub fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }
}

impl SpeechSink for RecSpeech {
    fn delta(&self, text: &str) {
        self.text.lock().unwrap().push_str(text);
    }
    fn flush(&self) {
        self.flushes.fetch_add(1, Ordering::SeqCst);
        self.text.lock().unwrap().push('|');
    }
    fn cancel(&self) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
    }
}

// ---------- сборка ----------

#[derive(Default)]
pub struct Opts {
    pub config: AgentConfig,
    pub confirm: bool,
    pub memory: RecMemory,
    pub redactor: Option<Redactor>,
    pub protected: Option<ProtectedPathCheck>,
}

pub struct Harness {
    pub agent: Agent,
    pub model: Arc<ScriptModel>,
    pub sink: Arc<CollectSink>,
    pub speech: Arc<RecSpeech>,
    pub confirmer: Arc<RecConfirmer>,
    pub memory: Arc<RecMemory>,
}

pub fn harness(model: Arc<ScriptModel>, tools: impl IntoIterator<Item = Arc<FakeTool>>, opts: Opts) -> Harness {
    let mut reg = ToolRegistry::new();
    for t in tools {
        reg.register(t as Arc<dyn Tool>);
    }
    let sink = Arc::new(CollectSink::default());
    let speech = Arc::new(RecSpeech::default());
    let confirmer = Arc::new(RecConfirmer { answer: opts.confirm, ..Default::default() });
    let memory = Arc::new(opts.memory);
    let mut b = Agent::builder(model.clone(), Arc::new(reg))
        .config(opts.config)
        .events(sink.clone())
        .speech(speech.clone())
        .confirmer(confirmer.clone())
        .memory(memory.clone());
    if let Some(r) = opts.redactor {
        b = b.redactor(r);
    }
    if let Some(p) = opts.protected {
        b = b.protected_paths(p);
    }
    Harness { agent: b.build(), model, sink, speech, confirmer, memory }
}

pub fn default_harness(model: Arc<ScriptModel>, tools: impl IntoIterator<Item = Arc<FakeTool>>) -> Harness {
    harness(model, tools, Opts::default())
}

// ---------- проверки ----------

/// Сообщения роли Tool из запроса.
pub fn tool_messages(req: &ChatRequest) -> Vec<ChatMessage> {
    req.messages.iter().filter(|m| m.role == Role::Tool).cloned().collect()
}

pub fn all_text(req: &ChatRequest) -> String {
    req.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n")
}

pub fn states(events: &[Event]) -> Vec<jarvis_core::AssistantState> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::State { state } => Some(*state),
            _ => None,
        })
        .collect()
}

pub fn position(events: &[Event], pred: impl Fn(&Event) -> bool) -> Option<usize> {
    events.iter().position(pred)
}

pub fn action_statuses(events: &[Event]) -> Vec<(String, jarvis_core::event::ActionStatus)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Action { tool, status, .. } => Some((tool.clone(), *status)),
            _ => None,
        })
        .collect()
}
