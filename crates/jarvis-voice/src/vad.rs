//! `Vad`: детектор речи на кадрах 16 мс (256 сэмплов, 16 кГц) — начало/конец фразы, pre-roll,
//! минимальная длительность речи, тишина для конца фразы (800–1200 мс, настраивается).
//! Оценка кадра — нейросеть `earshot` (по умолчанию) или энергия (`EnergyScorer`, для тестов и запасной вариант).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::pcm::{FRAME_SAMPLES, STT_SAMPLE_RATE, rms_i16};
use crate::tone::{FeatureAccumulator, VoiceFeatures};

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
    /// Адаптивный порог: кадр тише «фона помещения + N дБ» — тишина. Фон оценивается по кадрам вне речи.
    /// `None` — выключено.
    pub noise_margin_db: Option<f32>,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            release_threshold: 0.35,
            start_frames: 3,
            end_silence_ms: 700,
            min_speech_ms: 250,
            pre_roll_ms: 300,
            post_roll_ms: 160,
            max_utterance_ms: 15_000,
            energy_gate_dbfs: Some(-60.0),
            noise_margin_db: Some(8.0),
        }
    }
}

impl VadConfig {
    /// Тишина для конца фразы в пределах 500–1200 мс.
    pub fn with_end_silence(mut self, ms: u32) -> Self {
        self.end_silence_ms = ms.clamp(500, 1200);
        self
    }
}

/// Начальная оценка фона помещения (dBFS), пока не набралась статистика.
const NOISE_FLOOR_INIT: f32 = -70.0;

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
    /// Громкость, перегруз и фон фразы (по звучащим кадрам) — для оценки тона голоса.
    pub features: VoiceFeatures,
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
    /// Оценка фона помещения (dBFS): минимум громкости кадров за последние ~3 с (между словами
    /// всегда есть провалы, поэтому минимум — это фон, а не речь).
    noise_floor: f32,
    levels: VecDeque<f32>,
    /// Замеры кадров-кандидатов (подтверждают начало фразы) и звучащих кадров текущей фразы.
    cand_stats: FeatureAccumulator,
    utt_stats: FeatureAccumulator,
}

/// Окно оценки фона: 188 кадров × 16 мс ≈ 3 с.
const NOISE_WINDOW_FRAMES: usize = 188;
/// Пока кадров меньше — фон не оценивается (старт, после сброса).
const NOISE_MIN_FRAMES: usize = 30;

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
            noise_floor: NOISE_FLOOR_INIT,
            levels: VecDeque::with_capacity(NOISE_WINDOW_FRAMES + 1),
            cand_stats: FeatureAccumulator::default(),
            utt_stats: FeatureAccumulator::default(),
        }
    }

    /// Текущая оценка фона помещения, dBFS.
    pub fn noise_floor_dbfs(&self) -> f32 {
        self.noise_floor
    }

    /// Закончить текущую фразу сейчас (например, текст распознавания перестал меняться).
    /// `None`, если фразы нет.
    pub fn force_end(&mut self, now: Instant) -> Option<VadEvent> {
        self.active.then(|| self.finish(now))
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
        self.cand_stats.clear();
        self.utt_stats.clear();
    }

    pub fn push(&mut self, frame: &[i16]) -> Option<VadEvent> {
        self.push_at(frame, Instant::now())
    }

    /// То же с явным временем кадра (для тестов и для кадров из буфера).
    pub fn push_at(&mut self, frame: &[i16], now: Instant) -> Option<VadEvent> {
        let mut score = self.scorer.score(frame);
        let level = dbfs(frame);
        if self.cfg.energy_gate_dbfs.is_some_and(|g| level < g) {
            score = 0.0;
        }
        if let Some(margin) = self.cfg.noise_margin_db {
            self.levels.push_back(if level.is_finite() { level } else { -150.0 });
            if self.levels.len() > NOISE_WINDOW_FRAMES {
                self.levels.pop_front();
            }
            if self.levels.len() >= NOISE_MIN_FRAMES {
                self.noise_floor = self.levels.iter().copied().fold(f32::INFINITY, f32::min).max(NOISE_FLOOR_INIT);
            }
            if level < self.noise_floor + margin {
                score = 0.0;
            }
        }
        self.last_score = score;
        if !self.active {
            if score >= self.cfg.threshold {
                self.candidate.extend_from_slice(frame);
                self.candidate_frames += 1;
                self.cand_stats.push(frame, level);
                if self.candidate_frames >= self.cfg.start_frames.max(1) {
                    let mut audio: Vec<i16> = self.preroll.drain(..).collect();
                    audio.append(&mut self.candidate);
                    self.started_at = now.checked_sub(samples_ms(audio.len())).unwrap_or(now);
                    self.voiced_frames = self.candidate_frames;
                    self.candidate_frames = 0;
                    self.silence_frames = 0;
                    self.utt = audio.clone();
                    self.utt_stats.clear();
                    self.utt_stats.append(&mut self.cand_stats);
                    self.active = true;
                    return Some(VadEvent::SpeechStart { audio, started_at: self.started_at });
                }
            } else {
                let cand = std::mem::take(&mut self.candidate);
                self.candidate_frames = 0;
                self.cand_stats.clear();
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
            self.utt_stats.push(frame, level);
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
            self.utt_stats.clear();
            return VadEvent::Discarded { speech_ms };
        }
        let duration_ms = (pcm.len() as u64 * 1000 / STT_SAMPLE_RATE as u64) as u32;
        let features = self.utt_stats.finish(self.noise_floor, speech_ms, duration_ms);
        VadEvent::SpeechEnd(Utterance { pcm16k: pcm, started_at: self.started_at, ended_at: now, speech_ms, features })
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
    fn force_end_finishes_active_phrase() {
        let mut vad = energy_vad(VadConfig::default());
        assert!(vad.force_end(Instant::now()).is_none(), "нет фразы — нечего заканчивать");
        let ev = run(&mut vad, &signal(&[(300, 0.0), (600, 0.3)]));
        assert!(matches!(ev.as_slice(), [VadEvent::SpeechStart { .. }]), "{ev:?}");
        assert!(vad.is_active());
        assert!(matches!(vad.force_end(Instant::now()), Some(VadEvent::SpeechEnd(_))));
        assert!(!vad.is_active());
    }

    #[test]
    fn noise_floor_follows_room_and_gates_hum() {
        // Энергетический скорер считает «речью» всё громче −40 dBFS; гул −35 dBFS сначала проходит,
        // но фон его выучивает, и через несколько секунд гул — уже тишина.
        let mut vad = Vad::with_scorer(VadConfig::default(), Box::new(EnergyScorer { threshold_dbfs: -40.0 }));
        let hum = signal(&[(30_000, 0.025)]);
        let ev = run(&mut vad, &hum);
        assert!(vad.noise_floor_dbfs() > -45.0, "фон выучен: {}", vad.noise_floor_dbfs());
        let ends = ev.iter().filter(|e| matches!(e, VadEvent::SpeechEnd(_))).count();
        assert!(ends <= 2, "гул не должен давать поток фраз: {ev:?}");
        // Голос заметно громче фона по-прежнему распознаётся.
        let ev = run(&mut vad, &signal(&[(600, 0.4), (1000, 0.025)]));
        assert!(ev.iter().any(|e| matches!(e, VadEvent::SpeechEnd(_))), "{ev:?}");
    }

    #[test]
    fn end_silence_configurable_and_max_length() {
        let cfg = VadConfig::default().with_end_silence(2000);
        assert_eq!(cfg.end_silence_ms, 1200, "ограничено 500–1200");
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
    fn only_utterance(ev: &[VadEvent]) -> &Utterance {
        let ends: Vec<&Utterance> = ev.iter().filter_map(|e| if let VadEvent::SpeechEnd(u) = e { Some(u) } else { None }).collect();
        assert_eq!(ends.len(), 1, "{ev:?}");
        ends[0]
    }

    /// dBFS синуса с амплитудой `amp` (доля полной шкалы): RMS = amp/√2.
    fn sine_dbfs(amp: f32) -> f32 {
        20.0 * (amp / std::f32::consts::SQRT_2).log10()
    }

    #[test]
    fn features_of_sine_at_known_level() {
        let mut vad = energy_vad(VadConfig::default());
        let ev = run(&mut vad, &signal(&[(800, 0.0), (1000, 0.3), (1400, 0.0)]));
        let f = only_utterance(&ev).features;
        let want = sine_dbfs(0.3); // ≈ −13,5 dBFS
        assert!((f.mean_dbfs - want).abs() < 0.5, "{f:?} / {want}");
        assert!((f.p90_dbfs - want).abs() < 0.5, "{f:?}");
        assert!((f.peak_dbfs - 20.0 * 0.3f32.log10()).abs() < 0.2, "пик — амплитуда синуса: {f:?}");
        assert_eq!(f.clipped_ratio, 0.0);
        assert!((980..=1020).contains(&f.speech_ms), "{f:?}");
        assert_eq!(f.duration_ms as u64, only_utterance(&ev).duration_ms());
        assert!(f.noise_floor_dbfs < -60.0, "фон — тихий шум: {f:?}");
    }

    #[test]
    fn features_ignore_pauses_and_p90_tracks_loud_part() {
        // Паузы внутри фразы не тянут громкость вниз: считаются только звучащие кадры.
        let mut vad = energy_vad(VadConfig::default());
        let ev = run(&mut vad, &signal(&[(500, 0.0), (400, 0.3), (500, 0.0), (400, 0.3), (1400, 0.0)]));
        let f = only_utterance(&ev).features;
        assert!((f.mean_dbfs - sine_dbfs(0.3)).abs() < 0.6, "{f:?}");
        assert!((f.p90_dbfs - sine_dbfs(0.3)).abs() < 0.5, "{f:?}");
        // Тихая и громкая половины: p90 — громкая часть, среднее по энергии — между ними, ближе к громкой.
        let mut vad = energy_vad(VadConfig::default());
        let ev = run(&mut vad, &signal(&[(500, 0.0), (600, 0.1), (600, 0.4), (1400, 0.0)]));
        let f = only_utterance(&ev).features;
        assert!((f.p90_dbfs - sine_dbfs(0.4)).abs() < 0.5, "{f:?}");
        let mean = 10.0 * ((0.1f32.powi(2) / 2.0 + 0.4f32.powi(2) / 2.0) / 2.0).log10();
        assert!((f.mean_dbfs - mean).abs() < 0.6, "{f:?} / {mean}");
        assert!(f.mean_dbfs > sine_dbfs(0.1) + 5.0);
    }

    #[test]
    fn features_detect_clipping() {
        let mut vad = energy_vad(VadConfig::default());
        // Амплитуда 1,5 — сэмплы упираются в i16::MAX/MIN.
        let ev = run(&mut vad, &signal(&[(500, 0.0), (800, 1.5), (1400, 0.0)]));
        let f = only_utterance(&ev).features;
        assert!(f.clipped_ratio > 0.3, "{f:?}");
        assert!(f.peak_dbfs > -0.1, "{f:?}");
        // Отброшенный щелчок не оставляет замеров следующей фразе.
        let ev = run(&mut vad, &signal(&[(300, 0.0), (100, 1.5), (1500, 0.0), (800, 0.2), (1400, 0.0)]));
        let f = only_utterance(&ev).features;
        assert_eq!(f.clipped_ratio, 0.0, "{f:?}");
        assert!((f.p90_dbfs - sine_dbfs(0.2)).abs() < 0.5, "{f:?}");
    }
}
