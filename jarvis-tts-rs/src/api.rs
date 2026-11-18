use crate::{
    audio,
    scheduler::{Pool, SubmitError, Ticket},
    text,
};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::json;
use std::{sync::atomic::Ordering::Relaxed, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Clone)]
pub struct StateData {
    pub pool: Pool,
    pub secret: Option<String>,
}
pub fn router(state: StateData) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({"ok":true})) }))
        .route(
            "/ready",
            get(|| async {
                Json(json!({"ready":true,"voice":"jarvis-ru","sample_rate":audio::HZ}))
            }),
        )
        .route("/metrics", get(metrics))
        .route("/tts", post(buffered))
        .route("/tts/stream", post(streaming))
        .route("/tts/{id}", delete(cancel))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}
struct ApiError(StatusCode, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
type Reply = Result<Response, ApiError>;
fn invalid() -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_input")
}
async fn auth(State(s): State<StateData>, req: Request, next: Next) -> Response {
    if let Some(key) = &s.secret {
        let supplied = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if !bool::from(supplied.as_bytes().ct_eq(key.as_bytes())) {
            return ApiError(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    }
    next.run(req).await
}
async fn metrics(State(s): State<StateData>) -> String {
    let m = s.pool.metrics;
    format!(
        "jarvis_jobs_total {}\njarvis_failed_total {}\njarvis_queue_microseconds_total {}\njarvis_load_milliseconds {}\n",
        m.jobs.load(Relaxed),
        m.failed.load(Relaxed),
        m.queue_us.load(Relaxed),
        m.load_ms.load(Relaxed)
    )
}
async fn cancel(State(s): State<StateData>, Path(id): Path<String>) -> StatusCode {
    if s.pool.cancel(&id) {
        StatusCode::ACCEPTED
    } else {
        StatusCode::NOT_FOUND
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Utterance {
    text: String,
    lang: Option<String>,
    voice: Option<String>,
    format: Option<String>,
    speed: Option<f32>,
}
async fn buffered(State(s): State<StateData>, req: Request) -> Reply {
    speech(s, req, false).await
}
async fn streaming(State(s): State<StateData>, req: Request) -> Reply {
    speech(s, req, true).await
}
fn request_id(headers: &HeaderMap) -> Result<String, ApiError> {
    match headers.get("x-request-id") {
        None => Ok(Uuid::new_v4().to_string()),
        Some(v) => {
            let id = v.to_str().map_err(|_| invalid())?;
            if id.is_empty()
                || id.len() > 64
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
            {
                return Err(invalid());
            }
            Ok(id.into())
        }
    }
}
async fn speech(s: StateData, req: Request, streaming: bool) -> Reply {
    static READERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(12);
    let _admission = READERS
        .try_acquire()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "queue_full"))?;
    let headers = req.headers();
    let id = request_id(headers)?;
    let plain = headers
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(';').next())
        == Some("text/plain");
    let (tx, rx) = mpsc::channel(2);
    if plain && !streaming {
        return Err(invalid());
    }
    let mut body = Some(req.into_body());
    let mut speed = 1.0;
    let mut mp3 = false;
    if !plain {
        let buf = tokio::time::timeout(
            Duration::from_secs(5),
            to_bytes(body.take().unwrap(), 16384),
        )
        .await
        .map_err(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "input_timeout"))?
        .map_err(|_| ApiError(StatusCode::PAYLOAD_TOO_LARGE, "input_too_large"))?;
        let u: Utterance = serde_json::from_slice(&buf).map_err(|_| invalid())?;
        if !text::valid(&u.text)
            || u.voice.as_deref().unwrap_or("jarvis-ru") != "jarvis-ru"
            || u.lang.as_deref().unwrap_or("ru") != "ru"
            || !(0.6..=1.6).contains(&u.speed.unwrap_or(1.))
            || (streaming && u.speed.unwrap_or(1.) != 1.)
            || u.format.as_deref().is_some_and(|f| {
                if streaming {
                    f != "pcm"
                } else {
                    !matches!(f, "wav" | "mp3")
                }
            })
        {
            return Err(invalid());
        }
        speed = u.speed.unwrap_or(1.);
        mp3 = u.format.as_deref() == Some("mp3");
        tx.send(u.text).await.map_err(|_| invalid())?;
    }
    let mut ticket = s.pool.submit(id, streaming, rx).map_err(|e| match e {
        SubmitError::Busy => ApiError(StatusCode::TOO_MANY_REQUESTS, "queue_full"),
        SubmitError::Duplicate => ApiError(StatusCode::CONFLICT, "duplicate_id"),
        SubmitError::Offline => ApiError(StatusCode::SERVICE_UNAVAILABLE, "worker_offline"),
    })?;
    if let Some(body) = body {
        let c = ticket.control.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = c.cancel.cancelled() => return,
                result = feed(body, &tx) => result,
            };
            if result.is_err() {
                c.invalid_input.store(true, Relaxed);
                c.stop();
            }
        });
    } else {
        drop(tx)
    }
    let id = ticket.id.clone();
    let first = next_frame(&mut ticket)
        .await?
        .ok_or(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "empty_audio"))?;
    let (body, kind) = if streaming {
        let frames = stream::try_unfold((Some(first), ticket), |(first, mut t)| async move {
            let frame = if let Some(b) = first {
                b
            } else {
                match next_frame(&mut t).await {
                    Ok(Some(f)) => f,
                    Ok(None) => return Ok(None),
                    Err(e) => return Err(std::io::Error::other(e.1)),
                }
            };
            Ok(Some((frame, (None, t))))
        });
        (Body::from_stream(frames), "application/octet-stream")
    } else {
        let mut data = first.to_vec();
        while let Some(frame) = next_frame(&mut ticket).await? {
            data.extend(frame);
        }
        if mp3 || speed != 1. {
            let result = audio::encode(data, mp3, speed)
                .await
                .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "encoder_unavailable"))?;
            (
                Body::from(result),
                if mp3 { "audio/mpeg" } else { "audio/wav" },
            )
        } else {
            (Body::from(audio::wav(&data)), "audio/wav")
        }
    };
    Ok(Response::builder()
        .header("Content-Type", kind)
        .header("Cache-Control", "no-store")
        .header("X-Request-Id", id)
        .header(
            "X-Audio-Format",
            if mp3 {
                "mp3;rate=24000;channels=1"
            } else {
                "s16le;rate=24000;channels=1"
            },
        )
        .body(body)
        .unwrap())
}
async fn next_frame(t: &mut Ticket) -> Result<Option<Bytes>, ApiError> {
    tokio::select! {
        biased;
        _ = t.control.cancel.cancelled() => {
            if t.control.invalid_input.load(Relaxed) {
                Err(invalid())
            } else {
                Err(ApiError(StatusCode::REQUEST_TIMEOUT, "cancelled_or_timed_out"))
            }
        },
        frame = t.rx.recv() => frame.transpose()
            .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "synthesis_failed")),
    }
}
// UTF-8 may straddle transport frames. A newline commits one complete phrase to the model.
async fn feed(body: Body, tx: &mpsc::Sender<String>) -> Result<(), ()> {
    let mut chunks = body.into_data_stream();
    let mut pending = Vec::new();
    let mut total = 0;
    let mut seen = 0;
    while let Some(bytes) = chunks.next().await {
        let bytes = bytes.map_err(|_| ())?;
        total += bytes.len();
        if total > 8192 {
            return Err(());
        }
        for &b in &bytes {
            if b == b'\n' {
                send_line(&mut pending, &mut seen, tx).await?
            } else {
                pending.push(b);
            }
        }
    }
    send_line(&mut pending, &mut seen, tx).await?;
    if seen == 0 {
        return Err(());
    }
    Ok(())
}
async fn send_line(
    line: &mut Vec<u8>,
    seen: &mut usize,
    tx: &mpsc::Sender<String>,
) -> Result<(), ()> {
    let text = String::from_utf8(std::mem::take(line)).map_err(|_| ())?;
    if text.trim().is_empty() {
        return Ok(());
    }
    *seen += text.chars().count();
    if *seen > 2000 || !text::valid(&text) {
        return Err(());
    }
    tx.send(text).await.map_err(|_| ())
}
