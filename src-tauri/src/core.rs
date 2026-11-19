use crate::events::{Event, Kind};
use crate::{
    audio::{self, Recognizer},
    provider::{self, Compatible, LlmProvider},
    storage::Store,
    tools::{Telemetry, ToolRegistry},
    tts,
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
type Bus = Arc<dyn Fn(Event) + Send + Sync>;
struct Turn {
    id: String,
    session: String,
    cancel: CancellationToken,
}
pub struct Core {
    pub db: Mutex<Store>,
    pub http: reqwest::Client,
    pub llm: Arc<dyn LlmProvider>,
    pub telemetry: Telemetry,
    pub stt: Recognizer,
    active: Mutex<Option<Turn>>,
    playback_phrases: Mutex<std::collections::VecDeque<String>>,
    pub capture: tokio::sync::Mutex<Option<crate::capture::Capture>>,
    pub bus: Bus,
}
impl Core {
    pub fn open(dir: &Path, bus: Bus) -> Result<Arc<Self>> {
        std::fs::create_dir_all(dir)?;
        let env = dir.join("backend.env");
        if env.exists() {
            dotenvy::from_path(env)?;
        }
        let http = provider::client()?;
        Ok(Arc::new(Self {
            db: Mutex::new(Store::open(&dir.join("jarvis.sqlite3"))?),
            http: tts::client()?,
            llm: Arc::new(Compatible(http)),
            telemetry: Telemetry::new(),
            stt: Recognizer::new(
                std::env::var_os("JARVIS_STT_DIR")
                    .map(Into::into)
                    .unwrap_or_else(|| dir.join("stt")),
            ),
            active: Mutex::new(None),
            playback_phrases: Default::default(),
            capture: Default::default(),
            bus,
        }))
    }
    pub fn for_turn<T>(&self, id: &str, run: impl FnOnce() -> T) -> Option<T> {
        let g = self.active.lock().unwrap();
        g.as_ref()
            .filter(|t| t.id == id && !t.cancel.is_cancelled())
            .map(|_| run())
    }
    pub fn event(&self, id: &str, kind: Kind, data: Value) {
        let g = self.active.lock().unwrap();
        if let Some(t) = g
            .as_ref()
            .filter(|t| t.id == id && !t.cancel.is_cancelled())
        {
            (self.bus)(Event::new(&t.session, Some(id), kind, data));
        }
    }
    pub fn cancel(&self) {
        let mut g = self.active.lock().unwrap();
        if let Some(t) = g.take() {
            t.cancel.cancel();
            audio::stop();
            (self.bus)(Event::new(
                &t.session,
                Some(&t.id),
                Kind::Cancelled,
                Value::Null,
            ));
        }
    }
    pub fn remember_playback(&self, text: &str) {
        let mut recent = self.playback_phrases.lock().unwrap();
        if recent.len() == 3 {
            recent.pop_front();
        }
        recent.push_back(normalize_speech(text));
    }
    pub fn is_playback_echo(&self, text: &str) -> bool {
        let heard = normalize_speech(text);
        let recent = self.playback_phrases.lock().unwrap();
        let reference = recent
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        if heard.is_empty() || reference.contains(&heard) {
            return true;
        }
        let mut total = 0;
        let mut hits = 0;
        for word in heard.split_whitespace() {
            total += 1;
            if reference.split_whitespace().any(|w| w == word) {
                hits += 1;
            }
        }
        total >= 3 && hits * 4 >= total * 3
    }
    pub fn start(self: &Arc<Self>, session: String, text: String) -> Result<String> {
        let text = text.trim();
        ensure!(
            !text.is_empty() && text.chars().count() <= 4000,
            "Допустимо от 1 до 4000 символов"
        );
        let id = Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        let (settings, messages) = {
            let mut g = self.active.lock().unwrap();
            let mut db = self.db.lock().unwrap();
            let s = db.settings()?;
            db.append(&session, "user", text)?;
            let rows = db.context(&session, &s)?;
            if let Some(t) = g.take() {
                t.cancel.cancel();
                audio::stop();
            }
            *g = Some(Turn {
                id: id.clone(),
                session: session.clone(),
                cancel: token.clone(),
            });
            (self.bus)(Event::new(&session, Some(&id), Kind::Start, json!(text)));
            (s, rows)
        };
        let core = self.clone();
        let turn = id.clone();
        tauri::async_runtime::spawn(async move {
            let result = core
                .answer(&turn, &session, settings, messages, token.clone())
                .await;
            let mut g = core.active.lock().unwrap();
            if g.as_ref().is_some_and(|t| t.id == turn) {
                let (kind, data) = match result {
                    Ok(()) => (Kind::Done, Value::Null),
                    Err(e) => {
                        audio::stop();
                        (Kind::Error, json!(format!("{e:#}")))
                    }
                };
                (core.bus)(Event::new(&session, Some(&turn), kind, data));
                g.take();
            }
            token.cancel();
        });
        Ok(id)
    }
    async fn answer(
        self: &Arc<Self>,
        id: &str,
        session: &str,
        s: crate::storage::Settings,
        mut context: Vec<Value>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let start = Instant::now();
        let (tx, rx) = mpsc::channel(48);
        let speaker = if s.speak {
            let core = self.clone();
            let turn = id.to_owned();
            let token = cancel.child_token();
            Some(tauri::async_runtime::spawn(async move {
                if let Err(e) = tts::speech(core.clone(), turn.clone(), token, rx, start).await {
                    core.for_turn(&turn, audio::stop);
                    core.event(&turn, Kind::Warning, json!(format!("{e:#}")));
                }
            }))
        } else {
            drop(rx);
            None
        };
        let mut text = String::new();
        let mut splitter = tts::Sentences::new();
        let mut first = true;
        let mut schemas = if s.tools {
            ToolRegistry::schemas()
        } else {
            vec![]
        };
        for round in 0..4 {
            let mut current = String::new();
            let mut callback = |value: &str| -> Result<()> {
                ensure!(text.len() + value.len() < 40000, "Превышен лимит ответа");
                if value.is_empty() {
                    return Ok(());
                }
                if first {
                    first = false;
                    self.event(id, Kind::FirstText, json!(start.elapsed().as_millis()));
                }
                text.push_str(value);
                current.push_str(value);
                self.event(id, Kind::Delta, json!(value));
                if s.speak && !tx.is_closed() {
                    for sentence in splitter.push(value, false) {
                        match tx.try_send(sentence) {
                            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => (),
                            Err(_) => bail!("Очередь озвучивания заполнена"),
                        }
                    }
                }
                Ok(())
            };
            let result = self
                .llm
                .stream(&s, &context, &schemas, &cancel, &mut callback)
                .await;
            let calls = match result {
                Err(e) if round == 0 && !schemas.is_empty() && first && !cancel.is_cancelled() => {
                    self.event(
                        id,
                        Kind::Warning,
                        json!(format!("Инструменты недоступны ({e}). Повторяю без них.")),
                    );
                    schemas.clear();
                    continue;
                }
                other => other?,
            };
            if calls.is_empty() {
                break;
            }
            ensure!(s.tools && round < 3, "Превышен лимит вызовов инструментов");
            context.push(json!({"role":"assistant","content":current,"tool_calls":calls}));
            for call in calls {
                let name = call["function"]["name"]
                    .as_str()
                    .context("Имя инструмента отсутствует")?;
                self.event(id, Kind::Executing, json!(name));
                let args: Value =
                    serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}"))?;
                let data = tokio::select! {_=cancel.cancelled()=>bail!("Отменено"),r=ToolRegistry::run(self,name,&args)=>r.unwrap_or_else(|e|json!({"error":e.to_string()}))};
                context.push(json!({"role":"tool","tool_call_id":call["id"],"content":serde_json::to_string(&data)?}));
            }
            self.event(id, Kind::Thinking, Value::Null);
        }
        ensure!(!text.trim().is_empty(), "Модель завершила ответ без текста");
        self.for_turn(id, || {
            self.db.lock().unwrap().append(session, "assistant", &text)
        })
        .context("Отменено")??;
        for sentence in splitter.push("", true) {
            if tx.send(sentence).await.is_err() {
                break;
            }
        }
        drop(tx);
        if let Some(worker) = speaker {
            worker.await?;
        }
        Ok(())
    }
}

fn normalize_speech(text: &str) -> String {
    text.to_lowercase()
        .replace('ё', "е")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod acceptance {
    use super::*;
    /// Tests the same queue, provider, STT, audio renderer and event stream as the desktop commands.
    /// Uses a fresh SQLite database, existing operator credentials and the original audio fixture.
    #[test]
    #[ignore = "Live Groq, forwarded TTS, Mac output device and STT assets required"]
    fn recorded_russian_turn_reaches_speakers() {
        tauri::async_runtime::block_on(async {
            let config = std::env::var_os("JARVIS_BACKEND_ENV").expect("JARVIS_BACKEND_ENV");
            dotenvy::from_path(config).unwrap();
            let folder = std::env::temp_dir().join(format!("jarvis-acceptance-{}", Uuid::new_v4()));
            let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let c = Core::open(
                &folder,
                Arc::new(move |v| {
                    let _ = events.send(serde_json::to_value(v).unwrap());
                }),
            )
            .unwrap();
            let wave =
                sherpa_onnx::Wave::read("../jarvis-tts-rs/examples/audio/old-short.wav").unwrap();
            let pcm: Vec<f32> = (0..wave.samples().len() * 2 / 3)
                .map(|i| wave.samples()[i * 3 / 2])
                .collect();
            let before = Instant::now();
            let text = c.stt.recognize(&pcm).unwrap();
            println!("STT {} ms, {}", before.elapsed().as_millis(), text);
            assert!(text.contains("добрый вечер"));
            let s = c.db.lock().unwrap().new_session().unwrap();
            c.start(s.clone(), text).unwrap();
            let mut voice = false;
            let mut first = false;
            tokio::time::timeout(std::time::Duration::from_secs(75), async {
                while let Some(e) = rx.recv().await {
                    match e["kind"].as_str().unwrap() {
                        "first_text" => {
                            first = true;
                            println!("LLM first text {} ms", e["data"])
                        }
                        "speaking" => {
                            voice = true;
                            println!(
                                "First PCM queued for actual playback {} ms",
                                e["data"]["ms"]
                            )
                        }
                        "error" | "warning" => panic!("Desktop audio turn: {}", e["data"]),
                        "done" => break,
                        _ => (),
                    }
                }
            })
            .await
            .unwrap();
            assert!(first && voice);
            assert_eq!(audio::pending(), 0);
            let rows = c.db.lock().unwrap().history(&s, 10).unwrap();
            assert_eq!(rows.len(), 2);
            println!("Groq: {}", rows[1]["content"]);
            println!(
                "STT + completed playback {} ms",
                before.elapsed().as_millis()
            );
            let interrupted = c
                .start(
                    s.clone(),
                    "Назови четыре планеты полными предложениями.".into(),
                )
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(45), async {
                loop {
                    let e = rx.recv().await.unwrap();
                    if e["turn"] == interrupted && e["kind"] == "speaking" {
                        break;
                    }
                    assert!(e["kind"] != "error" && e["kind"] != "warning", "{e}");
                }
            })
            .await
            .unwrap();
            let stopped = Instant::now();
            c.cancel();
            println!(
                "Interrupt after first PCM: {} ms, {} buffers",
                stopped.elapsed().as_millis(),
                audio::pending()
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            assert_eq!(audio::pending(), 0);
            drop(c);
            let _ = std::fs::remove_dir_all(folder);
        })
    }
}
