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
    #[cfg(windows)]
    pub kb: crate::kb::Knowledge,
    #[cfg(windows)]
    pub actions: crate::actions::Actions,
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
            #[cfg(windows)]
            kb: crate::kb::Knowledge::start(dir.to_owned()),
            #[cfg(windows)]
            actions: crate::actions::Actions::new(dir)?,
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
        #[cfg(windows)]
        if let Some(prompt) = context.first_mut().and_then(|m| m.get_mut("content")) {
            if let Some(template) = prompt.as_str() {
                *prompt = json!(template.replace("{{KB_MANIFEST}}", &self.kb.manifest()));
            }
        }
        // Raw deltas share the 40 kB per-turn limit below. Slow audio must never abort the HUD stream.
        let (tx, rx) = mpsc::unbounded_channel();
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
        let mut first = true;
        let mut schemas = if s.tools {
            ToolRegistry::schemas()
                .into_iter()
                .filter(|t| {
                    !s.read_only
                        || crate::tools::risk(t["function"]["name"].as_str().unwrap_or("")) == 0
                })
                .collect()
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
                    let _ = tx.send(value.to_owned());
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
            ensure!(
                s.tools && !schemas.is_empty() && round < 3,
                "Превышен лимит вызовов инструментов"
            );
            context.push(json!({"role":"assistant","content":current,"tool_calls":calls}));
            for call in calls {
                let name = call["function"]["name"]
                    .as_str()
                    .context("Имя инструмента отсутствует")?;
                self.event(id, Kind::Executing, json!(name));
                let args: Value =
                    serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}"))?;
                ensure!(
                    schemas
                        .iter()
                        .any(|schema| schema["function"]["name"] == name),
                    "Модель запросила недоступный инструмент"
                );
                #[cfg(windows)]
                let execution = crate::actions::dispatch(self, id, session, name, &args);
                #[cfg(not(windows))]
                let execution = ToolRegistry::run(self, name, &args);
                let data = tokio::select! {_=cancel.cancelled()=>bail!("Отменено"),r=execution=>r.unwrap_or_else(|e|json!({"error":e.to_string()}))};
                context.push(json!({"role":"tool","tool_call_id":call["id"],"content":serde_json::to_string(&data)?}));
            }
            self.event(id, Kind::Thinking, Value::Null);
        }
        ensure!(!text.trim().is_empty(), "Модель завершила ответ без текста");
        self.for_turn(id, || {
            self.db.lock().unwrap().append(session, "assistant", &text)
        })
        .context("Отменено")??;
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
    #[test]
    #[ignore = "Live Groq, original Rust TTS and real Mac output required"]
    fn formatted_groq_answer_reaches_output() {
        tauri::async_runtime::block_on(async {
            dotenvy::from_path(std::env::var_os("JARVIS_BACKEND_ENV").unwrap()).unwrap();
            let folder = std::env::temp_dir().join(format!("jarvis-formatted-{}", Uuid::new_v4()));
            let (events, mut rx) = mpsc::unbounded_channel();
            let c = Core::open(
                &folder,
                Arc::new(move |e| {
                    let _ = events.send(serde_json::to_value(e).unwrap());
                }),
            )
            .unwrap();
            let s = c.db.lock().unwrap().new_session().unwrap();
            let prompt="Ответь на русском. Для проверки формата воспроизведи следующий Markdown дословно, без блока кода и без вступления:\n## Проверка связи\nСначала проверим защищённое соединение с сервером.\n\n- Wi-Fi поддерживает WPA3.\n- VPN-протоколы защищают данные.\n\n| Формула | Результат |\n|---|---|\n| 3³ | 27 |\n| √16 | 4 |\n\nИтак, 2^10 = 1024. Проверка завершена.";
            c.start(s.clone(), prompt.into()).unwrap();
            let mut display = String::new();
            let mut speaking = false;
            let mut empty_since = None;
            let mut gaps = Vec::new();
            let mut blocks = 0;
            let mut max_queue = 0;
            let mut poll = tokio::time::interval(std::time::Duration::from_millis(5));
            tokio::time::timeout(std::time::Duration::from_secs(120),async {
                loop {
                    tokio::select! {
                        Some(e)=rx.recv()=> match e["kind"].as_str().unwrap() {
                            "first_text" => println!("Groq first delta {} ms", e["data"]),
                            "delta" =>display.push_str(e["data"].as_str().unwrap()),
                            "speaking"=>{speaking=true;println!("First PCM {} ms",e["data"]["ms"])},
                            "level"=> blocks+=1,
                            "error"|"warning"=>panic!("{}",e["data"]),
                            "done"=>break,
                            _=>()
                        },
                        _=poll.tick(),if speaking=> {
                            let q=audio::pending();
                            max_queue=max_queue.max(q);
                            if q==0 { empty_since.get_or_insert_with(Instant::now); }
                            else if let Some(at)=empty_since.take(){ gaps.push(at.elapsed().as_millis()); }
                        }
                    }
                }
            }).await.unwrap();
            println!("Queued {blocks} PCM buffers, peak {max_queue}, empty intervals (ms): {gaps:?}; HUD: {display}");
            assert!(speaking && blocks > 5 && max_queue <= 5 && audio::pending() == 0);
            assert!(
                display.contains("Wi-Fi")
                    && display.contains("WPA3")
                    && display.contains('|')
                    && display.contains("2^10")
            );
            let history = c.db.lock().unwrap().history(&s, 10).unwrap();
            assert_eq!(history[1]["content"], display);
            drop(c);
            let _ = std::fs::remove_dir_all(folder);
        });
    }

    #[test]
    #[ignore = "Requires Xeon TTS and real Mac audio output"]
    fn speech_starts_while_text_input_is_still_open() {
        tauri::async_runtime::block_on(async {
            dotenvy::from_path(std::env::var_os("JARVIS_BACKEND_ENV").unwrap()).unwrap();
            let folder = std::env::temp_dir().join(format!("jarvis-stream-{}", Uuid::new_v4()));
            let (started, mut playback) = mpsc::unbounded_channel();
            let core = Core::open(
                &folder,
                Arc::new(move |event| {
                    if matches!(event.kind, Kind::Speaking) {
                        let _ = started.send(event.data);
                    }
                }),
            )
            .unwrap();
            let turn = Uuid::new_v4().to_string();
            let cancel = CancellationToken::new();
            *core.active.lock().unwrap() = Some(Turn {
                id: turn.clone(),
                session: "stream-test".into(),
                cancel: cancel.clone(),
            });
            let (input, queue) = mpsc::unbounded_channel();
            let worker = tokio::spawn(tts::speech(
                core.clone(),
                turn,
                cancel,
                queue,
                Instant::now(),
            ));
            for piece in ["Первый пример: $2^", "{10}", " = 1024$. "] {
                input.send(piece.to_owned()).unwrap()
            }
            let first = tokio::time::timeout(std::time::Duration::from_secs(15), playback.recv())
                .await
                .unwrap()
                .unwrap();
            println!("First PCM with final delta withheld: {} ms", first["ms"]);
            input.send("Теперь √16 = 4.".into()).unwrap();
            drop(input);
            tokio::time::timeout(std::time::Duration::from_secs(45), worker)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(audio::pending(), 0);
            let heard = core.playback_phrases.lock().unwrap().clone();
            assert_eq!(heard.len(), 2);
            assert_eq!(
                heard[0],
                normalize_speech(
                    "Первый пример: два в десятой степени равно тысяче двадцати четырём."
                )
            );
            assert_eq!(
                heard[1],
                normalize_speech("Теперь квадратный корень из шестнадцати равен четырём.")
            );
            drop(core);
            let _ = std::fs::remove_dir_all(folder);
        });
    }

    /// Tests the same queue, provider, STT, audio renderer and event stream as the desktop commands.
    /// Uses a fresh SQLite database, existing operator credentials and the original audio fixture.
    #[test]
    #[ignore = "Live Groq, HTTPS TTS, Mac output device and STT assets required"]
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
