//! Живой STT против сервера Xeon (`https://192.168.50.38:8444`). Звук не играет и микрофон не нужен:
//! фраза синтезируется TTS-сервером, переводится в 16 кГц и отправляется в STT.
//! Запуск: `cargo test --test stt_live -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use jarvis_core::{CancellationToken, Config};
use jarvis_voice::pcm::{S16Decoder, f32_to_i16};
use jarvis_voice::resample::resample_all;
use jarvis_voice::stt::{ServerStt, SttEngine, SttEvent};
use jarvis_voice::tls::default_project_root;
use jarvis_voice::tts::TtsClient;
use tokio::sync::mpsc;

async fn phrase_16k(text: &str) -> Vec<i16> {
    let root = default_project_root();
    let cfg = Config::load(root.join("config.json")).unwrap_or_default();
    let tts = TtsClient::new(&cfg.tts, &root).unwrap();
    let bytes = tts.synthesize_stream(text, None, CancellationToken::new()).await.unwrap().collect_all().await.unwrap();
    let mut f = Vec::new();
    S16Decoder::new().decode_into(&bytes, &mut f);
    resample_all(&f, 24_000, 16_000).unwrap().into_iter().map(f32_to_i16).collect()
}

fn server() -> ServerStt {
    let root = default_project_root();
    let cfg = Config::load(root.join("config.json")).unwrap_or_default();
    ServerStt::new(&cfg.stt, &root).unwrap()
}

#[tokio::test]
#[ignore = "сеть: STT-сервер Xeon :8444"]
async fn stt_live_health_and_post() {
    let stt = server();
    let h = stt.health().await.expect("/health");
    eprintln!("{} /health → {h:?}", stt.base_url());
    assert!(h.ok);
    let pcm = phrase_16k("Джарвис, открой браузер.").await;
    let t = Instant::now();
    let r = stt.transcribe(&pcm, &CancellationToken::new()).await.expect("POST /stt");
    eprintln!("POST /stt за {:?}: {r:?}", t.elapsed());
    assert!(!r.text.trim().is_empty());
}

#[tokio::test]
#[ignore = "сеть: STT-сервер Xeon :8444"]
async fn stt_live_stream() {
    let stt = server();
    let pcm = phrase_16k("Джарвис, сколько будет семь плюс пять?").await;
    let (tx, mut rx) = mpsc::channel(64);
    let s = stt.open_stream(tx).unwrap();
    // В реальном темпе кадрами по 16 мс не нужно — сервер принимает любой длины.
    for chunk in pcm.chunks(1600) {
        s.send_audio(chunk);
    }
    let t = Instant::now();
    s.end();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("таймаут").expect("канал");
        eprintln!("{ev:?}");
        if let SttEvent::Final { text, .. } = ev {
            eprintln!("final через {:?} после end", t.elapsed());
            assert!(!text.trim().is_empty());
            break;
        }
        if let SttEvent::Disconnected { reason } = ev {
            panic!("STT недоступен: {reason}");
        }
    }
}
