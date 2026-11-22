//! Интеграционные проверки развёрнутого сервиса с ПК (Windows) — по сети, TLS через закреплённый CA.
//!
//! ```powershell
//! cargo test --test remote -- --ignored --nocapture --test-threads=1
//! ```
//! Переменные: `JARVIS_STT_URL` (по умолчанию `https://192.168.50.38:8444`), `JARVIS_TTS_URL`
//! (`https://192.168.50.38:8443`), `JARVIS_STT_CA` (`src-tauri/config/xeon-tts-ca.pem`).
//! Звук для проверки синтезирует наш TTS (PCM 24 кГц), ресемплинг в 16 кГц — `audio::resample`.

use futures_util::{SinkExt, StreamExt};
use jarvis_core::stt_protocol::{
    ClientMsg, DEFAULT_URL, Health, STREAM_PATH, ServerMsg, SttResult,
};
use jarvis_stt_rs::{audio, text};
use rustls::pki_types::{CertificateDer, pem::PemObject};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_tungstenite::{Connector, tungstenite::Message};

fn stt_url() -> String {
    std::env::var("JARVIS_STT_URL").unwrap_or_else(|_| DEFAULT_URL.into())
}
fn tts_url() -> String {
    std::env::var("JARVIS_TTS_URL").unwrap_or_else(|_| "https://192.168.50.38:8443".into())
}
fn ca_pem() -> Vec<u8> {
    let path = std::env::var("JARVIS_STT_CA").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src-tauri/config/xeon-tts-ca.pem"
        )
        .into()
    });
    std::fs::read(&path).unwrap_or_else(|e| panic!("CA {path}: {e}"))
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .use_rustls_tls()
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(&ca_pem()).unwrap())
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}

fn tls_connector() -> Connector {
    let mut roots = rustls::RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(&ca_pem()) {
        roots.add(c.unwrap()).unwrap();
    }
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    Connector::Rustls(Arc::new(cfg))
}

/// Синтез TTS → 16 кГц mono f32.
async fn tts(client: &reqwest::Client, phrase: &str) -> Vec<f32> {
    let r = client
        .post(format!("{}/tts/stream", tts_url()))
        .json(&serde_json::json!({ "text": phrase }))
        .send()
        .await
        .expect("TTS доступен");
    assert!(r.status().is_success(), "TTS: {}", r.status());
    let pcm = r.bytes().await.unwrap();
    let samples = audio::s16le_to_f32(&pcm[..pcm.len() & !1]).unwrap();
    audio::resample(&samples, 24_000, audio::SAMPLE_RATE)
}

fn pad(speech: &[f32], pre_ms: usize, post_ms: usize) -> Vec<f32> {
    let mut v = vec![0.0; pre_ms * 16];
    v.extend_from_slice(speech);
    v.extend(std::iter::repeat_n(0.0, post_ms * 16));
    v
}

#[tokio::test]
#[ignore = "нужен развёрнутый сервис на Xeon"]
async fn remote_health_and_post() {
    let c = http();
    let h: Health = c
        .get(format!("{}/health", stt_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("health: {h:?}");
    assert!(h.ok);
    assert_eq!(h.sample_rate, 16000);
    assert!(!h.model.is_empty());

    for phrase in [
        "Джарвис, открой блокнот.",
        "Сколько будет семь плюс пять?",
        "Включи вай-фай.",
    ] {
        let speech = tts(&c, phrase).await;
        let body = audio::f32_to_s16le(&pad(&speech, 200, 300));
        let t = Instant::now();
        let r: SttResult = c
            .post(format!("{}/stt", stt_url()))
            .header("content-type", "audio/L16; rate=16000")
            .body(body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let s = text::score(phrase, &r.text);
        println!(
            "POST /stt {:>4} мс  CER {:>5.1} %  {phrase:?} → {:?}",
            t.elapsed().as_millis(),
            s.cer() * 100.0,
            r.text
        );
        assert!(
            s.cer() <= 0.15,
            "слишком далеко от исходного: {:?} vs {phrase:?}",
            r.text
        );
        assert!(r.duration_ms > 500);
    }

    // WAV 24 кГц напрямую (ресемплинг на сервере)
    let raw = c
        .post(format!("{}/tts/stream", tts_url()))
        .json(&serde_json::json!({"text": "Да, сэр."}))
        .send()
        .await
        .unwrap();
    let pcm = raw.bytes().await.unwrap();
    let wav = audio::write_wav_pcm16(
        &audio::s16le_to_f32(&pcm[..pcm.len() & !1]).unwrap(),
        24_000,
    );
    let r: SttResult = c
        .post(format!("{}/stt", stt_url()))
        .header("content-type", "audio/wav")
        .body(wav)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("POST /stt WAV 24 кГц → {:?}", r.text);
    assert!(text::score("да сэр", &r.text).cer() <= 0.2);
}

struct StreamStats {
    final_text: String,
    partials: usize,
    first_partial_ms: Option<f64>,
    end_lag_ms: Option<f64>,
    final_ms: f64,
}

/// Отправляет фразу в реальном времени (куски 40 мс) и замеряет задержки по часам ПК.
async fn stream_phrase<S, R>(sink: &mut S, rx: &mut R, speech: &[f32]) -> StreamStats
where
    S: futures_util::Sink<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Debug,
    R: futures_util::Stream<Item = (Instant, ServerMsg)> + Unpin,
{
    const PRE_MS: usize = 300;
    const POST_MS: usize = 900; // как у клиента: VAD ждёт тишину 800–1200 мс
    let samples = pad(speech, PRE_MS, POST_MS);
    let (onset_s, offset_s) = (PRE_MS * 16, PRE_MS * 16 + speech.len());
    let chunk = 640;
    let start = tokio::time::Instant::now();
    let (mut t_onset, mut t_offset) = (None, None);
    for (i, c) in samples.chunks(chunk).enumerate() {
        tokio::time::sleep_until(start + Duration::from_millis(40 * i as u64)).await;
        sink.send(Message::Binary(audio::f32_to_s16le(c).into()))
            .await
            .unwrap();
        let end = (i + 1) * chunk;
        if t_onset.is_none() && end >= onset_s {
            t_onset = Some(Instant::now());
        }
        if t_offset.is_none() && end >= offset_s {
            t_offset = Some(Instant::now());
        }
    }
    let t_end = Instant::now();
    sink.send(Message::Text(
        serde_json::to_string(&ClientMsg::End).unwrap().into(),
    ))
    .await
    .unwrap();
    let mut partial_times = Vec::new();
    // partial, пришедшие во время отправки, уже лежат в канале; дочитываем до final
    loop {
        let (t, m) = tokio::time::timeout(Duration::from_secs(10), rx.next())
            .await
            .expect("final вовремя")
            .unwrap();
        match m {
            ServerMsg::Partial { .. } => partial_times.push(t),
            ServerMsg::Final { text, .. } => {
                // a − b в мс со знаком
                let ms = |a: Instant, b: Instant| {
                    (a.saturating_duration_since(b).as_secs_f64()
                        - b.saturating_duration_since(a).as_secs_f64())
                        * 1000.0
                };
                return StreamStats {
                    final_text: text,
                    partials: partial_times.len(),
                    first_partial_ms: partial_times.first().map(|p| ms(*p, t_onset.unwrap())),
                    end_lag_ms: partial_times.last().map(|p| ms(*p, t_offset.unwrap())),
                    final_ms: ms(t, t_end),
                };
            }
            other => panic!("неожиданное сообщение {other:?}"),
        }
    }
}

async fn ws_connect() -> (
    impl futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    tokio_stream_like::Rx,
    String,
) {
    let url = stt_url().replacen("https://", "wss://", 1) + STREAM_PATH;
    let (ws, _) =
        tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(tls_connector()))
            .await
            .expect("WS");
    let (sink, mut stream) = ws.split();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(Ok(m)) = stream.next().await {
            if let Message::Text(t) = m {
                let msg: ServerMsg = serde_json::from_str(t.as_str()).unwrap();
                if tx.send((Instant::now(), msg)).is_err() {
                    break;
                }
            }
        }
    });
    let mut rx = tokio_stream_like::Rx(rx);
    let model = match tokio::time::timeout(Duration::from_secs(5), rx.next())
        .await
        .unwrap()
        .unwrap()
        .1
    {
        ServerMsg::Ready { model } => model,
        other => panic!("ожидался ready: {other:?}"),
    };
    (sink, rx, model)
}

mod tokio_stream_like {
    use jarvis_core::stt_protocol::ServerMsg;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Instant;
    pub struct Rx(pub tokio::sync::mpsc::UnboundedReceiver<(Instant, ServerMsg)>);
    impl futures_util::Stream for Rx {
        type Item = (Instant, ServerMsg);
        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.0.poll_recv(cx)
        }
    }
}

#[tokio::test]
#[ignore = "нужен развёрнутый сервис на Xeon"]
async fn remote_websocket_stream() {
    let c = http();
    let (mut sink, mut rx, model) = ws_connect().await;
    println!("ready: {model}");
    // две фразы в одном соединении + reset между ними
    for phrase in [
        "Посмотри ошибку в терминале.",
        "Джарвис, открой диспетчер задач.",
    ] {
        let speech = tts(&c, phrase).await;
        let st = stream_phrase(&mut sink, &mut rx, &speech).await;
        let s = text::score(phrase, &st.final_text);
        println!(
            "WS {phrase:?} → {:?}: partial {} шт., первый через {:?} мс от начала речи, последний {:?} мс после конца речи, final {:.0} мс после end, CER {:.1} %",
            st.final_text,
            st.partials,
            st.first_partial_ms.map(|v| v.round()),
            st.end_lag_ms.map(|v| v.round()),
            st.final_ms,
            s.cer() * 100.0
        );
        assert!(st.partials >= 1, "нет partial");
        assert!(s.cer() <= 0.15, "final далеко от исходного");
        assert!(
            st.final_ms < 1500.0,
            "final слишком долго: {} мс",
            st.final_ms
        );
        sink.send(Message::Text(
            serde_json::to_string(&ClientMsg::Reset).unwrap().into(),
        ))
        .await
        .unwrap();
    }
    sink.send(Message::Close(None)).await.ok();
}

/// Отчёт по всем фразам `bench/phrases.tsv` в реальном времени (WER, задержки с ПК).
#[tokio::test]
#[ignore = "нужен развёрнутый сервис на Xeon; долгий (~1,5 мин)"]
async fn remote_latency_report() {
    let c = http();
    let tsv = include_str!("../bench/phrases.tsv");
    let (mut sink, mut rx, model) = ws_connect().await;
    println!("model: {model}");
    let mut total = text::Score::default();
    let (mut firsts, mut lags, mut finals, mut rates) = (vec![], vec![], vec![], vec![]);
    for line in tsv.lines().filter(|l| !l.trim().is_empty()) {
        let (id, phrase) = line.split_once('\t').unwrap();
        let speech = tts(&c, phrase.trim()).await;
        let st = stream_phrase(&mut sink, &mut rx, &speech).await;
        let s = text::score(phrase, &st.final_text);
        total.add(s);
        firsts.extend(st.first_partial_ms);
        lags.extend(st.end_lag_ms);
        finals.push(st.final_ms);
        rates.push(st.partials as f64 / (speech.len() as f64 / 16000.0));
        println!(
            "{id} final {:>4.0} мс  first {:>5?}  lag {:>5?}  partial {:>2}  WER {:>3}/{}  {:?}",
            st.final_ms,
            st.first_partial_ms.map(|v| v.round()),
            st.end_lag_ms.map(|v| v.round()),
            st.partials,
            s.word_errors,
            s.words,
            st.final_text
        );
    }
    let pct = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() - 1) as f64 * p).round() as usize]
    };
    println!(
        "ИТОГО: WER {:.1} %, CER {:.1} %; final p50 {:.0} / p95 {:.0} мс; первый partial p50 {:.0} мс; lag конца p50 {:.0} / p95 {:.0} мс; partial/с {:.1}",
        total.wer() * 100.0,
        total.cer() * 100.0,
        pct(&mut finals, 0.5),
        pct(&mut finals, 0.95),
        pct(&mut firsts, 0.5),
        pct(&mut lags, 0.5),
        pct(&mut lags, 0.95),
        rates.iter().sum::<f64>() / rates.len() as f64
    );
    assert!(total.wer() <= 0.2);
}
