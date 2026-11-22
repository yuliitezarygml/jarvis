//! Озвучка: `SpeechSink` агента → очередь реплик → `VoicePipeline::speak`.
//!
//! `VoicePipeline::speak` прерывает предыдущую речь, поэтому реплики (каждое сообщение модели, вопрос
//! подтверждения, предупреждение resource guard) идут в одну очередь и озвучиваются по порядку отдельной задачей.
//! `cancel()` (стоп, новая команда, Ctrl+Alt+K) выбрасывает очередь и обрывает текущую речь (барж-ин).
//! Без голоса (сервер/динамики недоступны) реплики просто выбрасываются — субтитры агент шлёт сам.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use jarvis_agent::SpeechSink;
use jarvis_core::{AssistantState, Event, EventSink, Speaker};
use jarvis_voice::VoicePipeline;
use tokio::sync::mpsc;

type Utterance = (u64, mpsc::UnboundedReceiver<String>);

pub struct SpeechQueue {
    voice: RwLock<Option<VoicePipeline>>,
    current: Mutex<Option<mpsc::UnboundedSender<String>>>,
    queue: mpsc::UnboundedSender<Utterance>,
    generation: AtomicU64,
    pending: AtomicU64,
    last_end: Mutex<Option<Instant>>,
}

impl SpeechQueue {
    /// Создать очередь; задача озвучки запускается на `handle`.
    pub fn new(handle: &tokio::runtime::Handle) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let q = Arc::new(Self {
            voice: RwLock::new(None),
            current: Mutex::new(None),
            queue: tx,
            generation: AtomicU64::new(0),
            pending: AtomicU64::new(0),
            last_end: Mutex::new(None),
        });
        handle.spawn(worker(Arc::downgrade(&q), rx));
        q
    }

    pub fn set_voice(&self, voice: Option<VoicePipeline>) {
        if let Some(old) = self.voice.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
            old.barge_in();
        }
        *self.voice.write().unwrap_or_else(|e| e.into_inner()) = voice;
    }

    pub fn voice(&self) -> Option<VoicePipeline> {
        self.voice.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Сказать готовую фразу целиком (после уже поставленных в очередь).
    pub fn say(&self, text: &str) {
        self.delta(text);
        self.flush();
    }

    /// Режим «жду да, сэр» в голосовом фильтре.
    pub fn set_awaiting_confirm(&self, on: bool) {
        if let Some(v) = self.voice() {
            v.set_awaiting_confirm(on);
        }
    }

    /// Когда закончилась речь TTS (для `ConfirmGate`): `None` — ещё говорит или есть очередь.
    pub fn tts_finished_at(&self) -> Option<Instant> {
        let speaking = self.voice().is_some_and(|v| v.is_speaking());
        if speaking || self.pending.load(Ordering::Acquire) > 0 {
            return None;
        }
        Some(self.last_end.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_else(|| Instant::now() - Duration::from_secs(10)))
    }
}

impl SpeechSink for SpeechQueue {
    fn delta(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if cur.is_none() {
            let (tx, rx) = mpsc::unbounded_channel();
            self.pending.fetch_add(1, Ordering::AcqRel);
            if self.queue.send((self.generation.load(Ordering::Acquire), rx)).is_err() {
                self.pending.fetch_sub(1, Ordering::AcqRel);
                return;
            }
            *cur = Some(tx);
        }
        if let Some(tx) = cur.as_ref() {
            let _ = tx.send(text.to_string());
        }
    }

    fn flush(&self) {
        // Закрытый канал = конец реплики: чанкер дочитает хвост фразы.
        self.current.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    fn cancel(&self) {
        self.current.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(v) = self.voice() {
            v.barge_in();
        }
    }
}

async fn worker(q: std::sync::Weak<SpeechQueue>, mut rx: mpsc::UnboundedReceiver<Utterance>) {
    while let Some((generation, mut deltas)) = rx.recv().await {
        let Some(q) = q.upgrade() else { return };
        let stale = generation != q.generation.load(Ordering::Acquire);
        match q.voice() {
            Some(voice) if !stale => {
                let (btx, brx) = mpsc::channel::<String>(64);
                let forward = tokio::spawn(async move {
                    while let Some(d) = deltas.recv().await {
                        if btx.send(d).await.is_err() {
                            break;
                        }
                    }
                });
                if let Err(e) = voice.speak(brx).await {
                    eprintln!("[jarvis] озвучка: {e}");
                }
                forward.abort();
                *q.last_end.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            }
            _ => {
                // Без голоса или реплика отменена — дочитать и выбросить, не задерживая очередь.
                drop(deltas);
            }
        }
        q.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Фильтр событий голосового конвейера на пути в HUD.
///
/// - субтитры Джарвиса шлёт агент (полный текст ответа); пофразовые субтитры конвейера убираем, чтобы не мигали;
/// - пока агент работает, `State` от конвейера не перебивает состояние агента (thinking/controlling);
/// - одинаковые ошибки (недоступный TTS даёт ошибку на каждую фразу) — не чаще раза в 30 с.
pub struct VoiceEvents {
    inner: Arc<dyn EventSink>,
    busy: Arc<AtomicBool>,
    recent_errors: Mutex<HashMap<String, Instant>>,
}

impl VoiceEvents {
    pub fn new(inner: Arc<dyn EventSink>, busy: Arc<AtomicBool>) -> Self {
        Self { inner, busy, recent_errors: Mutex::new(HashMap::new()) }
    }
}

const ERROR_REPEAT: Duration = Duration::from_secs(30);

impl EventSink for VoiceEvents {
    fn emit(&self, event: Event) {
        match &event {
            Event::Subtitle { speaker: Speaker::Jarvis, .. } => return,
            Event::State { state } if self.busy.load(Ordering::Acquire) && *state != AssistantState::Listening => return,
            Event::Error { message } => {
                let mut recent = self.recent_errors.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                if recent.get(message).is_some_and(|t| now.duration_since(*t) < ERROR_REPEAT) {
                    return;
                }
                recent.retain(|_, t| now.duration_since(*t) < ERROR_REPEAT);
                recent.insert(message.clone(), now);
            }
            _ => {}
        }
        self.inner.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::CollectSink;

    #[test]
    fn voice_events_filter() {
        let sink = Arc::new(CollectSink::default());
        let busy = Arc::new(AtomicBool::new(true));
        let f = VoiceEvents::new(sink.clone(), busy.clone());
        f.emit(Event::Subtitle { speaker: Speaker::Jarvis, text: "фраза".into(), is_final: true });
        f.emit(Event::Subtitle { speaker: Speaker::User, text: "Джарвис, привет".into(), is_final: true });
        f.emit(Event::State { state: AssistantState::Idle });
        f.emit(Event::State { state: AssistantState::Listening });
        f.emit(Event::Error { message: "Синтез речи: нет связи".into() });
        f.emit(Event::Error { message: "Синтез речи: нет связи".into() });
        busy.store(false, Ordering::Release);
        f.emit(Event::State { state: AssistantState::Idle });
        let ev = sink.take();
        assert_eq!(ev.len(), 4, "{ev:?}");
        assert!(matches!(&ev[0], Event::Subtitle { speaker: Speaker::User, .. }));
        assert_eq!(ev[1], Event::State { state: AssistantState::Listening });
        assert!(matches!(&ev[2], Event::Error { .. }));
        assert_eq!(ev[3], Event::State { state: AssistantState::Idle });
    }

    #[tokio::test]
    async fn queue_without_voice_drains_and_reports_finished() {
        let q = SpeechQueue::new(&tokio::runtime::Handle::current());
        q.delta("Привет, ");
        q.delta("сэр.");
        q.flush();
        q.say("Вторая реплика.");
        for _ in 0..50 {
            if q.tts_finished_at().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(q.tts_finished_at().is_some());
        q.cancel();
        assert!(q.tts_finished_at().is_some());
    }
}
