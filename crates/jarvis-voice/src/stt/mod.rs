//! Распознавание речи: трейт `SttEngine` и реализации — `ServerStt` (Xeon, WebSocket + `POST /stt`)
//! и `GroqWhisperStt` (резерв, `POST {base}/audio/transcriptions`).

mod groq;
mod server;

pub use groq::GroqWhisperStt;
pub use jarvis_core::stt_protocol::SttResult;
pub use server::{ServerStt, ServerSttOptions};

use async_trait::async_trait;
use jarvis_core::CancellationToken;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::error::Result;

/// События потокового распознавания.
#[derive(Debug, Clone, PartialEq)]
pub enum SttEvent {
    /// Соединение установлено, сервер назвал модель.
    Ready { model: String },
    /// Промежуточный текст текущей фразы (может меняться).
    Partial { text: String },
    /// Окончательный текст фразы (ответ на `end()`).
    Final { text: String, duration_ms: u64 },
    /// Ошибка от сервера (соединение живо).
    Error { message: String },
    /// Соединение потеряно; клиент переподключается сам и досылает звук текущей фразы.
    Disconnected { reason: String },
}

#[derive(Debug)]
pub(crate) enum StreamCmd {
    Audio(Vec<i16>),
    End,
    Reset,
    Close,
}

/// Ручка потоковой сессии. Команды не блокируют (очередь без ограничения: звук идёт в реальном времени).
pub struct SttStream {
    tx: mpsc::UnboundedSender<StreamCmd>,
    task: JoinHandle<()>,
}

impl SttStream {
    pub(crate) fn new(tx: mpsc::UnboundedSender<StreamCmd>, task: JoinHandle<()>) -> Self {
        Self { tx, task }
    }

    /// Кадр PCM s16 16 кГц mono. `false` — сессия закрыта.
    pub fn send_audio(&self, pcm: &[i16]) -> bool {
        self.tx.send(StreamCmd::Audio(pcm.to_vec())).is_ok()
    }

    /// Конец фразы: сервер ответит `Final`.
    pub fn end(&self) -> bool {
        self.tx.send(StreamCmd::End).is_ok()
    }

    /// Сбросить фразу без результата (барж-ин, слишком короткий звук).
    pub fn reset(&self) -> bool {
        self.tx.send(StreamCmd::Reset).is_ok()
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed() || self.task.is_finished()
    }

    pub fn close(self) {
        // Drop сделает всё нужное.
    }
}

impl Drop for SttStream {
    fn drop(&mut self) {
        let _ = self.tx.send(StreamCmd::Close);
        // Задача сама закроет WebSocket; если зависла на сети — снять через 2 с.
        let abort = self.task.abort_handle();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                abort.abort();
            });
        }
    }
}

/// Любой распознаватель: сервер Xeon, Groq Whisper, подмена в тестах.
#[async_trait]
pub trait SttEngine: Send + Sync {
    fn name(&self) -> &str;

    /// Распознать целую фразу (PCM s16 16 кГц mono).
    async fn transcribe(&self, pcm16k: &[i16], cancel: &CancellationToken) -> Result<SttResult>;

    /// Потоковая сессия, если движок её поддерживает. События приходят в `events`.
    fn open_stream(&self, _events: mpsc::Sender<SttEvent>) -> Option<SttStream> {
        None
    }
}

/// Выбрать движок по `config.stt.engine`: `server` (по умолчанию) или `groq_whisper`.
pub fn engine_from_config(cfg: &jarvis_core::Config, project_root: &std::path::Path) -> Result<std::sync::Arc<dyn SttEngine>> {
    Ok(match cfg.stt.engine.as_str() {
        "groq_whisper" | "groq" => std::sync::Arc::new(GroqWhisperStt::from_config(cfg)?),
        _ => std::sync::Arc::new(ServerStt::new(&cfg.stt, project_root)?),
    })
}
