//! `VoicePipeline` — связка голоса.
//!
//! Слух: Capture → (EchoGuard: полу-дуплекс) → Vad → SttEngine (поток или целая фраза) → EchoGuard (слова) →
//! WakeFilter → колбэк `Intent`. Стоп-слово или новая команда во время речи — барж-ин.
//!
//! Речь: дельты LLM → PhraseChunker → SpeechNormalizer → split ≤260 → TtsEngine (следующая фраза качается,
//! пока играет текущая) → AudioOutput (очередь ≤5 буферов). Барж-ин отменяет всё и шлёт `DELETE /tts/{id}`.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use jarvis_core::event::Speaker;
use jarvis_core::{AssistantState, CancellationToken, Event, EventSink};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::capture::{Capture, CaptureConfig};
use crate::chunker::{MAX_PHRASE_CHARS, PhraseChunker, split_long};
use crate::echo::{EchoGuard, EchoGuardConfig};
use crate::error::{Result, VoiceError};
use crate::normalizer::SpeechNormalizer;
use crate::pcm::S16Decoder;
use crate::player::{AudioOutput, Player, PlayerConfig};
use crate::stt::{SttEngine, SttEvent, SttStream};
use crate::tts::{TtsClient, TtsEngine};
use crate::vad::{EnergyScorer, Utterance, Vad, VadConfig, VadEvent};
use crate::wake::{Intent, WakeContext, WakeFilter};

#[derive(Debug, Clone)]
pub struct VoiceConfig {
    pub vad: VadConfig,
    /// `Some(dbfs)` — VAD по энергии вместо earshot (запасной вариант, тесты).
    pub energy_vad_dbfs: Option<f32>,
    pub echo: EchoGuardConfig,
    pub player: PlayerConfig,
    pub capture: CaptureConfig,
    pub max_phrase_chars: usize,
    /// Сколько ждать `final` от потокового STT, прежде чем отправить фразу через `transcribe`.
    pub final_timeout: Duration,
    /// Окно после «Джарвис» / ответа, когда команда принимается без обращения.
    pub follow_up: Duration,
    /// Слушать во время речи (барж-ин голосом). Если false — чистый полу-дуплекс.
    pub barge_in_while_speaking: bool,
    /// Размер буфера, которым звук уходит в плеер.
    pub buffer_ms: u32,
    /// Сколько фраз синтезировать заранее, пока играет текущая.
    pub prefetch_phrases: usize,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            vad: VadConfig::default(),
            energy_vad_dbfs: None,
            echo: EchoGuardConfig::default(),
            player: PlayerConfig::default(),
            capture: CaptureConfig::default(),
            max_phrase_chars: MAX_PHRASE_CHARS,
            final_timeout: Duration::from_secs(6),
            follow_up: Duration::from_secs(8),
            barge_in_while_speaking: true,
            buffer_ms: 100,
            prefetch_phrases: 2,
        }
    }
}

impl VoiceConfig {
    /// Устройства и громкость из `config.json` → `audio` (не задано — устройства Windows по умолчанию).
    pub fn with_audio(mut self, audio: &jarvis_core::config::AudioConfig) -> Self {
        self.capture.device = audio.input().map(str::to_string);
        self.player.device = audio.output().map(str::to_string);
        self.player.volume = audio.gain();
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakOutcome {
    Completed,
    Interrupted,
}

struct Inner {
    cfg: VoiceConfig,
    tts: Arc<dyn TtsEngine>,
    stt: Arc<dyn SttEngine>,
    out: Arc<dyn AudioOutput>,
    events: Arc<dyn EventSink>,
    echo: Arc<EchoGuard>,
    wake: WakeFilter,
    normalizer: SpeechNormalizer,
    speak_token: Mutex<CancellationToken>,
    speaking: AtomicBool,
    awaiting_confirm: AtomicBool,
    follow_up_until: Mutex<Option<Instant>>,
}

#[derive(Clone)]
pub struct VoicePipeline {
    inner: Arc<Inner>,
}

/// Работающее прослушивание; остановка — `stop()` или drop.
pub struct ListenHandle {
    token: CancellationToken,
    task: Option<JoinHandle<()>>,
    _capture: Option<Capture>,
}

impl ListenHandle {
    pub fn stop(&mut self) {
        self.token.cancel();
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl Drop for ListenHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl VoicePipeline {
    /// Со своими TTS/STT/выводом (подмены в тестах).
    pub fn new(
        cfg: VoiceConfig,
        tts: Arc<dyn TtsEngine>,
        stt: Arc<dyn SttEngine>,
        out: Arc<dyn AudioOutput>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        let echo = Arc::new(EchoGuard::new(cfg.echo.clone(), out.monitor()));
        Self {
            inner: Arc::new(Inner {
                cfg,
                tts,
                stt,
                out,
                events,
                echo,
                wake: WakeFilter::new(),
                normalizer: SpeechNormalizer::new(),
                speak_token: Mutex::new(CancellationToken::new()),
                speaking: AtomicBool::new(false),
                awaiting_confirm: AtomicBool::new(false),
                follow_up_until: Mutex::new(None),
            }),
        }
    }

    /// Боевая сборка из `config.json`: TTS Xeon, STT по `stt.engine`, микрофон/динамики/громкость из `audio`
    /// (устройство не найдено — берётся устройство Windows по умолчанию с предупреждением в журнале).
    /// Устройства, явно заданные в `cfg`, важнее `audio`.
    pub fn from_config(
        cfg: VoiceConfig,
        app: &jarvis_core::Config,
        project_root: &Path,
        events: Arc<dyn EventSink>,
    ) -> Result<Self> {
        let explicit = (cfg.capture.device.clone(), cfg.player.device.clone());
        let mut cfg = cfg.with_audio(&app.audio);
        if explicit.0.is_some() {
            cfg.capture.device = explicit.0;
        }
        if explicit.1.is_some() {
            cfg.player.device = explicit.1;
        }
        let tts = Arc::new(TtsClient::new(&app.tts, project_root)?);
        let stt = crate::stt::engine_from_config(app, project_root)?;
        let player = Arc::new(Player::start(cfg.player.clone(), events.clone())?);
        Ok(Self::new(cfg, tts, stt, player, events))
    }

    pub fn echo_guard(&self) -> Arc<EchoGuard> {
        self.inner.echo.clone()
    }

    pub fn is_speaking(&self) -> bool {
        self.inner.speaking.load(Ordering::Acquire)
    }

    /// Режим ожидания «да, сэр» для действия T3.
    pub fn set_awaiting_confirm(&self, on: bool) {
        self.inner.awaiting_confirm.store(on, Ordering::Release);
    }

    /// Барж-ин: оборвать речь немедленно (TTS отменяется на сервере).
    pub fn barge_in(&self) {
        self.inner.barge_in();
    }

    /// Открыть микрофон и слушать.
    pub fn start_listening(&self, on_intent: impl Fn(Intent) + Send + Sync + 'static) -> Result<ListenHandle> {
        let (capture, frames) = Capture::start(self.inner.cfg.capture.clone(), self.inner.events.clone())?;
        let mut h = self.listen_frames(frames, on_intent);
        h._capture = Some(capture);
        Ok(h)
    }

    /// Слушать готовый поток кадров (256 сэмплов 16 кГц) — без устройства.
    pub fn listen_frames(&self, frames: mpsc::Receiver<Vec<i16>>, on_intent: impl Fn(Intent) + Send + Sync + 'static) -> ListenHandle {
        let token = CancellationToken::new();
        let task = tokio::spawn(listen_loop(self.inner.clone(), frames, Arc::new(on_intent), token.clone()));
        ListenHandle { token, task: Some(task), _capture: None }
    }

    /// Произнести поток дельт текста (ответ LLM). Новый вызов прерывает предыдущий.
    pub async fn speak(&self, deltas: mpsc::Receiver<String>) -> Result<SpeakOutcome> {
        self.inner.clone().speak(deltas).await
    }

    pub async fn say(&self, text: &str) -> Result<SpeakOutcome> {
        let (tx, rx) = mpsc::channel(1);
        tx.send(text.to_string()).await.ok();
        drop(tx);
        self.speak(rx).await
    }
}

impl Inner {
    fn barge_in(&self) {
        self.speak_token.lock().unwrap().cancel();
        self.out.stop();
    }

    fn in_follow_up(&self) -> bool {
        self.follow_up_until.lock().unwrap().is_some_and(|t| Instant::now() < t)
    }

    async fn speak(self: Arc<Self>, mut deltas: mpsc::Receiver<String>) -> Result<SpeakOutcome> {
        let token = CancellationToken::new();
        {
            let mut cur = self.speak_token.lock().unwrap();
            cur.cancel();
            *cur = token.clone();
        }
        self.out.stop();
        self.speaking.store(true, Ordering::Release);
        self.events.emit(Event::State { state: AssistantState::Speaking });

        // 1) Фразы: чанкер → нормализатор → дорезка.
        let (ph_tx, mut ph_rx) = mpsc::channel::<String>(32);
        let me = self.clone();
        let t1 = token.clone();
        let phrases = tokio::spawn(async move {
            let mut chunker = PhraseChunker::with_max_len(me.cfg.max_phrase_chars);
            let send = |raw: String| {
                let norm = me.normalizer.normalize(&raw);
                split_long(&norm, me.cfg.max_phrase_chars)
                    .into_iter()
                    .filter(|p| SpeechNormalizer::is_speakable(p))
                    .collect::<Vec<_>>()
            };
            loop {
                let delta = tokio::select! {
                    _ = t1.cancelled() => return,
                    d = deltas.recv() => d,
                };
                let done = delta.is_none();
                let raw = match delta {
                    Some(d) => chunker.push(&d),
                    None => chunker.flush(),
                };
                for p in raw.into_iter().flat_map(send) {
                    if ph_tx.send(p).await.is_err() {
                        return;
                    }
                }
                if done {
                    return;
                }
            }
        });

        // 2) Синтез: фразы по очереди, с опережением до `prefetch_phrases`.
        let (au_tx, mut au_rx) = mpsc::channel::<(u32, mpsc::UnboundedReceiver<Result<bytes::Bytes>>)>(self.cfg.prefetch_phrases.max(1));
        let me = self.clone();
        let t2 = token.clone();
        let synth = tokio::spawn(async move {
            while let Some(phrase) = tokio::select! { _ = t2.cancelled() => None, p = ph_rx.recv() => p } {
                me.echo.note_spoken(&phrase);
                me.events.emit(Event::Subtitle { speaker: Speaker::Jarvis, text: phrase.clone(), is_final: true });
                let stream = match me.tts.synthesize(&phrase, None, t2.clone()).await {
                    Ok(s) => s,
                    Err(VoiceError::Cancelled) => return,
                    Err(e) => {
                        tracing::warn!(error = %e, "TTS: фраза пропущена");
                        me.events.emit(Event::Error { message: format!("Синтез речи: {e}") });
                        continue;
                    }
                };
                let rate = stream.sample_rate;
                let (tx, rx) = mpsc::unbounded_channel();
                if au_tx.send((rate, rx)).await.is_err() {
                    return;
                }
                let mut stream = stream;
                while let Some(chunk) = tokio::select! { _ = t2.cancelled() => None, c = stream.next() => c } {
                    let stop = chunk.is_err();
                    if tx.send(chunk).is_err() || stop {
                        break;
                    }
                }
            }
        });

        // 3) Воспроизведение по порядку.
        let result = self.play_all(&mut au_rx, &token).await;
        let interrupted = token.is_cancelled() || matches!(result, Err(VoiceError::Cancelled));
        if !interrupted {
            self.out.drain(&token).await;
        }
        let interrupted = interrupted || token.is_cancelled();
        if interrupted {
            self.out.stop();
        }
        token.cancel(); // добить фоновые задачи
        let _ = phrases.await;
        let _ = synth.await;
        self.speaking.store(false, Ordering::Release);
        if !interrupted {
            *self.follow_up_until.lock().unwrap() = Some(Instant::now() + self.cfg.follow_up);
        }
        self.events.emit(Event::State { state: AssistantState::Idle });
        match result {
            Err(e) if !matches!(e, VoiceError::Cancelled) => Err(e),
            _ => Ok(if interrupted { SpeakOutcome::Interrupted } else { SpeakOutcome::Completed }),
        }
    }

    async fn play_all(
        &self,
        au_rx: &mut mpsc::Receiver<(u32, mpsc::UnboundedReceiver<Result<bytes::Bytes>>)>,
        token: &CancellationToken,
    ) -> Result<()> {
        while let Some((rate, mut chunks)) = tokio::select! { _ = token.cancelled() => None, a = au_rx.recv() => a } {
            let mut dec = S16Decoder::new();
            let target = (rate * self.cfg.buffer_ms / 1000).max(256) as usize;
            let mut buf: Vec<f32> = Vec::with_capacity(target * 2);
            loop {
                let chunk = tokio::select! {
                    _ = token.cancelled() => return Err(VoiceError::Cancelled),
                    c = chunks.recv() => c,
                };
                match chunk {
                    Some(Ok(bytes)) => {
                        dec.decode_into(&bytes, &mut buf);
                        if buf.len() >= target {
                            let b = std::mem::replace(&mut buf, Vec::with_capacity(target * 2));
                            self.play(b, rate, token).await?;
                        }
                    }
                    Some(Err(VoiceError::Cancelled)) => return Err(VoiceError::Cancelled),
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "TTS: поток оборвался");
                        break;
                    }
                    None => break,
                }
            }
            if !buf.is_empty() {
                self.play(buf, rate, token).await?;
            }
            self.out.finish().await?;
        }
        if token.is_cancelled() { Err(VoiceError::Cancelled) } else { Ok(()) }
    }

    async fn play(&self, buf: Vec<f32>, rate: u32, token: &CancellationToken) -> Result<()> {
        tokio::select! {
            _ = token.cancelled() => Err(VoiceError::Cancelled),
            r = self.out.play(buf, rate) => r,
        }
    }

    /// Что делать с распознанным текстом.
    fn handle_transcript(&self, text: &str, started_at: Instant, on_intent: &(dyn Fn(Intent) + Send + Sync)) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self.echo.is_echo(text, started_at) {
            tracing::debug!(text, "эхо TTS отброшено");
            return;
        }
        let during_speech = self.speaking.load(Ordering::Acquire) || self.echo.in_echo_zone(started_at);
        let ctx = WakeContext {
            awaiting_confirm: self.awaiting_confirm.load(Ordering::Acquire),
            follow_up: self.in_follow_up() && !during_speech,
        };
        let intent = self.wake.classify(text, ctx);
        match &intent {
            Intent::Ignored => return,
            // «Да» засчитывается только после окончания речи Джарвиса — защита от самоподтверждения.
            Intent::Confirm if !self.echo.is_clean_confirmation(started_at) => return,
            Intent::Stop => self.barge_in(),
            Intent::Command(_) => {
                if self.speaking.load(Ordering::Acquire) {
                    self.barge_in();
                }
                *self.follow_up_until.lock().unwrap() = None;
            }
            Intent::WakeOnly => {
                if self.speaking.load(Ordering::Acquire) {
                    self.barge_in();
                }
                *self.follow_up_until.lock().unwrap() = Some(Instant::now() + self.cfg.follow_up);
                self.events.emit(Event::State { state: AssistantState::Listening });
            }
            Intent::Confirm | Intent::Deny => {}
        }
        self.events.emit(Event::Subtitle { speaker: Speaker::User, text: text.to_string(), is_final: true });
        on_intent(intent);
    }
}

struct Pending {
    utt: Utterance,
    deadline: Instant,
}

async fn listen_loop(
    inner: Arc<Inner>,
    mut frames: mpsc::Receiver<Vec<i16>>,
    on_intent: Arc<dyn Fn(Intent) + Send + Sync>,
    token: CancellationToken,
) {
    let mut vad = match inner.cfg.energy_vad_dbfs {
        Some(db) => Vad::with_scorer(inner.cfg.vad.clone(), Box::new(EnergyScorer { threshold_dbfs: db })),
        None => Vad::new(inner.cfg.vad.clone()),
    };
    let (ev_tx, mut ev_rx) = mpsc::channel::<SttEvent>(64);
    let stream: Option<SttStream> = inner.stt.open_stream(ev_tx);
    let mut pending: VecDeque<Pending> = VecDeque::new();
    let mut was_muted = false;
    let (tr_tx, mut tr_rx) = mpsc::channel::<(String, Instant)>(8);
    let transcribe = |utt: Utterance| {
        let (stt, tx, t) = (inner.stt.clone(), tr_tx.clone(), token.clone());
        tokio::spawn(async move {
            match stt.transcribe(&utt.pcm16k, &t).await {
                Ok(r) => {
                    let _ = tx.send((r.text, utt.started_at)).await;
                }
                Err(e) => tracing::warn!(error = %e, "STT: фраза не распознана"),
            }
        });
    };
    loop {
        let deadline = pending.front().map(|p| p.deadline);
        tokio::select! {
            _ = token.cancelled() => break,
            f = frames.recv() => {
                let Some(frame) = f else { break };
                let muted = inner.echo.mic_muted() && !inner.cfg.barge_in_while_speaking;
                if muted {
                    if vad.is_active() && let Some(s) = &stream {
                        s.reset();
                    }
                    if !was_muted {
                        vad.reset();
                    }
                    was_muted = true;
                    continue;
                }
                was_muted = false;
                match vad.push(&frame) {
                    Some(VadEvent::SpeechStart { audio, .. }) => {
                        if let Some(s) = &stream { s.send_audio(&audio); }
                    }
                    Some(VadEvent::Speech) => {
                        if let Some(s) = &stream { s.send_audio(&frame); }
                    }
                    Some(VadEvent::SpeechEnd(utt)) => match &stream {
                        Some(s) if !s.is_closed() => {
                            s.send_audio(&frame);
                            s.end();
                            pending.push_back(Pending { deadline: Instant::now() + inner.cfg.final_timeout, utt });
                        }
                        _ => transcribe(utt),
                    },
                    Some(VadEvent::Discarded { .. }) => {
                        if let Some(s) = &stream { s.reset(); }
                    }
                    None => {}
                }
            }
            ev = ev_rx.recv(), if stream.is_some() => match ev {
                Some(SttEvent::Final { text, .. }) => {
                    let started = pending.pop_front().map(|p| p.utt.started_at).unwrap_or_else(Instant::now);
                    inner.handle_transcript(&text, started, on_intent.as_ref());
                }
                Some(SttEvent::Partial { text }) => {
                    if inner.wake.has_wake_prefix(&text) || inner.in_follow_up() {
                        inner.events.emit(Event::Subtitle { speaker: Speaker::User, text, is_final: false });
                    }
                }
                Some(SttEvent::Error { message }) => tracing::warn!(%message, "STT"),
                Some(SttEvent::Disconnected { reason }) => tracing::warn!(%reason, "STT: переподключение"),
                Some(SttEvent::Ready { model }) => tracing::info!(%model, "STT готов"),
                None => {}
            },
            r = tr_rx.recv() => if let Some((text, started)) = r {
                inner.handle_transcript(&text, started, on_intent.as_ref());
            },
            _ = sleep_until(deadline), if deadline.is_some() => {
                // Потоковый STT молчит — та же фраза через `transcribe` (POST /stt).
                if let Some(p) = pending.pop_front() {
                    if let Some(s) = &stream { s.reset(); }
                    transcribe(p.utt);
                }
            }
        }
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(tokio::time::Instant::from_std(t)).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::echo::{ManualMonitor, PlaybackMonitor};
    use crate::stt::SttResult;
    use crate::tts::TtsStream;
    use async_trait::async_trait;
    use bytes::Bytes;
    use jarvis_core::CollectSink;

    #[derive(Default)]
    struct FakeTts {
        texts: Mutex<Vec<String>>,
        /// После первого куска поток зависает (для барж-ина).
        hang: bool,
    }

    #[async_trait]
    impl TtsEngine for FakeTts {
        async fn synthesize(&self, text: &str, _v: Option<&str>, _c: CancellationToken) -> Result<TtsStream> {
            self.texts.lock().unwrap().push(text.to_string());
            let chunk = Bytes::from([0u8, 16].repeat(2400)); // 100 мс при 24 кГц
            let s = futures_util::stream::iter(vec![Ok(chunk.clone()), Ok(chunk)]);
            if self.hang {
                Ok(TtsStream::new("hang", 24_000, Box::pin(s.chain(futures_util::stream::pending()))))
            } else {
                Ok(TtsStream::new("ok", 24_000, Box::pin(s)))
            }
        }
        async fn cancel(&self, _id: &str) -> Result<bool> {
            Ok(true)
        }
    }

    #[derive(Default)]
    struct FakeOut {
        samples: Mutex<usize>,
        stops: Mutex<usize>,
        monitor: Arc<ManualMonitor>,
    }

    #[async_trait]
    impl AudioOutput for FakeOut {
        async fn play(&self, samples: Vec<f32>, rate: u32) -> Result<()> {
            assert_eq!(rate, 24_000);
            *self.samples.lock().unwrap() += samples.len();
            Ok(())
        }
        async fn finish(&self) -> Result<()> {
            Ok(())
        }
        fn stop(&self) {
            *self.stops.lock().unwrap() += 1;
        }
        async fn drain(&self, _c: &CancellationToken) {}
        fn monitor(&self) -> Arc<dyn PlaybackMonitor> {
            self.monitor.clone()
        }
    }

    struct FakeStt(&'static str);

    #[async_trait]
    impl SttEngine for FakeStt {
        fn name(&self) -> &str {
            "fake"
        }
        async fn transcribe(&self, pcm: &[i16], _c: &CancellationToken) -> Result<SttResult> {
            assert!(!pcm.is_empty());
            Ok(SttResult { text: self.0.into(), duration_ms: 0 })
        }
    }

    fn pipeline(tts: Arc<FakeTts>, out: Arc<FakeOut>, stt: &'static str) -> (VoicePipeline, Arc<CollectSink>) {
        let sink = Arc::new(CollectSink::default());
        let cfg = VoiceConfig { energy_vad_dbfs: Some(-40.0), ..Default::default() };
        (VoicePipeline::new(cfg, tts, Arc::new(FakeStt(stt)), out, sink.clone()), sink)
    }

    #[tokio::test]
    async fn speak_streams_normalized_phrases_in_order() {
        let (tts, out) = (Arc::new(FakeTts::default()), Arc::new(FakeOut::default()));
        let (p, sink) = pipeline(tts.clone(), out.clone(), "");
        let (tx, rx) = mpsc::channel(8);
        for d in ["Привет, ", "сэр. Найдено 5 ", "файлов", " на диске D.", "\n```\ncode\n```"] {
            tx.send(d.to_string()).await.unwrap();
        }
        drop(tx);
        assert_eq!(p.speak(rx).await.unwrap(), SpeakOutcome::Completed);
        assert_eq!(*tts.texts.lock().unwrap(), ["Привет, сэр.", "Найдено пять файлов на диске D.", "Пример кода."]);
        assert_eq!(*out.samples.lock().unwrap(), 3 * 4800);
        assert!(p.echo_guard().overlap("найдено пять файлов") > 0.99, "сказанное запомнено для фильтра эха");
        let ev = sink.take();
        assert_eq!(ev.first(), Some(&Event::State { state: AssistantState::Speaking }));
        assert_eq!(ev.last(), Some(&Event::State { state: AssistantState::Idle }));
    }

    #[tokio::test]
    async fn barge_in_interrupts_quickly() {
        let (tts, out) = (Arc::new(FakeTts { hang: true, ..Default::default() }), Arc::new(FakeOut::default()));
        let (p, _) = pipeline(tts, out.clone(), "");
        let p2 = p.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            p2.barge_in();
        });
        let t = Instant::now();
        assert_eq!(p.say("Это очень длинный ответ.").await.unwrap(), SpeakOutcome::Interrupted);
        assert!(t.elapsed() < Duration::from_millis(400));
        assert!(*out.stops.lock().unwrap() >= 2);
        assert!(!p.is_speaking());
    }

    fn burst() -> Vec<Vec<i16>> {
        let mut pcm = vec![0i16; 16_000 / 2];
        pcm.extend((0..16_000 * 6 / 10).map(|i| ((i as f32 * 440.0 / 16_000.0 * std::f32::consts::TAU).sin() * 9000.0) as i16));
        pcm.extend(vec![0i16; 16_000 * 3 / 2]);
        pcm.as_chunks::<256>().0.iter().map(|c| c.to_vec()).collect()
    }

    async fn listen(stt: &'static str, setup: impl FnOnce(&VoicePipeline, &FakeOut)) -> Vec<Intent> {
        let (tts, out) = (Arc::new(FakeTts::default()), Arc::new(FakeOut::default()));
        let (p, _) = pipeline(tts, out.clone(), stt);
        setup(&p, &out);
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let (tx, rx) = mpsc::channel(4096);
        let _h = p.listen_frames(rx, move |i| g.lock().unwrap().push(i));
        for f in burst() {
            tx.send(f).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        got.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn listen_detects_command() {
        let got = listen("Джарвис, открой браузер", |_, _| {}).await;
        assert_eq!(got, vec![Intent::Command("открой браузер".into())]);
        let got = listen("просто разговор", |_, _| {}).await;
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn echo_and_self_confirmation_are_filtered() {
        // Эхо: Джарвис только что сказал эти слова и ещё играет.
        let got = listen("джарвис открой браузер", |p, out| {
            p.echo_guard().note_spoken("Джарвис, открой браузер");
            out.monitor.set_playing(true);
        })
        .await;
        assert!(got.is_empty(), "{got:?}");
        // «Да» во время речи не подтверждает.
        let got = listen("да", |p, out| {
            p.set_awaiting_confirm(true);
            out.monitor.set_playing(true);
        })
        .await;
        assert!(got.is_empty(), "{got:?}");
        let got = listen("да, сэр", |p, _| p.set_awaiting_confirm(true)).await;
        assert_eq!(got, vec![Intent::Confirm]);
    }
}
