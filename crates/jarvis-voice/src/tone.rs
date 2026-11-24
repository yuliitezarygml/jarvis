//! Тон голоса: громкость и темп фразы относительно *обычного* голоса пользователя.
//!
//! - `VoiceFeatures` — замеры одной фразы (собирает `Vad`, только по звучащим кадрам).
//! - `speech_rate` — слоги (гласные) распознанного текста ÷ длительность речи.
//! - `VoiceProfile` — «обычный» голос: скользящее среднее по спокойным фразам, отдельно для каждого
//!   микрофона, сброс при смене усиления, JSON на диске (`data/voice-profile.json`).
//! - `VoiceTone` — вывод: громче на N дБ / кричит / быстро / медленно и служебная пометка для модели.
//!
//! **Почему громкость — по p90, а не по среднему.** Среднее по «звучащим» кадрам тянут вниз тихие
//! согласные, затухающие окончания слов и паузы, которые VAD ещё держит внутри фразы; их доля зависит от
//! темпа и длины фразы, а не от того, насколько человек повысил голос. Пик — один кадр/сэмпл (щелчок,
//! «п», удар по столу) и при перегрузе упирается в 0 dBFS. 90-й перцентиль уровней кадров — это ударные
//! гласные, «громкая часть» фразы: он устойчив к паузам и к одиночным выбросам, а при крике растёт
//! сильнее всего. Среднее (по энергии) и пик пишутся в журнал для отладки.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Громче обычного на столько дБ (по p90) — «громко».
pub const LOUD_DB: f32 = 8.0;
/// Громче обычного на столько дБ — «кричит».
pub const SHOUT_DB: f32 = 12.0;
/// Доля перегруженных сэмплов, при которой фраза считается криком (перегруз «срезает» p90, и дБ
/// недооценивают громкость), если она при этом хотя бы на `CLIP_MIN_DB` громче обычного.
pub const CLIP_SHOUT_RATIO: f32 = 0.01;
pub const CLIP_MIN_DB: f32 = 4.0;
/// Темп ≥ обычного × это — «быстро».
pub const FAST_RATIO: f32 = 1.3;
/// Темп ≤ обычного × это — «медленно».
pub const SLOW_RATIO: f32 = 0.75;
/// Фразы короче (по речевым кадрам) не оцениваются и не учат профиль.
pub const MIN_SPEECH_MS: u32 = 600;
/// Меньше слогов — темп не считается, и фраза не учит профиль.
pub const MIN_SYLLABLES: u32 = 3;
/// Сколько спокойных фраз нужно, прежде чем делать выводы («знакомство»).
pub const WARMUP_UTTERANCES: u32 = 15;
/// Вес новой фразы в скользящем среднем после знакомства (во время знакомства — обычное среднее).
pub const EMA_ALPHA: f32 = 0.1;
/// Сохранять профиль на диск не чаще.
pub const SAVE_INTERVAL: Duration = Duration::from_secs(30);
/// Во время знакомства фраза громче предварительного среднего на `SHOUT_DB` не учит профиль
/// (после стольких фраз среднее уже чего-то стоит).
const WARMUP_GUARD_FROM: u32 = 5;
/// Усиление микрофона «то же самое», если отличается меньше.
const GAIN_EPS: f32 = 0.01;
/// Уровень «ничего не было» (нет звучащих кадров).
pub const SILENCE_DBFS: f32 = -150.0;

/// Замеры одной фразы (только звучащие кадры, кроме `noise_floor_dbfs` и `duration_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VoiceFeatures {
    /// Средняя громкость по энергии (среднее мощности → дБ), dBFS.
    pub mean_dbfs: f32,
    /// 90-й перцентиль уровней кадров — «громкая часть» фразы, dBFS.
    pub p90_dbfs: f32,
    /// Пиковый сэмпл, dBFS.
    pub peak_dbfs: f32,
    /// Фон помещения на конец фразы, dBFS.
    pub noise_floor_dbfs: f32,
    /// Длительность речевых кадров.
    pub speech_ms: u32,
    /// Длина всей фразы (pre-roll + речь + хвост тишины).
    pub duration_ms: u32,
    /// Доля перегруженных сэмплов (|s| ≥ `CLIP_LEVEL`) среди звучащих.
    pub clipped_ratio: f32,
}

impl Default for VoiceFeatures {
    fn default() -> Self {
        Self {
            mean_dbfs: SILENCE_DBFS,
            p90_dbfs: SILENCE_DBFS,
            peak_dbfs: SILENCE_DBFS,
            noise_floor_dbfs: SILENCE_DBFS,
            speech_ms: 0,
            duration_ms: 0,
            clipped_ratio: 0.0,
        }
    }
}

/// Сэмпл считается перегруженным от этого модуля (≈ −0,2 dBFS).
pub const CLIP_LEVEL: u16 = 32_000;

/// Накопитель замеров по звучащим кадрам (используется `Vad`).
#[derive(Debug, Clone, Default)]
pub struct FeatureAccumulator {
    levels: Vec<f32>,
    power_sum: f64,
    peak: u16,
    clipped: u64,
    samples: u64,
}

impl FeatureAccumulator {
    /// Добавить звучащий кадр (`level` — его dBFS, уже посчитанный).
    pub fn push(&mut self, frame: &[i16], level: f32) {
        let mut sum = 0.0f64;
        for &s in frame {
            let a = s.unsigned_abs();
            self.peak = self.peak.max(a);
            if a >= CLIP_LEVEL {
                self.clipped += 1;
            }
            sum += (s as f64 / 32768.0).powi(2);
        }
        if !frame.is_empty() {
            self.power_sum += sum / frame.len() as f64;
        }
        self.samples += frame.len() as u64;
        self.levels.push(if level.is_finite() { level } else { SILENCE_DBFS });
    }

    /// Перенести кадры другого накопителя (кадры, подтвердившие начало фразы).
    pub fn append(&mut self, other: &mut FeatureAccumulator) {
        self.levels.append(&mut other.levels);
        self.power_sum += other.power_sum;
        self.peak = self.peak.max(other.peak);
        self.clipped += other.clipped;
        self.samples += other.samples;
        other.clear();
    }

    pub fn frames(&self) -> usize {
        self.levels.len()
    }

    pub fn clear(&mut self) {
        self.levels.clear();
        self.power_sum = 0.0;
        self.peak = 0;
        self.clipped = 0;
        self.samples = 0;
    }

    /// Итог (накопитель очищается).
    pub fn finish(&mut self, noise_floor_dbfs: f32, speech_ms: u32, duration_ms: u32) -> VoiceFeatures {
        let n = self.levels.len();
        let mut f = VoiceFeatures { noise_floor_dbfs, speech_ms, duration_ms, ..Default::default() };
        if n > 0 {
            f.mean_dbfs = to_db(self.power_sum / n as f64);
            self.levels.sort_by(f32::total_cmp);
            f.p90_dbfs = self.levels[percentile_index(n, 90)];
            f.peak_dbfs = if self.peak == 0 { SILENCE_DBFS } else { 20.0 * (self.peak as f32 / 32768.0).log10() };
            f.clipped_ratio = if self.samples == 0 { 0.0 } else { (self.clipped as f64 / self.samples as f64) as f32 };
        }
        self.clear();
        f
    }
}

fn to_db(power: f64) -> f32 {
    if power <= 0.0 { SILENCE_DBFS } else { (10.0 * power.log10()).max(SILENCE_DBFS as f64) as f32 }
}

/// Индекс перцентиля `pct` % (метод «ближайший ранг») в отсортированном массиве длины `n ≥ 1`.
fn percentile_index(n: usize, pct: usize) -> usize {
    (n * pct).div_ceil(100).clamp(1, n) - 1
}

/// Слоги = гласные (русские и латинские). Цифры и знаки не считаются.
pub fn count_syllables(text: &str) -> u32 {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| "аеёиоуыэюяaeiouy".contains(*c))
        .count() as u32
}

/// Темп, слогов в секунду: гласные текста ÷ длительность речи. `None` для фраз короче `MIN_SPEECH_MS`.
pub fn speech_rate(text: &str, speech_ms: u32) -> Option<f32> {
    (speech_ms >= MIN_SPEECH_MS).then(|| count_syllables(text) as f32 / (speech_ms as f32 / 1000.0))
}

/// Вывод о тоне одной фразы.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct VoiceTone {
    /// На сколько дБ (по p90) громче обычного (минус — тише). 0, если профиль ещё не готов.
    pub loudness_db_vs_normal: f32,
    /// ≥ `LOUD_DB` (и всегда при `shouting`).
    pub loud: bool,
    /// ≥ `SHOUT_DB` или перегруз микрофона при заметно повышенном голосе.
    pub shouting: bool,
    /// Темп фразы, слогов/с (`None` — фраза короткая или мало слогов).
    pub syllables_per_sec: Option<f32>,
    /// Темп ÷ обычный темп.
    pub rate_vs_normal: Option<f32>,
    /// Темп ≥ обычного × `FAST_RATIO`.
    pub fast: bool,
    /// Темп ≤ обычного × `SLOW_RATIO`.
    pub slow: bool,
    /// Профиль этого микрофона готов (≥ `WARMUP_UTTERANCES` спокойных фраз). Иначе флаги всегда `false`.
    pub calibrated: bool,
    /// Сколько спокойных фраз этого микрофона в профиле.
    pub baseline_samples: u32,
}

impl VoiceTone {
    /// Служебная пометка для модели: «голос: громче обычного на 11 дБ, быстро», «голос: кричит (+14 дБ)».
    /// `None` — ничего заметного или профиль ещё не готов.
    pub fn note_ru(&self) -> Option<String> {
        if !self.calibrated {
            return None;
        }
        let db = self.loudness_db_vs_normal.round();
        let mut parts = Vec::new();
        if self.shouting {
            parts.push(format!("кричит ({db:+.0} дБ)"));
        } else if self.loud {
            parts.push(format!("громче обычного на {db:.0} дБ"));
        }
        if self.fast {
            parts.push("быстро".to_string());
        } else if self.slow {
            parts.push("медленно".to_string());
        }
        (!parts.is_empty()).then(|| format!("голос: {}", parts.join(", ")))
    }

    /// Для журнала: «громче на 11 дБ, темп ×1.4» / «знакомлюсь 7/15».
    pub fn summary_ru(&self) -> String {
        if !self.calibrated {
            return format!("знакомлюсь {}/{}", self.baseline_samples, WARMUP_UTTERANCES);
        }
        let db = self.loudness_db_vs_normal.round();
        let mut s = if db >= 0.0 { format!("громче на {db:.0} дБ") } else { format!("тише на {:.0} дБ", -db) };
        if let Some(r) = self.rate_vs_normal {
            s.push_str(&format!(", темп ×{r:.2}"));
        }
        if let Some(n) = self.note_ru() {
            s.push_str(&format!(" → [{n}]"));
        }
        s
    }
}

/// «Обычный» голос на одном микрофоне.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceBaseline {
    /// Усиление микрофона, при котором собран профиль (`None` — неизвестно).
    pub gain: Option<f32>,
    /// Сколько спокойных фраз учтено.
    pub samples: u32,
    pub p90_dbfs: f32,
    pub mean_dbfs: f32,
    pub syllables_per_sec: f32,
}

impl DeviceBaseline {
    fn new(gain: Option<f32>) -> Self {
        Self { gain, samples: 0, p90_dbfs: 0.0, mean_dbfs: 0.0, syllables_per_sec: 0.0 }
    }

    pub fn calibrated(&self) -> bool {
        self.samples >= WARMUP_UTTERANCES
    }

    fn same_gain(&self, gain: Option<f32>) -> bool {
        match (self.gain, gain) {
            (None, None) => true,
            (Some(a), Some(b)) => (a - b).abs() < GAIN_EPS,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct ProfileFile {
    #[serde(default = "version")]
    version: u32,
    #[serde(default)]
    devices: BTreeMap<String, DeviceBaseline>,
}

fn version() -> u32 {
    1
}

/// Профиль обычного голоса (по микрофонам) с сохранением в JSON.
#[derive(Debug)]
pub struct VoiceProfile {
    path: Option<PathBuf>,
    data: ProfileFile,
    dirty: bool,
    last_save: Option<Instant>,
    save_interval: Duration,
}

impl Default for VoiceProfile {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl VoiceProfile {
    /// Профиль без файла (тесты, временные конвейеры).
    pub fn in_memory() -> Self {
        Self { path: None, data: ProfileFile { version: 1, devices: BTreeMap::new() }, dirty: false, last_save: None, save_interval: SAVE_INTERVAL }
    }

    /// Прочитать профиль; файла нет или он испорчен — пустой профиль (испорченный перезапишется).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str::<ProfileFile>(&s).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "профиль голоса испорчен — начинаю заново");
                ProfileFile { version: 1, ..Default::default() }
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProfileFile { version: 1, ..Default::default() },
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "профиль голоса не прочитан");
                ProfileFile { version: 1, ..Default::default() }
            }
        };
        Self { path: Some(path), data, ..Self::in_memory() }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Как часто `save()` реально пишет на диск.
    pub fn with_save_interval(mut self, every: Duration) -> Self {
        self.save_interval = every;
        self
    }

    pub fn baseline(&self, device: &str) -> Option<&DeviceBaseline> {
        self.data.devices.get(device)
    }

    /// Есть несохранённые изменения.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Сохранить, если есть изменения и с прошлой записи прошло `save_interval` (можно звать часто).
    /// `Ok(true)` — записано.
    pub fn save(&mut self) -> std::io::Result<bool> {
        if !self.dirty || self.path.is_none() || self.last_save.is_some_and(|t| t.elapsed() < self.save_interval) {
            return Ok(false);
        }
        self.save_now().map(|()| true)
    }

    /// Сохранить сейчас (атомарно: временный файл → переименование).
    pub fn save_now(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(&self.data).map_err(std::io::Error::other)?;
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        self.dirty = false;
        self.last_save = Some(Instant::now());
        Ok(())
    }

    /// Оценить фразу относительно обычного голоса на `device` (усиление `gain`). Профиль не меняется.
    pub fn judge(&self, device: &str, gain: Option<f32>, f: &VoiceFeatures, text: &str) -> VoiceTone {
        let syllables = count_syllables(text);
        let rate = speech_rate(text, f.speech_ms).filter(|_| syllables >= MIN_SYLLABLES);
        let base = self.data.devices.get(device).filter(|b| b.same_gain(gain));
        let mut tone = VoiceTone { syllables_per_sec: rate, baseline_samples: base.map_or(0, |b| b.samples), ..Default::default() };
        let Some(base) = base.filter(|b| b.calibrated()) else { return tone };
        tone.calibrated = true;
        tone.loudness_db_vs_normal = f.p90_dbfs - base.p90_dbfs;
        tone.rate_vs_normal = rate.filter(|_| base.syllables_per_sec > 0.0).map(|r| r / base.syllables_per_sec);
        if f.speech_ms < MIN_SPEECH_MS {
            return tone; // короткие фразы не оцениваются (дБ — только для журнала)
        }
        let db = tone.loudness_db_vs_normal;
        tone.shouting = db >= SHOUT_DB || (f.clipped_ratio >= CLIP_SHOUT_RATIO && db >= CLIP_MIN_DB);
        tone.loud = tone.shouting || db >= LOUD_DB;
        if let Some(r) = tone.rate_vs_normal {
            tone.fast = r >= FAST_RATIO;
            tone.slow = r <= SLOW_RATIO;
        }
        tone
    }

    /// Учесть фразу в обычном голосе, если она «спокойная»: ≥ `MIN_SPEECH_MS`, ≥ `MIN_SYLLABLES`,
    /// не громкая и не перегруженная. Смена усиления сбрасывает профиль микрофона. `true` — учтена.
    pub fn observe(&mut self, device: &str, gain: Option<f32>, f: &VoiceFeatures, text: &str) -> bool {
        let base = self.data.devices.entry(device.to_string()).or_insert_with(|| DeviceBaseline::new(gain));
        if !base.same_gain(gain) {
            tracing::info!(device, old = ?base.gain, new = ?gain, "профиль голоса: усиление микрофона изменилось — знакомлюсь заново");
            *base = DeviceBaseline::new(gain);
            self.dirty = true;
        }
        let syllables = count_syllables(text);
        if f.speech_ms < MIN_SPEECH_MS || syllables < MIN_SYLLABLES || f.p90_dbfs <= SILENCE_DBFS {
            return false;
        }
        let delta = f.p90_dbfs - base.p90_dbfs;
        let too_loud = if base.calibrated() { delta >= LOUD_DB } else { base.samples >= WARMUP_GUARD_FROM && delta >= SHOUT_DB };
        if too_loud || f.clipped_ratio >= CLIP_SHOUT_RATIO {
            return false;
        }
        let rate = syllables as f32 / (f.speech_ms as f32 / 1000.0);
        base.samples += 1;
        let a = if base.calibrated() && base.samples > WARMUP_UTTERANCES { EMA_ALPHA } else { 1.0 / base.samples as f32 };
        base.p90_dbfs += a * (f.p90_dbfs - base.p90_dbfs);
        base.mean_dbfs += a * (f.mean_dbfs - base.mean_dbfs);
        base.syllables_per_sec += a * (rate - base.syllables_per_sec);
        self.dirty = true;
        true
    }

    /// `judge`, затем `observe` (оценка — по профилю *до* этой фразы).
    pub fn assess(&mut self, device: &str, gain: Option<f32>, f: &VoiceFeatures, text: &str) -> VoiceTone {
        let tone = self.judge(device, gain, f, text);
        self.observe(device, gain, f, text);
        tone
    }
}

impl Drop for VoiceProfile {
    fn drop(&mut self) {
        if self.dirty && self.path.is_some()
            && let Err(e) = self.save_now()
        {
            tracing::warn!(error = %e, "профиль голоса не сохранён");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(p90: f32, speech_ms: u32) -> VoiceFeatures {
        VoiceFeatures { mean_dbfs: p90 - 3.0, p90_dbfs: p90, peak_dbfs: p90 + 6.0, noise_floor_dbfs: -65.0, speech_ms, duration_ms: speech_ms + 400, clipped_ratio: 0.0 }
    }

    /// 10 слогов: «открой мне пожалуйста браузер» — о-о-е-о-а-у-а-а-у-е.
    const CALM: &str = "открой мне пожалуйста браузер";

    fn warmed(device: &str, gain: Option<f32>) -> VoiceProfile {
        let mut p = VoiceProfile::in_memory();
        for i in 0..WARMUP_UTTERANCES {
            // −30 dBFS ± 1, 2000 мс речи на 10 слогов = 5 слогов/с.
            let jitter = if i % 2 == 0 { 1.0 } else { -1.0 };
            assert!(p.observe(device, gain, &feat(-30.0 + jitter, 2000), CALM), "фраза {i}");
        }
        p
    }

    #[test]
    fn syllables_and_rate() {
        assert_eq!(count_syllables(CALM), 10);
        // Латиница — грубо (все гласные буквы): дж-а-рв-и-с + o-e + Y-o-u-u-e.
        assert_eq!(count_syllables("Джарвис, open YouTube"), 2 + 2 + 5);
        assert_eq!(count_syllables("ЁЖ 123 + 7"), 1);
        assert_eq!(speech_rate(CALM, 2000), Some(5.0));
        assert_eq!(speech_rate(CALM, 599), None, "короткие фразы не считаются");
        assert!((speech_rate("да да да", 600).unwrap() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn warmup_gives_no_judgment() {
        let mut p = VoiceProfile::in_memory();
        for i in 0..WARMUP_UTTERANCES {
            let t = p.assess("mic", None, &feat(-30.0, 2000), CALM);
            assert!(!t.calibrated, "фраза {i}: ещё знакомство");
            assert_eq!(t.baseline_samples, i);
            assert_eq!(t.note_ru(), None);
        }
        // Громкая фраза сразу после знакомства — уже вывод.
        let t = p.judge("mic", None, &feat(-19.0, 2000), CALM);
        assert!(t.calibrated && t.loud && !t.shouting, "{t:?}");
        assert!((t.loudness_db_vs_normal - 11.0).abs() < 0.01);
        assert!(t.summary_ru().contains("громче на 11 дБ"), "{}", t.summary_ru());
    }

    #[test]
    fn loudness_thresholds_vs_baseline() {
        let p = warmed("mic", None);
        let b = p.baseline("mic").unwrap();
        assert!((b.p90_dbfs - -30.0).abs() < 0.1, "{b:?}");
        assert!((b.syllables_per_sec - 5.0).abs() < 0.01);
        let j = |db: f32| p.judge("mic", None, &feat(-30.0 + db, 2000), CALM);
        let t = j(3.0);
        assert!(t.calibrated && !t.loud && !t.shouting && t.note_ru().is_none(), "{t:?}");
        let t = j(8.5);
        assert!(t.loud && !t.shouting, "{t:?}");
        let t = j(14.0);
        assert!(t.loud && t.shouting, "{t:?}");
        // Перегруз микрофона: дБ «срезаны», но это крик.
        let t = p.judge("mic", None, &VoiceFeatures { clipped_ratio: 0.05, ..feat(-25.0, 2000) }, CALM);
        assert!(t.shouting, "{t:?}");
        let t = p.judge("mic", None, &VoiceFeatures { clipped_ratio: 0.05, ..feat(-29.0, 2000) }, CALM);
        assert!(!t.shouting, "перегруз при обычной громкости — не крик: {t:?}");
        // Короткая фраза не оценивается.
        let t = p.judge("mic", None, &feat(-10.0, 400), "стоп");
        assert!(t.calibrated && !t.loud && !t.shouting && !t.fast, "{t:?}");
    }

    #[test]
    fn rate_fast_and_slow() {
        let p = warmed("mic", None);
        // 10 слогов за 1,4 с ≈ 7,1 слога/с = ×1,43.
        let t = p.judge("mic", None, &feat(-30.0, 1400), CALM);
        assert!(t.fast && !t.slow, "{t:?}");
        assert!((t.rate_vs_normal.unwrap() - 1.428).abs() < 0.01);
        let t = p.judge("mic", None, &feat(-30.0, 3000), CALM);
        assert!(t.slow && !t.fast, "{t:?}");
        let t = p.judge("mic", None, &feat(-30.0, 2100), CALM);
        assert!(!t.fast && !t.slow);
        // Мало слогов — темп не считается.
        let t = p.judge("mic", None, &feat(-30.0, 700), "да да");
        assert_eq!(t.syllables_per_sec, None);
        assert!(!t.fast);
    }

    #[test]
    fn loud_utterances_do_not_teach_baseline() {
        let mut p = warmed("mic", None);
        let before = p.baseline("mic").unwrap().clone();
        assert!(!p.observe("mic", None, &feat(-20.0, 2000), CALM), "громкая не учит");
        assert!(!p.observe("mic", None, &VoiceFeatures { clipped_ratio: 0.2, ..feat(-29.0, 2000) }, CALM), "перегруз не учит");
        assert!(!p.observe("mic", None, &feat(-30.0, 400), CALM), "короткая не учит");
        assert!(!p.observe("mic", None, &feat(-30.0, 2000), "да"), "мало слогов не учит");
        assert_eq!(p.baseline("mic").unwrap(), &before);
        // Спокойная — учит понемногу (EMA).
        assert!(p.observe("mic", None, &feat(-26.0, 2000), CALM));
        let b = p.baseline("mic").unwrap();
        assert!((b.p90_dbfs - (before.p90_dbfs + EMA_ALPHA * (-26.0 - before.p90_dbfs))).abs() < 1e-3, "{b:?}");
        // Во время знакомства явный крик тоже не учит.
        let mut p = VoiceProfile::in_memory();
        for _ in 0..WARMUP_GUARD_FROM {
            p.observe("mic", None, &feat(-30.0, 2000), CALM);
        }
        assert!(!p.observe("mic", None, &feat(-15.0, 2000), CALM));
        assert!(p.observe("mic", None, &feat(-25.0, 2000), CALM));
    }

    #[test]
    fn devices_are_separate_and_gain_change_resets() {
        let mut p = warmed("Микрофон (USB)", Some(1.0));
        let t = p.judge("Гарнитура", Some(1.0), &feat(-10.0, 2000), CALM);
        assert!(!t.calibrated, "у другого микрофона свой профиль: {t:?}");
        assert!(p.judge("Микрофон (USB)", Some(1.0), &feat(-19.0, 2000), CALM).loud);
        // Усиление изменилось — профиль не годится и начинается заново.
        let t = p.judge("Микрофон (USB)", Some(0.5), &feat(-19.0, 2000), CALM);
        assert!(!t.calibrated, "{t:?}");
        assert!(p.observe("Микрофон (USB)", Some(0.5), &feat(-36.0, 2000), CALM));
        let b = p.baseline("Микрофон (USB)").unwrap();
        assert_eq!((b.samples, b.gain), (1, Some(0.5)));
        assert!((b.p90_dbfs - -36.0).abs() < 1e-4);
        // Почти то же усиление — не сброс.
        assert!(p.observe("Микрофон (USB)", Some(0.505), &feat(-36.0, 2000), CALM));
        assert_eq!(p.baseline("Микрофон (USB)").unwrap().samples, 2);
    }

    #[test]
    fn note_text() {
        let base = VoiceTone { calibrated: true, baseline_samples: 20, ..Default::default() };
        assert_eq!(base.note_ru(), None, "ничего заметного");
        let t = VoiceTone { loudness_db_vs_normal: 10.6, loud: true, fast: true, ..base };
        assert_eq!(t.note_ru().as_deref(), Some("голос: громче обычного на 11 дБ, быстро"));
        let t = VoiceTone { loudness_db_vs_normal: 14.2, loud: true, shouting: true, ..base };
        assert_eq!(t.note_ru().as_deref(), Some("голос: кричит (+14 дБ)"));
        let t = VoiceTone { slow: true, ..base };
        assert_eq!(t.note_ru().as_deref(), Some("голос: медленно"));
        let t = VoiceTone { fast: true, ..base };
        assert_eq!(t.note_ru().as_deref(), Some("голос: быстро"));
        let t = VoiceTone { calibrated: false, loud: true, loudness_db_vs_normal: 11.0, ..base };
        assert_eq!(t.note_ru(), None, "не откалиброван");
        assert_eq!(VoiceTone { baseline_samples: 7, ..Default::default() }.summary_ru(), "знакомлюсь 7/15");
        let t = VoiceTone { loudness_db_vs_normal: -3.4, ..base };
        assert_eq!(t.summary_ru(), "тише на 3 дБ");
    }

    #[test]
    fn profile_save_load_roundtrip_and_throttle() {
        let dir = std::env::temp_dir().join(format!("jarvis-voice-profile-{}", uuid::Uuid::new_v4()));
        let path = dir.join("data").join("voice-profile.json");
        let mut p = VoiceProfile::load(&path);
        assert!(p.baseline("mic").is_none(), "файла нет — пустой профиль");
        assert!(!p.save().unwrap(), "нечего сохранять");
        for _ in 0..WARMUP_UTTERANCES {
            p.observe("mic", Some(0.8), &feat(-30.0, 2000), CALM);
        }
        assert!(p.save().unwrap(), "первое сохранение — сразу");
        assert!(path.exists());
        assert!(!dir.join("data").join("voice-profile.json.tmp").exists(), "временный файл переименован");
        p.observe("mic", Some(0.8), &feat(-31.0, 2000), CALM);
        assert!(!p.save().unwrap(), "второе — не раньше интервала");
        assert!(p.is_dirty());
        let saved = p.baseline("mic").unwrap().clone();
        drop(p); // сохраняет несохранённое
        let p = VoiceProfile::load(&path);
        assert_eq!(p.baseline("mic"), Some(&saved));
        assert!(p.judge("mic", Some(0.8), &feat(-20.0, 2000), CALM).loud);
        // Интервал 0 — пишет каждый раз; испорченный файл — пустой профиль.
        let mut p = VoiceProfile::load(&path).with_save_interval(Duration::ZERO);
        p.observe("mic2", None, &feat(-30.0, 2000), CALM);
        assert!(p.save().unwrap());
        p.observe("mic2", None, &feat(-30.0, 2000), CALM);
        assert!(p.save().unwrap());
        drop(p);
        std::fs::write(&path, "{не json").unwrap();
        assert!(VoiceProfile::load(&path).baseline("mic").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accumulator_percentile_and_energy_mean() {
        let mut acc = FeatureAccumulator::default();
        // 9 кадров тише, 1 громкий: p90 (ближайший ранг) — 9-й по порядку, т.е. ещё тихий.
        for _ in 0..9 {
            acc.push(&[1000; 256], -30.3);
        }
        acc.push(&[10_000; 256], -10.3);
        let f = acc.finish(-60.0, 160, 400);
        assert!((f.p90_dbfs - -30.3).abs() < 1e-4, "{f:?}");
        // Среднее по энергии тянется к громкому кадру сильнее, чем среднее дБ.
        let expect = 10.0 * ((9.0 * (1000.0f64 / 32768.0).powi(2) + (10_000.0f64 / 32768.0).powi(2)) / 10.0).log10();
        assert!((f.mean_dbfs as f64 - expect).abs() < 1e-3, "{f:?}");
        assert!((f.peak_dbfs - 20.0 * (10_000.0f32 / 32768.0).log10()).abs() < 1e-3);
        assert_eq!(f.clipped_ratio, 0.0);
        assert_eq!(acc.frames(), 0, "очищен");
        assert_eq!(acc.finish(-60.0, 0, 0).p90_dbfs, SILENCE_DBFS, "пусто — тишина");
        assert_eq!(percentile_index(1, 90), 0);
        assert_eq!(percentile_index(20, 90), 17);
    }
}
