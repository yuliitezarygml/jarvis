//! Распознавание речи: трейт `SttEngine` и реализации — `ServerStt` (Xeon, WebSocket + `POST /stt`),
//! `GroqWhisperStt` (облако, `POST {base}/audio/transcriptions`), `LocalStt` (GigaAM на этом ПК) и
//! `HybridStt` (живой текст локально, финал — гонка облако vs локальный). Замер точности — `bench`.

pub mod bench;
mod groq;
pub mod hints;
mod hybrid;
#[cfg(feature = "local-stt")]
mod local;
mod server;
pub mod terms;

pub use groq::GroqWhisperStt;
pub use hybrid::{HybridStt, Winner};
#[cfg(feature = "local-stt")]
pub use local::LocalStt;
pub use jarvis_core::stt_protocol::SttResult;
pub use server::{ServerStt, ServerSttOptions};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Выбрать движок по `config.stt.engine`: `local` (на этом ПК), `server` (Xeon), `groq` / `groq_whisper`
/// (облако) или `hybrid` (живой текст локально, финал — гонка облако vs локальный).
/// `local` без модели или без поддержки в сборке — запасной вариант: сервер (с предупреждением в журнале).
/// Гибрид без локальной модели — только облако, без ключа Groq — только локальный, без обоих — сервер.
pub fn engine_from_config(cfg: &jarvis_core::Config, project_root: &std::path::Path) -> Result<Arc<dyn SttEngine>> {
    engine_from_config_with_hints(cfg, project_root, &[])
}

/// То же + подсказки для облака от приложения (имена из памяти и т. п.) — к `config.stt.hints`
/// и встроенному словарю (`hints::stt_hints`).
pub fn engine_from_config_with_hints(
    cfg: &jarvis_core::Config,
    project_root: &std::path::Path,
    extra_hints: &[String],
) -> Result<Arc<dyn SttEngine>> {
    Ok(match cfg.stt.engine.as_str() {
        "groq_whisper" | "groq" => Arc::new(GroqWhisperStt::from_config_with_hints(cfg, extra_hints)?),
        "local" => match local_engine(cfg, project_root) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "локальный STT недоступен — распознаёт сервер");
                Arc::new(ServerStt::new(&cfg.stt, project_root)?)
            }
        },
        "hybrid" => match hybrid_engine(cfg, project_root, extra_hints) {
            Ok(h) => Arc::new(h),
            Err(e) => {
                tracing::warn!(error = %e, "гибридный STT недоступен — распознаёт сервер");
                Arc::new(ServerStt::new(&cfg.stt, project_root)?)
            }
        },
        _ => Arc::new(ServerStt::new(&cfg.stt, project_root)?),
    })
}

/// Таймаут облака гибрида: из конфига, но так, чтобы финал успел раньше `final_timeout` конвейера (2,5 с).
pub fn hybrid_cloud_timeout(cfg: &jarvis_core::config::SttConfig) -> std::time::Duration {
    std::time::Duration::from_millis(cfg.cloud_timeout_ms.clamp(200, 2_000))
}

/// Гибрид из конфига; недостающая половина — с предупреждением (один раз за запуск).
pub fn hybrid_engine(cfg: &jarvis_core::Config, project_root: &std::path::Path, extra_hints: &[String]) -> Result<HybridStt> {
    static WARNED_LOCAL: AtomicBool = AtomicBool::new(false);
    static WARNED_CLOUD: AtomicBool = AtomicBool::new(false);
    let local = match local_engine(cfg, project_root) {
        Ok(e) => Some(e),
        Err(e) => {
            if !WARNED_LOCAL.swap(true, Ordering::Relaxed) {
                tracing::warn!(error = %e, "гибридный STT: локальной модели нет — распознаёт только облако");
            }
            None
        }
    };
    let cloud: Option<Arc<dyn SttEngine>> = match GroqWhisperStt::from_config_with_hints(cfg, extra_hints) {
        Ok(g) => Some(Arc::new(g)),
        Err(e) => {
            if !WARNED_CLOUD.swap(true, Ordering::Relaxed) {
                tracing::warn!(error = %e, "гибридный STT: нет ключа Groq — распознаёт только локальный движок");
            }
            None
        }
    };
    HybridStt::new(local, cloud, hybrid_cloud_timeout(&cfg.stt))
}

/// Локальный движок (GigaAM) из `data_dir`; ошибка — модели нет или сборка без фичи `local-stt`.
#[cfg(feature = "local-stt")]
pub fn local_engine(cfg: &jarvis_core::Config, project_root: &std::path::Path) -> Result<Arc<dyn SttEngine>> {
    Ok(Arc::new(LocalStt::new(LocalStt::default_dir(project_root, &cfg.data_dir))?))
}

/// Локальный движок (GigaAM) из `data_dir`; ошибка — модели нет или сборка без фичи `local-stt`.
#[cfg(not(feature = "local-stt"))]
pub fn local_engine(_cfg: &jarvis_core::Config, _root: &std::path::Path) -> Result<Arc<dyn SttEngine>> {
    Err(crate::error::VoiceError::Config("сборка без локального распознавания (фича local-stt)".into()))
}
