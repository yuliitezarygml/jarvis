//! Клиент STT-сервиса `server/jarvis-stt-rs` строго по `jarvis_core::stt_protocol`:
//! `GET /health`, `POST /stt` (PCM `audio/L16; rate=16000`), `WS /stt/stream` (бинарные кадры PCM + `{"type":"end"|"reset"}`).
//! Потоковое соединение живёт долго; при обрыве переподключается с экспоненциальной паузой
//! и досылает звук текущей фразы (с последнего `end`/`reset`), чтобы фраза не потерялась.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use jarvis_core::CancellationToken;
use jarvis_core::config::SttConfig;
use jarvis_core::stt_protocol::{ClientMsg, Health, STREAM_PATH, SttResult, ServerMsg};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use super::{SttEngine, SttEvent, SttStream, StreamCmd};
use crate::error::{Result, VoiceError};
use crate::pcm::{STT_SAMPLE_RATE, i16_to_le_bytes};
use crate::tls;
use crate::tts::check_status;

#[derive(Debug, Clone)]
pub struct ServerSttOptions {
    pub connect_timeout: Duration,
    /// `POST /stt` целиком.
    pub request_timeout: Duration,
    pub reconnect_min: Duration,
    pub reconnect_max: Duration,
    /// Сколько звука текущей фразы держать для досылки после переподключения.
    pub max_buffer_secs: u32,
    pub ping_interval: Duration,
}

impl Default for ServerSttOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(3),
            request_timeout: Duration::from_secs(20),
            reconnect_min: Duration::from_millis(250),
            reconnect_max: Duration::from_secs(5),
            max_buffer_secs: 60,
            ping_interval: Duration::from_secs(15),
        }
    }
}

#[derive(Clone)]
pub struct ServerStt {
    base: String,
    http: reqwest::Client,
    ws_tls: Option<Arc<rustls::ClientConfig>>,
    opts: ServerSttOptions,
}

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

impl ServerStt {
    pub fn new(cfg: &SttConfig, project_root: &Path) -> Result<Self> {
        Self::with_options(cfg, project_root, ServerSttOptions::default())
    }

    pub fn with_options(cfg: &SttConfig, project_root: &Path, opts: ServerSttOptions) -> Result<Self> {
        let base = cfg.url.trim_end_matches('/').to_string();
        if !base.starts_with("https://") && !base.starts_with("http://") {
            return Err(VoiceError::Config(format!("адрес STT должен начинаться с https://: {base}")));
        }
        let ca = tls::resolve_path(project_root, &cfg.ca_path);
        let http = tls::pinned_http_client(&base, &ca, opts.connect_timeout)?;
        // Для WebSocket — отдельный конфиг без ALPN (апгрейд идёт по HTTP/1.1).
        let ws_tls = if base.starts_with("https://") { Some(Arc::new(tls::pinned_config(&ca)?)) } else { None };
        Ok(Self { base, http, ws_tls, opts })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn stream_url(&self) -> String {
        tls::to_ws_url(&self.base, STREAM_PATH)
    }

    pub async fn health(&self) -> Result<Health> {
        let resp = self.http.get(format!("{}/health", self.base)).timeout(self.opts.connect_timeout * 2).send().await?;
        let resp = check_status(resp).await?;
        resp.json::<Health>().await.map_err(|e| VoiceError::BadResponse(e.to_string()))
    }

    /// `POST /stt` с сырым PCM (резервный путь, если поток недоступен).
    pub async fn transcribe_once(&self, pcm16k: &[i16], cancel: &CancellationToken) -> Result<SttResult> {
        let body = i16_to_le_bytes(pcm16k);
        self.post(body, "audio/L16; rate=16000", cancel).await
    }

    /// `POST /stt` с WAV.
    pub async fn transcribe_wav(&self, wav: Vec<u8>, cancel: &CancellationToken) -> Result<SttResult> {
        self.post(wav, "audio/wav", cancel).await
    }

    async fn post(&self, body: Vec<u8>, content_type: &str, cancel: &CancellationToken) -> Result<SttResult> {
        let send = self
            .http
            .post(format!("{}/stt", self.base))
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .timeout(self.opts.request_timeout)
            .body(body)
            .send();
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(VoiceError::Cancelled),
            r = send => r?,
        };
        let resp = check_status(resp).await?;
        resp.json::<SttResult>().await.map_err(|e| VoiceError::BadResponse(e.to_string()))
    }

    /// Открыть долгоживущую потоковую сессию (сама переподключается).
    pub fn stream(&self, events: mpsc::Sender<SttEvent>) -> SttStream {
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run_stream(self.stream_url(), self.ws_tls.clone(), self.opts.clone(), rx, events));
        SttStream::new(tx, task)
    }
}

#[async_trait]
impl SttEngine for ServerStt {
    fn name(&self) -> &str {
        "server"
    }

    async fn transcribe(&self, pcm16k: &[i16], cancel: &CancellationToken) -> Result<SttResult> {
        self.transcribe_once(pcm16k, cancel).await
    }

    fn open_stream(&self, events: mpsc::Sender<SttEvent>) -> Option<SttStream> {
        Some(self.stream(events))
    }
}

async fn connect(url: &str, tls: Option<Arc<rustls::ClientConfig>>, timeout: Duration) -> Result<Ws> {
    let connector = tls.map(Connector::Rustls);
    let fut = tokio_tungstenite::connect_async_tls_with_config(url, None, true, connector);
    match tokio::time::timeout(timeout, fut).await {
        Err(_) => Err(VoiceError::Timeout("подключение к STT")),
        Ok(Err(e)) => Err(VoiceError::Network(format!("WebSocket {url}: {e}"))),
        Ok(Ok((ws, _resp))) => Ok(ws),
    }
}

/// Состояние фразы, переживающее переподключения.
struct Phrase {
    buf: Vec<i16>,
    pending_end: bool,
    max: usize,
}

impl Phrase {
    fn push(&mut self, pcm: &[i16]) {
        self.buf.extend_from_slice(pcm);
        if self.buf.len() > self.max {
            let drop = self.buf.len() - self.max;
            self.buf.drain(..drop);
        }
    }
    /// Применить команду без соединения. `false` — сессию закрыли.
    fn apply_offline(&mut self, cmd: Option<StreamCmd>) -> bool {
        match cmd {
            None | Some(StreamCmd::Close) => return false,
            Some(StreamCmd::Audio(pcm)) => self.push(&pcm),
            Some(StreamCmd::End) => self.pending_end = true,
            Some(StreamCmd::Reset) => {
                self.buf.clear();
                self.pending_end = false;
            }
        }
        true
    }
}

fn text_msg(m: &ClientMsg) -> Message {
    Message::Text(serde_json::to_string(m).unwrap_or_default().into())
}

/// Подождать `dur`, продолжая принимать команды. `false` — сессию закрыли.
async fn idle(dur: Duration, rx: &mut mpsc::UnboundedReceiver<StreamCmd>, phrase: &mut Phrase) -> bool {
    let sleep = tokio::time::sleep(dur);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return true,
            cmd = rx.recv() => if !phrase.apply_offline(cmd) { return false },
        }
    }
}

async fn emit(events: &mpsc::Sender<SttEvent>, ev: SttEvent) -> bool {
    events.send(ev).await.is_ok()
}

async fn run_stream(
    url: String,
    tls: Option<Arc<rustls::ClientConfig>>,
    opts: ServerSttOptions,
    mut rx: mpsc::UnboundedReceiver<StreamCmd>,
    events: mpsc::Sender<SttEvent>,
) {
    let mut phrase = Phrase { buf: Vec::new(), pending_end: false, max: (opts.max_buffer_secs * STT_SAMPLE_RATE) as usize };
    let mut backoff = opts.reconnect_min;
    let mut down_reported = false;
    loop {
        // Подключаемся, не переставая принимать звук.
        let conn = connect(&url, tls.clone(), opts.connect_timeout);
        tokio::pin!(conn);
        let res = loop {
            tokio::select! {
                r = &mut conn => break r,
                cmd = rx.recv() => if !phrase.apply_offline(cmd) { return },
            }
        };
        let mut ws = match res {
            Ok(ws) => ws,
            Err(e) => {
                if !down_reported {
                    down_reported = true;
                    if !emit(&events, SttEvent::Disconnected { reason: e.to_string() }).await {
                        return;
                    }
                }
                if !idle(backoff, &mut rx, &mut phrase).await {
                    return;
                }
                backoff = (backoff * 2).min(opts.reconnect_max);
                continue;
            }
        };
        backoff = opts.reconnect_min;
        let reason = match session(&mut ws, &mut rx, &mut phrase, &events, &opts).await {
            SessionEnd::Closed => {
                let _ = ws.close(None).await;
                return;
            }
            SessionEnd::Lost(reason) => reason,
        };
        down_reported = true;
        if !emit(&events, SttEvent::Disconnected { reason }).await {
            return;
        }
        if !idle(backoff, &mut rx, &mut phrase).await {
            return;
        }
    }
}

enum SessionEnd {
    Closed,
    Lost(String),
}

async fn session(
    ws: &mut Ws,
    rx: &mut mpsc::UnboundedReceiver<StreamCmd>,
    phrase: &mut Phrase,
    events: &mpsc::Sender<SttEvent>,
    opts: &ServerSttOptions,
) -> SessionEnd {
    let lost = |e: &dyn std::fmt::Display| SessionEnd::Lost(format!("STT: {e}"));
    // Досылка звука фразы, начатой до обрыва (кусками по 100 мс).
    for chunk in phrase.buf.chunks((STT_SAMPLE_RATE / 10) as usize) {
        if let Err(e) = ws.send(Message::Binary(Bytes::from(i16_to_le_bytes(chunk)))).await {
            return lost(&e);
        }
    }
    if phrase.pending_end
        && let Err(e) = ws.send(text_msg(&ClientMsg::End)).await
    {
        return lost(&e);
    }
    let mut ping = tokio::time::interval(opts.ping_interval);
    ping.tick().await;
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let r = match cmd {
                    None | Some(StreamCmd::Close) => return SessionEnd::Closed,
                    Some(StreamCmd::Audio(pcm)) => {
                        phrase.push(&pcm);
                        ws.send(Message::Binary(Bytes::from(i16_to_le_bytes(&pcm)))).await
                    }
                    Some(StreamCmd::End) => {
                        phrase.pending_end = true;
                        ws.send(text_msg(&ClientMsg::End)).await
                    }
                    Some(StreamCmd::Reset) => {
                        phrase.buf.clear();
                        phrase.pending_end = false;
                        ws.send(text_msg(&ClientMsg::Reset)).await
                    }
                };
                if let Err(e) = r {
                    return lost(&e);
                }
            }
            msg = ws.next() => {
                let ev = match msg {
                    Some(Ok(Message::Text(t))) => match serde_json::from_str::<ServerMsg>(t.as_str()) {
                        Ok(ServerMsg::Ready { model }) => SttEvent::Ready { model },
                        Ok(ServerMsg::Partial { text }) => SttEvent::Partial { text },
                        Ok(ServerMsg::Final { text, duration_ms }) => {
                            phrase.buf.clear();
                            phrase.pending_end = false;
                            SttEvent::Final { text, duration_ms }
                        }
                        Ok(ServerMsg::Error { message }) => SttEvent::Error { message },
                        Err(e) => SttEvent::Error { message: format!("непонятное сообщение STT: {e}") },
                    },
                    Some(Ok(Message::Close(frame))) => {
                        return SessionEnd::Lost(format!("сервер закрыл соединение {frame:?}"));
                    }
                    Some(Ok(_)) => continue, // ping/pong/binary
                    Some(Err(e)) => return lost(&e),
                    None => return SessionEnd::Lost("соединение закрыто".into()),
                };
                if !emit(events, ev).await {
                    return SessionEnd::Closed;
                }
            }
            _ = ping.tick() => {
                if let Err(e) = ws.send(Message::Ping(Bytes::new())).await {
                    return lost(&e);
                }
            }
        }
    }
}
