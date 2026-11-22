//! Модели ролей с горячей заменой: агент, рефлектор и `web_research` держат `Arc<ModelSlot>`, а после
//! `config_set` слот получает новую цепочку из `jarvis-llm` — без пересборки агента и без потери истории диалога.

use std::path::Path;
use std::sync::{Arc, RwLock};

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

/// Модель роли, которую можно заменить на лету.
pub struct ModelSlot {
    role: ModelRole,
    state: RwLock<SlotState>,
}

impl ModelSlot {
    pub fn empty(role: ModelRole, problem: impl Into<String>) -> Arc<Self> {
        Arc::new(Self { role, state: RwLock::new(SlotState { model: None, problem: Some(problem.into()) }) })
    }

    pub fn fixed(role: ModelRole, model: Arc<dyn ChatModel>) -> Arc<Self> {
        Arc::new(Self { role, state: RwLock::new(SlotState { model: Some(model), problem: None }) })
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
    build_role_model_with(cfg, role, &opts).map_err(|e| if no_key(cfg, chain) { NO_KEY_TEXT.to_string() } else { e.to_string() })
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
        m.chat(req, cancel).await
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<ChatEvent>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, LlmError> {
        let m = self.current().ok_or_else(|| self.missing())?;
        m.chat_stream(req, tx, cancel).await
    }
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
        self.reflection.current().or_else(|| self.brain.current())
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
        let mut cfg = Config::default();
        cfg.providers = vec![ProviderConfig {
            id: "groq".into(),
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key: format!("{}{}", "gsk_", "test0000000000000000"),
            enabled: true,
            ..Default::default()
        }];
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
    fn local_provider_is_not_no_key() {
        let mut cfg = Config::default();
        cfg.providers = vec![ProviderConfig { id: "ollama".into(), base_url: "http://127.0.0.1:11434/v1".into(), enabled: true, ..Default::default() }];
        assert!(!no_key(&cfg, &["ollama/qwen3".to_string()]));
        assert!(no_key(&Config::default(), &Config::default().roles.brain));
    }
}
