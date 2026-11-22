//! Обнаружение: проверка ключа (`GET /models` или `/key`), список моделей с метаданными провайдера/каталога, кэш 24 ч.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jarvis_core::{CancellationToken, Config, LlmError, ProviderConfig, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::anthropic::{anthropic_request, normalize_base};
use crate::catalog::{Catalog, SpeedHint};
use crate::http;
use crate::presets::ProviderSpec;

pub(crate) const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Итог проверки ключа.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum KeyStatus {
    Valid,
    Invalid,
    /// Провайдер недоступен (сеть, таймаут, 5xx, неверный адрес) — человеческое описание.
    Unreachable(String),
}

impl KeyStatus {
    pub fn is_valid(&self) -> bool {
        matches!(self, KeyStatus::Valid)
    }

    /// Текст для экрана настроек.
    pub fn describe_ru(&self) -> String {
        match self {
            KeyStatus::Valid => "Ключ рабочий ✓".into(),
            KeyStatus::Invalid => "Ключ не принят: неверный, отозван или пустой".into(),
            KeyStatus::Unreachable(why) => format!("Провайдер недоступен: {why}"),
        }
    }
}

/// Откуда метаданные модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaSource {
    /// Контекст, tools и зрение — от провайдера.
    Provider,
    /// Всё из каталога.
    Catalog,
    /// Часть от провайдера, остальное из каталога.
    Mixed,
}

/// Через какой API звать модель, если не через API провайдера по умолчанию (OpenCode Zen отдаёт это в `/models`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiRoute {
    pub kind: ProviderKind,
    /// `None` — `base_url` провайдера.
    pub base_url: Option<String>,
    /// Родной API модели, если он не поддерживается здесь и вызов идёт через перевод (`responses`, `interactions`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_api: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub provider: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Контекст в токенах.
    pub ctx: Option<u32>,
    pub tools: bool,
    pub vision: bool,
    pub reasoning: bool,
    /// 1–5 из каталога.
    pub quality: u8,
    pub speed: SpeedHint,
    /// USD за 1M входных токенов (0 — бесплатно, `None` — неизвестно).
    pub price_in: Option<f64>,
    /// USD за 1M выходных токенов.
    pub price_out: Option<f64>,
    pub source: MetaSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<ApiRoute>,
}

impl ModelInfo {
    /// «provider/model».
    pub fn model_ref(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }

    /// Только по каталогу (провайдер не отдал метаданных).
    pub fn from_catalog(provider: &str, id: &str, catalog: &Catalog) -> Self {
        let fam = catalog.find(id);
        let f = fam.unwrap_or(&catalog.default);
        Self {
            provider: provider.into(),
            id: id.into(),
            name: None,
            ctx: f.ctx,
            tools: f.tools,
            vision: f.vision,
            reasoning: f.reasoning,
            quality: f.quality,
            speed: f.speed,
            price_in: None,
            price_out: None,
            source: MetaSource::Catalog,
            family: fam.map(|f| f.family.clone()),
            api: None,
        }
    }
}

// ---------- разбор метаданных провайдеров ----------

fn as_u32(v: &Value) -> Option<u32> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).filter(|n| *n > 0).map(|n| n.min(u32::MAX as u64) as u32)
}

/// bool или `{supported: bool}`.
fn flag(v: &Value) -> Option<bool> {
    v.as_bool().or_else(|| v.get("supported").and_then(|s| s.as_bool()))
}

fn has_image(arr: &Value) -> Option<bool> {
    arr.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).any(|s| s.starts_with("image")))
}

fn meta_ctx(v: &Value) -> Option<u32> {
    for k in ["context_length", "context_window", "max_context_length", "max_input_tokens", "contextLength", "inputTokenLimit"]
    {
        if let Some(n) = v.get(k).and_then(as_u32) {
            return Some(n);
        }
    }
    v.pointer("/limit/context").and_then(as_u32).or_else(|| v.pointer("/top_provider/context_length").and_then(as_u32))
}

fn supported_params(v: &Value) -> Option<Vec<&str>> {
    v.get("supported_parameters").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect())
}

fn meta_tools(v: &Value) -> Option<bool> {
    if let Some(p) = supported_params(v) {
        return Some(p.contains(&"tools"));
    }
    v.pointer("/capabilities/function_calling")
        .and_then(|x| x.as_bool())
        .or_else(|| v.pointer("/capabilities/tools").and_then(flag))
        .or_else(|| v.get("supportsTools").and_then(|x| x.as_bool()))
}

fn meta_vision(v: &Value) -> Option<bool> {
    v.pointer("/architecture/input_modalities")
        .and_then(has_image)
        .or_else(|| v.pointer("/modalities/input").and_then(has_image))
        .or_else(|| v.get("input_modalities").and_then(has_image))
        .or_else(|| v.pointer("/capabilities/vision").and_then(|x| x.as_bool()))
        .or_else(|| v.pointer("/capabilities/image_input").and_then(flag))
        .or_else(|| v.get("supportsImageInput").and_then(|x| x.as_bool()))
}

fn meta_reasoning(v: &Value) -> Option<bool> {
    if let Some(p) = supported_params(v) {
        return Some(p.contains(&"reasoning") || p.contains(&"include_reasoning"));
    }
    v.pointer("/capabilities/reasoning").and_then(flag)
}

fn price_str(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).filter(|p| *p >= 0.0)
}

/// (вход, выход) в USD за 1M токенов.
fn meta_price(v: &Value) -> (Option<f64>, Option<f64>) {
    // OpenRouter: USD за токен строкой.
    if let Some(p) = v.get("pricing").filter(|p| p.get("prompt").is_some()) {
        let per_m = |x: Option<f64>| x.map(|x| (x * 1e6 * 1e6).round() / 1e6);
        return (per_m(p.get("prompt").and_then(price_str)), per_m(p.get("completion").and_then(price_str)));
    }
    // OpenCode Zen: USD за 1M.
    if let Some(c) = v.pointer("/cost/text") {
        return (c.pointer("/input/price").and_then(price_str), c.pointer("/output/price").and_then(price_str));
    }
    (None, None)
}

const NON_CHAT_TYPES: &[&str] =
    &["embedding", "embeddings", "image", "video", "audio", "realtime", "decision", "tts", "stt", "rerank", "moderation"];

/// Нечатовая по метаданным провайдера.
fn non_chat_by_meta(v: &Value) -> bool {
    if v.get("type").and_then(|t| t.as_str()).is_some_and(|t| NON_CHAT_TYPES.contains(&t)) {
        return true;
    }
    if let Some(out) = v.pointer("/architecture/output_modalities").and_then(|a| a.as_array()) {
        let outs: Vec<&str> = out.iter().filter_map(|x| x.as_str()).collect();
        if !outs.contains(&"text") || outs.iter().any(|o| matches!(*o, "image" | "audio" | "video")) {
            return true;
        }
    }
    v.pointer("/capabilities/completion_chat").and_then(|x| x.as_bool()) == Some(false)
}

/// Маршрут из `api` OpenCode Zen. Чужой хост в `base_url` не принимаем, чтобы ключ не ушёл на другой домен.
fn api_route(v: &Value, provider_base: &str) -> Option<ApiRoute> {
    let api = v.get("api")?.as_object()?;
    let recommended = v.get("recommended_api").and_then(|r| r.as_str());
    let base_of = |name: &str| -> Option<String> {
        let url = api.get(name)?.get("base_url")?.as_str()?;
        (http::host_of(url).is_some() && http::host_of(url) == http::host_of(provider_base)).then(|| url.to_string())
    };
    if recommended == Some("messages") || (!api.contains_key("chat-completions") && api.contains_key("messages")) {
        return Some(ApiRoute { kind: ProviderKind::Anthropic, base_url: base_of("messages"), native_api: None });
    }
    if api.contains_key("chat-completions") {
        return Some(ApiRoute { kind: ProviderKind::OpenaiCompat, base_url: base_of("chat-completions"), native_api: None });
    }
    let native = recommended.map(str::to_string).or_else(|| api.keys().next().cloned());
    Some(ApiRoute { kind: ProviderKind::OpenaiCompat, base_url: None, native_api: native })
}

/// Одна запись `/models` → `ModelInfo` (или `None`, если модель нечатовая/неактивная).
pub fn model_from_json(spec: &ProviderSpec, v: &Value, catalog: &Catalog) -> Option<ModelInfo> {
    let raw_id = v.get("id").or_else(|| v.get("name")).and_then(|x| x.as_str())?;
    let id = raw_id.strip_prefix("models/").unwrap_or(raw_id).trim();
    if id.is_empty() || v.get("active").and_then(|a| a.as_bool()) == Some(false) {
        return None;
    }
    if non_chat_by_meta(v) || !catalog.is_chat_model(id) {
        return None;
    }
    let mut m = ModelInfo::from_catalog(&spec.id, id, catalog);
    let (ctx, tools, vision, reasoning) = (meta_ctx(v), meta_tools(v), meta_vision(v), meta_reasoning(v));
    let from_provider = [ctx.is_some(), tools.is_some(), vision.is_some()].iter().filter(|b| **b).count();
    m.ctx = ctx.or(m.ctx);
    m.tools = tools.unwrap_or(m.tools);
    m.vision = vision.unwrap_or(m.vision);
    m.reasoning = reasoning.unwrap_or(m.reasoning);
    m.name = v
        .get("display_name")
        .or_else(|| v.get("name"))
        .and_then(|n| n.as_str())
        .filter(|n| *n != raw_id && *n != id)
        .map(str::to_string);
    (m.price_in, m.price_out) = meta_price(v);
    if spec.local {
        m.price_in = Some(0.0);
        m.price_out = Some(0.0);
    }
    m.source = match from_provider {
        3 => MetaSource::Provider,
        0 => MetaSource::Catalog,
        _ => MetaSource::Mixed,
    };
    m.api = api_route(v, &spec.base_url);
    Some(m)
}

/// Разобрать ответ `/models`: `{data:[…]}`, голый массив или `{models:[…]}`.
pub fn parse_models_response(spec: &ProviderSpec, body: &Value, catalog: &Catalog) -> Vec<ModelInfo> {
    let items = body
        .as_array()
        .or_else(|| body.get("data").and_then(|d| d.as_array()))
        .or_else(|| body.get("models").and_then(|d| d.as_array()));
    let mut out: Vec<ModelInfo> =
        items.into_iter().flatten().filter_map(|v| model_from_json(spec, v, catalog)).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

fn get_request(spec: &ProviderSpec, http: &reqwest::Client, path: &str) -> reqwest::RequestBuilder {
    match spec.kind {
        ProviderKind::Anthropic => {
            let url = http::join_url(&normalize_base(&spec.base_url), path);
            anthropic_request(http.get(url), spec, spec.auth, &spec.anthropic_version)
        }
        ProviderKind::OpenaiCompat => {
            let url = http::join_url(&spec.base_url, path);
            http::apply_auth(http.get(url), spec.auth, &spec.api_key, &spec.extra_headers)
        }
    }
    .timeout(DISCOVERY_TIMEOUT)
}

/// Загрузить список моделей у провайдера (без кэша).
pub async fn fetch_models(
    spec: &ProviderSpec,
    http: &reqwest::Client,
    catalog: &Catalog,
    cancel: &CancellationToken,
) -> Result<Vec<ModelInfo>, LlmError> {
    if spec.base_url.is_empty() {
        return Err(LlmError::Other("не указан base_url".into()));
    }
    let key = spec.api_key.as_str();
    let result = match spec.kind {
        ProviderKind::Anthropic => {
            // Пагинация: limit до 1000, after_id = last_id, пока has_more.
            let mut all = Vec::new();
            let mut after: Option<String> = None;
            for _ in 0..20 {
                let mut path = format!("{}?limit=1000", spec.models_path);
                if let Some(a) = &after {
                    path.push_str("&after_id=");
                    path.push_str(a);
                }
                let resp = http::send(get_request(spec, http, &path), cancel, "models", key).await?;
                let v = http::read_json(resp, cancel, key).await?;
                all.extend(v["data"].as_array().cloned().unwrap_or_default());
                match (v["has_more"].as_bool(), v["last_id"].as_str()) {
                    (Some(true), Some(last)) => after = Some(last.to_string()),
                    _ => break,
                }
            }
            Ok(parse_models_response(spec, &json!({ "data": all }), catalog))
        }
        ProviderKind::OpenaiCompat => {
            match http::send(get_request(spec, http, &spec.models_path), cancel, "models", key).await {
                Ok(resp) => Ok(parse_models_response(spec, &http::read_json(resp, cancel, key).await?, catalog)),
                Err(e) => Err(e),
            }
        }
    };
    match result {
        Err(LlmError::ModelNotFound(_)) if !spec.fallback_models.is_empty() => {
            tracing::info!(provider = %spec.id, "/models недоступен (404) — беру fallback_models из пресета");
            Ok(spec.fallback_models.iter().map(|id| ModelInfo::from_catalog(&spec.id, id, catalog)).collect())
        }
        Err(LlmError::ModelNotFound(_)) => {
            Err(LlmError::Other(format!("{}{} не найден (404) — проверьте base_url", spec.base_url, spec.models_path)))
        }
        other => other,
    }
}

/// Список моделей провайдера (сеть, без кэша), нечатовые отфильтрованы, отсортировано по id.
pub async fn list_models(pc: &ProviderConfig) -> Result<Vec<ModelInfo>, LlmError> {
    fetch_models(&ProviderSpec::from_config(pc), &http::shared_client(), Catalog::builtin(), &CancellationToken::new())
        .await
}

/// Проверка ключа: `GET {base}{key_check_path}` → 2xx рабочий / 401·403 (и 400 «invalid api key») неверный / иначе недоступен.
pub async fn validate_key(pc: &ProviderConfig) -> KeyStatus {
    validate_key_spec(&ProviderSpec::from_config(pc), &http::shared_client(), &CancellationToken::new()).await
}

pub async fn validate_key_spec(spec: &ProviderSpec, http: &reqwest::Client, cancel: &CancellationToken) -> KeyStatus {
    if spec.base_url.is_empty() {
        return KeyStatus::Unreachable("не указан base_url".into());
    }
    if spec.needs_key && spec.api_key.is_empty() {
        return KeyStatus::Invalid;
    }
    let r = http::send(get_request(spec, http, &spec.key_check_path), cancel, "models", &spec.api_key).await;
    match r {
        Ok(_) => KeyStatus::Valid,
        // /models не реализован (Z.AI?) — проверяем минимальным запросом к модели из пресета.
        Err(LlmError::ModelNotFound(_)) if !spec.fallback_models.is_empty() => {
            validate_by_chat(spec, http, &spec.fallback_models[0], cancel).await
        }
        Err(e) => key_status_from_error(e, &format!("{}{}", spec.base_url, spec.key_check_path)),
    }
}

fn key_status_from_error(e: LlmError, url: &str) -> KeyStatus {
    match e {
        LlmError::Auth => KeyStatus::Invalid,
        // Лимит — значит, ключ принят.
        LlmError::RateLimited { .. } => KeyStatus::Valid,
        LlmError::ModelNotFound(_) => KeyStatus::Unreachable(format!("адрес {url} не найден (404) — проверьте base_url")),
        LlmError::Timeout => KeyStatus::Unreachable("таймаут ответа".into()),
        LlmError::Network(m) => KeyStatus::Unreachable(format!("нет соединения ({m})")),
        LlmError::Server(c) => KeyStatus::Unreachable(format!("ошибка сервера провайдера (HTTP {c})")),
        other => KeyStatus::Unreachable(other.to_string()),
    }
}

async fn validate_by_chat(spec: &ProviderSpec, http: &reqwest::Client, model: &str, cancel: &CancellationToken) -> KeyStatus {
    use jarvis_core::{ChatMessage, ChatModel, ChatRequest};
    let client = crate::openai::OpenAiCompatClient::new(spec.clone(), model)
        .with_http(http.clone())
        .with_timeouts(DISCOVERY_TIMEOUT, DISCOVERY_TIMEOUT);
    let req = ChatRequest { messages: vec![ChatMessage::user("ok")], max_tokens: Some(1), ..Default::default() };
    match client.chat(req, cancel.clone()).await {
        Ok(_) => KeyStatus::Valid,
        // Модель могли переименовать, но до неё дошло — значит, ключ принят.
        Err(LlmError::ModelNotFound(_)) | Err(LlmError::Other(_)) => KeyStatus::Valid,
        Err(e) => key_status_from_error(e, &spec.base_url),
    }
}

// ---------- кэш ----------

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    provider: String,
    base_url: String,
    /// Unix-время загрузки, секунды.
    fetched_at: u64,
    models: Vec<ModelInfo>,
}

/// Кэш списков моделей: `{root}/{data_dir}/cache/models-<provider>.json`, по умолчанию 24 ч.
/// В файле только id и метаданные моделей — без ключей.
#[derive(Debug, Clone)]
pub struct ModelCache {
    dir: PathBuf,
    ttl: Duration,
}

impl ModelCache {
    pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 3600);

    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), ttl: Self::DEFAULT_TTL }
    }

    /// `root` — корень проекта (`d:\jarvis`), `data_dir` берётся из конфига (относительный — от корня).
    pub fn for_config(root: &Path, cfg: &Config) -> Self {
        Self::new(root.join(&cfg.data_dir).join("cache"))
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path_for(&self, provider: &str) -> PathBuf {
        let safe: String = provider
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '_' })
            .collect();
        self.dir.join(format!("models-{safe}.json"))
    }

    fn read(&self, provider: &str, base_url: &str) -> Option<CacheFile> {
        let s = std::fs::read_to_string(self.path_for(provider)).ok()?;
        let f: CacheFile = serde_json::from_str(&s).ok()?;
        (f.provider == provider && f.base_url.trim_end_matches('/') == base_url.trim_end_matches('/')).then_some(f)
    }

    /// Свежий (моложе TTL) список.
    pub fn load_fresh(&self, provider: &str, base_url: &str) -> Option<Vec<ModelInfo>> {
        let f = self.read(provider, base_url)?;
        let age = now_secs().saturating_sub(f.fetched_at);
        (age < self.ttl.as_secs()).then_some(f.models)
    }

    /// Любой сохранённый список (в том числе устаревший) и время загрузки.
    pub fn load_any(&self, provider: &str, base_url: &str) -> Option<(Vec<ModelInfo>, SystemTime)> {
        let f = self.read(provider, base_url)?;
        Some((f.models, UNIX_EPOCH + Duration::from_secs(f.fetched_at)))
    }

    /// Атомарная запись (временный файл + переименование).
    pub fn store(&self, provider: &str, base_url: &str, models: &[ModelInfo]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let f = CacheFile {
            provider: provider.into(),
            base_url: base_url.trim_end_matches('/').into(),
            fetched_at: now_secs(),
            models: models.to_vec(),
        };
        let path = self.path_for(provider);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&f).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }

    pub fn invalidate(&self, provider: &str) {
        let _ = std::fs::remove_file(self.path_for(provider));
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Список моделей с кэшем: свежий кэш → без сети; иначе загрузка и запись; при сбое сети — устаревший кэш, если есть.
pub async fn list_models_cached(
    pc: &ProviderConfig,
    cache: &ModelCache,
    force_refresh: bool,
) -> Result<Vec<ModelInfo>, LlmError> {
    let spec = ProviderSpec::from_config(pc);
    list_models_cached_spec(&spec, cache, force_refresh, &http::shared_client(), &CancellationToken::new()).await
}

pub async fn list_models_cached_spec(
    spec: &ProviderSpec,
    cache: &ModelCache,
    force_refresh: bool,
    http: &reqwest::Client,
    cancel: &CancellationToken,
) -> Result<Vec<ModelInfo>, LlmError> {
    if !force_refresh && let Some(m) = cache.load_fresh(&spec.id, &spec.base_url) {
        return Ok(m);
    }
    match fetch_models(spec, http, Catalog::builtin(), cancel).await {
        Ok(models) => {
            if let Err(e) = cache.store(&spec.id, &spec.base_url, &models) {
                tracing::warn!(provider = %spec.id, "не удалось записать кэш моделей: {e}");
            }
            Ok(models)
        }
        Err(e @ (LlmError::Network(_) | LlmError::Timeout | LlmError::Server(_))) => match cache.load_any(&spec.id, &spec.base_url)
        {
            Some((m, _)) => {
                tracing::warn!(provider = %spec.id, "список моделей недоступен ({e}) — беру устаревший кэш");
                Ok(m)
            }
            None => Err(e),
        },
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, base: &str) -> ProviderSpec {
        ProviderSpec::from_config(&ProviderConfig { id: id.into(), base_url: base.into(), ..Default::default() })
    }

    #[test]
    fn zen_entry() {
        let v = json!({"id":"claude-sonnet-5-5","object":"model","name":"Claude Sonnet 5.5","type":"chat",
            "modalities":{"input":["text","image/png"],"output":["text"]},
            "capabilities":{"tools":{"supported":true},"reasoning":{"supported":true}},
            "limit":{"context":1000000,"output":128000},
            "cost":{"text":{"input":{"price":2,"cache":{"read":0.1}},"output":{"price":10}}},
            "api":{"messages":{"base_url":"https://opencode.ai/inference/anthropic/v1"}},"recommended_api":"messages"});
        let s = spec("opencode-zen", "https://opencode.ai/inference/openai/v1");
        let m = model_from_json(&s, &v, Catalog::builtin()).unwrap();
        assert_eq!(m.ctx, Some(1_000_000));
        assert!(m.tools && m.vision && m.reasoning);
        assert_eq!((m.price_in, m.price_out), (Some(2.0), Some(10.0)));
        assert_eq!(m.source, MetaSource::Provider);
        assert_eq!(m.name.as_deref(), Some("Claude Sonnet 5.5"));
        let r = m.api.unwrap();
        assert_eq!(r.kind, ProviderKind::Anthropic);
        assert_eq!(r.base_url.as_deref(), Some("https://opencode.ai/inference/anthropic/v1"));

        // Чужой хост в base_url не принимаем.
        let mut evil = v.clone();
        evil["api"]["messages"]["base_url"] = json!("https://evil.example/v1");
        let m = model_from_json(&s, &evil, Catalog::builtin()).unwrap();
        assert_eq!(m.api.unwrap().base_url, None);

        // responses-модель идёт через chat/completions с пометкой.
        let g = json!({"id":"gpt-5.5","type":"chat","api":{"responses":{"base_url":"https://opencode.ai/inference/openai/v1"}},"recommended_api":"responses"});
        let r = model_from_json(&s, &g, Catalog::builtin()).unwrap().api.unwrap();
        assert_eq!((r.kind, r.native_api.as_deref()), (ProviderKind::OpenaiCompat, Some("responses")));

        let e = json!({"id":"text-embedding-3-small","type":"embedding"});
        assert!(model_from_json(&s, &e, Catalog::builtin()).is_none());
        let d = json!({"id":"some-new-thing","type":"decision"});
        assert!(model_from_json(&s, &d, Catalog::builtin()).is_none());
    }

    #[test]
    fn mistral_and_anthropic_and_gemini_shapes() {
        let s = spec("mistral", "https://api.mistral.ai/v1");
        let arr = json!([
            {"id":"mistral-large-latest","max_context_length":131072,
             "capabilities":{"completion_chat":true,"function_calling":true,"vision":true}},
            {"id":"mistral-moderation-latest","capabilities":{"completion_chat":false}},
            {"id":"codestral-embed","capabilities":{"completion_chat":false}}
        ]);
        let ms = parse_models_response(&s, &arr, Catalog::builtin());
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].source, MetaSource::Provider);
        assert!(ms[0].vision && ms[0].tools);

        let a = ProviderSpec::from_config(&ProviderConfig {
            id: "anthropic".into(),
            kind: ProviderKind::Anthropic,
            ..Default::default()
        });
        let v = json!({"data":[{"type":"model","id":"claude-haiku-5-5","display_name":"Claude Haiku 5.5",
            "max_input_tokens":1000000,"capabilities":{"image_input":{"supported":true}}}],"has_more":false});
        let ms = parse_models_response(&a, &v, Catalog::builtin());
        assert_eq!(ms[0].ctx, Some(1_000_000));
        assert!(ms[0].vision);
        assert!(ms[0].tools, "tools из каталога");
        assert_eq!(ms[0].source, MetaSource::Mixed);

        let g = spec("gemini", "https://generativelanguage.googleapis.com/v1beta/openai");
        let v = json!({"object":"list","data":[{"id":"models/gemini-3.8-flash","object":"model","owned_by":"google"}]});
        let ms = parse_models_response(&g, &v, Catalog::builtin());
        assert_eq!(ms[0].id, "gemini-3.8-flash");
        assert_eq!(ms[0].source, MetaSource::Catalog);
        assert_eq!(ms[0].family.as_deref(), Some("gemini-flash"));
    }

    #[test]
    fn local_models_are_free() {
        let s = spec("ollama", "http://localhost:11434/v1");
        let v = json!({"object":"list","data":[{"id":"qwen3:8b","object":"model"},{"id":"nomic-embed-text:latest"}]});
        let ms = parse_models_response(&s, &v, Catalog::builtin());
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].price_in, Some(0.0));
    }

    #[test]
    fn key_status_text() {
        assert!(KeyStatus::Valid.describe_ru().contains('✓'));
        assert_eq!(serde_json::to_value(KeyStatus::Unreachable("x".into())).unwrap(), json!({"status":"unreachable","detail":"x"}));
        assert_eq!(serde_json::to_value(KeyStatus::Valid).unwrap(), json!({"status":"valid"}));
    }
}
