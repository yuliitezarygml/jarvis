//! Протокол сервера целиком (HTTP + WebSocket) на тестовом движке, без sherpa-onnx и без TLS.

use futures_util::{SinkExt, StreamExt};
use jarvis_core::stt_protocol::{ClientMsg, Health, STREAM_PATH, ServerMsg, SttResult};
use jarvis_stt_rs::audio;
use jarvis_stt_rs::config::Limits;
use jarvis_stt_rs::engine::MockEngine;
use jarvis_stt_rs::server::{AppState, router};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

async fn start(limits: Limits) -> String {
    let state = AppState::new(Arc::new(MockEngine::default()), limits);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    format!("127.0.0.1:{}", addr.port())
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(addr: &str) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{STREAM_PATH}"))
        .await
        .unwrap();
    ws
}

async fn next_msg(ws: &mut Ws) -> ServerMsg {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("ответ сервера")
            .unwrap()
            .unwrap();
        match m {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("неожиданный кадр {other:?}"),
        }
    }
}

async fn send_ctrl(ws: &mut Ws, m: &ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(m).unwrap().into()))
        .await
        .unwrap();
}

fn pcm(ms: usize) -> Vec<u8> {
    audio::f32_to_s16le(&vec![0.1; ms * 16])
}

#[tokio::test]
async fn health_and_post() {
    let addr = start(Limits::default()).await;
    let http = reqwest::Client::new();
    let h: Health = http
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        h,
        Health {
            ok: true,
            model: "mock".into(),
            sample_rate: 16000
        }
    );

    let r: SttResult = http
        .post(format!("http://{addr}/stt"))
        .header("content-type", "audio/L16; rate=16000")
        .body(pcm(1000))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        r,
        SttResult {
            text: MockEngine::text_for(16000),
            duration_ms: 1000
        }
    );

    // WAV 48 кГц стерео → 16 кГц mono
    let stereo: Vec<f32> = vec![0.1; 48000 * 2];
    let mut wav = audio::write_wav_pcm16(&stereo, 48000);
    wav[22..24].copy_from_slice(&2u16.to_le_bytes()); // channels = 2
    wav[32..34].copy_from_slice(&4u16.to_le_bytes()); // block_align
    wav[28..32].copy_from_slice(&(48000u32 * 4).to_le_bytes());
    let r: SttResult = http
        .post(format!("http://{addr}/stt"))
        .header("content-type", "audio/wav")
        .body(wav)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r.duration_ms, 1000);

    let st = |r: reqwest::Response| r.status().as_u16();
    let post = |ct: &'static str, body: Vec<u8>| {
        http.post(format!("http://{addr}/stt"))
            .header("content-type", ct)
            .body(body)
            .send()
    };
    assert_eq!(
        st(post("text/plain", b"hello".to_vec()).await.unwrap()),
        415
    );
    assert_eq!(
        st(post("audio/L16; rate=16000", vec![]).await.unwrap()),
        400
    );
    assert_eq!(
        st(post("audio/L16; rate=16000", vec![1, 2, 3]).await.unwrap()),
        422
    );
    assert_eq!(
        st(post("audio/L16; rate=16000", pcm(31_000)).await.unwrap()),
        413
    );
    assert_eq!(
        st(http
            .get(format!("http://{addr}/nope"))
            .send()
            .await
            .unwrap()),
        404
    );
    let m = http
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(m.contains("jarvis_stt_finals_total 2"), "{m}");
}

#[tokio::test]
async fn websocket_flow() {
    let addr = start(Limits::default()).await;
    let mut ws = connect(&addr).await;
    assert_eq!(
        next_msg(&mut ws).await,
        ServerMsg::Ready {
            model: "mock".into()
        }
    );

    // 800 мс кусками нечётной длины → partial растут, затем final
    let bytes = pcm(800);
    for c in bytes.chunks(1001) {
        ws.send(Message::Binary(c.to_vec().into())).await.unwrap();
    }
    let mut partials = Vec::new();
    for _ in 0..4 {
        match next_msg(&mut ws).await {
            ServerMsg::Partial { text } => partials.push(text),
            other => panic!("ожидался partial: {other:?}"),
        }
    }
    assert_eq!(partials.last().unwrap(), &MockEngine::text_for(12800));
    send_ctrl(&mut ws, &ClientMsg::End).await;
    assert_eq!(
        next_msg(&mut ws).await,
        ServerMsg::Final {
            text: MockEngine::text_for(12800),
            duration_ms: 800
        }
    );

    // reset: без ответа и без остатка в следующей фразе
    ws.send(Message::Binary(pcm(150).into())).await.unwrap();
    send_ctrl(&mut ws, &ClientMsg::Reset).await;
    send_ctrl(&mut ws, &ClientMsg::End).await;
    assert_eq!(
        next_msg(&mut ws).await,
        ServerMsg::Final {
            text: String::new(),
            duration_ms: 0
        }
    );

    // мусорный текст → error, соединение живо
    ws.send(Message::Text("{\"type\":\"bogus\"}".into()))
        .await
        .unwrap();
    assert!(matches!(next_msg(&mut ws).await, ServerMsg::Error { .. }));
    ws.send(Message::Binary(pcm(400).into())).await.unwrap();
    assert!(matches!(next_msg(&mut ws).await, ServerMsg::Partial { .. }));
    send_ctrl(&mut ws, &ClientMsg::End).await;
    assert_eq!(
        next_msg(&mut ws).await,
        ServerMsg::Final {
            text: MockEngine::text_for(6400),
            duration_ms: 400
        }
    );
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn limits_sessions_utterance_idle() {
    let limits = Limits {
        max_sessions: 1,
        max_utterance: Duration::from_millis(500),
        idle_timeout: Duration::from_millis(400),
        ..Limits::default()
    };
    let addr = start(limits).await;
    let mut ws = connect(&addr).await;
    assert!(matches!(next_msg(&mut ws).await, ServerMsg::Ready { .. }));

    // вторая сессия сверх лимита → HTTP 503 до апгрейда
    let second = tokio_tungstenite::connect_async(format!("ws://{addr}{STREAM_PATH}")).await;
    match second {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status().as_u16(), 503)
        }
        other => panic!("ожидался отказ 503, получено {other:?}"),
    }

    // фраза длиннее предела → final без end, остаток идёт в следующую фразу
    ws.send(Message::Binary(pcm(700).into())).await.unwrap();
    let mut got_final = None;
    let mut after = Vec::new();
    for _ in 0..4 {
        match next_msg(&mut ws).await {
            ServerMsg::Final { text, duration_ms } => got_final = Some((text, duration_ms)),
            ServerMsg::Partial { text } if got_final.is_some() => {
                after.push(text);
                break;
            }
            ServerMsg::Partial { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(got_final, Some((MockEngine::text_for(8000), 500)));
    assert_eq!(after, vec![MockEngine::text_for(3200)]);

    // молчание дольше idle_timeout → сервер закрывает соединение
    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                _ => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "сервер не закрыл простаивающее соединение");

    // после закрытия слот освобождается
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut ws2 = connect(&addr).await;
    assert!(matches!(next_msg(&mut ws2).await, ServerMsg::Ready { .. }));
}
