//! Потоковый ресемплер моно-сигнала поверх `rubato::Fft` (синхронный, фиксированное отношение частот).
//! Принимает куски произвольной длины, сам копит до размера чанка, обрезает стартовую задержку фильтра
//! и на `flush()` выдаёт ровно `ceil(in * to / from)` сэмплов от начала потока.

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::error::{Result, VoiceError};

pub struct StreamResampler {
    from: u32,
    to: u32,
    inner: Option<Fft<f32>>,
    pending: Vec<f32>,
    scratch: Vec<f32>,
    /// Сколько сэмплов задержки фильтра ещё нужно выбросить на выходе.
    skip: usize,
    total_in: u64,
    total_out: u64,
}

// Свой путь к адаптерам — rubato реэкспортирует audioadapter_buffers.
use rubato::audioadapter_buffers;

impl StreamResampler {
    pub fn new(from: u32, to: u32) -> Result<Self> {
        if from == 0 || to == 0 {
            return Err(VoiceError::InvalidInput("нулевая частота дискретизации".into()));
        }
        let inner = if from == to {
            None
        } else {
            // ~10 мс входа на чанк: малая задержка, FFT всё ещё эффективен.
            let chunk = (from as usize / 100).max(64);
            Some(
                Fft::<f32>::new(from as usize, to as usize, chunk, 1, FixedSync::Input)
                    .map_err(|e| VoiceError::Other(format!("ресемплер {from}→{to}: {e}")))?,
            )
        };
        let skip = inner.as_ref().map(|r| r.output_delay()).unwrap_or(0);
        Ok(Self { from, to, inner, pending: Vec::new(), scratch: Vec::new(), skip, total_in: 0, total_out: 0 })
    }

    pub fn rates(&self) -> (u32, u32) {
        (self.from, self.to)
    }

    pub fn is_passthrough(&self) -> bool {
        self.inner.is_none()
    }

    /// Обработать кусок, дописав готовые сэмплы в `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.total_in += input.len() as u64;
        let Some(r) = self.inner.as_mut() else {
            out.extend_from_slice(input);
            self.total_out += input.len() as u64;
            return Ok(());
        };
        self.pending.extend_from_slice(input);
        let mut offset = 0;
        loop {
            let need = r.input_frames_next();
            if self.pending.len() - offset < need {
                break;
            }
            Self::run_chunk(r, &self.pending[offset..offset + need], None, &mut self.scratch)?;
            offset += need;
            Self::emit(&mut self.scratch, &mut self.skip, &mut self.total_out, out, None);
        }
        self.pending.drain(..offset);
        Ok(())
    }

    /// Дожать хвост: досыпать тишину, чтобы вышли все сэмплы, и обрезать до точной длины.
    pub fn flush(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let expected = (self.total_in * self.to as u64).div_ceil(self.from as u64);
        let Some(r) = self.inner.as_mut() else {
            return Ok(());
        };
        let mut guard = 0;
        while self.total_out < expected && guard < 16 {
            guard += 1;
            let need = r.input_frames_next();
            let have = self.pending.len().min(need);
            let mut chunk = std::mem::take(&mut self.pending);
            chunk.resize(need, 0.0);
            Self::run_chunk(r, &chunk, Some(have), &mut self.scratch)?;
            let limit = (expected - self.total_out) as usize;
            Self::emit(&mut self.scratch, &mut self.skip, &mut self.total_out, out, Some(limit));
        }
        self.reset_inner();
        Ok(())
    }

    /// Сбросить состояние (барж-ин, новая фраза после остановки).
    pub fn reset(&mut self) {
        self.reset_inner();
    }

    fn reset_inner(&mut self) {
        self.pending.clear();
        self.total_in = 0;
        self.total_out = 0;
        if let Some(r) = self.inner.as_mut() {
            r.reset();
            self.skip = r.output_delay();
        }
    }

    fn run_chunk(r: &mut Fft<f32>, chunk: &[f32], partial: Option<usize>, scratch: &mut Vec<f32>) -> Result<()> {
        let err = |e: &dyn std::fmt::Display| VoiceError::Other(format!("ресемплер: {e}"));
        let input = InterleavedSlice::new(chunk, 1, chunk.len()).map_err(|e| err(&e))?;
        let max_out = r.output_frames_max();
        scratch.clear();
        scratch.resize(max_out, 0.0);
        let mut output = InterleavedSlice::new_mut(scratch.as_mut_slice(), 1, max_out).map_err(|e| err(&e))?;
        let indexing = partial.map(|p| Indexing { input_offset: 0, output_offset: 0, partial_len: Some(p), active_channels_mask: None });
        let (_, produced) = r.process_into_buffer(&input, &mut output, indexing.as_ref()).map_err(|e| err(&e))?;
        scratch.truncate(produced);
        Ok(())
    }

    fn emit(scratch: &mut Vec<f32>, skip: &mut usize, total_out: &mut u64, out: &mut Vec<f32>, limit: Option<usize>) {
        let drop_n = (*skip).min(scratch.len());
        *skip -= drop_n;
        let mut slice = &scratch[drop_n..];
        if let Some(l) = limit {
            slice = &slice[..slice.len().min(l)];
        }
        out.extend_from_slice(slice);
        *total_out += slice.len() as u64;
    }
}

/// Ресемплинг целого клипа.
pub fn resample_all(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>> {
    let mut r = StreamResampler::new(from, to)?;
    let mut out = Vec::with_capacity((input.len() as u64 * to as u64 / from.max(1) as u64) as usize + 16);
    r.process(input, &mut out)?;
    r.flush(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn sine(rate: u32, hz: f32, secs: f32, amp: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).map(|i| (i as f32 / rate as f32 * hz * TAU).sin() * amp).collect()
    }

    /// Частота по числу пересечений нуля (в середине сигнала, без краёв).
    fn zero_cross_hz(x: &[f32], rate: u32) -> f32 {
        let mid = &x[x.len() / 10..x.len() * 9 / 10];
        let n = mid.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        n as f32 / 2.0 / (mid.len() as f32 / rate as f32)
    }

    #[test]
    fn upsample_tts_24k_to_48k_streaming() {
        let x = sine(24_000, 440.0, 1.0, 0.5);
        let mut r = StreamResampler::new(24_000, 48_000).unwrap();
        let mut out = Vec::new();
        // Неровные сетевые куски.
        for c in x.chunks(777) {
            r.process(c, &mut out).unwrap();
        }
        r.flush(&mut out).unwrap();
        assert_eq!(out.len(), 48_000);
        assert!((zero_cross_hz(&out, 48_000) - 440.0).abs() < 3.0);
        let mid = &out[4800..43_200];
        assert!((crate::pcm::rms(mid) - 0.5 / 2f32.sqrt()).abs() < 0.01);
        // Задержка фильтра обрезана: сигнал выровнен по фазе с идеальной синусоидой.
        let ideal = sine(48_000, 440.0, 1.0, 0.5);
        let err: f32 = out[4800..9600].iter().zip(&ideal[4800..9600]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 0.05, "фазовая ошибка {err}");
    }

    #[test]
    fn downsample_mic_48k_and_44k_to_16k() {
        for from in [48_000u32, 44_100] {
            let x = sine(from, 1000.0, 0.5, 0.3);
            let out = resample_all(&x, from, 16_000).unwrap();
            assert_eq!(out.len(), 8000, "from {from}");
            assert!((zero_cross_hz(&out, 16_000) - 1000.0).abs() < 5.0);
        }
    }

    #[test]
    fn antialiasing_removes_tone_above_nyquist() {
        // 10 кГц при 48 кГц после перевода в 16 кГц (Найквист 8 кГц) должен почти исчезнуть.
        let x = sine(48_000, 10_000.0, 0.5, 0.5);
        let out = resample_all(&x, 48_000, 16_000).unwrap();
        assert!(crate::pcm::rms(&out[800..7200]) < 0.02);
    }

    #[test]
    fn passthrough_and_reset() {
        let mut r = StreamResampler::new(16_000, 16_000).unwrap();
        assert!(r.is_passthrough());
        let mut out = Vec::new();
        r.process(&[0.1, 0.2], &mut out).unwrap();
        r.flush(&mut out).unwrap();
        assert_eq!(out, vec![0.1, 0.2]);
        let mut r = StreamResampler::new(24_000, 48_000).unwrap();
        let mut a = Vec::new();
        r.process(&sine(24_000, 300.0, 0.1, 0.5), &mut a).unwrap();
        r.reset();
        let mut b = Vec::new();
        r.process(&[0.0; 2400], &mut b).unwrap();
        r.flush(&mut b).unwrap();
        assert_eq!(b.len(), 4800);
        assert!(b.iter().all(|s| s.abs() < 1e-3), "после reset не должно остаться хвоста прошлой фразы");
        assert!(StreamResampler::new(0, 16_000).is_err());
    }
}
