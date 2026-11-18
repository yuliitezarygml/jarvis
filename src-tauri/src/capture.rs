use crate::events::{Event, Kind};
use crate::{audio, core::Core};
use anyhow::{ensure, Result};
use serde_json::json;
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
    task: tauri::async_runtime::JoinHandle<()>,
}
pub async fn end(c: &Arc<Core>, submit: bool) {
    let mut g = c.capture.lock().await;
    if let Some(job) = g.take() {
        let _ = job
            .stop
            .send(if submit { Stop::Submit } else { Stop::Discard });
        let _ = job.task.await;
    }
}
pub async fn start(c: &Arc<Core>, session: String, continuous: bool) -> Result<()> {
    end(c, false).await;
    c.cancel();
    ensure!(c.stt.available(), "Русская модель STT не установлена");
    let mut g = c.capture.lock().await;
    tokio::task::spawn_blocking(move || audio::mic_start(continuous)).await??;
    let (tx, rx) = watch::channel(Stop::Live);
    let core = c.clone();
    let handle = tauri::async_runtime::spawn(async move {
        capture(core, session, continuous, rx).await;
    });
    *g = Some(Capture {
        stop: tx,
        task: handle,
    });
    Ok(())
}
async fn capture(
    c: Arc<Core>,
    session: String,
    continuous: bool,
    mut control: watch::Receiver<Stop>,
) {
    let emit =
        |kind: Kind, data: serde_json::Value| (c.bus)(Event::new(&session, None, kind, data));
    emit(Kind::Mic, json!(if continuous { "auto" } else { "ptt" }));
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
            break;
        }
        let n = audio::read(&mut buffer);
        let chunk = &buffer[..n];
        let level = audio::level(chunk);
        if n > 0 {
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
                }
            } else {
                utterance.extend_from_slice(chunk);
            }
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
                let core = c.clone();
                let data = std::mem::take(&mut utterance);
                let result = tokio::task::spawn_blocking(move || core.stt.recognize(&data)).await;
                if *control.borrow() == Stop::Discard {
                    break;
                }
                match result {
                    Ok(Ok(text)) if !text.is_empty() => {
                        emit(Kind::Transcript, json!(text));
                        if let Err(e) = c.start(session.clone(), text) {
                            emit(Kind::MicError, json!(e.to_string()));
                        }
                    }
                    Ok(Ok(_)) => emit(Kind::MicError, json!("Речь не распознана")),
                    Ok(Err(e)) => emit(Kind::MicError, json!(e.to_string())),
                    Err(_) => emit(Kind::MicError, json!("Ошибка распознавания")),
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
            while audio::read(&mut buffer) > 0 {}
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(20))=>(),_=control.changed()=>()}
    }
    if capture_open {
        audio::mic_end();
        emit(Kind::Mic, json!("off"));
    }
}
