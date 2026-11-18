use crate::events::{Event, Kind, SpeechUpdate};
use crate::{audio, core::Core};
use anyhow::{ensure, Result};
use serde_json::json;
use sherpa_onnx::OnlineRecognizer;
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Clone, Copy, PartialEq)]
enum Stop {
    Live,
    Submit,
    Discard,
}
pub struct Capture {
    stop: watch::Sender<Stop>,
    finished: watch::Receiver<()>,
}
impl Capture {
    fn request(&self, mode: Stop) -> watch::Receiver<()> {
        self.stop.send_modify(|state| {
            if *state != Stop::Discard {
                *state = mode;
            }
        });
        self.finished.clone()
    }
}
pub async fn end(c: &Arc<Core>, submit: bool) {
    let finished = c
        .capture
        .lock()
        .await
        .as_ref()
        .map(|job| job.request(if submit { Stop::Submit } else { Stop::Discard }));
    if let Some(mut finished) = finished {
        let _ = finished.changed().await;
    }
}
pub async fn start(c: &Arc<Core>, session: String, continuous: bool) -> Result<()> {
    let mut g = c.capture.lock().await;
    if let Some(job) = g.take() {
        let _ = job.request(Stop::Discard).changed().await;
    }
    c.cancel();
    ensure!(c.stt.available(), "Русская модель STT не установлена");
    let core = c.clone();
    let online = tokio::task::spawn_blocking(move || -> Result<_> {
        let online = core.stt.streaming_engine();
        ensure!(
            online.is_some() || core.stt.offline_available(),
            "Не удалось открыть русскую модель STT"
        );
        audio::mic_start(continuous)?;
        Ok(online)
    })
    .await??;
    let (tx, rx) = watch::channel(Stop::Live);
    let (done, finished) = watch::channel(());
    let core = c.clone();
    tokio::task::spawn_blocking(move || {
        capture(core, session, continuous, rx, online, audio::read);
        drop(done);
    });
    *g = Some(Capture { stop: tx, finished });
    Ok(())
}
fn capture(
    c: Arc<Core>,
    session: String,
    continuous: bool,
    control: watch::Receiver<Stop>,
    online: Option<Arc<OnlineRecognizer>>,
    mut read: impl FnMut(&mut [f32]) -> usize,
) {
    let emit =
        |kind: Kind, data: serde_json::Value| (c.bus)(Event::new(&session, None, kind, data));
    emit(Kind::Mic, json!(if continuous { "auto" } else { "ptt" }));
    let speech = |update| (c.bus)(Event::speech(&session, update));
    let listening = if online.is_some() {
        SpeechUpdate::Listening
    } else {
        SpeechUpdate::Fallback
    };
    speech(listening);
    let mut live = online.as_ref().map(audio::LiveUtterance::new);
    let mut buffer = vec![0f32; 4096];
    let mut pre: VecDeque<f32> = VecDeque::new();
    let mut utterance = vec![];
    let mut loud = 0;
    let mut quiet = 0;
    let mut speaking = false;
    let mut noise = 0.002f32;
    let mut meter = Instant::now();
    let mut capture_open = true;
    let opened = Instant::now();
    loop {
        let mut mode = *control.borrow();
        if !continuous
            && !speaking
            && opened.elapsed() > Duration::from_secs(12)
            && mode == Stop::Live
        {
            mode = Stop::Submit;
        }
        if mode == Stop::Discard {
            speech(SpeechUpdate::Cleared);
            break;
        }
        let n = read(&mut buffer);
        let chunk = &buffer[..n];
        let level = audio::level(chunk);
        if n > 0 {
            let previous_len = utterance.len();
            if meter.elapsed() > Duration::from_millis(65) {
                emit(Kind::MicLevel, json!(level));
                meter = Instant::now();
            }
            let voiced = level > (noise * 3.3).clamp(0.008, 0.048);
            if voiced {
                loud += n;
                quiet = 0;
            } else {
                loud = 0;
                quiet += n;
                if !speaking {
                    noise = noise * 0.96 + level * 0.04;
                }
            }
            if !continuous {
                // Push-to-talk preserves quiet syllables until the button is released.
                utterance.extend_from_slice(chunk);
                if !speaking && loud >= 1200 {
                    speaking = true;
                    emit(Kind::VoiceStart, json!(true));
                }
            } else if !speaking {
                pre.extend(chunk);
                while pre.len() > 4800 {
                    pre.pop_front();
                }
                if loud >= 1200 {
                    speaking = true;
                    utterance.extend(pre.drain(..));
                    c.cancel();
                    emit(Kind::VoiceStart, json!(true));
                    speech(listening);
                }
            } else {
                utterance.extend_from_slice(chunk);
            }
            if utterance.len() > previous_len {
                if let Some(text) = live
                    .as_mut()
                    .and_then(|s| s.push(&utterance[previous_len..]))
                {
                    (c.bus)(Event::speech(&session, SpeechUpdate::Partial(text)));
                }
            }
        }
        if mode == Stop::Submit && n == buffer.len() && utterance.len() < 16000 * 24 {
            continue;
        }
        let finish = mode == Stop::Submit
            || (continuous && speaking && quiet >= 10400)
            || utterance.len() >= 16000 * 24;
        if finish {
            if !continuous || mode == Stop::Submit {
                audio::mic_end();
                capture_open = false;
                emit(Kind::Mic, json!("off"));
            }
            if utterance.len() > 2400 {
                if continuous && quiet < utterance.len() {
                    utterance.truncate(utterance.len() - quiet.saturating_sub(3200));
                }
                emit(Kind::Recognizing, json!(true));
                let result = c.stt.finalize(&utterance, live.take());
                if *control.borrow() == Stop::Discard {
                    speech(SpeechUpdate::Cleared);
                    break;
                }
                match result {
                    Ok(text) if !text.is_empty() => {
                        (c.bus)(Event::speech(&session, SpeechUpdate::Final(&text)));
                        if let Err(e) = c.start(session.clone(), text) {
                            emit(Kind::MicError, json!(e.to_string()));
                        }
                    }
                    Ok(_) => emit(Kind::MicError, json!("Речь не распознана")),
                    Err(e) => emit(Kind::MicError, json!(e.to_string())),
                }
            } else if mode == Stop::Submit {
                emit(
                    Kind::MicError,
                    json!("Сигнал речи слишком короткий или микрофон не получил звук"),
                );
            }
            if !continuous || mode != Stop::Live {
                break;
            }
            speaking = false;
            loud = 0;
            quiet = 0;
            pre.clear();
            utterance.clear();
            live = online.as_ref().map(audio::LiveUtterance::new);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    if capture_open {
        audio::mic_end();
        emit(Kind::Mic, json!("off"));
    }
}

#[cfg(test)]
mod acceptance {
    use super::*;
    use std::sync::Mutex;

    #[test]
    #[ignore = "Requires JARVIS_STT_DIR with both installed Russian models"]
    fn continuous_speech_submits_only_finals_and_discards_last_command() {
        // Drive the same capture loop with real PCM, independent of room acoustics.
        let (stop, control) = watch::channel(Stop::Live);
        let trace = Arc::new(Mutex::new(Vec::new()));
        let recorded = trace.clone();
        let discard = stop.clone();
        let dir = std::env::temp_dir().join(format!("jarvis-vad-{}", uuid::Uuid::new_v4()));
        let core = Core::open(
            &dir,
            Arc::new(move |event| {
                if !matches!(event.kind, Kind::Speech | Kind::Start) {
                    return;
                }
                let mut events = recorded.lock().unwrap();
                let submissions = events
                    .iter()
                    .filter(|e: &&Event| matches!(e.kind, Kind::Start))
                    .count();
                if submissions == 2 && event.data["state"] == "partial" {
                    discard.send(Stop::Discard).unwrap();
                }
                events.push(event);
            }),
        )
        .unwrap();
        let session = {
            let db = core.db.lock().unwrap();
            let mut settings = db.settings().unwrap();
            settings.url.clear(); // This test stops at command dispatch, before any network call.
            settings.speak = false;
            db.save(&settings).unwrap();
            db.new_session().unwrap()
        };
        let engine = core.stt.streaming_engine().expect("streaming model");
        let mut pcm = Vec::new();
        for name in ["old-long", "old-short", "old-long"] {
            let wave =
                sherpa_onnx::Wave::read(&format!("../jarvis-tts-rs/examples/audio/{name}.wav"))
                    .unwrap();
            assert_eq!(wave.sample_rate(), 24000);
            pcm.extend((0..wave.samples().len() * 2 / 3).map(|i| wave.samples()[i * 3 / 2]));
            pcm.extend(std::iter::repeat_n(0.0, 16000));
        }
        let mut frames = pcm.chunks(320);
        capture(
            core.clone(),
            session.clone(),
            true,
            control,
            Some(engine),
            |out| {
                if let Some(frame) = frames.next() {
                    out[..frame.len()].copy_from_slice(frame);
                    frame.len()
                } else {
                    stop.send(Stop::Discard).unwrap();
                    0
                }
            },
        );
        let trace = trace.lock().unwrap();
        let mut finals = Vec::new();
        let mut sent = Vec::new();
        let mut changes = 0;
        for event in trace.iter() {
            match event.data["state"].as_str() {
                Some("listening") => changes = 0,
                Some("partial") => changes += 1,
                Some("final") => {
                    assert!(changes > 0);
                    finals.push(event.data["text"].as_str().unwrap());
                    println!(
                        "{changes} partials before final: {}",
                        finals.last().unwrap()
                    );
                }
                _ if matches!(event.kind, Kind::Start) => {
                    sent.push(event.data.as_str().unwrap());
                    assert_eq!(sent, finals);
                }
                _ => (),
            }
        }
        assert_eq!(sent.len(), 2);
        assert!(sent[0].contains("диагностика завершена"));
        assert_eq!(sent[1], "добрый вечер сэр");
        assert!(changes > 0);
        assert_eq!(trace.last().unwrap().data["state"], "cleared");
        let rows = core.db.lock().unwrap().history(&session, 20).unwrap();
        assert_eq!(rows.iter().filter(|r| r["role"] == "user").count(), 2);
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
