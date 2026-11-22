//! STT-клиенты против локальных моков: WS `/stt/stream` и `POST /stt` строго по `jarvis_core::stt_protocol`,
//! Groq `/audio/transcriptions` (multipart).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jarvis_core::config::SttConfig;
use jarvis_core::stt_protocol::{ClientMsg, Health, ServerMsg, SttResult};
use jarvis_core::CancellationToken;
use jarvis_voice::stt::{GroqWhisperStt, ServerStt, ServerSttOptions, SttEngine, SttEvent};
use jarvis_voice::VoiceError;
use tokio::sync::mpsc;

#[derive(Clone, Default)]
struct Mock {
    connections: Arc<AtomicUsize>,
    /// Первое соединение рвётся после первого кадра звука.
    drop_first: bool,
    content_types: Arc<Mutex<Vec<String>>>,
}

async fn send(ws: &mut WebSocket, m: &ServerMsg) -> bool {
    ws.send(Message::Text(serde_json::to_string(m).unwrap().into())).await.is_ok()
}

async fn handle(mut ws: WebSocket, mock: Mock) {
    let n = mock.connections.fetch_add(1, Ordering::SeqCst);
    if !send(&mut ws, &ServerMsg::Ready { model: "mock-ru".into() }).await {
        return;
    }
    let mut samples = 0usize;
    while let Some(Ok(msg)) = ws.recv().await {
        match msg {
            Message::Binary(b) => {
                samples += b.len() / 2;
                if mock.drop_first && n == 0 {
                    return; // обрыв соединения посреди фразы
                }
                send(&mut ws, &ServerMsg::Partial { text: format!("{samples}") }).await;
            }
            Message::Text(t) => match serde_json::from_str::<ClientMsg>(t.as_str()) {
                Ok(ClientMsg::End) => {
                    let msg = ServerMsg::Final { text: format!("сэмплов {samples}"), duration_ms: (samples / 16) as u64 };
                    send(&mut ws, &msg).await;
                    samples = 0;
                }
                Ok(ClientMsg::Reset) => samples = 0,
                Err(_) => {
                    send(&mut ws, &ServerMsg::Error { message: "bad".into() }).await;
                }
            },
            _ => {}
        }
    }
}

async fn ws_route(ws: WebSocketUpgrade, State(m): State<Mock>) -> Response {
    ws.on_upgrade(move |s| handle(s, m))
}

async fn post_stt(State(m): State<Mock>, headers: HeaderMap, body: Bytes) -> Json<SttResult> {
    let ct = headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    m.content_types.lock().unwrap().push(ct);
    Json(SttResult { text: format!("байт {}", body.len()), duration_ms: body.len() as u64 / 32 })
}

async fn health() -> Json<Health> {
    Json(Health { ok: true, model: "mock-ru".into(), sample_rate: 16_000 })
}

async fn start(mock: Mock) -> (ServerStt, Mock) {
    let app = Router::new()
        .route("/stt/stream", get(ws_route))
        .route("/stt", post(post_stt))
        .route("/health", get(health))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = SttConfig { engine: "server".into(), url: format!("http://{addr}"), ca_path: "нет.pem".into() };
    let opts = ServerSttOptions { reconnect_min: Duration::from_millis(20), ..Default::default() };
    (ServerStt::with_options(&cfg, std::path::Path::new("."), opts).unwrap(), mock)
}

async fn next_final(rx: &mut mpsc::Receiver<SttEvent>, seen: &mut Vec<SttEvent>) -> (String, u64) {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("таймаут STT").expect("канал");
        seen.push(ev.clone());
        if let SttEvent::Final { text, duration_ms } = ev {
            return (text, duration_ms);
        }
    }
}

#[tokio::test]
async fn ws_stream_partial_final_reset() {
    let (stt, mock) = start(Mock::default()).await;
    let (tx, mut rx) = mpsc::channel(64);
    let s = stt.open_stream(tx).expect("сервер умеет поток");
    let frame = vec![100i16; 256];
    for _ in 0..4 {
        assert!(s.send_audio(&frame));
    }
    s.end();
    let mut seen = Vec::new();
    assert_eq!(next_final(&mut rx, &mut seen).await, ("сэмплов 1024".into(), 64));
    assert_eq!(seen.first(), Some(&SttEvent::Ready { model: "mock-ru".into() }));
    assert!(seen.iter().any(|e| matches!(e, SttEvent::Partial { .. })));
    // Reset сбрасывает фразу; следующая фраза в том же соединении.
    s.send_audio(&frame);
    s.reset();
    s.send_audio(&frame[..100]);
    s.end();
    assert_eq!(next_final(&mut rx, &mut seen).await.0, "сэмплов 100");
    assert_eq!(mock.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ws_reconnects_and_replays_phrase() {
    let (stt, mock) = start(Mock { drop_first: true, ..Default::default() }).await;
    let (tx, mut rx) = mpsc::channel(64);
    let s = stt.stream(tx);
    let frame = vec![7i16; 256];
    for _ in 0..3 {
        s.send_audio(&frame);
    }
    s.end();
    let mut seen = Vec::new();
    let (text, _) = next_final(&mut rx, &mut seen).await;
    assert_eq!(text, "сэмплов 768", "фраза дослана целиком после переподключения");
    assert!(seen.iter().any(|e| matches!(e, SttEvent::Disconnected { .. })), "{seen:?}");
    assert_eq!(mock.connections.load(Ordering::SeqCst), 2);
    drop(s);
}

#[tokio::test]
async fn ws_waits_for_server_and_buffers() {
    // Сервера ещё нет: звук копится, после появления сервера фраза распознаётся.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let cfg = SttConfig { engine: "server".into(), url: format!("http://{addr}"), ca_path: String::new() };
    let opts = ServerSttOptions { reconnect_min: Duration::from_millis(20), reconnect_max: Duration::from_millis(50), ..Default::default() };
    let stt = ServerStt::with_options(&cfg, std::path::Path::new("."), opts).unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    let s = stt.stream(tx);
    s.send_audio(&[1i16; 512]);
    s.end();
    let first = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
    assert!(matches!(first, SttEvent::Disconnected { .. }), "{first:?}");
    let mock = Mock::default();
    let app = Router::new().route("/stt/stream", get(ws_route)).with_state(mock);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut seen = Vec::new();
    assert_eq!(next_final(&mut rx, &mut seen).await.0, "сэмплов 512");
}

#[tokio::test]
async fn post_stt_and_health() {
    let (stt, mock) = start(Mock::default()).await;
    let h = stt.health().await.unwrap();
    assert!(h.ok && h.sample_rate == 16_000);
    let r = stt.transcribe(&vec![0i16; 1600], &CancellationToken::new()).await.unwrap();
    assert_eq!(r, SttResult { text: "байт 3200".into(), duration_ms: 100 });
    assert_eq!(mock.content_types.lock().unwrap().as_slice(), ["audio/L16; rate=16000"]);
    let t = CancellationToken::new();
    t.cancel();
    assert_eq!(stt.transcribe(&[0; 10], &t).await.err(), Some(VoiceError::Cancelled));
}

#[derive(Clone, Default)]
struct GroqMock {
    fields: Arc<Mutex<Vec<(String, String)>>>,
    auth: Arc<Mutex<String>>,
}

async fn groq_route(State(m): State<GroqMock>, headers: HeaderMap, mut mp: Multipart) -> Response {
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    *m.auth.lock().unwrap() = auth.clone();
    if auth != "Bearer gsk_test_key" {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": {"message": "Invalid API Key"}}))).into_response();
    }
    while let Some(field) = mp.next_field().await.unwrap() {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            let meta = format!("{}|{}", field.file_name().unwrap_or_default(), field.content_type().unwrap_or_default());
            let data = field.bytes().await.unwrap();
            let riff = &data[..4] == b"RIFF" && &data[8..12] == b"WAVE";
            m.fields.lock().unwrap().push((name, format!("{meta}|riff={riff}|{}", data.len())));
        } else {
            let v = field.text().await.unwrap();
            m.fields.lock().unwrap().push((name, v));
        }
    }
    Json(serde_json::json!({"text": "  Джарвис, открой браузер. ", "x_groq": {"id": "req_1"}})).into_response()
}

#[tokio::test]
async fn groq_whisper_multipart() {
    let mock = GroqMock::default();
    let app = Router::new().route("/openai/v1/audio/transcriptions", post(groq_route)).with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{addr}/openai/v1");

    let mut cfg = jarvis_core::Config::default();
    cfg.providers[0].base_url = base.clone();
    cfg.providers[0].api_key = "gsk_test_key".into();
    let stt = GroqWhisperStt::from_config(&cfg).unwrap();
    assert_eq!(stt.name(), "groq_whisper");
    let r = stt.transcribe(&vec![0i16; 16_000], &CancellationToken::new()).await.unwrap();
    assert_eq!(r, SttResult { text: "Джарвис, открой браузер.".into(), duration_ms: 1000 });
    let f = mock.fields.lock().unwrap().clone();
    let get = |k: &str| f.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    assert_eq!(get("model"), "whisper-large-v3-turbo");
    assert_eq!(get("language"), "ru");
    assert_eq!(get("response_format"), "json");
    assert_eq!(get("file"), format!("audio.wav|audio/wav|riff=true|{}", 44 + 32_000));

    let bad = GroqWhisperStt::new(&base, "wrong").unwrap();
    assert_eq!(bad.transcribe(&vec![0i16; 1600], &CancellationToken::new()).await.err(), Some(VoiceError::Auth(401)));
    cfg.providers[0].api_key.clear();
    assert!(matches!(GroqWhisperStt::from_config(&cfg), Err(VoiceError::Config(_))));
}
