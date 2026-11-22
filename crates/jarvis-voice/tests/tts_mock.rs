//! TtsClient против локального мок-сервера с тем же API, что `server/jarvis-tts-rs/src/api.rs`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use jarvis_core::CancellationToken;
use jarvis_core::config::TtsConfig;
use jarvis_voice::VoiceError;
use jarvis_voice::tts::{TtsClient, TtsEngine};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Mock {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    deletes: Arc<Mutex<Vec<String>>>,
}

async fn ready() -> Json<Value> {
    Json(json!({"ready": true, "voice": "jarvis-ru", "sample_rate": 24000}))
}

async fn stream(State(m): State<Mock>, headers: HeaderMap, Json(v): Json<Value>) -> Response {
    let id = headers.get("x-request-id").and_then(|h| h.to_str().ok()).unwrap_or_default().to_string();
    m.requests.lock().unwrap().push((id.clone(), v.clone()));
    let text = v["text"].as_str().unwrap_or_default().to_string();
    if text == "ошибка" {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": "invalid_input"}))).into_response();
    }
    // Куски нечётной длины: сэмпл режется на границе.
    let chunks = vec![Ok::<_, std::io::Error>(Bytes::from_static(&[1, 0, 2])), Ok(Bytes::from_static(&[0, 3, 0]))];
    let body = if text.contains("медленно") {
        Body::from_stream(futures_util::stream::iter(chunks).chain(futures_util::stream::pending()))
    } else {
        Body::from_stream(futures_util::stream::iter(chunks))
    };
    Response::builder()
        .header("Content-Type", "application/octet-stream")
        .header("X-Request-Id", id)
        .header("X-Audio-Format", "s16le;rate=24000;channels=1")
        .body(body)
        .unwrap()
}

async fn cancel(State(m): State<Mock>, Path(id): Path<String>) -> StatusCode {
    m.deletes.lock().unwrap().push(id);
    StatusCode::ACCEPTED
}

async fn start() -> (TtsClient, Mock) {
    let mock = Mock::default();
    let app = Router::new()
        .route("/ready", get(ready))
        .route("/tts/stream", post(stream))
        .route("/tts/{id}", delete(cancel))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cfg = TtsConfig { url: format!("http://{addr}"), voice: "jarvis-ru".into(), ca_path: "нет.pem".into() };
    (TtsClient::new(&cfg, std::path::Path::new(".")).unwrap(), mock)
}

async fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn ready_and_full_stream() {
    let (c, mock) = start().await;
    let r = c.ready().await.unwrap();
    assert!(r.ready);
    assert_eq!((r.voice.as_str(), r.sample_rate), ("jarvis-ru", 24000));

    let s = c.synthesize_stream("  Привет, сэр.  ", None, CancellationToken::new()).await.unwrap();
    let id = s.request_id.clone();
    assert_eq!(s.sample_rate, 24000);
    assert_eq!(s.collect_all().await.unwrap(), vec![1, 0, 2, 0, 3, 0]);
    let reqs = mock.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].0, id);
    assert!(id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric()), "формат id: {id}");
    // Ровно два поля: сервер с deny_unknown_fields отверг бы лишнее.
    assert_eq!(reqs[0].1, json!({"text": "Привет, сэр.", "voice": "jarvis-ru"}));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(mock.deletes.lock().unwrap().is_empty(), "дочитанный поток не отменяется");
}

#[tokio::test]
async fn token_cancel_interrupts_stream_and_deletes_on_server() {
    let (c, mock) = start().await;
    let token = CancellationToken::new();
    let mut s = c.synthesize_stream("говори медленно", Some("jarvis-ru"), token.clone()).await.unwrap();
    let id = s.request_id.clone();
    assert!(s.next().await.unwrap().is_ok());
    assert!(s.next().await.unwrap().is_ok());
    let t = Instant::now();
    let c2 = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c2.cancel();
    });
    assert_eq!(s.next().await.unwrap().err(), Some(VoiceError::Cancelled));
    assert!(t.elapsed() < Duration::from_millis(500), "барж-ин должен быть мгновенным");
    assert!(s.next().await.is_none());
    assert!(wait_for(|| mock.deletes.lock().unwrap().contains(&id)).await, "DELETE /tts/{{id}} не пришёл");
}

#[tokio::test]
async fn dropped_stream_is_cancelled_on_server() {
    let (c, mock) = start().await;
    let mut s = c.synthesize_stream("медленно и долго", None, CancellationToken::new()).await.unwrap();
    let id = s.request_id.clone();
    let _ = s.next().await;
    drop(s);
    assert!(wait_for(|| mock.deletes.lock().unwrap().contains(&id)).await);
}

#[tokio::test]
async fn server_error_and_explicit_cancel() {
    let (c, mock) = start().await;
    match c.synthesize_stream("ошибка", None, CancellationToken::new()).await {
        Err(VoiceError::Http { status: 422, body }) => assert!(body.contains("invalid_input")),
        other => panic!("ожидалась 422: {:?}", other.err()),
    }
    assert!(TtsEngine::cancel(&c, "abc").await.unwrap());
    assert_eq!(mock.deletes.lock().unwrap().as_slice(), ["abc"]);
}

#[tokio::test]
async fn connection_refused_is_network_error() {
    let cfg = TtsConfig { url: "http://127.0.0.1:9".into(), ..Default::default() };
    let c = TtsClient::new(&cfg, std::path::Path::new(".")).unwrap();
    let e = c.ready().await.unwrap_err();
    assert!(matches!(e, VoiceError::Network(_)), "{e:?}");
    assert!(e.is_transient());
}
