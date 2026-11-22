//! `Vad`: детектор речи на кадрах 16 мс (256 сэмплов, 16 кГц) — начало/конец фразы, pre-roll,
//! минимальная длительность речи, тишина для конца фразы (800–1200 мс, настраивается).
//! Оценка кадра — нейросеть `earshot` (по умолчанию) или энергия (`EnergyScorer`, для тестов и запасной вариант).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::pcm::{FRAME_SAMPLES, STT_SAMPLE_RATE, rms_i16};

const FRAME_MS: u32 = (FRAME_SAMPLES as u32 * 1000) / STT_SAMPLE_RATE;

/// Вероятность речи в кадре 0..=1.
pub trait FrameScorer: Send {
    fn score(&mut self, frame: &[i16]) -> f32;
    fn reset(&mut self);
}

/// Нейросетевой VAD `earshot` (≈8 КиБ состояния, чистый Rust).
pub struct EarshotScorer {
    det: Box<earshot::Detector>,
}

impl Default for EarshotScorer {
    fn default() -> Self {
        Self { det: earshot::Detector::default_boxed() }
    }
}

impl FrameScorer for EarshotScorer {
    fn score(&mut self, frame: &[i16]) -> f32 {
        if frame.len() != FRAME_SAMPLES {
            return 0.0;
        }
        self.det.predict_i16(frame).clamp(0.0, 1.0)
    }
    fn reset(&mut self) {
        self.det.reset();
    }
}

/// Порог по громкости: 1.0 выше `threshold_dbfs`, 0.0 ниже.
pub struct EnergyScorer {
    pub threshold_dbfs: f32,
}

impl FrameScorer for EnergyScorer {
    fn score(&mut self, frame: &[i16]) -> f32 {
        if dbfs(frame) >= self.threshold_dbfs { 1.0 } else { 0.0 }
    }
    fn reset(&mut self) {}
}

/// Уровень кадра в dBFS (тишина ≈ −∞, полная шкала = 0).
pub fn dbfs(frame: &[i16]) -> f32 {
    20.0 * rms_i16(frame).max(1e-9).log10()
}

#[derive(Debug, Clone, PartialEq)]
pub struct VadConfig {
    /// Порог вероятности речи для начала.
    pub threshold: f32,
    /// Порог для продолжения (гистерезис): внутри фразы кадр считается речью при score ≥ этого.
    pub release_threshold: f32,
    /// Сколько подряд речевых кадров нужно, чтобы начать фразу (3 × 16 мс = 48 мс).
    pub start_frames: usize,
    /// Тишина, после которой фраза закончена (800–1200 мс).
    pub end_silence_ms: u32,
    /// Фразы короче (по речевым кадрам) отбрасываются — кашель, щелчок.
    pub min_speech_ms: u32,
    /// Сколько звука до начала речи приложить к фразе.
    pub pre_roll_ms: u32,
    /// Сколько тишины оставить в конце фразы.
    pub post_roll_ms: u32,
    /// Принудительный конец слишком длинной фразы.
    pub max_utterance_ms: u32,
    /// Кадры тише этого уровня — тишина, что бы ни сказала нейросеть (шум вентилятора).
    pub energy_gate_dbfs: Option<f32>,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            release_threshold: 0.35,
            start_frames: 3,
            end_silence_ms: 1000,
            min_speech_ms: 250,
            pre_roll_ms: 300,
            post_roll_ms: 160,
            max_utterance_ms: 30_000,
            energy_gate_dbfs: Some(-60.0),
        }
    }
}

impl VadConfig {
    /// Тишина для конца фразы в рекомендованных пределах 800–1200 мс.
    pub fn with_end_silence(mut self, ms: u32) -> Self {
        self.end_silence_ms = ms.clamp(800, 1200);
        self
    }
}

/// Готовая фраза для STT.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    /// PCM s16 16 кГц mono: pre-roll + речь + немного тишины.
    pub pcm16k: Vec<i16>,
    /// Начало звука фразы (с учётом pre-roll).
    pub started_at: Instant,
    pub ended_at: Instant,
    /// Длительность речевых кадров.
    pub speech_ms: u32,
}

impl Utterance {
    pub fn duration_ms(&self) -> u64 {
        self.pcm16k.len() as u64 * 1000 / STT_SAMPLE_RATE as u64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VadEvent {
    /// Речь началась. `audio` — pre-roll и кадры, подтвердившие начало (их нужно первыми отправить в STT).
    SpeechStart { audio: Vec<i16>, started_at: Instant },
    /// Текущий кадр — часть фразы (передать в потоковый STT).
    Speech,
    /// Фраза закончилась.
    SpeechEnd(Utterance),
    /// Слишком короткий всплеск — отброшен (потоковому STT — `Reset`).
    Discarded { speech_ms: u32 },
}

pub struct Vad {
    cfg: VadConfig,
    scorer: Box<dyn FrameScorer>,
    preroll: VecDeque<i16>,
    candidate: Vec<i16>,
    candidate_frames: usize,
    active: bool,
    utt: Vec<i16>,
    voiced_frames: usize,
    silence_frames: usize,
    started_at: Instant,
    last_score: f32,
}

fn samples_ms(n: usize) -> Duration {
    Duration::from_micros(n as u64 * 1_000_000 / STT_SAMPLE_RATE as u64)
}

impl Vad {
    pub fn new(cfg: VadConfig) -> Self {
        Self::with_scorer(cfg, Box::new(EarshotScorer::default()))
    }

    pub fn with_scorer(cfg: VadConfig, scorer: Box<dyn FrameScorer>) -> Self {
        let cap = (cfg.pre_roll_ms * STT_SAMPLE_RATE / 1000) as usize;
        Self {
            cfg,
            scorer,
            preroll: VecDeque::with_capacity(cap + FRAME_SAMPLES),
            candidate: Vec::new(),
            candidate_frames: 0,
            active: false,
            utt: Vec::new(),
            voiced_frames: 0,
            silence_frames: 0,
            started_at: Instant::now(),
            last_score: 0.0,
        }
    }

    pub fn config(&self) -> &VadConfig {
        &self.cfg
    }

    /// Идёт ли сейчас фраза.
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn last_score(&self) -> f32 {
        self.last_score
    }

    /// Сбросить фразу и состояние нейросети (после полу-дуплексной паузы, смены устройства).
    pub fn reset(&mut self) {
        self.scorer.reset();
        self.preroll.clear();
        self.candidate.clear();
        self.candidate_frames = 0;
        self.active = false;
        self.utt.clear();
        self.voiced_frames = 0;
        self.silence_frames = 0;
    }

    pub fn push(&mut self, frame: &[i16]) -> Option<VadEvent> {
        self.push_at(frame, Instant::now())
    }

    /// То же с явным временем кадра (для тестов и для кадров из буфера).
    pub fn push_at(&mut self, frame: &[i16], now: Instant) -> Option<VadEvent> {
        let mut score = self.scorer.score(frame);
        if self.cfg.energy_gate_dbfs.is_some_and(|g| dbfs(frame) < g) {
            score = 0.0;
        }
        self.last_score = score;
        if !self.active {
            if score >= self.cfg.threshold {
                self.candidate.extend_from_slice(frame);
                self.candidate_frames += 1;
                if self.candidate_frames >= self.cfg.start_frames.max(1) {
                    let mut audio: Vec<i16> = self.preroll.drain(..).collect();
                    audio.append(&mut self.candidate);
                    self.started_at = now.checked_sub(samples_ms(audio.len())).unwrap_or(now);
                    self.voiced_frames = self.candidate_frames;
                    self.candidate_frames = 0;
                    self.silence_frames = 0;
                    self.utt = audio.clone();
                    self.active = true;
                    return Some(VadEvent::SpeechStart { audio, started_at: self.started_at });
                }
            } else {
                let cand = std::mem::take(&mut self.candidate);
                self.candidate_frames = 0;
                self.preroll.extend(cand);
                self.preroll.extend(frame.iter().copied());
                let cap = (self.cfg.pre_roll_ms * STT_SAMPLE_RATE / 1000) as usize;
                while self.preroll.len() > cap {
                    self.preroll.pop_front();
                }
            }
            return None;
        }

        self.utt.extend_from_slice(frame);
        if score >= self.cfg.release_threshold {
            self.voiced_frames += 1;
            self.silence_frames = 0;
        } else {
            self.silence_frames += 1;
        }
        let silence_ms = self.silence_frames as u32 * FRAME_MS;
        let total_ms = samples_ms(self.utt.len()).as_millis() as u32;
        if silence_ms >= self.cfg.end_silence_ms || total_ms >= self.cfg.max_utterance_ms {
            return Some(self.finish(now));
        }
        Some(VadEvent::Speech)
    }

    fn finish(&mut self, now: Instant) -> VadEvent {
        let keep_silence = (self.cfg.post_roll_ms / FRAME_MS) as usize;
        let drop_frames = self.silence_frames.saturating_sub(keep_silence);
        let new_len = self.utt.len().saturating_sub(drop_frames * FRAME_SAMPLES);
        self.utt.truncate(new_len);
        let speech_ms = self.voiced_frames as u32 * FRAME_MS;
        let pcm = std::mem::take(&mut self.utt);
        self.active = false;
        self.voiced_frames = 0;
        self.silence_frames = 0;
        self.preroll.clear();
        if speech_ms < self.cfg.min_speech_ms {
            return VadEvent::Discarded { speech_ms };
        }
        VadEvent::SpeechEnd(Utterance { pcm16k: pcm, started_at: self.started_at, ended_at: now, speech_ms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// Синтетика: (длительность мс, амплитуда) — синус 440 Гц или тишина с лёгким шумом.
    fn signal(parts: &[(u32, f32)]) -> Vec<i16> {
        let mut out = Vec::new();
        let mut seed = 12345u32;
        for &(ms, amp) in parts {
            let n = (ms * STT_SAMPLE_RATE / 1000) as usize;
            for i in 0..n {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = ((seed >> 16) as f32 / 65536.0 - 0.5) * 20.0; // ≈ −70 dBFS
                let s = (i as f32 / STT_SAMPLE_RATE as f32 * 440.0 * TAU).sin() * amp * 32767.0 + noise;
                out.push(s as i16);
            }
        }
        out
    }

    fn run(vad: &mut Vad, pcm: &[i16]) -> Vec<VadEvent> {
        let t0 = Instant::now();
        pcm.as_chunks::<FRAME_SAMPLES>()
            .0
            .iter()
            .enumerate()
            .filter_map(|(i, f)| vad.push_at(f, t0 + Duration::from_millis(i as u64 * 16)))
            .filter(|e| *e != VadEvent::Speech)
            .collect()
    }

    fn energy_vad(cfg: VadConfig) -> Vad {
        Vad::with_scorer(cfg, Box::new(EnergyScorer { threshold_dbfs: -40.0 }))
    }

    #[test]
    fn burst_becomes_utterance_with_preroll() {
        let mut vad = energy_vad(VadConfig::default());
        let ev = run(&mut vad, &signal(&[(800, 0.0), (640, 0.3), (1400, 0.0)]));
        assert_eq!(ev.len(), 2, "{ev:?}");
        let VadEvent::SpeechStart { audio, .. } = &ev[0] else { panic!("{ev:?}") };
        // pre-roll 300 мс + 3 кадра подтверждения.
        assert_eq!(audio.len(), 4800 + 3 * FRAME_SAMPLES);
        let VadEvent::SpeechEnd(u) = &ev[1] else { panic!("{ev:?}") };
        assert!((620..=660).contains(&u.speech_ms), "речь {} мс", u.speech_ms);
        // 300 pre-roll + 640 речь + 160 post-roll ≈ 1100 мс.
        assert!((1060..=1140).contains(&u.duration_ms()), "длина {} мс", u.duration_ms());
        assert!(u.ended_at > u.started_at);
        assert!(!vad.is_active());
    }

    #[test]
    fn short_click_discarded_and_pause_inside_phrase_kept() {
        let mut vad = energy_vad(VadConfig::default());
        let ev = run(&mut vad, &signal(&[(300, 0.0), (100, 0.5), (1500, 0.0)]));
        assert!(matches!(ev.as_slice(), [VadEvent::SpeechStart { .. }, VadEvent::Discarded { .. }]), "{ev:?}");
        // Пауза 500 мс внутри фразы (< 1000 мс) не рвёт её.
        let ev = run(&mut vad, &signal(&[(300, 0.0), (400, 0.3), (500, 0.0), (400, 0.3), (1200, 0.0)]));
        assert_eq!(ev.iter().filter(|e| matches!(e, VadEvent::SpeechEnd(_))).count(), 1, "{ev:?}");
    }

    #[test]
    fn end_silence_configurable_and_max_length() {
        let cfg = VadConfig::default().with_end_silence(2000);
        assert_eq!(cfg.end_silence_ms, 1200, "ограничено 800–1200");
        let mut vad = energy_vad(VadConfig::default().with_end_silence(800));
        // Пауза 900 мс уже рвёт фразу при пороге 800.
        let ev = run(&mut vad, &signal(&[(300, 0.0), (400, 0.3), (900, 0.0), (400, 0.3), (1000, 0.0)]));
        assert_eq!(ev.iter().filter(|e| matches!(e, VadEvent::SpeechEnd(_))).count(), 2, "{ev:?}");
        let mut vad = energy_vad(VadConfig { max_utterance_ms: 1000, ..Default::default() });
        let ev = run(&mut vad, &signal(&[(200, 0.0), (3000, 0.3)]));
        assert!(ev.iter().filter(|e| matches!(e, VadEvent::SpeechEnd(_))).count() >= 2, "длинная речь режется: {ev:?}");
    }

    #[test]
    fn earshot_ignores_silence_and_noise_gate() {
        let mut vad = Vad::new(VadConfig::default());
        let ev = run(&mut vad, &signal(&[(2000, 0.0)]));
        assert!(ev.is_empty(), "{ev:?}");
        let mut s = EarshotScorer::default();
        let frame = vec![0i16; FRAME_SAMPLES];
        let v = s.score(&frame);
        assert!((0.0..=1.0).contains(&v));
        assert_eq!(s.score(&[0; 10]), 0.0, "неверная длина кадра");
        assert!(dbfs(&frame) < -150.0);
        assert!((dbfs(&[i16::MAX; 256]) - 0.0).abs() < 0.01);
    }
}
