//! HTTP/WebSocket API по `jarvis_core::stt_protocol`: `GET /health`, `POST /stt`, `WS /stt/stream`,
//! плюс `GET /metrics`.

use crate::audio::{self, AudioError, PcmDecoder, SAMPLE_RATE};
use crate::config::Limits;
use crate::engine::{Engine, Recognition};
use crate::metrics::Metrics;
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{
        State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jarvis_core::stt_protocol::{ClientMsg, Health, STREAM_PATH, ServerMsg, SttResult};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<dyn Engine>,
    pub limits: Arc<Limits>,
    pub sessions: Arc<Semaphore>,
    pub requests: Arc<Semaphore>,
    pub metrics: Arc<Metrics>,
}

impl AppState {
    pub fn new(engine: Arc<dyn Engine>, limits: Limits) -> Self {
        Self {
            sessions: Arc::new(Semaphore::new(limits.max_sessions)),
            requests: Arc::new(Semaphore::new(limits.max_requests)),
            limits: Arc::new(limits),
            metrics: Arc::new(Metrics::default()),
            engine,
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(|State(s): State<AppState>| async move { s.metrics.render() }))
        .route("/stt", post(transcribe))
        .route(STREAM_PATH, get(stream))
        .fallback(|| async { err(StatusCode::NOT_FOUND, "not_found") })
        .with_state(state)
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

async fn health(State(s): State<AppState>) -> Json<Health> {
    Json(Health { ok: true, model: s.engine.model().to_string(), sample_rate: SAMPLE_RATE })
}

fn samples_ms(n: usize) -> u64 {
    n as u64 * 1000 / SAMPLE_RATE as u64
}

// ---------------------------------------------------------------- POST /stt

#[derive(Debug, PartialEq)]
pub enum UploadError {
    UnsupportedType(String),
    Audio(AudioError),
}

/// Тело `POST /stt` → 16 кГц mono. WAV распознаётся по сигнатуре RIFF/WAVE или по типу
/// `audio/wav|x-wav|wave|vnd.wave`; `audio/L16` — little-endian (как в протоколе) с параметрами
/// `rate` (по умолчанию 16000) и `channels` (1); без типа / `application/octet-stream` — L16 16 кГц mono.
pub fn decode_upload(content_type: Option<&str>, body: &[u8]) -> Result<Vec<f32>, UploadError> {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    let mut parts = ct.split(';').map(str::trim);
    let mime = parts.next().unwrap_or("");
    let param = |name: &str| -> Option<String> {
        ct.split(';').skip(1).filter_map(|p| p.trim().split_once('=')).find(|(k, _)| k.trim() == name).map(|(_, v)| {
            v.trim().trim_matches('"').to_string()
        })
    };
    let is_riff = body.len() >= 12 && &body[0..4] == b"RIFF" && &body[8..12] == b"WAVE";
    if is_riff || matches!(mime, "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave") {
        let w = audio::parse_wav(body).map_err(UploadError::Audio)?;
        return Ok(audio::resample(&w.samples, w.sample_rate, SAMPLE_RATE));
    }
    let (rate, channels) = match mime {
        "audio/l16" => {
            let rate: u32 = match param("rate") {
                None => SAMPLE_RATE,
                Some(v) => v.parse().map_err(|_| UploadError::Audio(AudioError::BadParameter(format!("rate={v}"))))?,
            };
            let channels: u16 = match param("channels") {
                None => 1,
                Some(v) => v.parse().map_err(|_| UploadError::Audio(AudioError::BadParameter(format!("channels={v}"))))?,
            };
            if !(8000..=48000).contains(&rate) || !(1..=8).contains(&channels) {
                return Err(UploadError::Audio(AudioError::BadParameter(format!("rate={rate}, channels={channels}"))));
            }
            (rate, channels)
        }
        "" | "application/octet-stream" => (SAMPLE_RATE, 1),
        other => return Err(UploadError::UnsupportedType(other.to_string())),
    };
    if body.len() % (2 * channels as usize) != 0 {
        return Err(UploadError::Audio(AudioError::OddPcmLength));
    }
    let pcm = audio::s16le_to_f32(body).map_err(UploadError::Audio)?;
    Ok(audio::resample(&audio::downmix(&pcm, channels), rate, SAMPLE_RATE))
}

async fn transcribe(State(s): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    s.metrics.requests_total.fetch_add(1, Relaxed);
    let Ok(Ok(_permit)) = tokio::time::timeout(s.limits.queue_timeout, s.requests.clone().acquire_owned()).await else {
        s.metrics.requests_rejected.fetch_add(1, Relaxed);
        return err(StatusCode::SERVICE_UNAVAILABLE, "busy");
    };
    let bytes = match tokio::time::timeout(s.limits.body_timeout, to_bytes(body, s.limits.max_body_bytes)).await {
        Err(_) => return err(StatusCode::REQUEST_TIMEOUT, "body_timeout"),
        Ok(Err(_)) => return err(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large"),
        Ok(Ok(b)) => b,
    };
    let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let samples = match decode_upload(ct, &bytes) {
        Ok(s) => s,
        Err(UploadError::UnsupportedType(t)) => {
            tracing::info!(content_type = %t, "POST /stt: неподдерживаемый тип");
            return err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type");
        }
        Err(UploadError::Audio(e)) => {
            tracing::info!(error = %e, "POST /stt: неверное аудио");
            return err(StatusCode::UNPROCESSABLE_ENTITY, "invalid_audio");
        }
    };
    if samples.is_empty() {
        return err(StatusCode::BAD_REQUEST, "empty_audio");
    }
    let duration_ms = samples_ms(samples.len());
    if duration_ms > s.limits.max_utterance.as_millis() as u64 {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "audio_too_long");
    }
    let engine = s.engine.clone();
    let started = Instant::now();
    let job = tokio::task::spawn_blocking(move || engine.transcribe(&samples));
    match tokio::time::timeout(s.limits.decode_timeout, job).await {
        Ok(Ok(Ok(text))) => {
            let latency = started.elapsed();
            s.metrics.final_done(latency, duration_ms);
            tracing::info!(audio_ms = duration_ms, decode_ms = latency.as_millis() as u64, chars = text.chars().count(), "POST /stt");
            tracing::debug!(%text, "POST /stt text");
            Json(SttResult { text, duration_ms }).into_response()
        }
        Ok(Ok(Err(e))) => {
            s.metrics.errors_total.fetch_add(1, Relaxed);
            tracing::warn!(error = %e, "POST /stt: ошибка движка");
            err(StatusCode::SERVICE_UNAVAILABLE, "engine_error")
        }
        Ok(Err(_)) => {
            s.metrics.errors_total.fetch_add(1, Relaxed);
            err(StatusCode::INTERNAL_SERVER_ERROR, "engine_panic")
        }
        Err(_) => {
            s.metrics.errors_total.fetch_add(1, Relaxed);
            tracing::warn!("POST /stt: таймаут декодирования");
            err(StatusCode::GATEWAY_TIMEOUT, "decode_timeout")
        }
    }
}

// ---------------------------------------------------------------- WS /stt/stream

async fn stream(State(s): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let Ok(permit) = s.sessions.clone().try_acquire_owned() else {
        s.metrics.sessions_rejected.fetch_add(1, Relaxed);
        tracing::warn!("WS: лимит сессий");
        return err(StatusCode::SERVICE_UNAVAILABLE, "too_many_sessions");
    };
    ws.max_message_size(1 << 20)
        .max_frame_size(1 << 20)
        .on_upgrade(move |socket| run_session(socket, s, permit))
}

enum Fatal {
    /// Клиент ушёл.
    Closed,
    /// Ошибка движка/таймаут: сообщаем клиенту и закрываем.
    Engine(String),
}

struct ActiveGuard(Arc<Metrics>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.sessions_active.fetch_sub(1, Relaxed);
    }
}

async fn send(socket: &mut WebSocket, msg: &ServerMsg) -> Result<(), Fatal> {
    let text = serde_json::to_string(msg).expect("ServerMsg serializes");
    socket.send(Message::Text(text.into())).await.map_err(|_| Fatal::Closed)
}

struct Session {
    id: u64,
    s: AppState,
    rec: Option<Box<dyn Recognition>>,
    dec: PcmDecoder,
    /// Сэмплов в текущей фразе.
    samples: usize,
    last_partial: String,
    phrases: u64,
}

impl Session {
    /// Блокирующий вызов движка с таймаутом. Если поток не вернулся вовремя, распознаватель потерян.
    async fn blocking<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut dyn Recognition) -> anyhow::Result<R> + Send + 'static,
    ) -> Result<R, Fatal> {
        let mut rec = self.rec.take().ok_or_else(|| Fatal::Engine("распознаватель недоступен".into()))?;
        let job = tokio::task::spawn_blocking(move || {
            let out = f(rec.as_mut());
            (rec, out)
        });
        match tokio::time::timeout(self.s.limits.decode_timeout, job).await {
            Ok(Ok((rec, out))) => {
                self.rec = Some(rec);
                out.map_err(|e| Fatal::Engine(format!("ошибка распознавания: {e}")))
            }
            Ok(Err(_)) => Err(Fatal::Engine("сбой распознавателя".into())),
            Err(_) => Err(Fatal::Engine("таймаут распознавания".into())),
        }
    }

    fn max_samples(&self) -> usize {
        (self.s.limits.max_utterance.as_secs_f64() * SAMPLE_RATE as f64) as usize
    }

    async fn audio(&mut self, socket: &mut WebSocket, bytes: &[u8]) -> Result<(), Fatal> {
        let mut pcm = Vec::with_capacity(bytes.len() / 2 + 1);
        self.dec.push(bytes, &mut pcm);
        let mut rest = pcm.as_slice();
        while !rest.is_empty() {
            let room = self.max_samples().saturating_sub(self.samples).max(1);
            let (now, later) = rest.split_at(room.min(rest.len()));
            rest = later;
            let chunk = now.to_vec();
            self.samples += chunk.len();
            let partial = self.blocking(move |r| r.accept(&chunk)).await?;
            if !partial.is_empty() && partial != self.last_partial {
                send(socket, &ServerMsg::Partial { text: partial.clone() }).await?;
                self.s.metrics.partials_total.fetch_add(1, Relaxed);
                self.last_partial = partial;
            }
            if self.samples >= self.max_samples() {
                tracing::info!(session = self.id, "фраза достигла предела длины: final без end");
                self.end(socket).await?;
            }
        }
        Ok(())
    }

    async fn end(&mut self, socket: &mut WebSocket) -> Result<(), Fatal> {
        let started = Instant::now();
        let text = self.blocking(|r| r.finish()).await?;
        let duration_ms = samples_ms(self.samples);
        send(socket, &ServerMsg::Final { text: text.clone(), duration_ms }).await?;
        let latency = started.elapsed();
        self.s.metrics.final_done(latency, duration_ms);
        self.phrases += 1;
        tracing::info!(
            session = self.id,
            audio_ms = duration_ms,
            final_ms = latency.as_millis() as u64,
            chars = text.chars().count(),
            "final"
        );
        tracing::debug!(session = self.id, %text, "final text");
        self.clear();
        Ok(())
    }

    async fn reset(&mut self) -> Result<(), Fatal> {
        self.blocking(|r| {
            r.reset();
            Ok(())
        })
        .await?;
        self.clear();
        Ok(())
    }

    fn clear(&mut self) {
        self.samples = 0;
        self.last_partial.clear();
        self.dec.clear();
    }

    async fn handle(&mut self, socket: &mut WebSocket, msg: Message) -> Result<bool, Fatal> {
        match msg {
            Message::Binary(b) => self.audio(socket, &b).await?,
            Message::Text(t) => match serde_json::from_str::<ClientMsg>(t.as_str()) {
                Ok(ClientMsg::End) => self.end(socket).await?,
                Ok(ClientMsg::Reset) => self.reset().await?,
                Err(_) => {
                    send(socket, &ServerMsg::Error { message: "неизвестное сообщение; ожидается {\"type\":\"end\"|\"reset\"}".into() })
                        .await?
                }
            },
            Message::Close(_) => return Ok(false),
            Message::Ping(_) | Message::Pong(_) => {}
        }
        Ok(true)
    }
}

async fn close(socket: &mut WebSocket, code: u16, reason: &str) {
    let _ = socket.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
}

async fn run_session(mut socket: WebSocket, s: AppState, _permit: OwnedSemaphorePermit) {
    let id = s.metrics.sessions_total.fetch_add(1, Relaxed) + 1;
    s.metrics.sessions_active.fetch_add(1, Relaxed);
    let _active = ActiveGuard(s.metrics.clone());
    let opened = Instant::now();
    tracing::info!(session = id, "WS: подключение");

    let engine = s.engine.clone();
    let rec = match tokio::task::spawn_blocking(move || engine.open()).await {
        Ok(Ok(r)) => r,
        other => {
            let msg = match other {
                Ok(Err(e)) => format!("не удалось открыть сессию: {e}"),
                _ => "сбой при открытии сессии".into(),
            };
            s.metrics.errors_total.fetch_add(1, Relaxed);
            tracing::warn!(session = id, %msg);
            let _ = send(&mut socket, &ServerMsg::Error { message: msg }).await;
            close(&mut socket, close_code::ERROR, "engine").await;
            return;
        }
    };
    if send(&mut socket, &ServerMsg::Ready { model: s.engine.model().to_string() }).await.is_err() {
        return;
    }
    let mut session = Session {
        id,
        s: s.clone(),
        rec: Some(rec),
        dec: PcmDecoder::default(),
        samples: 0,
        last_partial: String::new(),
        phrases: 0,
    };
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + s.limits.ping_interval, s.limits.ping_interval);
    let mut deadline = tokio::time::Instant::now() + s.limits.idle_timeout;
    let reason = loop {
        tokio::select! {
            msg = socket.recv() => {
                deadline = tokio::time::Instant::now() + s.limits.idle_timeout;
                let Some(Ok(msg)) = msg else { break "клиент отключился" };
                match session.handle(&mut socket, msg).await {
                    Ok(true) => {}
                    Ok(false) => break "клиент закрыл соединение",
                    Err(Fatal::Closed) => break "клиент отключился",
                    Err(Fatal::Engine(message)) => {
                        s.metrics.errors_total.fetch_add(1, Relaxed);
                        tracing::warn!(session = id, %message, "WS: ошибка");
                        let _ = send(&mut socket, &ServerMsg::Error { message }).await;
                        close(&mut socket, close_code::ERROR, "engine").await;
                        break "ошибка движка";
                    }
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    break "ping не отправлен";
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                close(&mut socket, close_code::AWAY, "idle timeout").await;
                break "таймаут простоя";
            }
        }
    };
    tracing::info!(session = id, phrases = session.phrases, secs = opened.elapsed().as_secs(), reason, "WS: закрыто");
}

// ---------------------------------------------------------------- запуск

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Сообщить systemd (`Type=notify`), что сокет открыт: front-сокет не пустит клиентов раньше.
fn notify_systemd_ready() {
    #[cfg(target_os = "linux")]
    {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::{SocketAddr, UnixDatagram};
        let Some(path) = std::env::var_os("NOTIFY_SOCKET") else { return };
        let path = path.to_string_lossy().into_owned();
        let addr = match path.strip_prefix('@') {
            Some(name) => SocketAddr::from_abstract_name(name.as_bytes()),
            None => SocketAddr::from_pathname(&path),
        };
        let sent = addr.and_then(|a| UnixDatagram::unbound()?.send_to_addr(b"READY=1", &a));
        if let Err(e) = sent {
            tracing::warn!(error = %e, "sd_notify READY=1 не отправлен");
        }
    }
}

/// Запуск HTTPS (или HTTP на loopback без TLS). Завершается по SIGTERM/Ctrl+C (≤2 с на закрытие).
pub async fn serve(
    bind: std::net::SocketAddr,
    tls: Option<(std::path::PathBuf, std::path::PathBuf)>,
    state: AppState,
) -> anyhow::Result<()> {
    let app = router(state).into_make_service();
    let handle = axum_server::Handle::new();
    let h = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("остановка");
        h.graceful_shutdown(Some(Duration::from_secs(2)));
    });
    let h = handle.clone();
    tokio::spawn(async move {
        if h.listening().await.is_some() {
            notify_systemd_ready();
        }
    });
    if let Some((cert, key)) = tls {
        rustls::crypto::ring::default_provider().install_default().ok();
        let cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
        tracing::info!(%bind, "HTTPS готов");
        axum_server::bind_rustls(bind, cfg).handle(handle).serve(app).await?;
    } else {
        tracing::info!(%bind, "HTTP готов (без TLS, только loopback)");
        axum_server::bind(bind).handle(handle).serve(app).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_types() {
        let pcm = audio::f32_to_s16le(&vec![0.25; 1600]);
        assert_eq!(decode_upload(Some("audio/L16; rate=16000"), &pcm).unwrap().len(), 1600);
        assert_eq!(decode_upload(Some("audio/l16;rate=8000;channels=1"), &pcm).unwrap().len(), 3200);
        assert_eq!(decode_upload(Some("audio/L16; rate=16000; channels=2"), &pcm).unwrap().len(), 800);
        assert_eq!(decode_upload(None, &pcm).unwrap().len(), 1600);
        assert_eq!(decode_upload(Some("application/octet-stream"), &pcm).unwrap().len(), 1600);
        let wav = audio::write_wav_pcm16(&vec![0.1; 4800], 48000);
        assert_eq!(decode_upload(Some("audio/wav"), &wav).unwrap().len(), 1600);
        // WAV по сигнатуре даже с общим типом
        assert_eq!(decode_upload(Some("application/octet-stream"), &wav).unwrap().len(), 1600);
        assert!(matches!(decode_upload(Some("text/plain"), &pcm), Err(UploadError::UnsupportedType(_))));
        assert_eq!(decode_upload(Some("audio/L16"), &[1, 2, 3]), Err(UploadError::Audio(AudioError::OddPcmLength)));
        assert!(matches!(decode_upload(Some("audio/L16; rate=abc"), &pcm), Err(UploadError::Audio(_))));
        assert!(matches!(decode_upload(Some("audio/L16; rate=100"), &pcm), Err(UploadError::Audio(_))));
        assert!(matches!(decode_upload(Some("audio/wav"), b"not a wav at all"), Err(UploadError::Audio(_))));
    }
}
