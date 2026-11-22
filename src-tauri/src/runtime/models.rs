//! Модели ролей с горячей заменой: агент, рефлектор и `web_research` держат `Arc<ModelSlot>`, а после
//! `config_set` слот получает новую цепочку из `jarvis-llm` — без пересборки агента и без потери истории диалога.

use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jarvis_core::{CancellationToken, ChatEvent, ChatModel, ChatRequest, ChatResponse, Config, LlmError, ModelRef};
use jarvis_llm::{BuildOptions, CircuitBreaker, ModelRole, build_role_model_with};
use tokio::sync::mpsc;

/// Что показать пользователю, если ключа LLM нет.
pub const NO_KEY_TEXT: &str = "Нет ключа LLM — добавьте его в Настройках → «Провайдеры и модели»";

struct SlotState {
    model: Option<Arc<dyn ChatModel>>,
    /// Почему модели нет (для HUD), если `model == None`.
    problem: Option<String>,
}

/// Когда слот последний раз ходил в модель и когда получил 429 (для рефлексии, которая делит лимит с «мозгом»).
#[derive(Debug, Default, Clone, Copy)]
pub struct SlotActivity {
    pub last_call: Option<Instant>,
    pub last_rate_limit: Option<Instant>,
}

/// Модель роли, которую можно заменить на лету.
pub struct ModelSlot {
    role: ModelRole,
    state: RwLock<SlotState>,
    activity: Mutex<SlotActivity>,
}

impl ModelSlot {
    fn with_state(role: ModelRole, state: SlotState) -> Arc<Self> {
        Arc::new(Self { role, state: RwLock::new(state), activity: Mutex::default() })
    }

    pub fn empty(role: ModelRole, problem: impl Into<String>) -> Arc<Self> {
        Self::with_state(role, SlotState { model: None, problem: Some(problem.into()) })
    }

    pub fn fixed(role: ModelRole, model: Arc<dyn ChatModel>) -> Arc<Self> {
        Self::with_state(role, SlotState { model: Some(model), problem: None })
    }

    pub fn activity(&self) -> SlotActivity {
        *self.activity.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn note_call(&self) {
        self.activity.lock().unwrap_or_else(|e| e.into_inner()).last_call = Some(Instant::now());
    }

    fn note_result<T>(&self, r: &Result<T, LlmError>) {
        if let Err(LlmError::RateLimited { .. }) = r {
            self.activity.lock().unwrap_or_else(|e| e.into_inner()).last_rate_limit = Some(Instant::now());
        }
    }

    pub fn role(&self) -> ModelRole {
        self.role
    }

    pub fn current(&self) -> Option<Arc<dyn ChatModel>> {
        self.state.read().unwrap_or_else(|e| e.into_inner()).model.clone()
    }

    /// `None` — модель готова; иначе человеческое объяснение.
    pub fn problem(&self) -> Option<String> {
        let s = self.state.read().unwrap_or_else(|e| e.into_inner());
        if s.model.is_some() { None } else { s.problem.clone().or_else(|| Some("модель не настроена".into())) }
    }

    pub fn set(&self, model: Option<Arc<dyn ChatModel>>, problem: Option<String>) {
        *self.state.write().unwrap_or_else(|e| e.into_inner()) = SlotState { model, problem };
    }

    /// Пересобрать из конфига (цепочка роли с failover). Ошибка сборки не роняет приложение — слот пустеет.
    pub fn rebuild(&self, cfg: &Config, root: &Path) {
        match build_model(cfg, self.role, root) {
            Ok(m) => self.set(Some(m), None),
            Err(why) => self.set(None, Some(why)),
        }
    }

    fn missing(&self) -> LlmError {
        match self.problem() {
            Some(p) if p == NO_KEY_TEXT => LlmError::Auth,
            Some(p) => LlmError::Other(p),
            None => LlmError::Other("модель не настроена".into()),
        }
    }
}

/// Цепочка роли из `config.roles` + понятная причина, если собрать не удалось.
pub fn build_model(cfg: &Config, role: ModelRole, root: &Path) -> Result<Arc<dyn ChatModel>, String> {
    let chain = role.chain(&cfg.roles);
    if chain.is_empty() {
        return Err(if role == ModelRole::Brain {
            "Не выбрана модель «Мозг» — укажите её в Настройках → «Провайдеры и модели»".into()
        } else {
            format!("цепочка роли {role} пуста")
        });
    }
    let opts = BuildOptions { root: Some(root.to_path_buf()), breaker: CircuitBreaker::global() };
    build_role_model_with(cfg, role, &opts).map_err(|e| {
        if no_key(cfg, chain) {
            NO_KEY_TEXT.to_string()
        } else if let Some(hint) = disabled_hint(cfg, role, chain) {
            hint
        } else {
            e.to_string()
        }
    })
}

/// Цепочка не собралась, и в ней есть выключенные провайдеры — подсказать, что включить.
pub fn disabled_hint(cfg: &Config, role: ModelRole, chain: &[String]) -> Option<String> {
    let mut off: Vec<&str> = Vec::new();
    for r in chain.iter().filter_map(|r| ModelRef::parse(r)) {
        if let Some(p) = cfg.provider(&r.provider).filter(|p| !p.enabled)
            && !off.contains(&p.id.as_str())
        {
            off.push(p.id.as_str());
        }
    }
    if off.is_empty() {
        return None;
    }
    let names: Vec<String> = off
        .iter()
        .map(|id| jarvis_llm::Presets::builtin().get(id).map(|p| short_name(&p.name)).unwrap_or_else(|| id.to_string()))
        .collect();
    let list = names.join(", ");
    let what = if role == ModelRole::Brain { "Модель «Мозг» недоступна".to_string() } else { format!("Роль {role} без модели") };
    let verb = if names.len() == 1 { "выключен" } else { "выключены" };
    Some(format!("{what}: провайдер {list} {verb}. Включите {list} в Настройках → «Провайдеры и модели»."))
}

/// «Anthropic (Claude)» → «Anthropic»: для подсказки достаточно имени без пояснения в скобках.
fn short_name(name: &str) -> String {
    name.split(" (").next().unwrap_or(name).trim().to_string()
}

/// У всех провайдеров цепочки пустой ключ (самый частый случай при первом запуске).
pub fn no_key(cfg: &Config, chain: &[String]) -> bool {
    let refs: Vec<ModelRef> = chain.iter().filter_map(|r| ModelRef::parse(r)).collect();
    !refs.is_empty()
        && refs.iter().all(|r| cfg.provider(&r.provider).is_none_or(|p| p.api_key.trim().is_empty() && !is_local(&p.base_url)))
}

fn is_local(base_url: &str) -> bool {
    let u = base_url.to_ascii_lowercase();
    u.contains("://127.0.0.1") || u.contains("://localhost") || u.contains("://[::1]")
}

#[async_trait]
impl ChatModel for ModelSlot {
    fn id(&self) -> String {
        self.current().map(|m| m.id()).unwrap_or_else(|| format!("none/{}", self.role))
    }

    async fn chat(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatResponse, LlmError> {
        let m = self.current().ok_or_else(|| self.missing())?;
        self.note_call();
        let r = m.chat(req, cancel).await;
        self.note_result(&r);
        r
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<ChatEvent>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, LlmError> {
        let m = self.current().ok_or_else(|| self.missing())?;
        self.note_call();
        let r = m.chat_stream(req, tx, cancel).await;
        self.note_result(&r);
        r
    }
}

/// Своя цепочка рефлексии или «мозг» (тогда рефлексия делит с ним лимит провайдера, например 8000 TPM у Groq).
pub struct ReflectionChoice {
    pub model: Arc<dyn ChatModel>,
    /// Модель «мозга»: запускать отложенно и только если пользователь не занят.
    pub shared_with_brain: bool,
}

/// Через сколько после задачи пробовать рефлексию на модели «мозга» (окно лимита в минуту уже прошло).
pub const SHARED_REFLECT_DELAY: Duration = Duration::from_secs(65);
/// После 429 у «мозга» рефлексия на нём не запускается столько времени.
pub const SHARED_REFLECT_QUIET_AFTER_429: Duration = Duration::from_secs(180);
/// Бюджет ответа рефлексии на модели «мозга» (обычный — 1500).
pub const SHARED_REFLECT_MAX_TOKENS: u32 = 800;

/// Можно ли сейчас запустить рефлексию на модели «мозга»: с конца задачи `task_end` к «мозгу» никто не ходил
/// (пользователь не дал новую команду) и недавно не было 429.
pub fn shared_reflection_allowed(a: SlotActivity, task_end: Instant, now: Instant) -> bool {
    let idle = a.last_call.is_none_or(|t| t <= task_end);
    let quiet = a.last_rate_limit.is_none_or(|t| now.saturating_duration_since(t) >= SHARED_REFLECT_QUIET_AFTER_429);
    idle && quiet
}

/// Все роли приложения. Рефлексия без своей цепочки берёт «мозг».
pub struct Models {
    pub brain: Arc<ModelSlot>,
    pub fast: Arc<ModelSlot>,
    pub vision: Arc<ModelSlot>,
    pub reflection: Arc<ModelSlot>,
}

impl Models {
    pub fn from_config(cfg: &Config, root: &Path) -> Self {
        let m = Self {
            brain: ModelSlot::empty(ModelRole::Brain, "не собрано"),
            fast: ModelSlot::empty(ModelRole::Fast, "не собрано"),
            vision: ModelSlot::empty(ModelRole::Vision, "не собрано"),
            reflection: ModelSlot::empty(ModelRole::Reflection, "не собрано"),
        };
        m.rebuild(cfg, root);
        m
    }

    /// Подменить «мозг» готовой моделью (тесты, отладка).
    pub fn with_brain(model: Arc<dyn ChatModel>) -> Self {
        Self {
            brain: ModelSlot::fixed(ModelRole::Brain, model),
            fast: ModelSlot::empty(ModelRole::Fast, "не настроено"),
            vision: ModelSlot::empty(ModelRole::Vision, "не настроено"),
            reflection: ModelSlot::empty(ModelRole::Reflection, "не настроено"),
        }
    }

    pub fn rebuild(&self, cfg: &Config, root: &Path) {
        for slot in [&self.brain, &self.fast, &self.vision, &self.reflection] {
            slot.rebuild(cfg, root);
        }
    }

    /// Модель рефлектора: своя цепочка, иначе «мозг».
    pub fn reflection_model(&self) -> Option<Arc<dyn ChatModel>> {
        self.reflection_choice().map(|c| c.model)
    }

    /// То же + делит ли рефлексия лимит с «мозгом».
    pub fn reflection_choice(&self) -> Option<ReflectionChoice> {
        if let Some(model) = self.reflection.current() {
            return Some(ReflectionChoice { model, shared_with_brain: false });
        }
        self.brain.current().map(|model| ReflectionChoice { model, shared_with_brain: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::ProviderConfig;

    #[test]
    fn no_key_gives_friendly_problem_and_auth_error() {
        let cfg = Config::default(); // groq без ключа
        let root = std::env::temp_dir();
        let m = Models::from_config(&cfg, &root);
        assert_eq!(m.brain.problem().as_deref(), Some(NO_KEY_TEXT));
        assert!(m.brain.current().is_none());
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let err = rt.block_on(m.brain.chat(ChatRequest::default(), CancellationToken::new())).unwrap_err();
        assert_eq!(err, LlmError::Auth);
        assert!(m.reflection_model().is_none());
    }

    #[test]
    fn with_key_builds_chain_without_network() {
        let mut cfg = Config {
            providers: vec![ProviderConfig {
                id: "groq".into(),
                base_url: "https://api.groq.com/openai/v1".into(),
                api_key: format!("{}{}", "gsk_", "test0000000000000000"),
                enabled: true,
                ..Default::default()
            }],
            ..Config::default()
        };
        let m = Models::from_config(&cfg, &std::env::temp_dir());
        assert!(m.brain.problem().is_none(), "{:?}", m.brain.problem());
        assert_eq!(m.brain.id(), "groq/openai/gpt-oss-120b");
        assert!(m.reflection_model().is_some(), "рефлексия без своей цепочки берёт мозг");
        // ключ убрали — слот пустеет на лету
        cfg.providers[0].api_key.clear();
        m.rebuild(&cfg, &std::env::temp_dir());
        assert_eq!(m.brain.problem().as_deref(), Some(NO_KEY_TEXT));
    }

    #[test]
    fn disabled_provider_tells_how_to_fix() {
        let cfg = Config {
            providers: vec![ProviderConfig {
                id: "groq".into(),
                base_url: "https://api.groq.com/openai/v1".into(),
                api_key: format!("{}{}", "gsk_", "test0000000000000000"),
                enabled: false,
                ..Default::default()
            }],
            ..Config::default()
        };
        let m = Models::from_config(&cfg, &std::env::temp_dir());
        let p = m.brain.problem().unwrap();
        assert!(p.contains("Включите Groq в Настройках"), "{p}");
        assert!(disabled_hint(&Config::default(), ModelRole::Brain, &Config::default().roles.brain).is_none());
    }

    #[test]
    fn local_provider_is_not_no_key() {
        let cfg = Config {
            providers: vec![ProviderConfig { id: "ollama".into(), base_url: "http://127.0.0.1:11434/v1".into(), enabled: true, ..Default::default() }],
            ..Config::default()
        };
        assert!(!no_key(&cfg, &["ollama/qwen3".to_string()]));
        assert!(no_key(&Config::default(), &Config::default().roles.brain));
    }

    struct Limited;
    #[async_trait]
    impl ChatModel for Limited {
        fn id(&self) -> String {
            "groq/x".into()
        }
        async fn chat(&self, _r: ChatRequest, _c: CancellationToken) -> Result<ChatResponse, LlmError> {
            Err(LlmError::RateLimited { retry_after_ms: Some(30_000) })
        }
    }

    #[test]
    fn slot_tracks_calls_and_rate_limits_for_shared_reflection() {
        let slot = ModelSlot::fixed(ModelRole::Brain, Arc::new(Limited));
        assert!(slot.activity().last_call.is_none());
        let task_end = Instant::now();
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert!(rt.block_on(slot.chat(ChatRequest::default(), CancellationToken::new())).is_err());
        let a = slot.activity();
        assert!(a.last_call.is_some() && a.last_rate_limit.is_some());
        // К «мозгу» ходили после задачи и был 429 — рефлексию на нём не запускаем.
        assert!(!shared_reflection_allowed(a, task_end, Instant::now()));

        let now = Instant::now();
        let long_ago = now.checked_sub(SHARED_REFLECT_QUIET_AFTER_429 + Duration::from_secs(1));
        let Some(long_ago) = long_ago else { return }; // часы только что стартовали — пропуск
        let idle = SlotActivity { last_call: Some(long_ago), last_rate_limit: None };
        assert!(shared_reflection_allowed(idle, now, now), "простой, 429 не было");
        assert!(shared_reflection_allowed(SlotActivity::default(), now, now));
        let old_429 = SlotActivity { last_call: Some(long_ago), last_rate_limit: Some(long_ago) };
        assert!(shared_reflection_allowed(old_429, now, now), "429 был давно");
        let fresh_429 = SlotActivity { last_call: Some(long_ago), last_rate_limit: Some(now) };
        assert!(!shared_reflection_allowed(fresh_429, now, now));
        let busy = SlotActivity { last_call: Some(now), last_rate_limit: None };
        assert!(!shared_reflection_allowed(busy, long_ago, now), "пользователь дал новую команду");
    }

    #[test]
    fn reflection_choice_marks_shared_brain() {
        let m = Models::with_brain(Arc::new(Limited));
        let c = m.reflection_choice().unwrap();
        assert!(c.shared_with_brain);
        m.reflection.set(Some(Arc::new(Limited)), None);
        assert!(!m.reflection_choice().unwrap().shared_with_brain);
    }
}
