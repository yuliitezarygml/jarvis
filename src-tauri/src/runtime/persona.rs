//! Характер в рантайме: смена режима голосом, служебные пометки к ходу (голос, повторы, ругань, настроение).
//!
//! Модель видит пометки только в текущем ходе (`Agent::run_with`), в память — через протокол рефлектора.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use jarvis_core::config::PersonaMode;
use jarvis_voice::VoiceTone;

/// Сохранение режима характера (`ConfigStore::update` + `after_save` в приложении). В тестах не задано.
pub type PersonaSaver = Box<dyn Fn(PersonaMode) + Send + Sync>;

/// Сколько секунд команда считается «той же самой», если её повторили.
const REPEAT_WINDOW: Duration = Duration::from_secs(90);

pub struct PersonaRuntime {
    saver: OnceLock<PersonaSaver>,
    /// Недавние команды (для пометки «повторяет команду»).
    recent: Mutex<VecDeque<(Instant, String)>>,
}

impl Default for PersonaRuntime {
    fn default() -> Self {
        Self { saver: OnceLock::new(), recent: Mutex::new(VecDeque::new()) }
    }
}

impl PersonaRuntime {
    pub fn set_saver(&self, saver: PersonaSaver) {
        let _ = self.saver.set(saver);
    }

    /// Сохранить режим через приложение. `false` — сохранять нечем (тесты, консоль): вызывающий применит сам.
    pub fn save(&self, mode: PersonaMode) -> bool {
        match self.saver.get() {
            Some(s) => {
                s(mode);
                true
            }
            None => false,
        }
    }

    /// Пометки к ходу: тон голоса и повтор команды.
    pub fn notes(&self, text: &str, voice: Option<&VoiceTone>, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(n) = voice.and_then(|v| v.note_ru()) {
            out.push(n);
        }
        let key = normalize(text);
        let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        while recent.front().is_some_and(|(t, _)| now.duration_since(*t) > REPEAT_WINDOW) {
            recent.pop_front();
        }
        let repeats = recent.iter().filter(|(_, k)| similar(k, &key)).count();
        if repeats > 0 && key.chars().count() >= 4 {
            out.push(if repeats == 1 { "повторяет команду".into() } else { format!("повторяет команду {} раз", repeats + 1) });
        }
        recent.push_back((now, key));
        if recent.len() > 20 {
            recent.pop_front();
        }
        out
    }
}

/// Нижний регистр, ё→е, только буквы/цифры через пробел.
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .replace('ё', "е")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Та же команда: совпадает или отличается ≤20 % символов (ошибки распознавания).
fn similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    if la.abs_diff(lb) * 5 > la.max(lb) {
        return false;
    }
    levenshtein(a, b) * 5 <= la.max(lb)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Короткая фраза о смене режима (пока словарь реплик не загружен).
pub fn switched_phrase(mode: PersonaMode) -> &'static str {
    match mode {
        PersonaMode::Butler => "Как пожелаете, сэр. Перехожу на вежливый тон.",
        PersonaMode::Buddy => "Окей, давай по-простому, на «ты».",
        PersonaMode::Uncensored => "Ну всё, базарим без цензуры. Сам напросился.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_detection_with_asr_noise() {
        let p = PersonaRuntime::default();
        let t0 = Instant::now();
        assert!(p.notes("открой блокнот", None, t0).is_empty());
        let n = p.notes("открой блокнод", None, t0 + Duration::from_secs(10));
        assert_eq!(n, vec!["повторяет команду".to_string()]);
        let n = p.notes("Открой блокнот!", None, t0 + Duration::from_secs(20));
        assert_eq!(n, vec!["повторяет команду 3 раз".to_string()]);
        // Другая команда и старые повторы (> 90 с) не считаются.
        assert!(p.notes("закрой хром", None, t0 + Duration::from_secs(30)).is_empty());
        assert!(p.notes("открой блокнот", None, t0 + Duration::from_secs(200)).is_empty());
        // Короткие («да», «ок») — не повторы.
        assert!(p.notes("ок", None, t0 + Duration::from_secs(201)).is_empty());
        assert!(p.notes("ок", None, t0 + Duration::from_secs(202)).is_empty());
    }

    #[test]
    fn saver_optional() {
        let p = PersonaRuntime::default();
        assert!(!p.save(PersonaMode::Buddy));
        let hit = std::sync::Arc::new(Mutex::new(None));
        let h = hit.clone();
        p.set_saver(Box::new(move |m| *h.lock().unwrap() = Some(m)));
        assert!(p.save(PersonaMode::Uncensored));
        assert_eq!(*hit.lock().unwrap(), Some(PersonaMode::Uncensored));
    }
}
