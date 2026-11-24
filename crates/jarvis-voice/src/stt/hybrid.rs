//! Гибридное распознавание «как у Claude Code»: живой текст во время речи — от локального движка
//! (GigaAM на этом ПК), финал — гонка облако (Groq Whisper) vs локальный на том же звуке.
//!
//! Правило финала: облачный текст берётся, если пришёл за `cloud_timeout` после конца фразы и выглядит
//! здраво (не пустой, не типичная «галлюцинация» Whisper); иначе — локальный. Облачный запрос отменяется,
//! когда он уже не нужен (таймаут, отмена фразы, закрытие сессии). Нет локальной модели — только облако;
//! нет облака — только локальный.
//!
//! Таймаут облака (1,5 с по умолчанию) меньше `final_timeout` конвейера (2,5 с), поэтому финал успевает
//! прийти потоком и конвейер не переотправляет фразу через `transcribe`.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jarvis_core::CancellationToken;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{SttEngine, SttEvent, SttResult, SttStream, StreamCmd};
use crate::error::{Result, VoiceError};

/// Какой движок дал финальный текст.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Winner {
    Cloud,
    /// Локальный; причина — почему не облако.
    Local(String),
}

pub struct HybridStt {
    local: Option<Arc<dyn SttEngine>>,
    cloud: Option<Arc<dyn SttEngine>>,
    cloud_timeout: Duration,
    name: String,
}

impl HybridStt {
    /// Хотя бы один из движков обязателен.
    pub fn new(local: Option<Arc<dyn SttEngine>>, cloud: Option<Arc<dyn SttEngine>>, cloud_timeout: Duration) -> Result<Self> {
        let name = match (&local, &cloud) {
            (Some(l), Some(c)) => format!("hybrid({}+{})", l.name(), c.name()),
            (Some(l), None) => format!("hybrid({})", l.name()),
            (None, Some(c)) => format!("hybrid({})", c.name()),
            (None, None) => return Err(VoiceError::Config("гибрид: нет ни локальной модели, ни ключа Groq".into())),
        };
        Ok(Self { local, cloud, cloud_timeout, name })
    }

    pub fn has_local(&self) -> bool {
        self.local.is_some()
    }

    pub fn has_cloud(&self) -> bool {
        self.cloud.is_some()
    }

    pub fn cloud_timeout(&self) -> Duration {
        self.cloud_timeout
    }
}

/// Облачный текст годится? `Err(причина)` — нет.
pub(crate) fn cloud_text_ok(text: &str, audio_ms: u64) -> std::result::Result<(), &'static str> {
    let t = text.trim();
    if !t.chars().any(char::is_alphanumeric) {
        return Err("пустой ответ");
    }
    let low = t.to_lowercase().replace('ё', "е");
    let low = low.trim_end_matches(['.', '!', ' ', '…']);
    // Типичные «галлюцинации» Whisper на тишине/шуме (из субтитров обучающих данных).
    const HALLUCINATIONS: &[&str] =
        &["продолжение следует", "субтитры сделал", "субтитры создавал", "редактор субтитров", "корректор", "подписывайтесь на канал"];
    if HALLUCINATIONS.iter().any(|h| low.starts_with(h)) || low == "спасибо за просмотр" || low.contains("dimatorzok") {
        return Err("галлюцинация облака");
    }
    // Речь — не больше ~25 символов в секунду; заметно длиннее — выдумано.
    if t.chars().count() as u64 > 40 + audio_ms * 30 / 1000 {
        return Err("слишком длинный ответ");
    }
    Ok(())
}

/// Гонка финала: `local` — локальный результат (уже запущен), облако запускается здесь на `pcm`.
/// `cancel` — отмена всей фразы, `cloud_cancel` — отмена только облака (тогда идёт локальный текст).
async fn race_final<L>(
    local: L,
    cloud: &dyn SttEngine,
    pcm: &[i16],
    timeout: Duration,
    cancel: &CancellationToken,
    cloud_cancel: CancellationToken,
) -> Result<(SttResult, Winner)>
where
    L: Future<Output = Result<SttResult>>,
{
    let t0 = Instant::now();
    let audio_ms = pcm.len() as u64 / 16;
    let cloud_token = cloud_cancel.clone();
    let cloud_fut = async {
        match tokio::time::timeout(timeout, cloud.transcribe(pcm, &cloud_token)).await {
            Err(_) => Err("таймаут".to_string()),
            Ok(Err(VoiceError::Cancelled)) => Err("отменено".to_string()),
            Ok(Err(e)) => Err(format!("ошибка: {e}")),
            Ok(Ok(r)) => match cloud_text_ok(&r.text, audio_ms) {
                Ok(()) => Ok(r),
                Err(why) => Err(why.to_string()),
            },
        }
    };
    tokio::pin!(cloud_fut);
    tokio::pin!(local);
    let mut cloud_res: Option<std::result::Result<SttResult, String>> = None;
    let mut local_res: Option<(Result<SttResult>, u64)> = None;
    loop {
        if matches!(cloud_res, Some(Ok(_))) || (cloud_res.is_some() && local_res.is_some()) {
            break;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                cloud_token.cancel();
                return Err(VoiceError::Cancelled);
            }
            r = &mut cloud_fut, if cloud_res.is_none() => cloud_res = Some(r),
            r = &mut local, if local_res.is_none() => local_res = Some((r, t0.elapsed().as_millis() as u64)),
        }
    }
    // Облако больше не нужно в любом случае (выиграло — уже ответило; проиграло — снять запрос).
    cloud_token.cancel();
    let local_ms = local_res.as_ref().map(|(_, ms)| *ms);
    match cloud_res {
        Some(Ok(r)) => {
            let cloud_ms = t0.elapsed().as_millis() as u64;
            tracing::info!(engine = "cloud", cloud_ms, ?local_ms, audio_ms, "распознавание: облако {cloud_ms} мс");
            Ok((r, Winner::Cloud))
        }
        Some(Err(why)) => {
            let (res, ms) = local_res.expect("цикл выходит только с локальным результатом");
            match res {
                Ok(r) => {
                    tracing::info!(engine = "local", local_ms = ms, audio_ms, "распознавание: локально {ms} мс (облако: {why})");
                    Ok((r, Winner::Local(why)))
                }
                Err(e) => {
                    tracing::warn!(error = %e, cloud = %why, "распознавание: не удалось ни облаку, ни локально");
                    Err(e)
                }
            }
        }
        None => unreachable!("цикл выходит только с облачным результатом"),
    }
}

#[async_trait]
impl SttEngine for HybridStt {
    fn name(&self) -> &str {
        &self.name
    }

    async fn transcribe(&self, pcm16k: &[i16], cancel: &CancellationToken) -> Result<SttResult> {
        match (&self.local, &self.cloud) {
            (Some(local), Some(cloud)) => {
                let local_fut = local.transcribe(pcm16k, cancel);
                race_final(local_fut, cloud.as_ref(), pcm16k, self.cloud_timeout, cancel, cancel.child_token()).await.map(|(r, _)| r)
            }
            (Some(e), None) | (None, Some(e)) => e.transcribe(pcm16k, cancel).await,
            (None, None) => Err(VoiceError::Config("гибрид без движков".into())),
        }
    }

    fn open_stream(&self, events: mpsc::Sender<SttEvent>) -> Option<SttStream> {
        let (local, cloud) = match (&self.local, &self.cloud) {
            (Some(l), Some(c)) => (l.clone(), c.clone()),
            // Один движок — его собственный поток (у Groq потока нет: конвейер пойдёт через `transcribe`).
            (Some(e), None) | (None, Some(e)) => return e.open_stream(events),
            (None, None) => return None,
        };
        let (local_tx, mut local_rx) = mpsc::channel::<SttEvent>(64);
        let inner = local.open_stream(local_tx)?;
        let (tx, mut rx) = mpsc::unbounded_channel::<StreamCmd>();
        let timeout = self.cloud_timeout;
        let model = self.name.clone();
        let task = tokio::spawn(async move {
            // Финалы уходят строго по порядку фраз (конвейер сопоставляет их с очередью ожидания).
            let (out_tx, mut out_rx) = mpsc::unbounded_channel::<JoinHandle<SttEvent>>();
            let emitter = {
                let events = events.clone();
                tokio::spawn(async move {
                    while let Some(h) = out_rx.recv().await {
                        if let Ok(ev) = h.await
                            && events.send(ev).await.is_err()
                        {
                            break;
                        }
                    }
                })
            };
            let _ = events.send(SttEvent::Ready { model }).await;
            let session = CancellationToken::new();
            let mut round = session.child_token();
            let mut audio: Vec<i16> = Vec::new();
            // Фразы, ждущие локального финала (локальный поток отвечает на `end` по порядку).
            let mut waiting: VecDeque<oneshot::Sender<Result<SttResult>>> = VecDeque::new();
            loop {
                tokio::select! {
                    cmd = rx.recv() => match cmd {
                        None | Some(StreamCmd::Close) => break,
                        Some(StreamCmd::Audio(pcm)) => {
                            audio.extend_from_slice(&pcm);
                            if !inner.send_audio(&pcm) {
                                break;
                            }
                        }
                        Some(StreamCmd::End) => {
                            let pcm = std::mem::take(&mut audio);
                            let (otx, orx) = oneshot::channel();
                            waiting.push_back(otx);
                            inner.end();
                            let (cloud, cancel, cloud_cancel) = (cloud.clone(), session.child_token(), round.child_token());
                            let duration_ms = pcm.len() as u64 / 16;
                            let h = tokio::spawn(async move {
                                let local = async move {
                                    orx.await.unwrap_or_else(|_| Err(VoiceError::Other("локальный поток закрыт".into())))
                                };
                                match race_final(local, cloud.as_ref(), &pcm, timeout, &cancel, cloud_cancel).await {
                                    Ok((r, _)) => SttEvent::Final { text: r.text, duration_ms: r.duration_ms.max(duration_ms) },
                                    Err(e) => SttEvent::Error { message: e.to_string() },
                                }
                            });
                            let _ = out_tx.send(h);
                        }
                        Some(StreamCmd::Reset) => {
                            // Сброс без текущего звука — конвейер отказался от уже отправленной фразы
                            // (ушла через POST): её облачный запрос больше не нужен.
                            if audio.is_empty() {
                                round.cancel();
                                round = session.child_token();
                            }
                            audio.clear();
                            inner.reset();
                        }
                    },
                    ev = local_rx.recv() => match ev {
                        Some(SttEvent::Final { text, duration_ms }) => {
                            if let Some(w) = waiting.pop_front() {
                                let _ = w.send(Ok(SttResult { text, duration_ms }));
                            }
                        }
                        Some(SttEvent::Error { message }) => match waiting.pop_front() {
                            Some(w) => {
                                let _ = w.send(Err(VoiceError::Other(message)));
                            }
                            None => {
                                let _ = events.send(SttEvent::Error { message }).await;
                            }
                        },
                        Some(ev @ (SttEvent::Partial { .. } | SttEvent::Disconnected { .. })) => {
                            let _ = events.send(ev).await;
                        }
                        Some(SttEvent::Ready { .. }) => {}
                        // Локальный поток умер — закрываемся, конвейер перейдёт на `transcribe`.
                        None => break,
                    },
                }
            }
            session.cancel();
            drop(waiting);
            drop(out_tx);
            let _ = emitter.await;
        });
        Some(SttStream::new(tx, task))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Движок-подмена: отвечает `text` через `delay`; отмену запоминает.
    struct Fake {
        text: &'static str,
        delay: Duration,
        fail: bool,
        cancelled: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
        /// Поток: partial на каждый кадр, final на `end` через `delay`.
        stream: bool,
    }

    struct DropFlag(Option<Arc<AtomicBool>>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            if let Some(f) = &self.0 {
                f.store(true, Ordering::SeqCst);
            }
        }
    }

    impl Fake {
        fn new(text: &'static str, delay_ms: u64) -> Self {
            Self {
                text,
                delay: Duration::from_millis(delay_ms),
                fail: false,
                cancelled: Arc::default(),
                calls: Arc::default(),
                stream: false,
            }
        }
    }

    #[async_trait]
    impl SttEngine for Fake {
        fn name(&self) -> &str {
            self.text
        }

        async fn transcribe(&self, _pcm: &[i16], cancel: &CancellationToken) -> Result<SttResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Запрос «снят», если future бросили (как reqwest) или пришла отмена.
            let mut guard = DropFlag(Some(self.cancelled.clone()));
            tokio::select! {
                _ = cancel.cancelled() => Err(VoiceError::Cancelled),
                _ = tokio::time::sleep(self.delay) => if { guard.0 = None; self.fail } {
                    Err(VoiceError::Http { status: 500, body: String::new() })
                } else {
                    Ok(SttResult { text: self.text.into(), duration_ms: 0 })
                }
            }
        }

        fn open_stream(&self, events: mpsc::Sender<SttEvent>) -> Option<SttStream> {
            if !self.stream {
                return None;
            }
            let (tx, mut rx) = mpsc::unbounded_channel();
            let (text, delay) = (self.text, self.delay);
            let task = tokio::spawn(async move {
                let _ = events.send(SttEvent::Ready { model: "fake".into() }).await;
                while let Some(cmd) = rx.recv().await {
                    match cmd {
                        StreamCmd::Audio(_) => {
                            let _ = events.send(SttEvent::Partial { text: "живой".into() }).await;
                        }
                        StreamCmd::End => {
                            tokio::time::sleep(delay).await;
                            let _ = events.send(SttEvent::Final { text: text.into(), duration_ms: 0 }).await;
                        }
                        StreamCmd::Reset => {}
                        StreamCmd::Close => break,
                    }
                }
            });
            Some(SttStream::new(tx, task))
        }
    }

    fn hybrid(local: Fake, cloud: Fake, timeout_ms: u64) -> (HybridStt, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let (flag, calls) = (cloud.cancelled.clone(), cloud.calls.clone());
        let h = HybridStt::new(Some(Arc::new(local)), Some(Arc::new(cloud)), Duration::from_millis(timeout_ms)).unwrap();
        (h, flag, calls)
    }

    const PCM: [i16; 16_000] = [0; 16_000];

    #[tokio::test]
    async fn cloud_in_time_wins() {
        let (h, _, _) = hybrid(Fake::new("локальный", 10), Fake::new("облачный", 60), 500);
        let r = h.transcribe(&PCM, &CancellationToken::new()).await.unwrap();
        assert_eq!(r.text, "облачный");
    }

    #[tokio::test]
    async fn cloud_timeout_falls_back_to_local() {
        let (h, _, calls) = hybrid(Fake::new("локальный", 10), Fake::new("облачный", 2000), 100);
        let t0 = Instant::now();
        let r = h.transcribe(&PCM, &CancellationToken::new()).await.unwrap();
        assert_eq!(r.text, "локальный");
        // Ждали облако не дольше таймаута (его future снят вместе с HTTP-запросом).
        assert!(t0.elapsed() < Duration::from_millis(1000), "{:?}", t0.elapsed());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cloud_error_or_empty_or_hallucination_gives_local() {
        let mut bad = Fake::new("облачный", 10);
        bad.fail = true;
        let (h, _, _) = hybrid(Fake::new("локальный", 30), bad, 500);
        assert_eq!(h.transcribe(&PCM, &CancellationToken::new()).await.unwrap().text, "локальный");
        for text in ["  ", "Продолжение следует...", "Субтитры сделал DimaTorzok"] {
            let (h, _, _) = hybrid(Fake::new("локальный", 30), Fake::new(text, 5), 500);
            assert_eq!(h.transcribe(&PCM, &CancellationToken::new()).await.unwrap().text, "локальный", "{text}");
        }
    }

    #[tokio::test]
    async fn cancel_aborts_cloud_request() {
        let (h, cancelled, _) = hybrid(Fake::new("локальный", 2000), Fake::new("облачный", 2000), 1500);
        let token = CancellationToken::new();
        let t = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            t.cancel();
        });
        assert_eq!(h.transcribe(&PCM, &token).await.err(), Some(VoiceError::Cancelled));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(cancelled.load(Ordering::SeqCst), "облачный запрос отменён");
    }

    #[tokio::test]
    async fn single_engine_modes() {
        let h = HybridStt::new(None, Some(Arc::new(Fake::new("облачный", 5))), Duration::from_millis(100)).unwrap();
        assert!(!h.has_local());
        assert_eq!(h.transcribe(&PCM, &CancellationToken::new()).await.unwrap().text, "облачный");
        let (tx, _rx) = mpsc::channel(8);
        assert!(h.open_stream(tx).is_none(), "только облако — без потока (конвейер идёт через transcribe)");
        let mut l = Fake::new("локальный", 5);
        l.stream = true;
        let h = HybridStt::new(Some(Arc::new(l)), None, Duration::from_millis(100)).unwrap();
        assert_eq!(h.transcribe(&PCM, &CancellationToken::new()).await.unwrap().text, "локальный");
        assert!(HybridStt::new(None, None, Duration::ZERO).is_err());
    }

    async fn next_final(rx: &mut mpsc::Receiver<SttEvent>, partials: &mut usize) -> String {
        loop {
            match tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.expect("нет финала") {
                Some(SttEvent::Final { text, .. }) => return text,
                Some(SttEvent::Partial { .. }) => *partials += 1,
                Some(SttEvent::Error { message }) => panic!("ошибка: {message}"),
                Some(_) => {}
                None => panic!("поток закрыт"),
            }
        }
    }

    #[tokio::test]
    async fn stream_partials_local_final_race_in_order() {
        let mut local = Fake::new("локальный", 20);
        local.stream = true;
        // Облако отвечает за 250 мс; таймаут 150 мс → первая фраза — локальный текст.
        let (h, _, calls) = hybrid(local, Fake::new("облачный", 250), 150);
        let (tx, mut rx) = mpsc::channel(64);
        let s = h.open_stream(tx).unwrap();
        s.send_audio(&[0; 1600]);
        s.send_audio(&[0; 1600]);
        s.end();
        let mut partials = 0;
        assert_eq!(next_final(&mut rx, &mut partials).await, "локальный");
        assert_eq!(partials, 2, "живой текст идёт от локального движка");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "облако получило ту же фразу");

        let mut local = Fake::new("локальный", 20);
        local.stream = true;
        let (h, _, _) = hybrid(local, Fake::new("облачный", 30), 500);
        let (tx, mut rx) = mpsc::channel(64);
        let s = h.open_stream(tx).unwrap();
        for _ in 0..2 {
            s.send_audio(&[0; 1600]);
            s.end();
        }
        assert_eq!(next_final(&mut rx, &mut partials).await, "облачный");
        assert_eq!(next_final(&mut rx, &mut partials).await, "облачный");
        assert!(!s.is_closed());
    }

    #[tokio::test]
    async fn stream_close_cancels_cloud() {
        let mut local = Fake::new("локальный", 2000);
        local.stream = true;
        let (h, cancelled, _) = hybrid(local, Fake::new("облачный", 2000), 1500);
        let (tx, _rx) = mpsc::channel(64);
        let s = h.open_stream(tx).unwrap();
        s.send_audio(&[0; 1600]);
        s.end();
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(s);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(cancelled.load(Ordering::SeqCst), "закрытие сессии снимает облачный запрос");
    }

    #[tokio::test]
    async fn reset_after_end_drops_cloud_but_keeps_local_final() {
        let mut local = Fake::new("локальный", 20);
        local.stream = true;
        let (h, cancelled, _) = hybrid(local, Fake::new("облачный", 2000), 1500);
        let (tx, mut rx) = mpsc::channel(64);
        let s = h.open_stream(tx).unwrap();
        s.send_audio(&[0; 1600]);
        s.end();
        tokio::time::sleep(Duration::from_millis(50)).await;
        s.reset(); // конвейер ушёл на POST — облако этой фразы не нужно
        let mut partials = 0;
        assert_eq!(next_final(&mut rx, &mut partials).await, "локальный");
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn sanity_rules() {
        assert!(cloud_text_ok("Джарвис, включи субтитры", 2000).is_ok());
        assert!(cloud_text_ok("...", 2000).is_err());
        assert!(cloud_text_ok("Спасибо за просмотр!", 2000).is_err());
        assert!(cloud_text_ok(&"а".repeat(200), 1000).is_err());
    }
}
