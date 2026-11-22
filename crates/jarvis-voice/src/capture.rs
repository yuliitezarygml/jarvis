//! `Capture`: микрофон cpal → mono → 16 кГц → кадры по 256 сэмплов (16 мс) для VAD/STT.
//! Колбэк устройства только сводит каналы и отдаёт блок в канал (без блокировок); ресемплинг, нарезка
//! и события `AudioLevel { source: Mic }` (≈30 Гц) — в отдельном потоке ОС.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use jarvis_core::event::AudioSource;
use jarvis_core::{Event, EventSink};
use tokio::sync::mpsc;

use crate::device::{device_name, find_input};
use crate::error::{Result, VoiceError};
use crate::pcm::{FrameAssembler, LevelMeter, STT_SAMPLE_RATE, downmix_into};
use crate::resample::StreamResampler;

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Имя (или часть имени) микрофона; `None` — по умолчанию.
    pub device: Option<String>,
    /// Очередь кадров к потребителю (256 кадров ≈ 4 с).
    pub queue_frames: usize,
    pub level_hz: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { device: None, queue_frames: 256, level_hz: 30 }
    }
}

/// Преобразование «блок устройства → кадры 16 кГц» без устройства (тестируется отдельно).
pub struct FramePipe {
    channels: usize,
    resampler: StreamResampler,
    assembler: FrameAssembler,
    meter: LevelMeter,
    mono: Vec<f32>,
    resampled: Vec<f32>,
}

impl FramePipe {
    pub fn new(device_rate: u32, channels: usize, level_hz: u32) -> Result<Self> {
        Ok(Self {
            channels: channels.max(1),
            resampler: StreamResampler::new(device_rate, STT_SAMPLE_RATE)?,
            assembler: FrameAssembler::new(),
            meter: LevelMeter::new(STT_SAMPLE_RATE, level_hz),
            mono: Vec::new(),
            resampled: Vec::new(),
        })
    }

    /// Чередующиеся сэмплы устройства → кадры (`frame`) и уровни (`level`).
    pub fn push(&mut self, interleaved: &[f32], mut frame: impl FnMut(Vec<i16>), mut level: impl FnMut(f32)) -> Result<()> {
        self.mono.clear();
        downmix_into(interleaved, self.channels, &mut self.mono);
        self.push_mono(&mut frame, &mut level)
    }

    fn push_mono(&mut self, frame: &mut impl FnMut(Vec<i16>), level: &mut impl FnMut(f32)) -> Result<()> {
        self.resampled.clear();
        self.resampler.process(&self.mono, &mut self.resampled)?;
        for &s in &self.resampled {
            if let Some(r) = self.meter.push(s) {
                level(r);
            }
        }
        self.assembler.push(&self.resampled, frame);
        Ok(())
    }
}

pub struct Capture {
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    device_name: String,
    device_rate: u32,
    dropped: Arc<AtomicU64>,
}

impl Capture {
    /// Открыть микрофон. Кадры по 256 сэмплов i16 16 кГц приходят в возвращённый канал.
    pub fn start(cfg: CaptureConfig, events: Arc<dyn EventSink>) -> Result<(Self, mpsc::Receiver<Vec<i16>>)> {
        let (frames_tx, frames_rx) = mpsc::channel(cfg.queue_frames.max(8));
        let quit = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let (init_tx, init_rx) = std_mpsc::channel::<Result<(u32, String)>>();
        let (q, d) = (quit.clone(), dropped.clone());
        let thread = std::thread::Builder::new()
            .name("jarvis-capture".into())
            .spawn(move || {
                let (block_tx, block_rx) = std_mpsc::sync_channel::<Vec<f32>>(64);
                let (stream, rate, channels, name) = match open_input(&cfg, block_tx, d.clone()) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                let mut pipe = match FramePipe::new(rate, channels, cfg.level_hz) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                let _ = init_tx.send(Ok((rate, name)));
                while !q.load(Ordering::Acquire) {
                    let block = match block_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(b) => b,
                        Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let r = pipe.push(
                        &block,
                        |f| {
                            if frames_tx.try_send(f).is_err() {
                                d.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        |rms| events.emit(Event::AudioLevel { source: AudioSource::Mic, rms }),
                    );
                    if let Err(e) = r {
                        tracing::warn!(error = %e, "ошибка обработки микрофона");
                    }
                    if frames_tx.is_closed() {
                        break;
                    }
                }
                drop(stream);
            })
            .map_err(|e| VoiceError::Device(e.to_string()))?;
        let (device_rate, device_name) = init_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| VoiceError::Device("микрофон не ответил".into()))??;
        Ok((Self { quit, thread: Some(thread), device_name, device_rate, dropped }, frames_rx))
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// Сколько кадров/блоков потеряно из-за переполнения очередей.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn stop(&mut self) {
        self.quit.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn open_input(
    cfg: &CaptureConfig,
    tx: std_mpsc::SyncSender<Vec<f32>>,
    dropped: Arc<AtomicU64>,
) -> Result<(cpal::Stream, u32, usize, String)> {
    let device = find_input(cfg.device.as_deref())?;
    let name = device_name(&device);
    let supported = device.default_input_config().map_err(|e| VoiceError::Device(format!("{name}: {e}")))?;
    let rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, tx, dropped),
        SampleFormat::I16 => build::<i16>(&device, &config, tx, dropped),
        SampleFormat::U16 => build::<u16>(&device, &config, tx, dropped),
        SampleFormat::I32 => build::<i32>(&device, &config, tx, dropped),
        f => return Err(VoiceError::Device(format!("{name}: формат {f} не поддерживается"))),
    }?;
    stream.play().map_err(|e| VoiceError::Device(format!("{name}: {e}")))?;
    Ok((stream, rate, channels, name))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: std_mpsc::SyncSender<Vec<f32>>,
    dropped: Arc<AtomicU64>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let cb = move |data: &[T], _: &cpal::InputCallbackInfo| {
        let block: Vec<f32> = data.iter().map(|s| f32::from_sample(*s)).collect();
        if tx.try_send(block).is_err() {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
    };
    let err = |e: cpal::Error| tracing::warn!(error = %e, "ошибка потока микрофона");
    device
        .build_input_stream::<T, _, _>(*config, cb, err, Some(Duration::from_secs(5)))
        .map_err(|e| VoiceError::Device(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcm::FRAME_SAMPLES;

    #[test]
    fn stereo_48k_to_16k_frames_and_levels() {
        let mut pipe = FramePipe::new(48_000, 2, 30).unwrap();
        let mut frames = Vec::new();
        let mut levels = Vec::new();
        // 1 с стерео-синуса блоками по 10 мс (как WASAPI).
        for blk in 0..100 {
            let block: Vec<f32> = (0..480)
                .flat_map(|i| {
                    let t = (blk * 480 + i) as f32 / 48_000.0;
                    let v = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                    [v, v]
                })
                .collect();
            pipe.push(&block, |f| frames.push(f), |l| levels.push(l)).unwrap();
        }
        assert!(frames.iter().all(|f| f.len() == FRAME_SAMPLES));
        // 16000 сэмплов → 62 полных кадра (остаток ждёт), с учётом задержки фильтра.
        assert!((60..=62).contains(&frames.len()), "{}", frames.len());
        assert!((28..=30).contains(&levels.len()), "{}", levels.len());
        assert!(levels[10..].iter().all(|l| (l - 0.3535).abs() < 0.03), "{levels:?}");
    }
}
