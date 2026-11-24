//! Подрезка тишины в речи Джарвиса (потоково, перед воспроизведением).
//!
//! Ответ синтезируется по предложениям, и сервер голоса кладёт в каждый кусок ~0,1 с тишины в начале и
//! 0,15–0,5 с в конце; модель и внутри фразы делает после точки до 0,5 с. На стыках выходило 0,25–0,65 с —
//! на слух «запинается на точках». Здесь: тишина в начале куска почти убирается, любая пауза внутри ужимается
//! до `max_pause_ms`, хвост куска — до `tail_ms` (это и есть пауза между предложениями).

/// Настройки подрезки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrimConfig {
    /// Тише этого (дБFS, среднеквадратичное по окну 10 мс) — тишина.
    pub silence_dbfs: f32,
    /// Максимальная пауза внутри куска (после запятой/точки внутри фразы), мс.
    pub max_pause_ms: u32,
    /// Сколько тишины оставить в конце куска (пауза между предложениями), мс.
    pub tail_ms: u32,
    /// Сколько тишины оставить в начале куска (мягкий вход без щелчка), мс.
    pub lead_ms: u32,
}

impl Default for TrimConfig {
    fn default() -> Self {
        Self { silence_dbfs: -45.0, max_pause_ms: 220, tail_ms: 160, lead_ms: 15 }
    }
}

/// Потоковый подрезчик одного куска речи: `push` — по мере прихода звука, `finish` — в конце куска.
#[derive(Debug)]
pub struct SilenceTrimmer {
    cfg: TrimConfig,
    rate: u32,
    window: usize,
    threshold: f32,
    /// Звук ещё не начался (тишина в начале куска).
    leading: bool,
    /// Неполное окно (меньше `window` сэмплов).
    pending: Vec<f32>,
    /// Текущая пауза — ещё не выданная тишина (сколько оставить, решим, когда пауза кончится).
    quiet: Vec<f32>,
}

impl SilenceTrimmer {
    pub fn new(cfg: TrimConfig, rate: u32) -> Self {
        let window = (rate as usize / 100).max(1);
        let threshold = 10f32.powf(cfg.silence_dbfs / 20.0);
        Self { cfg, rate, window, threshold, leading: true, pending: Vec::new(), quiet: Vec::new() }
    }

    fn samples(&self, ms: u32) -> usize {
        self.rate as usize * ms as usize / 1000
    }

    fn is_quiet(&self, w: &[f32]) -> bool {
        let e = w.iter().map(|x| x * x).sum::<f32>() / w.len().max(1) as f32;
        e.sqrt() < self.threshold
    }

    /// Принять звук; в `out` — то, что уже можно играть.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.pending.extend_from_slice(input);
        let mut start = 0;
        while self.pending.len() - start >= self.window {
            let end = start + self.window;
            if self.is_quiet(&self.pending[start..end]) {
                let w = self.pending[start..end].to_vec();
                self.quiet.extend_from_slice(&w);
            } else {
                self.flush_pause(out, false);
                self.leading = false;
                out.extend_from_slice(&self.pending[start..end]);
            }
            start = end;
        }
        self.pending.drain(..start);
    }

    /// Пауза кончилась (дальше звук) или кончился кусок: выдать разрешённую часть накопленной тишины.
    fn flush_pause(&mut self, out: &mut Vec<f32>, at_end: bool) {
        if self.quiet.is_empty() {
            return;
        }
        let keep = if self.leading {
            self.samples(self.cfg.lead_ms)
        } else if at_end {
            self.samples(self.cfg.tail_ms)
        } else {
            self.samples(self.cfg.max_pause_ms)
        };
        let n = keep.min(self.quiet.len());
        // В начале оставляем конец паузы (мягкий вход в звук), дальше — начало паузы (затухание после звука).
        if self.leading {
            out.extend_from_slice(&self.quiet[self.quiet.len() - n..]);
        } else {
            out.extend_from_slice(&self.quiet[..n]);
        }
        self.quiet.clear();
    }

    /// Конец куска: остаток неполного окна и хвостовая пауза (не длиннее `tail_ms`).
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        let rest = std::mem::take(&mut self.pending);
        if !rest.is_empty() {
            if self.is_quiet(&rest) {
                self.quiet.extend_from_slice(&rest);
            } else {
                self.flush_pause(out, false);
                self.leading = false;
                out.extend_from_slice(&rest);
            }
        }
        if self.leading {
            // Кусок целиком из тишины — ничего не играем.
            self.quiet.clear();
        } else {
            self.flush_pause(out, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 24_000;

    fn tone(ms: u32) -> Vec<f32> {
        let n = RATE as usize * ms as usize / 1000;
        (0..n).map(|i| 0.3 * (i as f32 * 0.07).sin()).collect()
    }
    fn silence(ms: u32) -> Vec<f32> {
        vec![0.0; RATE as usize * ms as usize / 1000]
    }
    fn ms(n: usize) -> u32 {
        (n * 1000 / RATE as usize) as u32
    }

    fn run(parts: &[Vec<f32>], chunk: usize) -> Vec<f32> {
        let all: Vec<f32> = parts.concat();
        let mut t = SilenceTrimmer::new(TrimConfig::default(), RATE);
        let mut out = Vec::new();
        for c in all.chunks(chunk) {
            t.push(c, &mut out);
        }
        t.finish(&mut out);
        out
    }

    #[test]
    fn trims_lead_tail_and_long_pauses() {
        // Как приходит с сервера: 90 мс тишины, речь, 520 мс пауза после точки, речь, 515 мс хвост.
        let total = ms(run(&[silence(90), tone(400), silence(520), tone(300), silence(515)], 1000).len());
        // Речь 700 мс + начало ≤15 + пауза ≤220 + хвост ≤160 (± окно 10 мс).
        assert!((700..=700 + 15 + 220 + 160 + 20).contains(&total), "{total} мс");
    }

    #[test]
    fn short_pauses_and_speech_untouched() {
        // Пауза 150 мс (запятая) короче предела — остаётся; речь не теряется.
        let total = ms(run(&[tone(300), silence(150), tone(300)], 777).len());
        assert!((740..=780).contains(&total), "{total} мс");
    }

    /// Ручная проверка на настоящем голосе: `JARVIS_TRIM_DIR` с кусками `1.wav`, `2.wav`, … (как их синтезирует
    /// Джарвис по предложениям) → `before.wav` (склейка как есть) и `after.wav` (через подрезчик) + паузы на стыках.
    #[test]
    #[ignore = "нужна папка с записями JARVIS_TRIM_DIR"]
    fn live_pieces_before_after() {
        let Ok(dir) = std::env::var("JARVIS_TRIM_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut rate = RATE;
        for i in 1.. {
            let Ok(bytes) = std::fs::read(dir.join(format!("{i}.wav"))) else { break };
            let wav = crate::pcm::wav_decode(&bytes).unwrap();
            rate = wav.sample_rate;
            before.extend_from_slice(&wav.samples);
            let mut t = SilenceTrimmer::new(TrimConfig::default(), rate);
            t.push(&wav.samples, &mut after);
            t.finish(&mut after);
        }
        let gaps = |s: &[f32]| -> Vec<u32> {
            let w = rate as usize / 100;
            let mut runs = Vec::new();
            let mut cur = 0usize;
            let mut seen_sound = false;
            for c in s.chunks(w) {
                let q = (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt() < 10f32.powf(-45.0 / 20.0);
                if q {
                    cur += c.len();
                } else {
                    if seen_sound && cur >= w * 6 {
                        runs.push((cur * 1000 / rate as usize) as u32);
                    }
                    cur = 0;
                    seen_sound = true;
                }
            }
            runs
        };
        let enc = |s: &[f32]| crate::pcm::wav_encode(&s.iter().map(|x| crate::pcm::f32_to_i16(*x)).collect::<Vec<_>>(), rate, 1);
        std::fs::write(dir.join("before.wav"), enc(&before)).unwrap();
        std::fs::write(dir.join("after.wav"), enc(&after)).unwrap();
        println!("TRIM было: {} мс, паузы {:?} мс", before.len() * 1000 / rate as usize, gaps(&before));
        println!("TRIM стало: {} мс, паузы {:?} мс", after.len() * 1000 / rate as usize, gaps(&after));
    }

    #[test]
    fn chunking_independent_and_silent_piece_dropped() {
        let parts = [silence(90), tone(400), silence(520), tone(300), silence(515)];
        assert_eq!(run(&parts, 100).len(), run(&parts, 4096).len(), "результат не зависит от размера кусков потока");
        assert!(run(&[silence(300)], 512).is_empty(), "кусок из одной тишины не играется");
    }
}
