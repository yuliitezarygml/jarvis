//! Аудио: PCM s16le, разбор WAV, ресемплинг (windowed-sinc, полифазный), шум для замеров.

use std::fmt;

/// Частота, с которой работает сервис и протокол STT.
pub const SAMPLE_RATE: u32 = jarvis_core::stt_protocol::SAMPLE_RATE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    OddPcmLength,
    NotWav,
    Truncated,
    NoFormat,
    NoData,
    Unsupported(String),
    BadParameter(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OddPcmLength => f.write_str("PCM s16le: нечётная длина в байтах"),
            Self::NotWav => f.write_str("не RIFF/WAVE"),
            Self::Truncated => f.write_str("WAV обрезан"),
            Self::NoFormat => f.write_str("WAV без fmt-блока"),
            Self::NoData => f.write_str("WAV без data-блока"),
            Self::Unsupported(s) => write!(f, "неподдерживаемый формат: {s}"),
            Self::BadParameter(s) => write!(f, "неверный параметр: {s}"),
        }
    }
}

impl std::error::Error for AudioError {}

#[inline]
fn s16(lo: u8, hi: u8) -> f32 {
    i16::from_le_bytes([lo, hi]) as f32 / 32768.0
}

/// Целый буфер PCM s16le → f32 [-1, 1).
pub fn s16le_to_f32(bytes: &[u8]) -> Result<Vec<f32>, AudioError> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(AudioError::OddPcmLength);
    }
    Ok(pairs.iter().map(|c| s16(c[0], c[1])).collect())
}

/// f32 → PCM s16le (для тестов и примеров).
pub fn f32_to_s16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Потоковый декодер PCM s16le: кадры WebSocket могут иметь любую длину, в том числе нечётную,
/// поэтому половинка сэмпла переносится в следующий кадр.
#[derive(Debug, Default)]
pub struct PcmDecoder {
    carry: Option<u8>,
}

impl PcmDecoder {
    pub fn push(&mut self, mut bytes: &[u8], out: &mut Vec<f32>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(lo) = self.carry.take() {
            out.push(s16(lo, bytes[0]));
            bytes = &bytes[1..];
        }
        let (pairs, rest) = bytes.as_chunks::<2>();
        out.extend(pairs.iter().map(|c| s16(c[0], c[1])));
        if let [last] = rest {
            self.carry = Some(*last);
        }
    }

    pub fn clear(&mut self) {
        self.carry = None;
    }
}

/// Сводим многоканальный interleaved-сигнал в моно (среднее каналов).
pub fn downmix(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    /// Моно, f32.
    pub samples: Vec<f32>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

#[derive(Clone, Copy)]
struct Fmt {
    format: u16,
    channels: u16,
    rate: u32,
    block_align: u16,
    bits: u16,
}

/// Разбор WAV: PCM 8/16/24/32 бит, IEEE float 32/64, WAVE_FORMAT_EXTENSIBLE; многоканальный → моно.
/// Потоковые WAV с размером data 0/0xFFFFFFFF принимаются (берётся остаток файла).
pub fn parse_wav(data: &[u8]) -> Result<Wav, AudioError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(AudioError::NotWav);
    }
    let mut pos = 12;
    let mut fmt: Option<Fmt> = None;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let size = u32_at(data, pos + 4) as usize;
        let body = pos + 8;
        match id {
            b"fmt " => {
                if size < 16 || body + 16 > data.len() {
                    return Err(AudioError::Truncated);
                }
                let mut format = u16_at(data, body);
                if format == 0xFFFE {
                    // WAVE_FORMAT_EXTENSIBLE: cbSize(2) validBits(2) mask(4) subformat GUID(16)
                    if size < 40 || body + 40 > data.len() {
                        return Err(AudioError::Truncated);
                    }
                    format = u16_at(data, body + 24);
                }
                fmt = Some(Fmt {
                    format,
                    channels: u16_at(data, body + 2),
                    rate: u32_at(data, body + 4),
                    block_align: u16_at(data, body + 12),
                    bits: u16_at(data, body + 14),
                });
            }
            b"data" => {
                let f = fmt.ok_or(AudioError::NoFormat)?;
                let avail = data.len() - body;
                let len = if size == 0 || size > avail {
                    avail
                } else {
                    size
                };
                return decode_wav_data(&data[body..body + len], f);
            }
            _ => {}
        }
        // блоки выравниваются на чётную границу
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    if fmt.is_none() {
        Err(AudioError::NoFormat)
    } else {
        Err(AudioError::NoData)
    }
}

fn decode_wav_data(raw: &[u8], f: Fmt) -> Result<Wav, AudioError> {
    if !(1..=8).contains(&f.channels) {
        return Err(AudioError::Unsupported(format!("каналов: {}", f.channels)));
    }
    if !(4000..=192_000).contains(&f.rate) {
        return Err(AudioError::Unsupported(format!("частота: {}", f.rate)));
    }
    let bytes = (f.bits as usize).div_ceil(8);
    if bytes == 0 || f.block_align as usize != bytes * f.channels as usize {
        return Err(AudioError::Unsupported(format!(
            "block_align {} при {} бит × {} кан.",
            f.block_align, f.bits, f.channels
        )));
    }
    let frames = raw.len() / f.block_align as usize;
    let raw = &raw[..frames * f.block_align as usize];
    let interleaved: Vec<f32> = match (f.format, bytes) {
        (1, 1) => raw.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (1, 2) => raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| s16(c[0], c[1]))
            .collect(),
        (1, 3) => raw
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (1, 4) => raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c) as f32 / 2_147_483_648.0)
            .collect(),
        (3, 4) => raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
        (3, 8) => raw
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes(*c) as f32)
            .collect(),
        (fmt, _) => {
            return Err(AudioError::Unsupported(format!(
                "format_tag {fmt}, {} бит",
                f.bits
            )));
        }
    };
    Ok(Wav {
        sample_rate: f.rate,
        channels: f.channels,
        samples: downmix(&interleaved, f.channels),
    })
}

/// Простой WAV PCM16 mono (для тестов, примеров и отладки).
pub fn write_wav_pcm16(samples: &[f32], rate: u32) -> Vec<u8> {
    let pcm = f32_to_s16le(samples);
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Модифицированная функция Бесселя I0 (для окна Кайзера).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..50 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

/// Ресемплинг windowed-sinc (окно Кайзера, β=8.6 ≈ −86 дБ), полифазная таблица для рационального
/// отношения частот. Срез ФНЧ — 0.92 от меньшей частоты Найквиста. Длина выхода = ceil(n·to/from).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    assert!(from > 0 && to > 0, "sample rate must be positive");
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let g = gcd(from as u64, to as u64);
    let (up, down) = (to as u64 / g, from as u64 / g);
    let out_len = (input.len() as u64 * up).div_ceil(down) as usize;
    // частота среза относительно частоты Найквиста входа
    let cutoff = (to as f64 / from as f64).min(1.0) * 0.92;
    const ZEROS: f64 = 24.0;
    const BETA: f64 = 8.6;
    let half = ZEROS / cutoff; // полуширина ядра во входных отсчётах
    let k = half.ceil() as i64;
    let taps = (2 * k) as usize;
    let i0b = bessel_i0(BETA);
    // таблица: для фазы p (дробная часть p/up) коэффициенты при отсчётах i-k+1 ..= i+k
    let mut table = vec![0f32; up as usize * taps];
    for p in 0..up as usize {
        let frac = p as f64 / up as f64;
        let row = &mut table[p * taps..(p + 1) * taps];
        let mut sum = 0.0;
        let mut tmp = vec![0f64; taps];
        for (j, w) in tmp.iter_mut().enumerate() {
            let d = (j as i64 - (k - 1)) as f64 - frac; // расстояние до позиции выхода
            let x = d / half;
            if x.abs() >= 1.0 {
                continue;
            }
            let arg = std::f64::consts::PI * cutoff * d;
            let sinc = if arg.abs() < 1e-12 {
                1.0
            } else {
                arg.sin() / arg
            };
            let win = bessel_i0(BETA * (1.0 - x * x).sqrt()) / i0b;
            *w = cutoff * sinc * win;
            sum += *w;
        }
        for (dst, w) in row.iter_mut().zip(tmp) {
            *dst = (w / sum) as f32; // единичное усиление по постоянной составляющей
        }
    }
    let n = input.len() as i64;
    let mut out = Vec::with_capacity(out_len);
    for o in 0..out_len as u64 {
        let num = o * down;
        let i = (num / up) as i64;
        let p = (num % up) as usize;
        let row = &table[p * taps..(p + 1) * taps];
        let start = i - (k - 1);
        let mut acc = 0f32;
        if start >= 0 && start + taps as i64 <= n {
            let src = &input[start as usize..start as usize + taps];
            for (a, b) in src.iter().zip(row) {
                acc += a * b;
            }
        } else {
            for (j, c) in row.iter().enumerate() {
                let idx = start + j as i64;
                if (0..n).contains(&idx) {
                    acc += input[idx as usize] * c;
                }
            }
        }
        out.push(acc);
    }
    out
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt() as f32
}

/// Детерминированный белый гауссов шум единичного RMS (xorshift64* + Box–Muller).
pub fn white_noise(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..len)
        .map(|_| {
            let (u1, u2) = (next().max(1e-12), next());
            ((-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()) as f32
        })
        .collect()
}

/// Добавить белый шум с заданным SNR относительно `signal_rms` (по умолчанию — RMS всей записи).
pub fn add_white_noise_at(samples: &mut [f32], snr_db: f32, signal_rms: f32, seed: u64) {
    let level = signal_rms / 10f32.powf(snr_db / 20.0);
    let noise = white_noise(samples.len(), seed);
    for (s, n) in samples.iter_mut().zip(noise) {
        *s = (*s + n * level).clamp(-1.0, 1.0);
    }
}

/// Белый шум с заданным SNR по среднему уровню всей записи.
pub fn add_white_noise(samples: &mut [f32], snr_db: f32, seed: u64) {
    let r = rms(samples);
    add_white_noise_at(samples, snr_db, r, seed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, secs: f64) -> Vec<f32> {
        (0..(rate as f64 * secs) as usize)
            .map(|i| {
                (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32 * 0.5
            })
            .collect()
    }

    /// Амплитуда компоненты частоты `freq` (проекция на sin/cos) по середине сигнала.
    fn amplitude(x: &[f32], freq: f64, rate: u32) -> f64 {
        let x = &x[x.len() / 4..x.len() * 3 / 4];
        let (mut s, mut c) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64;
            s += *v as f64 * ph.sin();
            c += *v as f64 * ph.cos();
        }
        2.0 * (s * s + c * c).sqrt() / x.len() as f64
    }

    #[test]
    fn pcm_roundtrip_and_odd_length() {
        let bytes = f32_to_s16le(&[0.0, 0.5, -0.5, 1.0, -1.0]);
        let back = s16le_to_f32(&bytes).unwrap();
        assert!((back[1] - 0.5).abs() < 1e-3 && (back[2] + 0.5).abs() < 1e-3);
        assert!(back[4] <= -0.99);
        assert_eq!(s16le_to_f32(&[1, 2, 3]), Err(AudioError::OddPcmLength));
    }

    #[test]
    fn pcm_decoder_carries_half_samples() {
        let samples: Vec<f32> = (0..50).map(|i| (i as f32 - 25.0) / 30.0).collect();
        let bytes = f32_to_s16le(&samples);
        let whole = s16le_to_f32(&bytes).unwrap();
        let mut dec = PcmDecoder::default();
        let mut out = Vec::new();
        // режем на куски нечётной длины
        for chunk in bytes.chunks(7) {
            dec.push(chunk, &mut out);
        }
        assert_eq!(out, whole);
        dec.push(&[0x01], &mut out);
        dec.clear();
        dec.push(&[0x00, 0x40], &mut out);
        assert_eq!(*out.last().unwrap(), 0.5);
    }

    #[test]
    fn wav_pcm16_roundtrip() {
        let s = sine(440.0, 16000, 0.1);
        let wav = write_wav_pcm16(&s, 16000);
        let w = parse_wav(&wav).unwrap();
        assert_eq!(
            (w.sample_rate, w.channels, w.samples.len()),
            (16000, 1, s.len())
        );
        assert!(w.samples.iter().zip(&s).all(|(a, b)| (a - b).abs() < 1e-3));
    }

    fn wav_with(
        format: u16,
        channels: u16,
        rate: u32,
        bits: u16,
        data: &[u8],
        extensible: bool,
    ) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&(if extensible { 0xFFFE } else { format }).to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        let align = channels * bits / 8;
        fmt.extend_from_slice(&(rate * align as u32).to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        if extensible {
            fmt.extend_from_slice(&22u16.to_le_bytes());
            fmt.extend_from_slice(&bits.to_le_bytes());
            fmt.extend_from_slice(&3u32.to_le_bytes());
            fmt.extend_from_slice(&format.to_le_bytes());
            fmt.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
        }
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        // посторонний блок нечётной длины перед fmt (с байтом выравнивания)
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&[1, 2, 3, 0]);
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn wav_formats_and_stereo_downmix() {
        // стерео 16 бит: L=0.5, R=-0.25 → моно 0.125
        let mut d = Vec::new();
        for _ in 0..10 {
            d.extend_from_slice(&16384i16.to_le_bytes());
            d.extend_from_slice(&(-8192i16).to_le_bytes());
        }
        let w = parse_wav(&wav_with(1, 2, 48000, 16, &d, false)).unwrap();
        assert_eq!((w.sample_rate, w.channels, w.samples.len()), (48000, 2, 10));
        assert!((w.samples[0] - 0.125).abs() < 1e-4);

        // float32 extensible
        let d: Vec<u8> = [0.25f32, -0.75]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let w = parse_wav(&wav_with(3, 1, 22050, 32, &d, true)).unwrap();
        assert_eq!(w.samples, vec![0.25, -0.75]);

        // 24 бит
        let d = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0]; // 0.5, -0.5
        let w = parse_wav(&wav_with(1, 1, 16000, 24, &d, false)).unwrap();
        assert!((w.samples[0] - 0.5).abs() < 1e-6 && (w.samples[1] + 0.5).abs() < 1e-6);

        // 8 бит unsigned
        let w = parse_wav(&wav_with(1, 1, 8000, 8, &[128, 192, 64], false)).unwrap();
        assert_eq!(w.samples, vec![0.0, 0.5, -0.5]);
    }

    #[test]
    fn wav_errors_and_streamed_size() {
        assert_eq!(
            parse_wav(b"hello world, not a wav"),
            Err(AudioError::NotWav)
        );
        let mut w = write_wav_pcm16(&[0.1; 100], 16000);
        // размер data = 0xFFFFFFFF (потоковая запись) → берём остаток
        w[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_wav(&w).unwrap().samples.len(), 100);
        // a-law не поддерживаем
        let bad = wav_with(6, 1, 8000, 8, &[1, 2], false);
        assert!(matches!(parse_wav(&bad), Err(AudioError::Unsupported(_))));
        // нет data
        assert_eq!(parse_wav(&w[..36]), Err(AudioError::NoData));
    }

    #[test]
    fn resample_identity_and_lengths() {
        let x = sine(300.0, 16000, 0.05);
        assert_eq!(resample(&x, 16000, 16000), x);
        assert_eq!(resample(&vec![0.0; 2400], 24000, 16000).len(), 1600);
        assert_eq!(resample(&vec![0.0; 4800], 48000, 16000).len(), 1600);
        assert_eq!(resample(&vec![0.0; 441], 44100, 16000).len(), 160);
        assert_eq!(resample(&vec![0.0; 800], 8000, 16000).len(), 1600);
        assert_eq!(resample(&[0.0; 10], 24000, 16000).len(), 7); // ceil(10*2/3)
    }

    #[test]
    fn resample_keeps_passband_and_rejects_alias() {
        for (from, f) in [
            (24000, 1000.0),
            (24000, 5000.0),
            (48000, 5000.0),
            (44100, 3000.0),
            (8000, 1000.0),
        ] {
            let y = resample(&sine(f, from, 0.5), from, 16000);
            let a = amplitude(&y, f, 16000);
            assert!((a - 0.5).abs() < 0.01, "{from}->16k {f} Гц: амплитуда {a}");
        }
        // 11 кГц при 48 кГц выше Найквиста 16 кГц: без ФНЧ отразилось бы в 5 кГц
        let y = resample(&sine(11000.0, 48000, 0.5), 48000, 16000);
        assert!(
            rms(&y[2000..6000]) < 0.002,
            "alias rms {}",
            rms(&y[2000..6000])
        );
        // постоянная составляющая сохраняется
        let y = resample(&vec![0.3; 3000], 24000, 16000);
        assert!(y[200..1800].iter().all(|v| (v - 0.3).abs() < 1e-4));
    }

    #[test]
    fn noise_has_requested_snr_and_is_deterministic() {
        let clean = sine(500.0, 16000, 1.0);
        let mut a = clean.clone();
        let mut b = clean.clone();
        add_white_noise(&mut a, 10.0, 7);
        add_white_noise(&mut b, 10.0, 7);
        assert_eq!(a, b);
        let noise: Vec<f32> = a.iter().zip(&clean).map(|(x, y)| x - y).collect();
        let snr = 20.0 * (rms(&clean) / rms(&noise)).log10();
        assert!((snr - 10.0).abs() < 0.3, "snr {snr}");
    }
}
