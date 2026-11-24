//! `EchoGuard`: аппаратного AEC на Windows нет, поэтому
//! 1) полу-дуплекс — пока играет TTS и ещё `tail_ms` после, микрофон для STT «глушится»;
//! 2) фильтр по словам — транскрипт, ≥70 % слов которого совпали с недавно сказанным TTS, считается эхом.
//!
//! Регистр и ё/е не важны. Слова сравниваются «нечётко»: распознаватель коверкает речь из колонок
//! («Готов, говорите» → «готосе говорите», «…, сэр» → «… с»), поэтому похожие слова (правка ≤ 1/3 длины)
//! и обрубок последнего слова тоже считаются совпадением.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::wake::words;

/// Состояние воспроизведения (его даёт `Player`).
pub trait PlaybackMonitor: Send + Sync {
    fn is_playing(&self) -> bool;
    /// Момент, когда звук последний раз закончился (или был остановлен).
    fn last_played_at(&self) -> Option<Instant>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct EchoGuardConfig {
    /// Глушить STT, пока играет TTS (рекомендуется без наушников).
    pub half_duplex: bool,
    /// Хвост после окончания звука (реверберация, задержка вывода).
    pub tail_ms: u64,
    /// Доля совпавших слов, начиная с которой транскрипт — эхо.
    pub overlap_threshold: f32,
    /// Сколько помнить сказанное TTS после окончания звука.
    pub memory_ms: u64,
    /// Фильтр слов применяется к репликам, начавшимся во время звука или не позже этого после него.
    pub echo_window_ms: u64,
}

impl Default for EchoGuardConfig {
    fn default() -> Self {
        // Хвост 600 мс: у колонок ноутбука вывод запаздывает, а распознаватель ставит начало фразы раньше звука.
        Self { half_duplex: true, tail_ms: 600, overlap_threshold: 0.7, memory_ms: 15_000, echo_window_ms: 3_000 }
    }
}

struct Spoken {
    at: Instant,
    words: Vec<String>,
}

pub struct EchoGuard {
    cfg: EchoGuardConfig,
    monitor: Arc<dyn PlaybackMonitor>,
    recent: Mutex<VecDeque<Spoken>>,
}

impl EchoGuard {
    pub fn new(cfg: EchoGuardConfig, monitor: Arc<dyn PlaybackMonitor>) -> Self {
        Self { cfg, monitor, recent: Mutex::new(VecDeque::new()) }
    }

    pub fn config(&self) -> &EchoGuardConfig {
        &self.cfg
    }

    /// Запомнить текст, отправленный в TTS.
    pub fn note_spoken(&self, text: &str) {
        self.note_spoken_at(text, Instant::now());
    }

    pub fn note_spoken_at(&self, text: &str, at: Instant) {
        let w = words(text);
        if w.is_empty() {
            return;
        }
        let mut r = self.recent.lock().unwrap();
        r.push_back(Spoken { at, words: w });
        while r.len() > 64 {
            r.pop_front();
        }
    }

    pub fn clear(&self) {
        self.recent.lock().unwrap().clear();
    }

    /// Звук играет или ещё не прошёл хвост.
    pub fn in_echo_zone(&self, at: Instant) -> bool {
        if self.monitor.is_playing() {
            return true;
        }
        self.monitor.last_played_at().is_some_and(|end| at < end + Duration::from_millis(self.cfg.tail_ms))
    }

    /// Глушить ли сейчас микрофон для STT (полу-дуплекс).
    pub fn mic_muted(&self) -> bool {
        self.cfg.half_duplex && self.in_echo_zone(Instant::now())
    }

    fn recent_words(&self, now: Instant) -> Vec<String> {
        let end = self.monitor.last_played_at();
        let playing = self.monitor.is_playing();
        let mem = Duration::from_millis(self.cfg.memory_ms);
        let mut r = self.recent.lock().unwrap();
        // Память отсчитывается от конца звука: фраза могла долго ждать очереди.
        r.retain(|s| playing || now < s.at.max(end.unwrap_or(s.at)) + mem);
        let set: HashSet<String> = r.iter().flat_map(|s| s.words.iter().cloned()).collect();
        set.into_iter().collect()
    }

    /// Было ли это слово (нечётко) в недавней речи. `last` — последнее слово транскрипта (может быть обрубком).
    fn heard(recent: &[String], w: &str, last: bool) -> bool {
        recent.iter().any(|r| similar_word(w, r, last))
    }

    /// Доля слов транскрипта, встречавшихся в недавней речи TTS (0..=1).
    pub fn overlap(&self, transcript: &str) -> f32 {
        let w = words(transcript);
        if w.is_empty() {
            return 0.0;
        }
        let recent = self.recent_words(Instant::now());
        let n = w.len();
        let hit = w.iter().enumerate().filter(|(i, x)| Self::heard(&recent, x, *i + 1 == n)).count();
        hit as f32 / n as f32
    }

    /// Свои слова пользователя: транскрипт без кусков недавней речи Джарвиса (подряд ≥ 2 совпавших слов
    /// выбрасываются; одиночные совпадения — обычные слова — остаются). Нужен, когда человек отвечает
    /// поверх голоса Джарвиса и микрофон слышит обоих.
    pub fn own_words(&self, transcript: &str) -> Vec<String> {
        let w = words(transcript);
        let recent = self.recent_words(Instant::now());
        let n = w.len();
        let hit: Vec<bool> = w.iter().enumerate().map(|(i, x)| Self::heard(&recent, x, i + 1 == n)).collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < n {
            if hit[i] {
                let mut j = i;
                while j < n && hit[j] {
                    j += 1;
                }
                if j - i < 2 {
                    out.extend(w[i..j].iter().cloned());
                }
                i = j;
            } else {
                out.push(w[i].clone());
                i += 1;
            }
        }
        out
    }

    /// Звук играет или закончился меньше `echo_window_ms` назад относительно `at`.
    pub fn near_playback(&self, at: Instant) -> bool {
        let window = Duration::from_millis(self.cfg.echo_window_ms.max(self.cfg.tail_ms));
        self.monitor.is_playing() || self.monitor.last_played_at().is_some_and(|end| at < end + window)
    }

    /// Эхо ли это. `started_at` — начало реплики (из VAD): эхо возможно только во время звука и сразу после.
    pub fn is_echo(&self, transcript: &str, started_at: Instant) -> bool {
        self.near_playback(started_at) && self.overlap(transcript) >= self.cfg.overlap_threshold
    }

    /// «Да, сэр» принимается только если реплика началась после окончания звука и хвоста (защита от самоподтверждения).
    pub fn is_clean_confirmation(&self, started_at: Instant) -> bool {
        !self.monitor.is_playing() && !self.in_echo_zone(started_at)
    }
}

/// Похожие слова: одинаковые; отличаются правкой ≤ 1/3 длины (от 4 букв); `last` — обрубок 1–2 буквы
/// в конце фразы, с которого начинается слово речи («с» ← «сэр»).
pub fn similar_word(a: &str, b: &str, last: bool) -> bool {
    if a == b {
        return true;
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    if last && la <= 2 && lb > la && b.starts_with(a) {
        return true;
    }
    if la.min(lb) < 4 {
        return false;
    }
    let budget = (la.max(lb) / 3).max(1);
    la.abs_diff(lb) <= budget && crate::wake::edit_distance(a, b) <= budget
}

/// Монитор для тестов и для конфигураций без звука.
#[derive(Default)]
pub struct ManualMonitor {
    state: Mutex<(bool, Option<Instant>)>,
}

impl ManualMonitor {
    pub fn set_playing(&self, playing: bool) {
        let mut s = self.state.lock().unwrap();
        if s.0 && !playing {
            s.1 = Some(Instant::now());
        }
        s.0 = playing;
    }
    pub fn set_ended_at(&self, at: Instant) {
        *self.state.lock().unwrap() = (false, Some(at));
    }
}

impl PlaybackMonitor for ManualMonitor {
    fn is_playing(&self) -> bool {
        self.state.lock().unwrap().0
    }
    fn last_played_at(&self) -> Option<Instant> {
        self.state.lock().unwrap().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> (EchoGuard, Arc<ManualMonitor>) {
        let m = Arc::new(ManualMonitor::default());
        (EchoGuard::new(EchoGuardConfig::default(), m.clone()), m)
    }

    #[test]
    fn half_duplex_with_tail() {
        let (g, m) = guard();
        assert!(!g.mic_muted());
        m.set_playing(true);
        assert!(g.mic_muted());
        m.set_ended_at(Instant::now());
        assert!(g.mic_muted(), "хвост 600 мс");
        m.set_ended_at(Instant::now() - Duration::from_millis(700));
        assert!(!g.mic_muted());
        let off = EchoGuard::new(EchoGuardConfig { half_duplex: false, ..Default::default() }, m.clone());
        m.set_playing(true);
        assert!(!off.mic_muted());
    }

    #[test]
    fn word_overlap_filter() {
        let (g, m) = guard();
        g.note_spoken("Сэр, сейчас открою ваш браузер. Ещё что-нибудь?");
        m.set_playing(true);
        let now = Instant::now();
        assert!(g.is_echo("открою ваш браузер", now));
        assert!(g.is_echo("СЭР СЕЙЧАС ОТКРОЮ", now), "регистр не важен");
        assert!(!g.is_echo("Джарвис, стоп", now), "барж-ин проходит");
        // 2 из 3 слов совпали = 67 % < 70 %.
        assert!(!g.is_echo("открою ваш календарь", now));
        // Нечёткое сравнение: окончание, исковерканное распознавателем, — то же слово.
        assert_eq!(g.overlap("браузеры"), 1.0);
        assert_eq!(g.overlap("брат"), 0.0, "короткие слова — только точно");
        // ё = е
        g.note_spoken("Всё готово");
        assert_eq!(g.overlap("все готово"), 1.0);
    }

    #[test]
    fn echo_only_near_playback_and_memory_expires() {
        let (g, m) = guard();
        g.note_spoken("открыть браузер");
        // Реплика началась через 5 с после конца звука — это не эхо, а повтор пользователем.
        let end = Instant::now() - Duration::from_secs(5);
        m.set_ended_at(end);
        assert!(!g.is_echo("открыть браузер", Instant::now()));
        assert!(g.is_echo("открыть браузер", end + Duration::from_millis(500)));
        // Память кончилась.
        let short = EchoGuard::new(EchoGuardConfig { memory_ms: 1_000, ..Default::default() }, m.clone());
        short.note_spoken_at("далёкое прошлое", Instant::now() - Duration::from_secs(3));
        assert_eq!(short.overlap("далёкое прошлое"), 0.0);
        short.note_spoken("свежее");
        assert_eq!(short.overlap("свежее"), 1.0);
    }

    #[test]
    fn mangled_self_speech_is_echo() {
        // Из журнала 2026-11-24: Джарвис сказал — микрофон через колонки услышал искажённо.
        let (g, m) = guard();
        m.set_playing(true);
        let now = Instant::now();
        g.note_spoken("Готов, говорите.");
        assert!(g.is_echo("готосе говорите", now));
        g.note_spoken("К вашим услугам, сэр.");
        assert!(g.is_echo("вашим услугам с", now));
        g.note_spoken("Слушаю вас внимательно, сэр.");
        assert!(g.is_echo("слушаю вас внимательно сэр", now));
        assert!(!g.is_echo("хули ты меня не слушаешь", now), "ответ человека — не эхо");
    }

    #[test]
    fn own_words_strip_echo_runs() {
        let (g, _) = guard();
        g.note_spoken("Сэр, уточните, пожалуйста, что именно вы хотите, чтобы я сделал.");
        let own = g.own_words("хули ты блять уточните пожалуйста что именно вы хотите чтобы я сделал хули ты меня блять не слушаешь");
        assert_eq!(own.join(" "), "хули ты блять хули ты меня блять не слушаешь");
        // Целиком эхо — своих слов нет; одиночное совпадение («что») остаётся.
        assert!(g.own_words("уточните пожалуйста что именно").is_empty());
        assert_eq!(g.own_words("что за фигня").join(" "), "что за фигня");
    }

    #[test]
    fn clean_confirmation_requires_silence() {
        let (g, m) = guard();
        m.set_playing(true);
        assert!(!g.is_clean_confirmation(Instant::now()));
        let end = Instant::now();
        m.set_ended_at(end);
        assert!(!g.is_clean_confirmation(end + Duration::from_millis(100)));
        assert!(!g.is_clean_confirmation(end + Duration::from_millis(400)), "хвост 600 мс: колонки ноутбука запаздывают");
        assert!(g.is_clean_confirmation(end + Duration::from_millis(700)));
    }
}
