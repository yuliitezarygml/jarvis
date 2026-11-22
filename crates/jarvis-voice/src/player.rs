//! `Player`: PCM (TTS 24 кГц mono) → устройство вывода cpal с ресемплингом под его частоту.
//! Ограниченная очередь (≤5 буферов): `play()` ждёт свободного места; воспроизведение начинается с первого буфера;
//! `stop()` (барж-ин) мгновенно очищает очередь — следующий колбэк устройства уже пишет тишину.
//! Уровень RMS ≈30 Гц уходит событием `AudioLevel { source: Tts }`; флаг «играет» и момент окончания — для `EchoGuard`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use jarvis_core::event::AudioSource;
use jarvis_core::{CancellationToken, Event, EventSink};
use tokio::sync::{Notify, Semaphore};

use crate::device::{device_name, find_output};
use crate::echo::PlaybackMonitor;
use crate::error::{Result, VoiceError};
use crate::pcm::LevelMeter;
use crate::resample::StreamResampler;

/// Куда пайплайн отдаёт звук. Реализует `Player`; в тестах — подмена.
#[async_trait]
pub trait AudioOutput: Send + Sync {
    /// Буфер mono f32 частоты `sample_rate`. Ждёт, пока в очереди есть место. После `stop()` — `Err(Cancelled)`.
    async fn play(&self, samples: Vec<f32>, sample_rate: u32) -> Result<()>;
    /// Конец фразы: дожать хвост ресемплера.
    async fn finish(&self) -> Result<()>;
    /// Барж-ин: очистить очередь и замолчать немедленно.
    fn stop(&self);
    /// Дождаться, пока всё сыграно (или отмены).
    async fn drain(&self, cancel: &CancellationToken);
    fn monitor(&self) -> Arc<dyn PlaybackMonitor>;
}

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    /// Имя (или часть имени) устройства вывода; `None` — по умолчанию.
    pub device: Option<String>,
    pub max_buffers: usize,
    /// Частота событий уровня.
    pub level_hz: u32,
    /// Множитель громкости (1.0 — как есть); результат ограничивается [-1, 1].
    pub volume: f32,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self { device: None, max_buffers: 5, level_hz: 30, volume: 1.0 }
    }
}

/// Применить громкость к буферу (с ограничением, чтобы усиление не давало переполнения).
pub fn apply_gain(samples: &mut [f32], gain: f32) {
    if (gain - 1.0).abs() < f32::EPSILON || !gain.is_finite() {
        return;
    }
    for s in samples {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
}

/// Очередь буферов (частота устройства, mono). Чистая логика — тестируется без устройства.
#[derive(Debug, Default)]
pub struct PlayQueue {
    bufs: VecDeque<Vec<f32>>,
    pos: usize,
}

impl PlayQueue {
    pub fn len(&self) -> usize {
        self.bufs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bufs.is_empty()
    }
    pub fn push(&mut self, buf: Vec<f32>) {
        if !buf.is_empty() {
            self.bufs.push_back(buf);
        }
    }
    /// Следующий сэмпл; `freed` растёт, когда буфер доигран.
    #[inline]
    pub fn next_sample(&mut self, freed: &mut usize) -> Option<f32> {
        let front = self.bufs.front()?;
        let s = front[self.pos];
        self.pos += 1;
        if self.pos >= front.len() {
            self.bufs.pop_front();
            self.pos = 0;
            *freed += 1;
        }
        Some(s)
    }
    /// Очистить; вернуть число удалённых буферов.
    pub fn clear(&mut self) -> usize {
        let n = self.bufs.len();
        self.bufs.clear();
        self.pos = 0;
        n
    }
}

struct Shared {
    queue: Mutex<PlayQueue>,
    slots: Semaphore,
    playing: AtomicBool,
    /// Мс от `epoch` + 1 до последнего окончания звука (0 — ещё не было).
    ended: AtomicU64,
    epoch: Instant,
    generation: AtomicU64,
    level: AtomicU32,
    level_seq: AtomicU64,
    idle: Notify,
}

impl Shared {
    fn mark_ended(&self) {
        self.ended.store(self.epoch.elapsed().as_millis() as u64 + 1, Ordering::Release);
        self.idle.notify_waiters();
    }
}

impl PlaybackMonitor for Shared {
    fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire) || !self.queue.lock().unwrap().is_empty()
    }
    fn last_played_at(&self) -> Option<Instant> {
        match self.ended.load(Ordering::Acquire) {
            0 => None,
            ms => Some(self.epoch + Duration::from_millis(ms - 1)),
        }
    }
}

pub struct Player {
    shared: Arc<Shared>,
    resampler: Mutex<Option<StreamResampler>>,
    device_rate: u32,
    device_name: String,
    volume: f32,
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    /// Открыть устройство вывода (поток cpal живёт в своём потоке ОС, там же — события уровня).
    pub fn start(cfg: PlayerConfig, events: Arc<dyn EventSink>) -> Result<Self> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(PlayQueue::default()),
            slots: Semaphore::new(cfg.max_buffers.max(1)),
            playing: AtomicBool::new(false),
            ended: AtomicU64::new(0),
            epoch: Instant::now(),
            generation: AtomicU64::new(0),
            level: AtomicU32::new(0),
            level_seq: AtomicU64::new(0),
            idle: Notify::new(),
        });
        let quit = Arc::new(AtomicBool::new(false));
        let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<(u32, String)>>();
        let (sh, q) = (shared.clone(), quit.clone());
        let thread = std::thread::Builder::new()
            .name("jarvis-player".into())
            .spawn(move || {
                let stream = match open_output(&cfg, sh.clone()) {
                    Ok((stream, rate, name)) => {
                        let _ = init_tx.send(Ok((rate, name)));
                        stream
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                let mut last_seq = 0;
                let mut was_playing = false;
                let tick = Duration::from_millis(1000 / cfg.level_hz.max(1) as u64);
                while !q.load(Ordering::Acquire) {
                    std::thread::sleep(tick);
                    let seq = sh.level_seq.load(Ordering::Acquire);
                    let playing = sh.playing.load(Ordering::Acquire);
                    if playing && seq != last_seq {
                        last_seq = seq;
                        let rms = f32::from_bits(sh.level.load(Ordering::Acquire));
                        events.emit(Event::AudioLevel { source: AudioSource::Tts, rms });
                    } else if was_playing && !playing {
                        events.emit(Event::AudioLevel { source: AudioSource::Tts, rms: 0.0 });
                    }
                    was_playing = playing;
                }
                drop(stream);
            })
            .map_err(|e| VoiceError::Device(e.to_string()))?;
        let (device_rate, device_name) = init_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| VoiceError::Device("устройство вывода не ответило".into()))??;
        Ok(Self { shared, resampler: Mutex::new(None), device_rate, device_name, quit, thread: Some(thread) })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    pub fn is_playing(&self) -> bool {
        self.shared.is_playing()
    }

    pub fn last_played_at(&self) -> Option<Instant> {
        self.shared.last_played_at()
    }

    fn resample(&self, samples: &[f32], rate: u32, flush: bool) -> Result<Vec<f32>> {
        let mut guard = self.resampler.lock().unwrap();
        if guard.as_ref().is_none_or(|r| r.rates() != (rate, self.device_rate)) {
            *guard = Some(StreamResampler::new(rate, self.device_rate)?);
        }
        let r = guard.as_mut().unwrap();
        let mut out = Vec::with_capacity(samples.len() * self.device_rate as usize / rate.max(1) as usize + 64);
        r.process(samples, &mut out)?;
        if flush {
            r.flush(&mut out)?;
        }
        Ok(out)
    }

    async fn enqueue(&self, buf: Vec<f32>) -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let generation = self.shared.generation.load(Ordering::Acquire);
        let permit = self.shared.slots.acquire().await.map_err(|_| VoiceError::Cancelled)?;
        if self.shared.generation.load(Ordering::Acquire) != generation {
            return Err(VoiceError::Cancelled); // пока ждали места, был stop()
        }
        permit.forget(); // вернёт колбэк, доиграв буфер
        self.shared.queue.lock().unwrap().push(buf);
        Ok(())
    }
}

#[async_trait]
impl AudioOutput for Player {
    async fn play(&self, samples: Vec<f32>, sample_rate: u32) -> Result<()> {
        let buf = self.resample(&samples, sample_rate, false)?;
        self.enqueue(buf).await
    }

    async fn finish(&self) -> Result<()> {
        let tail = {
            let mut g = self.resampler.lock().unwrap();
            match g.as_mut() {
                Some(r) => {
                    let mut out = Vec::new();
                    r.flush(&mut out)?;
                    out
                }
                None => Vec::new(),
            }
        };
        self.enqueue(tail).await
    }

    fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        let removed = self.shared.queue.lock().unwrap().clear();
        self.shared.slots.add_permits(removed);
        if let Some(r) = self.resampler.lock().unwrap().as_mut() {
            r.reset();
        }
        if self.shared.playing.swap(false, Ordering::AcqRel) || removed > 0 {
            self.shared.mark_ended();
        }
    }

    async fn drain(&self, cancel: &CancellationToken) {
        loop {
            let notified = self.shared.idle.notified();
            if !self.shared.is_playing() {
                return;
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = notified => {}
                // Подстраховка от пропущенного уведомления.
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }

    fn monitor(&self) -> Arc<dyn PlaybackMonitor> {
        self.shared.clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn open_output(cfg: &PlayerConfig, shared: Arc<Shared>) -> Result<(cpal::Stream, u32, String)> {
    let device = find_output(cfg.device.as_deref())?;
    let name = device_name(&device);
    let supported = device.default_output_config().map_err(|e| VoiceError::Device(format!("{name}: {e}")))?;
    let rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config: cpal::StreamConfig = supported.config();
    let hz = cfg.level_hz;
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, channels, rate, hz, shared),
        SampleFormat::I16 => build::<i16>(&device, &config, channels, rate, hz, shared),
        SampleFormat::U16 => build::<u16>(&device, &config, channels, rate, hz, shared),
        SampleFormat::I32 => build::<i32>(&device, &config, channels, rate, hz, shared),
        f => return Err(VoiceError::Device(format!("{name}: формат {f} не поддерживается"))),
    }?;
    stream.play().map_err(|e| VoiceError::Device(format!("{name}: {e}")))?;
    Ok((stream, rate, name))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    rate: u32,
    level_hz: u32,
    shared: Arc<Shared>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let mut meter = LevelMeter::new(rate, level_hz);
    let silence = T::from_sample(0.0f32);
    let cb = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        let mut freed = 0;
        let mut any = false;
        {
            let mut q = shared.queue.lock().unwrap();
            for frame in data.chunks_mut(channels.max(1)) {
                let v = match q.next_sample(&mut freed) {
                    Some(s) => {
                        any = true;
                        s
                    }
                    None => 0.0,
                };
                let out = if v == 0.0 { silence } else { T::from_sample(v) };
                frame.fill(out);
                if any && let Some(r) = meter.push(v) {
                    shared.level.store(r.to_bits(), Ordering::Release);
                    shared.level_seq.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
        if freed > 0 {
            shared.slots.add_permits(freed);
        }
        if any {
            shared.playing.store(true, Ordering::Release);
        } else if shared.playing.swap(false, Ordering::AcqRel) {
            meter.reset();
            shared.mark_ended();
        }
    };
    let err = |e: cpal::Error| tracing::warn!(error = %e, "ошибка потока вывода");
    device
        .build_output_stream::<T, _, _>(*config, cb, err, Some(Duration::from_secs(5)))
        .map_err(|e| VoiceError::Device(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_plays_in_order_and_frees_slots() {
        let mut q = PlayQueue::default();
        q.push(vec![1.0, 2.0]);
        q.push(vec![]);
        q.push(vec![3.0]);
        assert_eq!(q.len(), 2, "пустые буферы не занимают место");
        let mut freed = 0;
        let got: Vec<f32> = std::iter::from_fn(|| q.next_sample(&mut freed)).collect();
        assert_eq!(got, vec![1.0, 2.0, 3.0]);
        assert_eq!(freed, 2);
        q.push(vec![0.5; 10]);
        q.push(vec![0.5; 10]);
        q.next_sample(&mut freed);
        assert_eq!(q.clear(), 2);
        assert!(q.next_sample(&mut freed).is_none(), "после stop — сразу тишина");
    }
}
