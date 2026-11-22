//! Цепочка моделей с failover и circuit breaker на провайдера; сборка модели роли из `config.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jarvis_core::{
    CancellationToken, ChatEvent, ChatModel, ChatRequest, ChatResponse, Config, LlmError, ModelRef, ProviderKind,
};
use tokio::sync::mpsc;

use crate::ModelRole;
use crate::anthropic::AnthropicClient;
use crate::discovery::{ApiRoute, ModelCache};
use crate::http;
use crate::openai::OpenAiCompatClient;
use crate::presets::{AuthStyle, ProviderSpec};

// ---------- circuit breaker ----------

#[derive(Debug, Default, Clone, Copy)]
struct BreakerState {
    failures: u32,
    open_until: Option<Instant>,
}

/// После `threshold` сбоев подряд провайдер пропускается `cooldown`; затем одна пробная попытка
/// (при новом сбое — снова открыт), успех сбрасывает счётчик.
#[derive(Debug)]
pub struct CircuitBreaker {
    threshold: u32,
    cooldown: Duration,
    state: Mutex<HashMap<String, BreakerState>>,
}

impl CircuitBreaker {
    pub const DEFAULT_THRESHOLD: u32 = 3;
    pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);

    pub fn new(threshold: u32, cooldown: Duration) -> Self {
        Self { threshold: threshold.max(1), cooldown, state: Mutex::new(HashMap::new()) }
    }

    /// Общий на процесс (3 сбоя → 60 с).
    pub fn global() -> Arc<CircuitBreaker> {
        static B: OnceLock<Arc<CircuitBreaker>> = OnceLock::new();
        B.get_or_init(|| Arc::new(CircuitBreaker::new(Self::DEFAULT_THRESHOLD, Self::DEFAULT_COOLDOWN))).clone()
    }

    /// Провайдер сейчас пропускается.
    pub fn is_open(&self, provider: &str) -> bool {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = st.get_mut(provider) else { return false };
        match s.open_until {
            Some(t) if Instant::now() < t => true,
            Some(_) => {
                // Полуоткрыт: следующая неудача сразу снова откроет.
                s.open_until = None;
                s.failures = self.threshold - 1;
                false
            }
            None => false,
        }
    }

    pub fn record_success(&self, provider: &str) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).remove(provider);
    }

    pub fn record_failure(&self, provider: &str) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let s = st.entry(provider.to_string()).or_default();
        s.failures += 1;
        if s.failures >= self.threshold {
            s.open_until = Some(Instant::now() + self.cooldown);
            tracing::warn!(provider, "circuit breaker: провайдер пропускается {:?}", self.cooldown);
        }
    }

    pub fn failures(&self, provider: &str) -> u32 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).get(provider).map(|s| s.failures).unwrap_or(0)
    }
}

// ---------- цепочка ----------

/// Событие переключения — для журнала/уроков в памяти и обновления списка моделей.
#[derive(Debug, Clone, PartialEq)]
pub struct FailoverEvent {
    /// Кто не ответил («provider/model»).
    pub model: String,
    pub error: LlmError,
    /// Кому передаём (если есть).
    pub next: Option<String>,
}

type Observer = Arc<dyn Fn(&FailoverEvent) + Send + Sync>;

/// Цепочка «provider/model» с failover: при `LlmError::should_failover` — к следующей модели.
/// Кто ответил, видно в `ChatResponse.model`.
///
/// Цепочка из одной модели переключаться некуда: при `RateLimited` с коротким `retry_after`
/// (≤ [`ModelChain::DEFAULT_RATE_LIMIT_WAIT`]) она ждёт (с отменой) и повторяет запрос один раз.
pub struct ModelChain {
    models: Vec<Arc<dyn ChatModel>>,
    breaker: Arc<CircuitBreaker>,
    observer: Option<Observer>,
    rate_limit_wait: Duration,
}

/// Итог ожидания перед повтором.
enum Wait {
    /// Повторить запрос.
    Retry,
    /// Не повторять — вернуть ошибку как есть.
    GiveUp,
    Cancelled,
}

impl ModelChain {
    /// Сколько максимум ждать `retry_after` в цепочке из одной модели.
    pub const DEFAULT_RATE_LIMIT_WAIT: Duration = Duration::from_secs(20);

    pub fn new(models: Vec<Arc<dyn ChatModel>>, breaker: Arc<CircuitBreaker>) -> Self {
        Self { models, breaker, observer: None, rate_limit_wait: Self::DEFAULT_RATE_LIMIT_WAIT }
    }

    /// Предел ожидания `retry_after` для цепочки из одной модели (`Duration::ZERO` — не ждать).
    pub fn with_rate_limit_wait(mut self, max: Duration) -> Self {
        self.rate_limit_wait = max;
        self
    }

    /// Ждать ли перед повтором: только цепочка из одной модели, только `RateLimited` с коротким `retry_after`.
    fn retry_delay(&self, err: &LlmError) -> Option<Duration> {
        if self.models.len() != 1 {
            return None;
        }
        match err {
            LlmError::RateLimited { retry_after_ms: Some(ms) } => {
                let d = Duration::from_millis(*ms);
                // Небольшой запас: окно лимита провайдера считается с точностью до секунды.
                (d <= self.rate_limit_wait && !self.rate_limit_wait.is_zero())
                    .then(|| d + Duration::from_millis(250))
            }
            _ => None,
        }
    }

    /// Вся цепочка упёрлась в лимит: какую модель ждать (освободится раньше всех) и сколько.
    /// `None` — не все ошибки «лимит» с известным `retry_after` в пределах `rate_limit_wait`.
    fn soonest_free(&self, limited: &[(Duration, usize)], attempted: usize) -> Option<(Duration, usize)> {
        if self.models.len() < 2 || self.rate_limit_wait.is_zero() || limited.len() != attempted || attempted == 0 {
            return None;
        }
        limited
            .iter()
            .copied()
            .min_by_key(|(d, _)| *d)
            .filter(|(d, _)| *d <= self.rate_limit_wait)
            .map(|(d, i)| (d + Duration::from_millis(250), i))
    }

    async fn sleep_or_cancel(d: Duration, cancel: &CancellationToken) -> bool {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => false,
            _ = tokio::time::sleep(d) => true,
        }
    }

    async fn wait_retry(&self, model: &str, err: &LlmError, cancel: &CancellationToken) -> Wait {
        let Some(d) = self.retry_delay(err) else { return Wait::GiveUp };
        tracing::info!(model, wait_ms = d.as_millis() as u64, "лимит запросов: жду и повторяю один раз");
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Wait::Cancelled,
            _ = tokio::time::sleep(d) => Wait::Retry,
        }
    }

    pub fn with_observer(mut self, f: impl Fn(&FailoverEvent) + Send + Sync + 'static) -> Self {
        self.observer = Some(Arc::new(f));
        self
    }

    pub fn models(&self) -> Vec<String> {
        self.models.iter().map(|m| m.id()).collect()
    }

    pub fn breaker(&self) -> &Arc<CircuitBreaker> {
        &self.breaker
    }

    fn provider_of(id: &str) -> String {
        id.split_once('/').map(|(p, _)| p).unwrap_or(id).to_string()
    }

    /// Порядок попыток: модели с закрытым breaker; если все открыты — всё равно пробуем по порядку.
    fn plan(&self) -> Vec<Arc<dyn ChatModel>> {
        let ready: Vec<_> =
            self.models.iter().filter(|m| !self.breaker.is_open(&Self::provider_of(&m.id()))).cloned().collect();
        if ready.is_empty() { self.models.clone() } else { ready }
    }

    /// Один потоковый запрос к модели; второе значение — успело ли уйти хоть одно событие.
    async fn stream_once(
        m: &Arc<dyn ChatModel>,
        req: &ChatRequest,
        tx: &mpsc::Sender<ChatEvent>,
        cancel: &CancellationToken,
    ) -> (Result<ChatResponse, LlmError>, bool) {
        let (itx, mut irx) = mpsc::channel::<ChatEvent>(64);
        let mut forwarded = false;
        let fut = m.chat_stream(req.clone(), itx, cancel.clone());
        let fwd = async {
            while let Some(ev) = irx.recv().await {
                forwarded = true;
                let _ = http::emit(tx, ev, cancel).await;
            }
        };
        let (res, ()) = tokio::join!(fut, fwd);
        (res, forwarded)
    }

    fn on_error(&self, model: &str, err: &LlmError, next: Option<String>) {
        let provider = Self::provider_of(model);
        // «Модель не найдена» — проблема модели, а не провайдера.
        if !matches!(err, LlmError::ModelNotFound(_)) {
            self.breaker.record_failure(&provider);
        }
        tracing::warn!(model, error = %err, next = ?next, "failover");
        if let Some(o) = &self.observer {
            o(&FailoverEvent { model: model.to_string(), error: err.clone(), next });
        }
    }
}

#[async_trait]
impl ChatModel for ModelChain {
    fn id(&self) -> String {
        self.models.first().map(|m| m.id()).unwrap_or_else(|| "chain/empty".into())
    }

    async fn chat(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatResponse, LlmError> {
        let plan = self.plan();
        let mut last = LlmError::Other("в цепочке нет моделей".into());
        let mut limited: Vec<(Duration, usize)> = Vec::new();
        for (i, m) in plan.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            let id = m.id();
            let mut res = m.chat(req.clone(), cancel.clone()).await;
            if let Err(LlmError::RateLimited { retry_after_ms: Some(ms) }) = &res {
                limited.push((Duration::from_millis(*ms), i));
            }
            if let Err(e) = &res {
                match self.wait_retry(&id, e, &cancel).await {
                    Wait::Retry => res = m.chat(req.clone(), cancel.clone()).await,
                    Wait::Cancelled => return Err(LlmError::Cancelled),
                    Wait::GiveUp => {}
                }
            }
            match res {
                Ok(r) => {
                    self.breaker.record_success(&Self::provider_of(&id));
                    return Ok(r);
                }
                Err(e) if e.should_failover() => {
                    self.on_error(&id, &e, plan.get(i + 1).map(|n| n.id()));
                    last = e;
                }
                Err(e) => return Err(e),
            }
        }
        // Все модели упёрлись в лимит, но одна скоро освободится — подождать её и повторить один раз.
        if let Some((d, i)) = self.soonest_free(&limited, plan.len()) {
            let m = &plan[i];
            tracing::info!(model = %m.id(), wait_ms = d.as_millis() as u64, "вся цепочка в лимите: жду ближайшую модель");
            if !Self::sleep_or_cancel(d, &cancel).await {
                return Err(LlmError::Cancelled);
            }
            let r = m.chat(req, cancel).await;
            if r.is_ok() {
                self.breaker.record_success(&Self::provider_of(&m.id()));
            }
            return r;
        }
        Err(last)
    }

    /// Переключение возможно только до первого события: если текст уже ушёл в субтитры/TTS, ошибка возвращается.
    async fn chat_stream(
        &self,
        req: ChatRequest,
        tx: mpsc::Sender<ChatEvent>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, LlmError> {
        let plan = self.plan();
        let mut last = LlmError::Other("в цепочке нет моделей".into());
        let mut limited: Vec<(Duration, usize)> = Vec::new();
        for (i, m) in plan.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            let id = m.id();
            let (mut res, mut forwarded) = Self::stream_once(m, &req, &tx, &cancel).await;
            if let (Err(LlmError::RateLimited { retry_after_ms: Some(ms) }), false) = (&res, forwarded) {
                limited.push((Duration::from_millis(*ms), i));
            }
            if let Err(e) = &res
                && !forwarded
            {
                match self.wait_retry(&id, e, &cancel).await {
                    Wait::Retry => (res, forwarded) = Self::stream_once(m, &req, &tx, &cancel).await,
                    Wait::Cancelled => return Err(LlmError::Cancelled),
                    Wait::GiveUp => {}
                }
            }
            match res {
                Ok(r) => {
                    self.breaker.record_success(&Self::provider_of(&id));
                    return Ok(r);
                }
                Err(e) if e.should_failover() && !forwarded => {
                    self.on_error(&id, &e, plan.get(i + 1).map(|n| n.id()));
                    last = e;
                }
                Err(e) => {
                    if e.should_failover() {
                        self.breaker.record_failure(&Self::provider_of(&id));
                    }
                    return Err(e);
                }
            }
        }
        // Все модели упёрлись в лимит, но одна скоро освободится — подождать её и повторить один раз.
        if let Some((d, i)) = self.soonest_free(&limited, plan.len()) {
            let m = &plan[i];
            tracing::info!(model = %m.id(), wait_ms = d.as_millis() as u64, "вся цепочка в лимите: жду ближайшую модель");
            if !Self::sleep_or_cancel(d, &cancel).await {
                return Err(LlmError::Cancelled);
            }
            let (r, _) = Self::stream_once(m, &req, &tx, &cancel).await;
            if r.is_ok() {
                self.breaker.record_success(&Self::provider_of(&m.id()));
            }
            return r;
        }
        Err(last)
    }
}

// ---------- сборка клиентов ----------

/// Клиент для модели провайдера. Маршрут: из списка моделей (Zen `api`), иначе из пресета, иначе тип провайдера.
pub fn client_for(spec: &ProviderSpec, model: &str, route: Option<&ApiRoute>) -> Arc<dyn ChatModel> {
    let spec = Arc::new(spec.clone());
    let preset_route = spec.route_for(model).cloned();
    let (kind, base, auth): (ProviderKind, Option<String>, Option<AuthStyle>) = match (route, preset_route) {
        (Some(r), pr) => {
            // Адрес из пресета надёжнее (если совпадает тип), иначе из списка моделей.
            let pr = pr.filter(|p| p.kind == r.kind);
            let base = r.base_url.clone().or_else(|| pr.as_ref().map(|p| p.base_url.clone()));
            let auth = pr.and_then(|p| p.auth).or_else(|| (r.kind != spec.kind).then_some(AuthStyle::Bearer));
            (r.kind, base, auth)
        }
        (None, Some(p)) => (p.kind, Some(p.base_url), p.auth),
        (None, None) => (spec.kind, None, None),
    };
    match kind {
        ProviderKind::OpenaiCompat => {
            let mut c = OpenAiCompatClient::from_arc(spec, model);
            if let Some(b) = base {
                c = c.with_base_url(b);
            }
            if let Some(a) = auth {
                c = c.with_auth(a);
            }
            Arc::new(c)
        }
        ProviderKind::Anthropic => {
            let mut c = AnthropicClient::from_arc(spec, model);
            if let Some(b) = base {
                c = c.with_base_url(b);
            }
            if let Some(a) = auth {
                c = c.with_auth(a);
            }
            Arc::new(c)
        }
    }
}

/// Настройки сборки модели роли.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Корень проекта (`d:\jarvis`) — для кэша моделей (маршруты Zen). `None` — без кэша.
    pub root: Option<PathBuf>,
    pub breaker: Arc<CircuitBreaker>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { root: default_project_root(), breaker: CircuitBreaker::global() }
    }
}

/// Корень проекта: переменная `JARVIS_ROOT`, иначе ближайший вверх от текущего каталога, где есть `config.json`
/// или `config.example.json`.
pub fn default_project_root() -> Option<PathBuf> {
    if let Some(r) = std::env::var_os("JARVIS_ROOT") {
        return Some(PathBuf::from(r));
    }
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("config.json").is_file() || dir.join("config.example.json").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Одна модель «provider/model» из конфига (провайдер должен быть включён).
pub fn model_from_ref(cfg: &Config, model_ref: &str, cache: Option<&ModelCache>) -> Result<Arc<dyn ChatModel>, LlmError> {
    let r = ModelRef::parse(model_ref).ok_or_else(|| LlmError::Other(format!("неверная ссылка на модель: {model_ref}")))?;
    let pc = cfg
        .provider(&r.provider)
        .ok_or_else(|| LlmError::Other(format!("провайдер {} не настроен", r.provider)))?;
    if !pc.enabled {
        return Err(LlmError::Other(format!("провайдер {} выключен", r.provider)));
    }
    let spec = ProviderSpec::from_config(pc);
    if spec.needs_key && spec.api_key.is_empty() {
        return Err(LlmError::Auth);
    }
    let route = cache
        .and_then(|c| c.load_any(&spec.id, &spec.base_url))
        .and_then(|(models, _)| models.into_iter().find(|m| m.id == r.model))
        .and_then(|m| m.api);
    Ok(client_for(&spec, &r.model, route.as_ref()))
}

/// Модель роли: цепочка из `config.roles.<role>` (пропуская выключенных провайдеров и без ключа).
pub fn build_role_model(cfg: &Config, role: ModelRole) -> Result<Arc<dyn ChatModel>, LlmError> {
    build_role_model_with(cfg, role, &BuildOptions::default())
}

pub fn build_role_model_with(cfg: &Config, role: ModelRole, opts: &BuildOptions) -> Result<Arc<dyn ChatModel>, LlmError> {
    let cache = opts.root.as_deref().map(|root| ModelCache::for_config(root, cfg));
    let mut models = Vec::new();
    let mut skipped = Vec::new();
    for r in role.chain(&cfg.roles) {
        match model_from_ref(cfg, r, cache.as_ref()) {
            Ok(m) => models.push(m),
            Err(e) => {
                tracing::warn!(role = %role, model = %r, "пропускаю: {e}");
                skipped.push(format!("{r}: {e}"));
            }
        }
    }
    if models.is_empty() {
        let why = if skipped.is_empty() { "цепочка пуста".to_string() } else { skipped.join("; ") };
        return Err(LlmError::Other(format!("для роли {role} нет доступных моделей ({why})")));
    }
    Ok(Arc::new(ModelChain::new(models, opts.breaker.clone())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::ProviderConfig;

    #[test]
    fn breaker_opens_and_half_opens() {
        let b = CircuitBreaker::new(3, Duration::from_millis(30));
        b.record_failure("p");
        b.record_failure("p");
        assert!(!b.is_open("p"));
        b.record_failure("p");
        assert!(b.is_open("p"));
        assert!(!b.is_open("other"));
        std::thread::sleep(Duration::from_millis(40));
        assert!(!b.is_open("p"), "после паузы — пробная попытка");
        b.record_failure("p");
        assert!(b.is_open("p"), "неудачная пробная попытка снова открывает");
        b.record_success("p");
        assert!(!b.is_open("p"));
        assert_eq!(b.failures("p"), 0);
    }

    /// Модель, которая первые `fail` раз отвечает 429 с `retry_ms`.
    struct Limited {
        id: &'static str,
        fail: usize,
        retry_ms: Option<u64>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Limited {
        fn new(id: &'static str, fail: usize, retry_ms: Option<u64>) -> Arc<Self> {
            Arc::new(Self { id, fail, retry_ms, calls: Default::default() })
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ChatModel for Limited {
        fn id(&self) -> String {
            self.id.into()
        }
        async fn chat(&self, _req: ChatRequest, _c: CancellationToken) -> Result<ChatResponse, LlmError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < self.fail {
                return Err(LlmError::RateLimited { retry_after_ms: self.retry_ms });
            }
            Ok(ChatResponse { message: jarvis_core::ChatMessage::assistant("ok"), model: self.id(), usage: None })
        }
    }

    fn chain_of(models: Vec<Arc<dyn ChatModel>>) -> ModelChain {
        ModelChain::new(models, Arc::new(CircuitBreaker::new(3, Duration::from_secs(60))))
    }

    #[tokio::test(start_paused = true)]
    async fn single_model_waits_short_retry_after_once() {
        let m = Limited::new("groq/x", 1, Some(3_000));
        let chain = chain_of(vec![m.clone()]);
        let t0 = tokio::time::Instant::now();
        let r = chain.chat(ChatRequest::default(), CancellationToken::new()).await.unwrap();
        assert_eq!(r.message.content, "ok");
        assert_eq!(m.calls(), 2);
        assert!(t0.elapsed() >= Duration::from_secs(3), "ждали retry_after");
        assert_eq!(chain.breaker().failures("groq"), 0);

        // Стрим тоже повторяет (до первого события ничего не ушло).
        let m = Limited::new("groq/x", 1, Some(1_000));
        let chain = chain_of(vec![m.clone()]);
        let (tx, _rx) = mpsc::channel(8);
        assert!(chain.chat_stream(ChatRequest::default(), tx, CancellationToken::new()).await.is_ok());
        assert_eq!(m.calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn whole_chain_limited_waits_for_soonest_model() {
        // groq: лимит 12 с, groq-2: 5 с → ждём 5 с и повторяем groq-2 (один раз).
        let a = Limited::new("groq/m", 1, Some(12_000));
        let b = Limited::new("groq-2/m", 1, Some(5_000));
        let chain = chain_of(vec![a.clone(), b.clone()]);
        let t0 = tokio::time::Instant::now();
        let (tx, _rx) = mpsc::channel(8);
        let r = chain.chat_stream(ChatRequest::default(), tx, CancellationToken::new()).await.unwrap();
        assert_eq!(r.model, "groq-2/m");
        assert_eq!((a.calls(), b.calls()), (1, 2));
        let waited = t0.elapsed();
        assert!(waited >= Duration::from_secs(5) && waited < Duration::from_secs(12), "{waited:?}");

        // Долгий лимит у всех (> 20 с) — сразу ошибка, без ожидания.
        let a = Limited::new("groq/m", 1, Some(40_000));
        let b = Limited::new("groq-2/m", 1, Some(30_000));
        assert!(chain_of(vec![a.clone(), b.clone()]).chat(ChatRequest::default(), CancellationToken::new()).await.is_err());
        assert_eq!((a.calls(), b.calls()), (1, 1));

        // Отмена во время ожидания.
        let a = Limited::new("groq/m", 1, Some(9_000));
        let b = Limited::new("groq-2/m", 1, Some(9_000));
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            c2.cancel();
        });
        let e = chain_of(vec![a, b]).chat(ChatRequest::default(), cancel).await.unwrap_err();
        assert_eq!(e, LlmError::Cancelled);
    }

    #[tokio::test(start_paused = true)]
    async fn single_model_retries_only_once_and_not_long_waits() {
        // Повтор тоже 429 — ошибка, третьей попытки нет.
        let m = Limited::new("groq/x", 5, Some(2_000));
        let err = chain_of(vec![m.clone()]).chat(ChatRequest::default(), CancellationToken::new()).await.unwrap_err();
        assert_eq!(err, LlmError::RateLimited { retry_after_ms: Some(2_000) });
        assert_eq!(m.calls(), 2);
        // Долгое ожидание (33 с) и неизвестное — сразу ошибка.
        for retry in [Some(33_000), None] {
            let m = Limited::new("groq/x", 1, retry);
            assert!(chain_of(vec![m.clone()]).chat(ChatRequest::default(), CancellationToken::new()).await.is_err());
            assert_eq!(m.calls(), 1);
        }
        // Ожидание выключено.
        let m = Limited::new("groq/x", 1, Some(1_000));
        let chain = chain_of(vec![m.clone()]).with_rate_limit_wait(Duration::ZERO);
        assert!(chain.chat(ChatRequest::default(), CancellationToken::new()).await.is_err());
        assert_eq!(m.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn multi_model_chain_fails_over_without_waiting() {
        // Два ключа Groq (разные организации, свои лимиты): 429 у первого — сразу второй, без ожидания.
        let a = Limited::new("groq/openai/gpt-oss-120b", 1, Some(3_000));
        let b = Limited::new("groq-2/openai/gpt-oss-120b", 0, None);
        let chain = chain_of(vec![a.clone(), b.clone()]);
        let t0 = tokio::time::Instant::now();
        let r = chain.chat(ChatRequest::default(), CancellationToken::new()).await.unwrap();
        assert_eq!(r.model, "groq-2/openai/gpt-oss-120b");
        assert_eq!((a.calls(), b.calls()), (1, 1));
        assert!(t0.elapsed() < Duration::from_secs(1));
        // Circuit breaker — по id провайдера: groq и groq-2 независимы.
        assert_eq!(chain.breaker().failures("groq"), 1);
        assert_eq!(chain.breaker().failures("groq-2"), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_for_rate_limit_is_cancellable() {
        let m = Limited::new("groq/x", 1, Some(15_000));
        let chain = Arc::new(chain_of(vec![m.clone()]));
        let cancel = CancellationToken::new();
        let (c2, cancel2) = (chain.clone(), cancel.clone());
        let task = tokio::spawn(async move { c2.chat(ChatRequest::default(), cancel2).await });
        tokio::time::sleep(Duration::from_secs(1)).await;
        cancel.cancel();
        assert_eq!(task.await.unwrap().unwrap_err(), LlmError::Cancelled);
        assert_eq!(m.calls(), 1, "после отмены повтора нет");
    }

    fn zen() -> ProviderSpec {
        ProviderSpec::from_config(&ProviderConfig {
            id: "opencode-zen".into(),
            base_url: "https://opencode.ai/inference/openai/v1".into(),
            api_key: "k".into(),
            ..Default::default()
        })
    }

    #[test]
    fn zen_route_from_preset() {
        // Куда реально уходит запрос, проверяет интеграционный тест на мок-сервере (tests/chain.rs).
        let c = client_for(&zen(), "claude-sonnet-5-5", None);
        assert_eq!(c.id(), "opencode-zen/claude-sonnet-5-5");
        assert_eq!(zen().route_for("claude-sonnet-5-5").unwrap().kind, ProviderKind::Anthropic);
    }

    #[test]
    fn build_role_skips_unusable() {
        let mut cfg = Config::default();
        cfg.providers[0].api_key = String::new(); // groq без ключа
        cfg.providers.push(ProviderConfig {
            id: "ollama".into(),
            base_url: "http://localhost:11434/v1".into(),
            enabled: true,
            ..Default::default()
        });
        cfg.roles.brain = vec!["groq/openai/gpt-oss-120b".into(), "ollama/qwen3:8b".into(), "nope/x".into()];
        let opts = BuildOptions { root: None, breaker: Arc::new(CircuitBreaker::new(3, Duration::from_secs(1))) };
        let m = build_role_model_with(&cfg, ModelRole::Brain, &opts).unwrap();
        assert_eq!(m.id(), "ollama/qwen3:8b");
        let err = build_role_model_with(&cfg, ModelRole::Vision, &opts).err().unwrap();
        assert!(err.to_string().contains("vision"));
    }
}
