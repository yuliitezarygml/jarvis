//! TTS-клиент сервера `jarvis-tts-rs` (Xeon): `GET /ready`, `POST /tts/stream` → PCM s16le 24 кГц mono,
//! `DELETE /tts/{id}` для отмены. Запрос строго по `server/jarvis-tts-rs/src/api.rs`: тело `{"text","voice"}`
//! (сервер отвергает лишние поля), id запроса задаёт клиент в `x-request-id`, чтобы отменять ещё до ответа.

use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use jarvis_core::CancellationToken;
use jarvis_core::config::TtsConfig;
use serde::Deserialize;

use crate::error::{Result, VoiceError};
use crate::pcm::TTS_SAMPLE_RATE;
use crate::tls;

/// Сервер принимает не больше 2000 символов за запрос (а фразы у нас ≤260).
pub const MAX_TEXT_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TtsReady {
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub sample_rate: u32,
    #[serde(default)]
    pub voice: String,
}

#[derive(Debug, Clone)]
pub struct TtsOptions {
    pub connect_timeout: Duration,
    /// До заголовков ответа: сервер отвечает, только когда готов первый звук (~0,7 с, под нагрузкой дольше).
    pub first_audio_timeout: Duration,
    /// Максимальная пауза между кусками потока.
    pub chunk_timeout: Duration,
    /// `/ready` и `DELETE`.
    pub control_timeout: Duration,
}

impl Default for TtsOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(3),
            first_audio_timeout: Duration::from_secs(15),
            chunk_timeout: Duration::from_secs(10),
            control_timeout: Duration::from_secs(3),
        }
    }
}

pub type PcmChunks = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;

/// Поток PCM одной фразы. `request_id` — для `cancel()`.
pub struct TtsStream {
    pub request_id: String,
    pub sample_rate: u32,
    inner: PcmChunks,
}

impl TtsStream {
    pub fn new(request_id: impl Into<String>, sample_rate: u32, inner: PcmChunks) -> Self {
        Self { request_id: request_id.into(), sample_rate, inner }
    }

    /// Готовые куски (для подменных движков в тестах).
    pub fn from_chunks(request_id: impl Into<String>, sample_rate: u32, chunks: Vec<Bytes>) -> Self {
        Self::new(request_id, sample_rate, Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok))))
    }

    pub fn into_parts(self) -> (String, PcmChunks) {
        (self.request_id, self.inner)
    }

    /// Дочитать весь поток в память.
    pub async fn collect_all(mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(chunk) = self.inner.next().await {
            out.extend_from_slice(&chunk?);
        }
        Ok(out)
    }
}

impl Stream for TtsStream {
    type Item = Result<Bytes>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Любой синтезатор речи: сервер Xeon или подмена в тестах.
#[async_trait]
pub trait TtsEngine: Send + Sync {
    async fn synthesize(&self, text: &str, voice: Option<&str>, cancel: CancellationToken) -> Result<TtsStream>;
    /// Отменить синтез на сервере. `Ok(false)` — такого запроса уже нет.
    async fn cancel(&self, request_id: &str) -> Result<bool>;
}

#[derive(Clone)]
pub struct TtsClient {
    http: reqwest::Client,
    base: String,
    voice: String,
    opts: TtsOptions,
}

impl TtsClient {
    /// `project_root` — откуда считать относительный `ca_path` (обычно `d:\jarvis`).
    pub fn new(cfg: &TtsConfig, project_root: &Path) -> Result<Self> {
        Self::with_options(cfg, project_root, TtsOptions::default())
    }

    pub fn with_options(cfg: &TtsConfig, project_root: &Path, opts: TtsOptions) -> Result<Self> {
        let base = cfg.url.trim_end_matches('/').to_string();
        if !base.starts_with("https://") && !base.starts_with("http://") {
            return Err(VoiceError::Config(format!("адрес TTS должен начинаться с https://: {base}")));
        }
        let ca = tls::resolve_path(project_root, &cfg.ca_path);
        let http = tls::pinned_http_client(&base, &ca, opts.connect_timeout)?;
        Ok(Self { http, base, voice: cfg.voice.clone(), opts })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn voice(&self) -> &str {
        &self.voice
    }

    pub async fn ready(&self) -> Result<TtsReady> {
        let resp = self.http.get(format!("{}/ready", self.base)).timeout(self.opts.control_timeout).send().await?;
        let resp = check_status(resp).await?;
        resp.json::<TtsReady>().await.map_err(|e| VoiceError::BadResponse(e.to_string()))
    }

    /// Начать синтез фразы. Отмена `cancel` (барж-ин) обрывает поток и сразу шлёт `DELETE /tts/{id}`;
    /// то же происходит, если поток бросили недочитанным.
    pub async fn synthesize_stream(&self, text: &str, voice: Option<&str>, cancel: CancellationToken) -> Result<TtsStream> {
        let text = text.trim();
        if !text.chars().any(|c| c.is_alphanumeric()) {
            return Err(VoiceError::InvalidInput("в тексте нечего произносить".into()));
        }
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(VoiceError::InvalidInput(format!("текст длиннее {MAX_TEXT_CHARS} символов")));
        }
        if cancel.is_cancelled() {
            return Err(VoiceError::Cancelled);
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let body = serde_json::json!({ "text": text, "voice": voice.unwrap_or(&self.voice) });
        let send = self.http.post(format!("{}/tts/stream", self.base)).header("x-request-id", &id).json(&body).send();
        let mut guard = CancelGuard { client: self.clone(), id: id.clone(), armed: true };
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                guard.fire();
                return Err(VoiceError::Cancelled);
            }
            r = tokio::time::timeout(self.opts.first_audio_timeout, send) => match r {
                Err(_) => {
                    guard.fire();
                    return Err(VoiceError::Timeout("ожидание первого звука TTS"));
                }
                Ok(r) => r?,
            },
        };
        let resp = match check_status(resp).await {
            Ok(r) => r,
            Err(e) => {
                guard.disarm(); // сервер запрос уже отверг
                return Err(e);
            }
        };
        let sample_rate = resp
            .headers()
            .get("x-audio-format")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_rate)
            .unwrap_or(TTS_SAMPLE_RATE);
        let state = StreamState {
            body: Box::pin(resp.bytes_stream()),
            cancel,
            guard,
            chunk_timeout: self.opts.chunk_timeout,
            finished: false,
        };
        let chunks = futures_util::stream::unfold(state, |mut st| async move {
            if st.finished {
                return None;
            }
            tokio::select! {
                biased;
                _ = st.cancel.cancelled() => {
                    st.finished = true;
                    st.guard.fire();
                    Some((Err(VoiceError::Cancelled), st))
                }
                r = tokio::time::timeout(st.chunk_timeout, st.body.next()) => match r {
                    Err(_) => {
                        st.finished = true;
                        st.guard.fire();
                        Some((Err(VoiceError::Timeout("пауза в потоке TTS")), st))
                    }
                    Ok(None) => {
                        st.guard.disarm();
                        None
                    }
                    Ok(Some(Ok(b))) => Some((Ok(b), st)),
                    Ok(Some(Err(e))) => {
                        st.finished = true;
                        st.guard.fire();
                        Some((Err(e.into()), st))
                    }
                },
            }
        });
        Ok(TtsStream::new(id, sample_rate, Box::pin(chunks)))
    }

    pub async fn cancel(&self, request_id: &str) -> Result<bool> {
        let resp = self
            .http
            .delete(format!("{}/tts/{request_id}", self.base))
            .timeout(self.opts.control_timeout)
            .send()
            .await?;
        match resp.status().as_u16() {
            200..=299 => Ok(true),
            404 => Ok(false),
            status => Err(VoiceError::Http { status, body: resp.text().await.unwrap_or_default() }),
        }
    }
}

#[async_trait]
impl TtsEngine for TtsClient {
    async fn synthesize(&self, text: &str, voice: Option<&str>, cancel: CancellationToken) -> Result<TtsStream> {
        self.synthesize_stream(text, voice, cancel).await
    }
    async fn cancel(&self, request_id: &str) -> Result<bool> {
        TtsClient::cancel(self, request_id).await
    }
}

struct StreamState {
    body: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    cancel: CancellationToken,
    guard: CancelGuard,
    chunk_timeout: Duration,
    finished: bool,
}

/// Гарантирует `DELETE /tts/{id}`, если синтез не дошёл до конца (отмена, таймаут, брошенный поток).
struct CancelGuard {
    client: TtsClient,
    id: String,
    armed: bool,
}

impl CancelGuard {
    /// Синтез завершён штатно — отменять нечего.
    fn disarm(&mut self) {
        self.armed = false;
    }

    fn fire(&mut self) {
        if !std::mem::replace(&mut self.armed, false) {
            return;
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let client = self.client.clone();
            let id = self.id.clone();
            rt.spawn(async move {
                if let Err(e) = client.cancel(&id).await {
                    tracing::debug!(%id, error = %e, "не удалось отменить синтез");
                }
            });
        }
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.fire();
    }
}

fn parse_rate(fmt: &str) -> Option<u32> {
    fmt.split(';').find_map(|p| p.trim().strip_prefix("rate=")).and_then(|r| r.parse().ok())
}

pub(crate) async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = tokio::time::timeout(Duration::from_secs(2), resp.text()).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    let body: String = body.chars().take(300).collect();
    Err(match status.as_u16() {
        401 | 403 => VoiceError::Auth(status.as_u16()),
        s => VoiceError::Http { status: s, body },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_format_header() {
        assert_eq!(parse_rate("s16le;rate=24000;channels=1"), Some(24_000));
        assert_eq!(parse_rate("mp3"), None);
    }

    #[test]
    fn rejects_bad_config_and_text() {
        let root = Path::new(".");
        let bad = TtsConfig { url: "ftp://x".into(), ..Default::default() };
        assert!(matches!(TtsClient::new(&bad, root), Err(VoiceError::Config(_))));
        let local = TtsConfig { url: "http://127.0.0.1:9".into(), ..Default::default() };
        let c = TtsClient::new(&local, root).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let e = c.synthesize_stream("…", None, CancellationToken::new()).await;
            assert!(matches!(e, Err(VoiceError::InvalidInput(_))));
            let long = "а".repeat(MAX_TEXT_CHARS + 1);
            assert!(matches!(c.synthesize_stream(&long, None, CancellationToken::new()).await, Err(VoiceError::InvalidInput(_))));
            let t = CancellationToken::new();
            t.cancel();
            assert_eq!(c.synthesize_stream("привет", None, t).await.err(), Some(VoiceError::Cancelled));
        });
    }
}
