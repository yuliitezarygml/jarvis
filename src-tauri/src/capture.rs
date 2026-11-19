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
    FinishPhrase,
    Submit,
    Discard,
}
pub enum End {
    Submit,
    Discard,
    CancelPushToTalk,
}
#[derive(Clone, Copy)]
struct ListenOptions {
    continuous: bool,
    silence_ms: u32,
    wake_word: bool,
}

fn wake_command(text: &str) -> Option<&str> {
    let (name, rest) = text.split_once(|c: char| !c.is_alphanumeric())?;
    let command = rest.trim_start_matches(|c: char| !c.is_alphanumeric());
    (name.to_lowercase() == "джарвис" && !command.is_empty()).then_some(command)
}

pub struct Capture {
    continuous: bool,
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
pub async fn end(c: &Arc<Core>, action: End) {
    let finished = c
        .capture
        .lock()
        .await
        .as_ref()
        .and_then(|job| match (action, job.continuous) {
            (End::CancelPushToTalk, true) => None,
            (End::Submit, true) => {
                job.request(Stop::FinishPhrase);
                None
            }
            (End::Submit, false) => Some(job.request(Stop::Submit)),
            _ => Some(job.request(Stop::Discard)),
        });
    if let Some(mut finished) = finished {
        let _ = finished.changed().await;
    }
}
pub async fn start(c: &Arc<Core>, session: String, continuous: bool) -> Result<()> {
    let mut g = c.capture.lock().await;
    if let Some(job) = g.take() {
        let _ = job.request(Stop::Discard).changed().await;
    }
    if !continuous {
        c.cancel();
    }
    ensure!(c.stt.available(), "Русская модель STT не установлена");
    let settings = c.db.lock().unwrap().settings()?;
    let options = ListenOptions {
        continuous,
        silence_ms: settings.silence_ms,
        wake_word: continuous && settings.wake_word,
    };
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
        capture(core, session, options, rx, online, audio::read);
        drop(done);
    });
    *g = Some(Capture {
        continuous,
        stop: tx,
        finished,
    });
    Ok(())
}
fn capture(
    c: Arc<Core>,
    session: String,
    options: ListenOptions,
    mut control: watch::Receiver<Stop>,
    online: Option<Arc<OnlineRecognizer>>,
    mut read: impl FnMut(&mut [f32]) -> usize,
) {
    let continuous = options.continuous;
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
    let mut buffer = [0f32; 256]; // Earshot consumes 16 ms frames at 16 kHz.
    let mut filled = 0;
    let mut detector = earshot::Detector::default();
    let silence_samples = options.silence_ms as usize * 16;
    let mut pre: VecDeque<f32> = VecDeque::new();
    let mut utterance = vec![];
    let mut loud = 0;
    let mut quiet = 0;
    let mut speaking = false;
    let mut voiced_samples = 0;
    let mut quietest_voice = f32::INFINITY;
    let mut loudest_voice = 0.0f32;
    let mut playback_tail = None;
    let mut verify_barge_in = false;
    let mut meter = Instant::now();
    let mut capture_open = true;
    let opened = Instant::now();
    loop {
        let mut mode = {
            let request = control.borrow_and_update();
            if *request == Stop::FinishPhrase && !request.has_changed() {
                Stop::Live
            } else {
                *request
            }
        };
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
        let n = read(&mut buffer[filled..]);
        filled += n;
        if filled < buffer.len() && mode == Stop::Live {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        let chunk = &buffer[..filled];
        let level = audio::level(chunk);
        if !chunk.is_empty() {
            let previous_len = utterance.len();
            if meter.elapsed() > Duration::from_millis(65) {
                emit(Kind::MicLevel, json!(level));
                meter = Instant::now();
            }
            let voiced =
                filled == buffer.len() && detector.predict_f32(chunk) >= 0.5 && level > 0.001;
            if voiced {
                loud += filled;
                voiced_samples += filled;
                quietest_voice = quietest_voice.min(level);
                loudest_voice = loudest_voice.max(level);
                quiet = 0;
            } else {
                loud = 0;
                quiet += filled;
                if !speaking {
                    voiced_samples = 0;
                    quietest_voice = f32::INFINITY;
                    loudest_voice = 0.0;
                }
            }
            if continuous && audio::pending() > 0 {
                playback_tail = Some(Instant::now());
            }
            if !speaking {
                pre.extend(chunk);
                while pre.len() > 9600 {
                    pre.pop_front();
                }
                // A steady fan or hum can transiently score as speech at startup.
                // Require a changing envelope as well as consecutive voiced frames.
                if loud >= 1024 && loudest_voice > quietest_voice * 1.6 {
                    speaking = true;
                    utterance.extend(pre.drain(..));
                    verify_barge_in = continuous
                        && playback_tail.is_some_and(|t| t.elapsed() < Duration::from_millis(500));
                    if continuous && !options.wake_word && !verify_barge_in {
                        c.cancel();
                    }
                    if !verify_barge_in {
                        emit(Kind::VoiceStart, json!(true));
                        speech(listening);
                    }
                }
            } else {
                utterance.extend_from_slice(chunk);
            }
            if utterance.len() > previous_len {
                if let Some(text) = live
                    .as_mut()
                    .and_then(|s| s.push(&utterance[previous_len..]))
                {
                    if !verify_barge_in || !c.is_playback_echo(text) {
                        (c.bus)(Event::speech(&session, SpeechUpdate::Partial(text)));
                        if verify_barge_in
                            && !options.wake_word
                            && text.split_whitespace().count() >= 2
                        {
                            c.cancel();
                        }
                    }
                }
            }
        }
        let full_frame = filled == buffer.len();
        filled = 0;
        if mode == Stop::Submit && full_frame && utterance.len() < 16000 * 24 {
            continue;
        }
        let finish = matches!(mode, Stop::Submit | Stop::FinishPhrase)
            || (speaking && quiet >= silence_samples)
            || utterance.len() >= 16000 * 24;
        if finish {
            let has_speech = speaking && voiced_samples >= 1536;
            if has_speech && (!continuous || (!verify_barge_in && !options.wake_word)) {
                emit(Kind::Recognizing, json!(true));
            }
            if !continuous || mode == Stop::Submit {
                audio::mic_end();
                capture_open = false;
                emit(Kind::Mic, json!("off"));
            }
            if has_speech {
                if quiet < utterance.len() {
                    utterance.truncate(utterance.len() - quiet.saturating_sub(3200));
                }
                let result = c.stt.finalize(&utterance, live.take());
                let decision = control.borrow();
                if *decision == Stop::Discard {
                    speech(SpeechUpdate::Cleared);
                    break;
                }
                match result {
                    Ok(text)
                        if !text.is_empty() && !(verify_barge_in && c.is_playback_echo(&text)) =>
                    {
                        (c.bus)(Event::speech(&session, SpeechUpdate::Final(&text)));
                        let command = if options.wake_word {
                            wake_command(&text)
                        } else {
                            Some(text.as_str())
                        };
                        if let Some(command) =
                            command.filter(|s| s.chars().any(char::is_alphanumeric))
                        {
                            if let Err(e) = c.start(session.clone(), command.to_owned()) {
                                emit(Kind::MicError, json!(e.to_string()));
                            }
                        }
                    }
                    Ok(_) if verify_barge_in => (),
                    Ok(_) => emit(Kind::MicError, json!("Речь не распознана")),
                    Err(e) => emit(Kind::MicError, json!(e.to_string())),
                }
            } else {
                speech(SpeechUpdate::Cleared);
            }
            if !continuous || mode == Stop::Submit {
                break;
            }
            speaking = false;
            voiced_samples = 0;
            quietest_voice = f32::INFINITY;
            loudest_voice = 0.0;
            detector.reset();
            loud = 0;
            quiet = 0;
            pre.clear();
            utterance.clear();
            live = online.as_ref().map(audio::LiveUtterance::new);
        }
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

    fn close_fixture(core: Arc<Core>, dir: std::path::PathBuf) {
        core.cancel();
        // Dispatched commands hold the SQLite connection until their tasks exit.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Arc::strong_count(&core) > 1 {
            assert!(Instant::now() < deadline, "command still owns the fixture");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wake_word_requires_a_complete_address_and_command() {
        assert_eq!(
            wake_command("Джарвис, сколько сейчас времени?"),
            Some("сколько сейчас времени?")
        );
        for other in [
            "Джарвис",
            "Джарвис!",
            "расскажи о Джарвисе",
            "джарвисов вопрос",
        ] {
            assert_eq!(wake_command(other), None);
        }
    }

    #[test]
    #[ignore = "Requires JARVIS_STT_DIR and the Russian test WAVs"]
    fn one_shot_endpoints_pause_noise_and_cancel() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = events.clone();
        let dir = std::env::temp_dir().join(format!("jarvis-endpoint-{}", uuid::Uuid::new_v4()));
        let core = Core::open(&dir, Arc::new(move |e| recorded.lock().unwrap().push(e))).unwrap();
        let session = {
            let db = core.db.lock().unwrap();
            let mut s = db.settings().unwrap();
            s.url.clear();
            s.speak = false;
            db.save(&s).unwrap();
            db.new_session().unwrap()
        };
        let engine = core.stt.streaming_engine().expect("T-one");
        let wav = |name| {
            let wave =
                sherpa_onnx::Wave::read(&format!("../jarvis-tts-rs/examples/audio/{name}.wav"))
                    .unwrap();
            let mut data: Vec<_> = (0..wave.samples().len() * 2 / 3)
                .map(|i| wave.samples()[i * 3 / 2])
                .collect();
            let start = data.iter().position(|v| v.abs() > 0.008).unwrap();
            let end = data.iter().rposition(|v| v.abs() > 0.008).unwrap() + 1;
            data.truncate(end);
            data.drain(..start);
            data
        };
        let short = wav("old-short");
        let long = wav("old-long");
        let mut combined = short.clone();
        combined.extend(std::iter::repeat_n(0., 7200)); // 450 ms inside a single utterance
        combined.extend_from_slice(&short);
        let mut seed = 29u32;
        let noise: Vec<f32> = (0..48000)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let hiss = (seed as f32 / u32::MAX as f32 - 0.5) * 0.07;
                hiss + 0.025 * (i as f32 * 0.02).sin()
            })
            .collect();
        for (name, mut pcm, ms, end_input, expected) in [
            ("short", short.clone(), 800, Stop::Discard, 1),
            ("long", long, 1200, Stop::Discard, 1),
            ("pause", combined, 1000, Stop::Discard, 1),
            ("noise", noise, 1000, Stop::Submit, 0),
            ("cancel", short.clone(), 1000, Stop::Discard, 0),
            ("manual", short, 1000, Stop::Submit, 1),
        ] {
            events.lock().unwrap().clear();
            let speech_end = pcm.len();
            if matches!(name, "short" | "long" | "pause") {
                pcm.extend(std::iter::repeat_n(0., 32000));
            }
            let (tx, rx) = watch::channel(Stop::Live);
            let mut offset = 0;
            capture(
                core.clone(),
                session.clone(),
                ListenOptions {
                    continuous: false,
                    silence_ms: ms,
                    wake_word: false,
                },
                rx,
                Some(engine.clone()),
                |out| {
                    let n = out.len().min(pcm.len() - offset);
                    out[..n].copy_from_slice(&pcm[offset..offset + n]);
                    offset += n;
                    if n == 0 {
                        tx.send(end_input).unwrap();
                    }
                    n
                },
            );
            let log = events.lock().unwrap();
            let commands: Vec<_> = log
                .iter()
                .filter(|e| matches!(e.kind, Kind::Start))
                .collect();
            assert_eq!(commands.len(), expected, "{name}");
            if expected > 0 {
                let text = commands[0].data.as_str().unwrap();
                let tail_ms = offset.saturating_sub(speech_end) / 16;
                println!("{name}: {tail_ms} ms after last audible sample; {text}");
                if name != "manual" {
                    assert!(
                        (ms as usize - 80..=ms as usize + 300).contains(&tail_ms),
                        "{name}: {tail_ms}"
                    );
                    assert!(
                        offset < pcm.len(),
                        "reached end of fixture without automatic endpoint"
                    );
                }
                if name == "pause" {
                    assert_eq!(text.matches("добрый вечер").count(), 2);
                }
            }
        }
        close_fixture(core, dir);
    }
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
        let mut manual_finish_at = None;
        for name in ["old-long", "old-short", "old-long"] {
            let wave =
                sherpa_onnx::Wave::read(&format!("../jarvis-tts-rs/examples/audio/{name}.wav"))
                    .unwrap();
            assert_eq!(wave.sample_rate(), 24000);
            pcm.extend((0..wave.samples().len() * 2 / 3).map(|i| wave.samples()[i * 3 / 2]));
            if manual_finish_at.is_none() {
                manual_finish_at = pcm.iter().rposition(|v| v.abs() > 0.008).map(|i| i + 256);
            }
            pcm.extend(std::iter::repeat_n(0.0, 24000));
        }
        let mut remaining = pcm.as_slice();
        capture(
            core.clone(),
            session.clone(),
            ListenOptions {
                continuous: true,
                silence_ms: 1000,
                wake_word: false,
            },
            control,
            Some(engine),
            |out| {
                if !remaining.is_empty() {
                    let n = out.len().min(remaining.len());
                    out[..n].copy_from_slice(&remaining[..n]);
                    remaining = &remaining[n..];
                    if manual_finish_at.is_some_and(|i| pcm.len() - remaining.len() >= i) {
                        stop.send(Stop::FinishPhrase).unwrap();
                        manual_finish_at = None;
                    }
                    n
                } else {
                    stop.send(Stop::Discard).unwrap();
                    0
                }
            },
        );
        assert!(manual_finish_at.is_none());
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
        drop(trace);
        close_fixture(core, dir);
    }
}
