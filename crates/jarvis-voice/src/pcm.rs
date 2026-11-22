//! PCM-утилиты: s16le ⇄ f32, WAV (запись и разбор), RMS и измеритель уровня для HUD.

use crate::error::{Result, VoiceError};

/// Частота TTS-сервера (PCM s16le mono).
pub const TTS_SAMPLE_RATE: u32 = 24_000;
/// Частота STT/VAD.
pub const STT_SAMPLE_RATE: u32 = jarvis_core::stt_protocol::SAMPLE_RATE;
/// Кадр VAD: 16 мс при 16 кГц.
pub const FRAME_SAMPLES: usize = 256;

#[inline]
pub fn i16_to_f32(s: i16) -> f32 {
    s as f32 / 32768.0
}

#[inline]
pub fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

pub fn i16_to_le_bytes(pcm: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Потоковый декодер s16le: сетевые куски могут резать сэмпл пополам — лишний байт ждёт следующего куска.
#[derive(Debug, Default)]
pub struct S16Decoder {
    carry: Option<u8>,
}

impl S16Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Декодировать кусок в f32 [-1, 1], дописывая в `out`.
    pub fn decode_into(&mut self, bytes: &[u8], out: &mut Vec<f32>) {
        let mut rest = bytes;
        if let Some(lo) = self.carry.take() {
            match rest.split_first() {
                Some((&hi, tail)) => {
                    out.push(i16_to_f32(i16::from_le_bytes([lo, hi])));
                    rest = tail;
                }
                None => {
                    self.carry = Some(lo);
                    return;
                }
            }
        }
        let mut chunks = rest.chunks_exact(2);
        out.extend(chunks.by_ref().map(|c| i16_to_f32(i16::from_le_bytes([c[0], c[1]]))));
        if let [b] = chunks.remainder() {
            self.carry = Some(*b);
        }
    }

    pub fn has_carry(&self) -> bool {
        self.carry.is_some()
    }

    pub fn reset(&mut self) {
        self.carry = None;
    }
}

/// RMS блока f32.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// RMS блока i16 в шкале [0, 1].
pub fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64 / 32768.0).powi(2)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Накопитель RMS: выдаёт значение раз в `window` сэмплов (≈30 Гц для HUD).
#[derive(Debug, Clone)]
pub struct LevelMeter {
    window: usize,
    sum: f64,
    n: usize,
}

impl LevelMeter {
    /// `rate` — частота сэмплов, `hz` — сколько значений в секунду.
    pub fn new(rate: u32, hz: u32) -> Self {
        Self { window: (rate / hz.max(1)).max(1) as usize, sum: 0.0, n: 0 }
    }

    /// Добавить сэмпл; когда окно заполнено — вернуть RMS окна.
    #[inline]
    pub fn push(&mut self, s: f32) -> Option<f32> {
        self.sum += (s as f64) * (s as f64);
        self.n += 1;
        if self.n >= self.window {
            let r = (self.sum / self.n as f64).sqrt() as f32;
            self.sum = 0.0;
            self.n = 0;
            Some(r)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.sum = 0.0;
        self.n = 0;
    }
}

/// Сводит многоканальные чередующиеся сэмплы в моно (среднее по каналам).
pub fn downmix_into(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let k = 1.0 / channels as f32;
    out.extend(interleaved.chunks_exact(channels).map(|f| f.iter().sum::<f32>() * k));
}

/// Нарезает поток f32 (16 кГц) на кадры по 256 сэмплов i16.
#[derive(Debug, Default)]
pub struct FrameAssembler {
    pending: Vec<i16>,
}

impl FrameAssembler {
    pub fn new() -> Self {
        Self { pending: Vec::with_capacity(FRAME_SAMPLES * 2) }
    }

    pub fn push(&mut self, samples: &[f32], mut emit: impl FnMut(Vec<i16>)) {
        for &s in samples {
            self.pending.push(f32_to_i16(s));
            if self.pending.len() == FRAME_SAMPLES {
                emit(std::mem::replace(&mut self.pending, Vec::with_capacity(FRAME_SAMPLES)));
            }
        }
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// WAV PCM16 mono/стерео.
pub fn wav_encode(pcm: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let block_align = channels * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&i16_to_le_bytes(pcm));
    out
}

/// Разобранный WAV: сэмплы приведены к f32, каналы чередуются.
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub samples: Vec<f32>,
}

impl Wav {
    /// Моно 16 кГц i16 — то, что ест STT (с даунмиксом и ресемплингом при необходимости).
    pub fn to_stt_pcm(&self) -> Result<Vec<i16>> {
        let mut mono = Vec::with_capacity(self.samples.len() / self.channels.max(1) as usize);
        downmix_into(&self.samples, self.channels as usize, &mut mono);
        let out = if self.sample_rate == STT_SAMPLE_RATE {
            mono
        } else {
            crate::resample::resample_all(&mono, self.sample_rate, STT_SAMPLE_RATE)?
        };
        Ok(out.into_iter().map(f32_to_i16).collect())
    }

    pub fn duration_ms(&self) -> u64 {
        let frames = self.samples.len() as u64 / self.channels.max(1) as u64;
        frames * 1000 / self.sample_rate.max(1) as u64
    }
}

/// Разбор RIFF/WAVE: PCM 8/16/24/32 бит и IEEE float 32 бит, WAVE_FORMAT_EXTENSIBLE, лишние чанки пропускаются.
pub fn wav_decode(bytes: &[u8]) -> Result<Wav> {
    let bad = |m: &str| VoiceError::InvalidInput(format!("WAV: {m}"));
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("нет заголовка RIFF/WAVE"));
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // format, channels, rate, bits
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        // Потоковые WAV пишут размер data = 0 или 0xFFFFFFFF — читаем до конца файла.
        let body_end = if id == b"data" && (size == 0 || size == u32::MAX as usize) {
            bytes.len()
        } else {
            body_start.checked_add(size).ok_or_else(|| bad("переполнение размера"))?.min(bytes.len())
        };
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(bad("короткий fmt"));
                }
                let mut format = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                if format == 0xFFFE && body.len() >= 26 {
                    // WAVE_FORMAT_EXTENSIBLE: первые 2 байта SubFormat GUID — настоящий формат.
                    format = u16::from_le_bytes([body[24], body[25]]);
                }
                fmt = Some((format, channels, rate, bits));
            }
            b"data" => {
                let (format, channels, sample_rate, bits) = fmt.ok_or_else(|| bad("data раньше fmt"))?;
                if channels == 0 || sample_rate == 0 {
                    return Err(bad("нулевые каналы или частота"));
                }
                let samples: Vec<f32> = match (format, bits) {
                    (1, 8) => body.iter().map(|b| (*b as f32 - 128.0) / 128.0).collect(),
                    (1, 16) => body.chunks_exact(2).map(|c| i16_to_f32(i16::from_le_bytes([c[0], c[1]]))).collect(),
                    (1, 24) => body
                        .chunks_exact(3)
                        .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
                        .collect(),
                    (1, 32) => body
                        .chunks_exact(4)
                        .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
                        .collect(),
                    (3, 32) => body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
                    _ => return Err(bad(&format!("неподдерживаемый формат {format}/{bits} бит"))),
                };
                return Ok(Wav { sample_rate, channels, bits_per_sample: bits, samples });
            }
            _ => {}
        }
        // Чанки выровнены по 2 байта.
        pos = body_start.saturating_add(size).saturating_add(size & 1);
    }
    Err(bad("нет чанка data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s16_decoder_handles_split_samples() {
        let pcm: Vec<i16> = vec![0, 1000, -1000, i16::MAX, i16::MIN, 12345];
        let bytes = i16_to_le_bytes(&pcm);
        let mut dec = S16Decoder::new();
        let mut out = Vec::new();
        // Режем на куски нечётной длины.
        for chunk in [&bytes[0..3], &bytes[3..4], &bytes[4..9], &bytes[9..12]] {
            dec.decode_into(chunk, &mut out);
        }
        assert!(!dec.has_carry());
        let back: Vec<i16> = out.iter().map(|s| f32_to_i16(*s)).collect();
        for (a, b) in pcm.iter().zip(&back) {
            assert!((*a as i32 - *b as i32).abs() <= 1, "{a} vs {b}");
        }
        assert_eq!(back.len(), pcm.len());
    }

    #[test]
    fn wav_roundtrip_16bit() {
        let pcm: Vec<i16> = (0..1600).map(|i| ((i as f32 * 0.05).sin() * 10000.0) as i16).collect();
        let bytes = wav_encode(&pcm, 16_000, 1);
        assert_eq!(bytes.len(), 44 + pcm.len() * 2);
        let wav = wav_decode(&bytes).unwrap();
        assert_eq!((wav.sample_rate, wav.channels, wav.bits_per_sample), (16_000, 1, 16));
        assert_eq!(wav.duration_ms(), 100);
        assert_eq!(wav.to_stt_pcm().unwrap(), pcm);
    }

    #[test]
    fn wav_skips_extra_chunks_and_reads_float_stereo() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        // Лишний чанк нечётной длины перед fmt (с байтом выравнивания).
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"abc\0");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes()); // float
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000u32 * 8).to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        let frames = 4800; // 100 мс
        bytes.extend_from_slice(&((frames * 8) as u32).to_le_bytes());
        for i in 0..frames {
            let v = (i as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            bytes.extend_from_slice(&v.to_le_bytes());
            bytes.extend_from_slice(&(-v).to_le_bytes());
        }
        let wav = wav_decode(&bytes).unwrap();
        assert_eq!((wav.sample_rate, wav.channels, wav.bits_per_sample), (48_000, 2, 32));
        assert_eq!(wav.samples.len(), frames * 2);
        assert_eq!(wav.duration_ms(), 100);
        // L и -L в сумме дают тишину; длина 48к→16к ровно в 3 раза меньше.
        let stt = wav.to_stt_pcm().unwrap();
        assert_eq!(stt.len(), frames / 3);
        assert!(stt.iter().all(|s| s.abs() < 50));
    }

    #[test]
    fn wav_rejects_garbage() {
        assert!(wav_decode(b"hello").is_err());
        assert!(wav_decode(b"RIFF\0\0\0\0WAVEdata\0\0\0\0").is_err());
        let mut b = wav_encode(&[1, 2, 3], 16_000, 1);
        b[20] = 85; // MP3 внутри WAV
        assert!(wav_decode(&b).is_err());
    }

    #[test]
    fn rms_and_level_meter() {
        let sine: Vec<f32> = (0..24_000).map(|i| (i as f32 / 24_000.0 * 300.0 * std::f32::consts::TAU).sin() * 0.5).collect();
        assert!((rms(&sine) - 0.5 / 2f32.sqrt()).abs() < 0.01);
        let mut m = LevelMeter::new(24_000, 30);
        let levels: Vec<f32> = sine.iter().filter_map(|s| m.push(*s)).collect();
        assert_eq!(levels.len(), 30);
        assert!(levels.iter().all(|l| (l - 0.3535).abs() < 0.02));
        assert_eq!(rms(&[]), 0.0);
        assert!((rms_i16(&[16384, -16384]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn downmix_and_frames() {
        let mut mono = Vec::new();
        downmix_into(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2, &mut mono);
        assert_eq!(mono, vec![0.5, 0.5, 0.0]);
        let mut fa = FrameAssembler::new();
        let mut frames = Vec::new();
        fa.push(&vec![0.25; 600], |f| frames.push(f));
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.len() == FRAME_SAMPLES && f[0] == 8192));
        assert_eq!(fa.pending(), 600 - 512);
    }
}
