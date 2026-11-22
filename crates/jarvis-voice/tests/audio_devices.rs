//! Тесты с реальными устройствами — ИГРАЮТ ЗВУК И ВКЛЮЧАЮТ МИКРОФОН. Только вручную:
//! `cargo test --test audio_devices -- --ignored --nocapture --test-threads=1`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use jarvis_core::{CancellationToken, CollectSink, Event};
use jarvis_voice::capture::{Capture, CaptureConfig};
use jarvis_voice::device::{input_devices, output_devices};
use jarvis_voice::player::{AudioOutput, Player, PlayerConfig};

#[test]
#[ignore = "перечисляет аудиоустройства"]
fn devices_list() {
    eprintln!("вывод: {:?}", output_devices().unwrap());
    eprintln!("ввод: {:?}", input_devices().unwrap());
}

#[tokio::test]
#[ignore = "играет звук"]
async fn player_beep_and_stop() {
    let sink = Arc::new(CollectSink::default());
    let p = Player::start(PlayerConfig::default(), sink.clone()).unwrap();
    eprintln!("устройство {} {} Гц", p.device_name(), p.device_rate());
    let beep: Vec<f32> = (0..24_000).map(|i| (i as f32 / 24_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.2).collect();
    for c in beep.chunks(2400) {
        p.play(c.to_vec(), 24_000).await.unwrap();
    }
    p.finish().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(p.is_playing());
    let t = Instant::now();
    p.stop();
    assert!(!p.is_playing(), "stop мгновенный: {:?}", t.elapsed());
    p.drain(&CancellationToken::new()).await;
    assert!(sink.take().iter().any(|e| matches!(e, Event::AudioLevel { .. })));
}

#[tokio::test]
#[ignore = "включает микрофон"]
async fn capture_frames() {
    let sink = Arc::new(CollectSink::default());
    let (cap, mut rx) = Capture::start(CaptureConfig::default(), sink.clone()).unwrap();
    eprintln!("микрофон {} {} Гц", cap.device_name(), cap.device_rate());
    let mut n = 0;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(1) {
        if let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            assert_eq!(f.len(), 256);
            n += 1;
        }
    }
    eprintln!("кадров за 1 с: {n}, потеряно {}", cap.dropped());
    assert!((50..=70).contains(&n));
    assert!(sink.take().iter().any(|e| matches!(e, Event::AudioLevel { .. })));
}
