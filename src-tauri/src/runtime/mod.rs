//! Ядро-оркестратор: собирает все модули в одно приложение.
//!
//! ```text
//! микрофон ─► VoicePipeline ─► Intent ─┬─ Command ─► Core::submit ─► Agent::run ─► ToolRegistry (sys/pc/web/kb/memory)
//! поле HUD ─► agent_submit ────────────┘   Stop ─► Core::stop_all (отмена, release_all_held, барж-ин)
//!                                          Confirm/Deny ─► ConfirmGate ◄── кнопки HUD (confirm_respond → UiHooks::confirm)
//! Agent ─ SpeechSink ─► SpeechQueue ─► VoicePipeline::speak (TTS Xeon)
//! Agent ─ events ─► GlowSink (свечение на всю задачу) ─► TauriSink ─► окна hud/corner/overlay
//! Agent ─ MemoryHooks ─► jarvis-memory (контекст, известные исправления, уроки, журнал, рефлектор в фоне)
//! телеметрия (jarvis-sys) ─► TauriSink; алерты resource guard ─► SpeechQueue
//! ```
//!
//! Ядро не зависит от Tauri: получает любой `EventSink` (в приложении — `TauriSink`, в тестах — `CollectSink`).

pub mod bridges;
pub mod models;
pub mod registry;
pub mod services;
pub mod speech;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use jarvis_agent::{Agent, AgentConfig, MemoryHooks, NoopMemory, SpeechSink, TaskOutcome};
use jarvis_core::{
    AssistantState, AudioSource, CancellationToken, ChatModel, Config, Event, EventSink, Speaker, Tool,
};
use jarvis_kb::Kb;
use jarvis_llm::{Presets, ProviderReport};
use jarvis_memory::{Memory, lesson_stub_from_correction};
use jarvis_pc::{PcOptions, PcToolkit};
use jarvis_sys::{ConfirmGate, GuardConfig, ResourceGuard, Shell, Sys, SysConfig, classify_command};
use jarvis_voice::{Intent, ListenHandle, VoiceConfig, VoicePipeline};
use jarvis_web::{Fetcher, FetchError, Page, PageFetcher, SearchChain, SearchError, SearchHit, SearchProvider, SystemOpener, WebState};
use serde::Serialize;

use bridges::{CONFIRM_TIMEOUT, GateConfirmer, GlowSink, MemoryBridge};
use models::{Models, NO_KEY_TEXT};
use speech::{SpeechQueue, VoiceEvents};

/// Что включать при старте (в тестах — без голоса, телеметрии и рефлектора).
#[derive(Clone)]
pub struct CoreOptions {
    pub root: PathBuf,
    pub voice: bool,
    pub telemetry: bool,
    pub reflect: bool,
    /// Подменить «мозг» (тесты). `None` — цепочка роли brain из конфига.
    pub brain_override: Option<Arc<dyn ChatModel>>,
    /// Таймаут подтверждения T3.
    pub confirm_timeout: Duration,
    /// Другая база знаний вместо `{data_dir}/kb.sqlite` (тесты на настоящей базе без чужой памяти).
    pub kb_path: Option<PathBuf>,
}

impl CoreOptions {
    pub fn app(root: PathBuf) -> Self {
        Self { root, voice: true, telemetry: true, reflect: true, brain_override: None, confirm_timeout: CONFIRM_TIMEOUT, kb_path: None }
    }

    pub fn headless(root: PathBuf) -> Self {
        Self { voice: false, telemetry: false, reflect: false, ..Self::app(root) }
    }
}

/// Состояние микрофона для кнопки в HUD.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MicStatus {
    /// Пользователь хочет, чтобы Джарвис слушал.
    pub enabled: bool,
    /// Микрофон реально открыт.
    pub listening: bool,
    /// Голосовой конвейер собран (TTS/STT/динамики).
    pub voice_ready: bool,
    /// Почему не слушает (по-русски).
    pub error: Option<String>,
}

/// Ссылка на модуль, который не открылся, — для HUD и отчёта.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleIssue {
    pub module: &'static str,
    pub message: String,
}

struct MicState {
    wanted: bool,
    error: Option<String>,
    handle: Option<ListenHandle>,
}

pub struct Core {
    pub root: PathBuf,
    handle: tokio::runtime::Handle,
    /// Сырые события в HUD (TauriSink).
    events: Arc<dyn EventSink>,
    glow_sink: Arc<GlowSink>,
    config: RwLock<Config>,
    pub models: Arc<Models>,
    pub memory: Option<Memory>,
    pub kb: Option<Arc<Kb>>,
    pub sys: Arc<Sys>,
    pub pc: PcToolkit,
    web_search: Arc<SwapSearch>,
    web_pages: Arc<SwapPages>,
    pub agent: Arc<Agent>,
    pub gate: ConfirmGate,
    pub speech: Arc<SpeechQueue>,
    pub presets: Presets,
    pub issues: Vec<ModuleIssue>,
    pub tool_owners: std::collections::BTreeMap<String, &'static str>,
    pub tool_duplicates: Vec<(String, &'static str, &'static str)>,
    busy: Arc<AtomicBool>,
    task: Mutex<Option<(u64, CancellationToken)>>,
    task_seq: AtomicU64,
    serial: tokio::sync::Mutex<()>,
    last_action: Mutex<Option<String>>,
    mic: Mutex<MicState>,
    telemetry: Mutex<Option<CancellationToken>>,
    /// Последние отчёты «Проверить» по провайдерам (для «Выбрать лучшее автоматически»).
    pub reports: Mutex<HashMap<String, ProviderReport>>,
    opts: CoreOptions,
    me: Weak<Core>,
}

impl Core {
    /// Собрать все модули. Ничего не падает: модуль, который не открылся, попадает в `issues` и в HUD.
    pub fn start(cfg: Config, events: Arc<dyn EventSink>, opts: CoreOptions, handle: tokio::runtime::Handle) -> Arc<Self> {
        let root = opts.root.clone();
        let _rt = handle.enter();
        let mut issues = Vec::new();

        // Память и база знаний.
        let memory = match Memory::open_for_config(&root, &cfg) {
            Ok(m) => Some(m),
            Err(e) => {
                issues.push(ModuleIssue { module: "memory", message: format!("Память не открылась: {e}") });
                None
            }
        };
        let session = memory.as_ref().and_then(|m| m.start_session(Some("Сеанс J.A.R.V.I.S.")).ok());
        let kb_opened = match &opts.kb_path {
            Some(p) => Kb::open(p),
            None => Kb::open_in_data_dir(&root, &cfg),
        };
        let kb = match kb_opened {
            Ok(k) => Some(Arc::new(k)),
            Err(e) => {
                issues.push(ModuleIssue { module: "kb", message: format!("База знаний недоступна: {e}") });
                None
            }
        };

        // Система, ПК (команды терминала классифицирует политика jarvis-sys), интернет.
        let sys = Sys::new(SysConfig::from_core(&cfg, &root));
        let pc = PcToolkit::new(
            PcOptions::from_config(&cfg, &root)
                .with_classifier(Arc::new(|cmd: &str| classify_command(Shell::PowerShell, cmd).tier)),
        );
        let web_search = Arc::new(SwapSearch(RwLock::new(Arc::new(SearchChain::from_config(
            &cfg.web_search,
            &jarvis_web::http::default_client(),
        )))));
        let web_pages = Arc::new(SwapPages(RwLock::new(Arc::new(Fetcher::from_config(&cfg)))));

        // Модели ролей.
        let models = Arc::new(match &opts.brain_override {
            Some(m) => Models::with_brain(m.clone()),
            None => Models::from_config(&cfg, &root),
        });

        // Реестр инструментов.
        let brain: Arc<dyn ChatModel> = models.brain.clone();
        let mut groups: Vec<(&'static str, Vec<Arc<dyn Tool>>)> = vec![
            ("jarvis-sys", jarvis_sys::tools(sys.clone())),
            ("jarvis-pc", pc.tools()),
            (
                "jarvis-web",
                jarvis_web::web_tools(
                    web_search.clone(),
                    web_pages.clone(),
                    brain.clone(),
                    Arc::new(SystemOpener),
                    Arc::new(WebState::default()),
                ),
            ),
        ];
        if let Some(kb) = &kb {
            groups.push(("jarvis-kb", jarvis_kb::tools(kb.clone())));
        }
        if let Some(mem) = &memory {
            groups.push(("jarvis-memory", jarvis_memory::memory_tools(mem.clone())));
        }
        let built = registry::build_registry(groups);

        // Агент.
        let speech = SpeechQueue::new(&handle);
        let gate = ConfirmGate::new(events.clone());
        let glow_sink = Arc::new(GlowSink::new(events.clone(), pc.glow()));
        let memory_hooks: Arc<dyn MemoryHooks> = match &memory {
            Some(mem) => Arc::new(MemoryBridge {
                mem: mem.clone(),
                models: models.clone(),
                session,
                handle: handle.clone(),
                reflect: opts.reflect,
            }),
            None => Arc::new(NoopMemory),
        };
        let protected = {
            let sys = sys.clone();
            Arc::new(move |tool: &str, args: &serde_json::Value| sys.classify_call(tool, args).is_some_and(|c| c.protected_path))
        };
        let agent = Arc::new(
            Agent::builder(brain, Arc::new(built.registry))
                .config(AgentConfig::from_core(&cfg))
                .events(glow_sink.clone())
                .confirmer(Arc::new(GateConfirmer {
                    gate: gate.clone(),
                    speech: speech.clone(),
                    events: events.clone(),
                    timeout: opts.confirm_timeout,
                }))
                .memory(memory_hooks)
                .speech(speech.clone())
                .redactor(Arc::new(|s: &str| jarvis_sys::redact_secrets(s).0))
                .protected_paths(protected)
                .build(),
        );

        let presets = Presets::load_or_builtin(root.join("crates/jarvis-llm/presets/providers.json"));
        let core = Arc::new_cyclic(|me| Core {
            root,
            handle: handle.clone(),
            events,
            glow_sink,
            config: RwLock::new(cfg),
            models,
            memory,
            kb,
            sys,
            pc,
            web_search,
            web_pages,
            agent,
            gate,
            speech,
            presets,
            issues,
            tool_owners: built.owners,
            tool_duplicates: built.duplicates,
            busy: Arc::new(AtomicBool::new(false)),
            task: Mutex::new(None),
            task_seq: AtomicU64::new(0),
            serial: tokio::sync::Mutex::new(()),
            last_action: Mutex::new(None),
            mic: Mutex::new(MicState { wanted: true, error: None, handle: None }),
            telemetry: Mutex::new(None),
            reports: Mutex::new(HashMap::new()),
            opts,
            me: me.clone(),
        });
        if core.opts.telemetry {
            core.start_telemetry();
        }
        if core.opts.voice {
            core.init_voice();
        } else {
            core.mic.lock().unwrap().error = Some("голос выключен".into());
        }
        core
    }

    fn arc(&self) -> Arc<Core> {
        self.me.upgrade().expect("Core жив, пока жив его метод")
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn events(&self) -> Arc<dyn EventSink> {
        self.events.clone()
    }

    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    /// Сообщения о том, что не так при старте (нет ключа LLM, модуль не открылся) — показать в HUD.
    pub fn startup_problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self.issues.iter().map(|i| i.message.clone()).collect();
        if let Some(p) = self.models.brain.problem() {
            out.insert(0, p);
        }
        out
    }

    // ───────────────────────────── задачи ─────────────────────────────

    /// Новая команда (голос или поле HUD). Предыдущая задача отменяется (барж-ин), речь обрывается.
    pub fn submit(&self, text: &str, from_voice: bool) -> Result<(), String> {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("Пустая команда".into());
        }
        let (id, token) = self.begin_command();
        if !from_voice {
            self.events.emit(Event::Subtitle { speaker: Speaker::User, text: text.clone(), is_final: true });
        }
        if self.refuse_without_model() {
            return Ok(());
        }
        let core = self.arc();
        self.handle.spawn(async move {
            core.run_task(id, text, token).await;
        });
        Ok(())
    }

    /// То же, что `submit`, но дождаться итога (тесты, отладка).
    pub async fn run_command(&self, text: &str) -> Option<TaskOutcome> {
        let (id, token) = self.begin_command();
        if self.refuse_without_model() {
            return None;
        }
        self.run_task(id, text.trim().to_string(), token).await
    }

    fn begin_command(&self) -> (u64, CancellationToken) {
        let id = self.task_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let token = CancellationToken::new();
        if let Some((_, prev)) = self.task.lock().unwrap().replace((id, token.clone())) {
            prev.cancel();
        }
        self.gate.cancel_all();
        self.speech.cancel();
        (id, token)
    }

    /// Нет модели «мозг» (чаще всего — нет ключа): сказать об этом, а не падать.
    fn refuse_without_model(&self) -> bool {
        let Some(problem) = self.models.brain.problem() else { return false };
        self.events.emit(Event::Error { message: problem.clone() });
        let phrase = if problem == NO_KEY_TEXT {
            "Сэр, у меня нет ключа к языковой модели. Добавьте его в настройках, раздел «Провайдеры и модели».".to_string()
        } else {
            format!("Сэр, языковая модель недоступна: {problem}")
        };
        self.events.emit(Event::Subtitle { speaker: Speaker::Jarvis, text: phrase.clone(), is_final: true });
        self.speech.say(&phrase);
        self.events.emit(Event::State { state: AssistantState::Error });
        true
    }

    async fn run_task(&self, id: u64, text: String, token: CancellationToken) -> Option<TaskOutcome> {
        // Задачи строго по очереди: предыдущая уже отменена и завершится сама.
        let _serial = tokio::select! {
            biased;
            _ = token.cancelled() => return None,
            g = self.serial.lock() => g,
        };
        // «Нет, не так» → заготовка урока о последнем действии (рефлектор доработает).
        let last = self.last_action.lock().unwrap().clone();
        let stub = lesson_stub_from_correction(&text, last.as_deref());
        if let (Some(mem), Some(stub)) = (&self.memory, stub) {
            let _ = mem.add_lesson_async(stub).await;
        }
        self.busy.store(true, Ordering::Release);
        self.glow_sink.begin_task();
        let outcome = self.agent.run(&text, token).await;
        self.glow_sink.end_task();
        self.busy.store(false, Ordering::Release);
        *self.last_action.lock().unwrap() =
            outcome.transcript.steps.last().map(|s| format!("{} {}", s.tool, s.args));
        let mut t = self.task.lock().unwrap();
        if t.as_ref().is_some_and(|(cur, _)| *cur == id) {
            *t = None;
        }
        Some(outcome)
    }

    /// Ctrl+Alt+K / «стоп» / кнопка: отменить задачу и подтверждения, отпустить клавиши, оборвать речь.
    pub fn stop_all(&self, source: &str) {
        if let Some((_, t)) = self.task.lock().unwrap().take() {
            t.cancel();
        }
        self.gate.cancel_all();
        self.speech.cancel();
        jarvis_pc::release_all_held();
        eprintln!("[jarvis] остановка ({source})");
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    // ───────────────────────────── голос ─────────────────────────────

    fn init_voice(&self) {
        let core = self.arc();
        self.handle.spawn(async move {
            let cfg = core.config();
            let root = core.root.clone();
            let sink: Arc<dyn EventSink> = Arc::new(VoiceEvents::new(core.events.clone(), core.busy.clone()));
            let built =
                tokio::task::spawn_blocking(move || VoicePipeline::from_config(VoiceConfig::default(), &cfg, &root, sink)).await;
            match built {
                Ok(Ok(p)) => {
                    core.speech.set_voice(Some(p));
                    let wanted = core.mic.lock().unwrap().wanted;
                    if wanted {
                        core.start_listening();
                    }
                }
                Ok(Err(e)) => core.voice_failed(format!("Голос недоступен: {e}. Джарвис отвечает только текстом.")),
                Err(e) => core.voice_failed(format!("Голос недоступен: {e}")),
            }
            core.check_voice_servers().await;
        });
    }

    fn voice_failed(&self, msg: String) {
        self.mic.lock().unwrap().error = Some(msg.clone());
        self.events.emit(Event::Error { message: msg });
    }

    /// Проверить серверы TTS/STT и сказать в HUD, если недоступны (приложение работает дальше).
    async fn check_voice_servers(&self) {
        let cfg = self.config();
        let tts = services::voice_check(&cfg, &self.root, "tts", &cfg.tts.url).await;
        if !tts.ok {
            self.events.emit(Event::Error { message: format!("{} Джарвис будет отвечать только текстом.", tts.message) });
        }
        if cfg.stt.engine == "server" || cfg.stt.engine.is_empty() {
            let stt = services::voice_check(&cfg, &self.root, "stt", &cfg.stt.url).await;
            if !stt.ok {
                let msg = format!("{} Голосовые команды не работают — пишите в поле «Сказать Джарвису…».", stt.message);
                self.mic.lock().unwrap().error = Some(msg.clone());
                self.events.emit(Event::Error { message: msg });
            }
        }
    }

    fn start_listening(&self) {
        let Some(voice) = self.speech.voice() else {
            let mut m = self.mic.lock().unwrap();
            m.error.get_or_insert_with(|| "голосовой конвейер не собран".into());
            return;
        };
        let weak = self.me.clone();
        let _rt = self.handle.enter();
        match voice.start_listening(move |intent| on_intent(&weak, intent)) {
            Ok(h) => {
                let mut m = self.mic.lock().unwrap();
                m.handle = Some(h);
                m.error = None;
                drop(m);
                self.events.emit(Event::State { state: AssistantState::Idle });
            }
            Err(e) => {
                let msg = format!("Микрофон не открылся: {e}");
                self.mic.lock().unwrap().error = Some(msg.clone());
                self.events.emit(Event::Error { message: msg });
            }
        }
    }

    pub fn mic_status(&self) -> MicStatus {
        let m = self.mic.lock().unwrap();
        MicStatus {
            enabled: m.wanted,
            listening: m.handle.is_some(),
            voice_ready: self.speech.voice().is_some(),
            error: m.error.clone(),
        }
    }

    /// Включить/выключить микрофон (кнопка в HUD).
    pub fn set_mic(&self, on: bool) -> MicStatus {
        {
            let mut m = self.mic.lock().unwrap();
            m.wanted = on;
            if !on {
                if let Some(mut h) = m.handle.take() {
                    h.stop();
                }
                m.error = None;
            }
        }
        if on && self.mic.lock().unwrap().handle.is_none() {
            self.start_listening();
        }
        self.mic_status()
    }

    fn restart_voice(&self) {
        {
            let mut m = self.mic.lock().unwrap();
            if let Some(mut h) = m.handle.take() {
                h.stop();
            }
            m.error = None;
        }
        self.speech.set_voice(None);
        self.init_voice();
    }

    // ───────────────────────────── телеметрия ─────────────────────────────

    fn start_telemetry(&self) {
        let token = CancellationToken::new();
        if let Some(prev) = self.telemetry.lock().unwrap().replace(token.clone()) {
            prev.cancel();
        }
        let guard = self.config().resource_guard.then(|| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<jarvis_sys::GuardAlert>();
            let (events, speech, t) = (self.events.clone(), self.speech.clone(), token.clone());
            self.handle.spawn(async move {
                loop {
                    let alert = tokio::select! { _ = t.cancelled() => break, a = rx.recv() => a };
                    let Some(alert) = alert else { break };
                    events.emit(Event::Subtitle { speaker: Speaker::Jarvis, text: alert.message.clone(), is_final: true });
                    speech.say(&alert.message);
                }
            });
            (ResourceGuard::new(GuardConfig::default()), tx)
        });
        jarvis_sys::spawn_telemetry(self.events.clone(), Duration::from_secs(1), guard, token);
    }

    // ───────────────────────────── конфиг на лету ─────────────────────────────

    /// Применить новый конфиг без перезапуска: модели и провайдеры, автономия/зрение, поиск, голос, guard.
    /// Возвращает то, что применится только после перезапуска.
    pub fn apply_config(&self, new: &Config) -> Vec<String> {
        let old = std::mem::replace(&mut *self.config.write().unwrap_or_else(|e| e.into_inner()), new.clone());
        if self.opts.brain_override.is_none() && (old.providers != new.providers || old.roles != new.roles) {
            self.models.rebuild(new, &self.root);
            self.reports.lock().unwrap().retain(|id, _| new.provider(id).is_some());
        }
        self.agent.set_config(AgentConfig::from_core(new));
        if old.web_search != new.web_search {
            *self.web_search.0.write().unwrap() =
                Arc::new(SearchChain::from_config(&new.web_search, &jarvis_web::http::default_client()));
            *self.web_pages.0.write().unwrap() = Arc::new(Fetcher::from_config(new));
        }
        if self.opts.voice && (old.tts != new.tts || old.stt != new.stt) {
            self.restart_voice();
        }
        if self.opts.telemetry && old.resource_guard != new.resource_guard {
            self.start_telemetry();
        }
        let mut restart = Vec::new();
        if old.data_dir != new.data_dir {
            restart.push("папка данных (data_dir)".to_string());
        }
        if old.search_roots != new.search_roots {
            restart.push("папки поиска файлов".to_string());
        }
        if !restart.is_empty() {
            self.events.emit(Event::Error {
                message: format!("Сохранено. Изменения ({}) применятся после перезапуска Джарвиса.", restart.join(", ")),
            });
        }
        match self.models.brain.problem() {
            Some(p) => self.events.emit(Event::Error { message: p }),
            // ключ появился — снять красное «ошибка» с реактора
            None if !self.is_busy() => self.events.emit(Event::State { state: AssistantState::Idle }),
            None => {}
        }
        restart
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if let Some(t) = self.telemetry.lock().unwrap_or_else(|e| e.into_inner()).take() {
            t.cancel();
        }
    }
}

/// Реплика пользователя из голосового конвейера.
fn on_intent(core: &Weak<Core>, intent: Intent) {
    let Some(core) = core.upgrade() else { return };
    match intent {
        Intent::Command(text) => {
            let _ = core.submit(&text, true);
        }
        Intent::Stop => core.stop_all("voice"),
        Intent::Confirm => {
            let v = core.gate.on_transcript("да", AudioSource::Mic, core.speech.tts_finished_at());
            eprintln!("[jarvis] голосовое подтверждение: {v:?}");
        }
        Intent::Deny => {
            core.gate.on_transcript("нет", AudioSource::Mic, None);
        }
        Intent::WakeOnly | Intent::Ignored => {}
    }
}

/// Кнопки и хоткеи HUD → ядро.
pub struct CoreHooks(pub Weak<Core>);

impl crate::hooks::UiHooks for CoreHooks {
    fn emergency_stop(&self, source: &str) {
        if let Some(c) = self.0.upgrade() {
            c.stop_all(source);
        }
    }

    fn confirm(&self, id: &str, approved: bool) {
        if let Some(c) = self.0.upgrade() {
            c.gate.on_click(id, approved);
        }
    }
}

// ───────────────────────────── поиск с заменой на лету ─────────────────────────────

struct SwapSearch(RwLock<Arc<SearchChain>>);

#[async_trait]
impl SearchProvider for SwapSearch {
    fn id(&self) -> &str {
        "chain"
    }
    async fn search(&self, query: &str, n: usize) -> Result<Vec<SearchHit>, SearchError> {
        let chain = self.0.read().unwrap().clone();
        chain.search(query, n).await
    }
}

struct SwapPages(RwLock<Arc<Fetcher>>);

#[async_trait]
impl PageFetcher for SwapPages {
    async fn fetch_page(&self, url: &str) -> Result<Page, FetchError> {
        let f = self.0.read().unwrap().clone();
        f.fetch_page(url).await
    }
}

/// Корень проекта по пути к `config.json`.
pub fn root_of(config_path: &Path) -> PathBuf {
    crate::config_store::project_root(config_path)
}

#[cfg(test)]
mod tests;
