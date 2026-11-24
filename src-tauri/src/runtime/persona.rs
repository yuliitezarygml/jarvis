//! Характер в рантайме: словарь и банк реплик (`config/persona`), настроение, «зеркало» мата, смена режима
//! голосом, служебные пометки к ходу (голос, речь, повторы, настроение, примеры реплик) и готовые фразы по режиму.
//!
//! Модель видит пометки только в текущем ходе (`Agent::run_with`), в память — через протокол рефлектора.
//! Настроение уходит в HUD событием `Event::PersonaMood` (в приложении — Tauri-событие `persona:mood`)
//! при каждой смене ярлыка «спокоен / раздражён / обижен».

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use jarvis_agent::Phrases;
use jarvis_core::config::{PersonaConfig, PersonaMode};
use jarvis_core::persona::{normalize as lex_normalize, tokenize};
use jarvis_core::{
    Event, EventSink, MirrorState, Mood, MoodLabel, NullSink, Persona, PickState, Situation, SpeechTone, SwearLevel, Vars,
    analyze,
};
use jarvis_voice::VoiceTone;

/// Сохранение режима характера (`ConfigStore::update` + `after_save` в приложении). В тестах не задано.
pub type PersonaSaver = Box<dyn Fn(PersonaMode) + Send + Sync>;

/// Сколько секунд команда считается «той же самой», если её повторили.
const REPEAT_WINDOW: Duration = Duration::from_secs(90);
/// Сколько примеров реплик давать модели (бюджет токенов: Groq — 8000 в минуту).
const STYLE_SAMPLES: usize = 5;
/// Начало пометки с примерами реплик (рефлектору не нужна: это слова Джарвиса, а не пользователя).
pub const SAMPLES_NOTE: &str = "примеры твоих реплик в такой ситуации";
/// Начало пометки «зеркала» мата.
pub const MAY_SWEAR_NOTE: &str = "можно материться";

/// Начало пометки со словечками пользователя (Джарвис может иногда вставлять их сам).
pub const SLANG_NOTE: &str = "его словечки";
/// Сколько словечек показывать модели за раз.
const SLANG_SHOWN: usize = 4;
/// Словечки — в каждом N-м ходе (и в любом «эмоциональном»), чтобы не тратить токены каждый раз.
const SLANG_EVERY: u64 = 3;

/// Пометка только для модели, не для рефлектора (примеры реплик, разрешение мата, словечки).
pub fn is_model_only_note(note: &str) -> bool {
    let n = note.trim_start_matches('[');
    n.starts_with(SAMPLES_NOTE) || n.starts_with(MAY_SWEAR_NOTE) || n.starts_with(SLANG_NOTE)
}

/// Словечки из `user.json` (`words`: строки или `{ "text": … }`).
fn read_slang(path: &Path) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(s.trim_start_matches('\u{feff}')) else { return Vec::new() };
    v.get("words")
        .and_then(|w| w.as_array())
        .into_iter()
        .flatten()
        .filter_map(|w| w.as_str().or_else(|| w.get("text").and_then(|t| t.as_str())))
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Изменяемое состояние характера между ходами.
struct TurnState {
    pick: PickState,
    mood: Mood,
    mirror: MirrorState,
    /// Ярлык настроения, который сейчас показан в HUD.
    shown: MoodLabel,
    /// Сколько ходов было (для словечек раз в несколько ходов).
    turns: u64,
}

pub struct PersonaRuntime {
    saver: OnceLock<PersonaSaver>,
    events: Arc<dyn EventSink>,
    cfg: RwLock<PersonaConfig>,
    persona: RwLock<Persona>,
    /// Откуда загружен характер: `config/persona` и `data/persona` (для перезагрузки и словечек).
    dirs: Mutex<Option<(PathBuf, Option<PathBuf>)>>,
    state: Mutex<TurnState>,
    /// Недавние команды (для пометки «повторяет команду»).
    recent: Mutex<VecDeque<(Instant, String)>>,
    /// Любимые словечки пользователя (`lexicon/user.json` → `words`).
    slang: RwLock<Vec<String>>,
}

impl Default for PersonaRuntime {
    fn default() -> Self {
        Self::new(&PersonaConfig::default(), Arc::new(NullSink))
    }
}

impl PersonaRuntime {
    /// Встроенный минимум словаря и фраз; файлы подключает [`Self::load`].
    pub fn new(cfg: &PersonaConfig, events: Arc<dyn EventSink>) -> Self {
        let mut persona = Persona::builtin();
        persona.replies.set_banned(&cfg.banned_words);
        Self {
            saver: OnceLock::new(),
            events,
            cfg: RwLock::new(cfg.clone()),
            persona: RwLock::new(persona),
            dirs: Mutex::new(None),
            state: Mutex::new(TurnState {
                pick: PickState::new(),
                mood: Mood::new(),
                mirror: MirrorState::new(),
                shown: MoodLabel::Calm,
                turns: 0,
            }),
            recent: Mutex::new(VecDeque::new()),
            slang: RwLock::new(Vec::new()),
        }
    }

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

    // ───────────────────────────── загрузка и настройки ─────────────────────────────

    /// Загрузить словарь и реплики из `persona_dir` (`config/persona`) и локальные подколки из `local_dir`
    /// (`data/persona`). При ошибке остаётся прежний (встроенный) характер. `Ok` — предупреждения загрузки.
    pub fn load(&self, persona_dir: &Path, local_dir: Option<&Path>) -> Result<Vec<String>, String> {
        *self.dirs.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((persona_dir.to_path_buf(), local_dir.map(Path::to_path_buf)));
        let mut p = Persona::load(persona_dir, local_dir).map_err(|e| e.to_string())?;
        p.replies.set_banned(&self.config().banned_words);
        let warnings = p.replies.warnings().to_vec();
        tracing::info!(
            lexicon = p.lexicon.len(),
            files = ?p.lexicon.files(),
            warnings = warnings.len(),
            "характер: словарь загружен"
        );
        *self.persona.write().unwrap_or_else(|e| e.into_inner()) = p;
        *self.slang.write().unwrap_or_else(|e| e.into_inner()) = read_slang(&persona_dir.join("lexicon").join("user.json"));
        Ok(warnings)
    }

    /// Перечитать файлы (после записи словечек в `user.json`).
    pub fn reload(&self) -> Result<(), String> {
        let dirs = self.dirs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some((dir, local)) = dirs else { return Err("характер не загружался из файлов".into()) };
        self.load(&dir, local.as_deref()).map(|_| ())
    }

    pub fn config(&self) -> PersonaConfig {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn mode(&self) -> PersonaMode {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).mode
    }

    /// Новые настройки характера (`apply_config`): запрещённые слова, режим; «обижаться» выключили — настроение
    /// сбрасывается (и HUD показывает «спокоен»).
    pub fn set_config(&self, cfg: &PersonaConfig) {
        let old = std::mem::replace(&mut *self.cfg.write().unwrap_or_else(|e| e.into_inner()), cfg.clone());
        if old.banned_words != cfg.banned_words {
            self.persona.write().unwrap_or_else(|e| e.into_inner()).replies.set_banned(&cfg.banned_words);
        }
        if old.mood && !cfg.mood {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).mood.reset();
            self.check_mood(Instant::now());
        }
    }

    // ───────────────────────────── готовые фразы ─────────────────────────────

    /// Фраза ситуации в текущем режиме: обращение из настроек, мат — только если «зеркало» разрешает.
    pub fn pick(&self, situation: Situation, vars: Vars) -> Option<String> {
        self.pick_in(self.mode(), situation, vars)
    }

    /// Фраза ситуации в заданном режиме (смена режима: ответ уже новым голосом).
    pub fn pick_in(&self, mode: PersonaMode, situation: Situation, vars: Vars) -> Option<String> {
        let cfg = PersonaConfig { mode, ..self.config() };
        let persona = self.persona.read().unwrap_or_else(|e| e.into_inner());
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let profanity = mode.allows_profanity() && st.mirror.may_swear(mode);
        let vars = vars.address(cfg.address()).profanity(profanity);
        persona.replies.pick(mode, situation, &mut st.pick, &vars)
    }

    /// Фраза ситуации или `fallback`, если подходящей нет.
    pub fn say(&self, situation: Situation, vars: Vars, fallback: &str) -> String {
        self.pick(situation, vars).unwrap_or_else(|| fallback.to_string())
    }

    /// Отклик на голое «Джарвис». Без слова «да»: эхо отклика не должно сойти за подтверждение.
    pub fn wake_phrase(&self) -> String {
        for _ in 0..6 {
            match self.pick(Situation::Wake, Vars::new()) {
                Some(p) if !has_word(&p, &["да"]) => return p,
                Some(_) => continue,
                None => break,
            }
        }
        match self.config().address() {
            Some(a) => format!("Слушаю, {a}."),
            None => "Слушаю.".into(),
        }
    }

    /// Вопрос перед опасным действием (`action` — «удалить файл», «остановить службу Spooler»).
    /// В любом режиме в вопросе есть «да» и «нет».
    pub fn confirm_phrase(&self, action: &str) -> String {
        let p = self.pick(Situation::ConfirmDangerous, Vars::new().set("action", action));
        ensure_yes_no(p.unwrap_or_else(|| format!("Сэр, {action}? Скажите «да» или «нет».")))
    }

    /// Повторное подтверждение для защищённой системной папки.
    pub fn protected_phrase(&self, action: &str) -> String {
        let p = self.pick(Situation::ProtectedPath, Vars::new().set("action", action));
        ensure_yes_no(p.unwrap_or_else(|| "Сэр, это защищённая системная папка. Подтвердите ещё раз: да или нет?".into()))
    }

    /// Фраза об алерте resource guard: память / процессор / диск (числа из алерта), иначе готовый текст алерта.
    pub fn guard_phrase(&self, alert: &jarvis_sys::GuardAlert) -> String {
        use jarvis_sys::guard::AlertKind;
        let pct = format!("{:.0}", alert.value_pct);
        let with_top = |v: Vars| match &alert.top {
            Some(t) => v.set("proc", t.clone()),
            None => v,
        };
        let (sit, vars) = match &alert.kind {
            AlertKind::Ram => (Situation::RamHigh, with_top(Vars::new().set("pct", pct))),
            AlertKind::Cpu => (Situation::CpuHigh, with_top(Vars::new().set("pct", pct))),
            AlertKind::Disk(mount) => {
                let v = Vars::new().set("disk", mount.trim_end_matches('\\')).set("pct", pct);
                (Situation::DiskLow, match &alert.free {
                    Some(f) => v.set("free", f.clone()),
                    None => v,
                })
            }
        };
        self.say(sit, vars, &alert.message)
    }

    // ───────────────────────────── пометки к ходу ─────────────────────────────

    /// Пометки к ходу (без скобок — агент обернёт): голос, речь (мат, посылание…), повтор команды, настроение,
    /// разрешение мата («Без цензуры»), примеры реплик для эмоциональной ситуации.
    /// `adaptive` выключен — без голоса, речи и примеров; настроение — по своему переключателю `mood`.
    pub fn notes(&self, text: &str, voice: Option<&VoiceTone>, now: Instant) -> Vec<String> {
        let cfg = self.config();
        let mode = cfg.mode;
        let mut out = Vec::new();
        let persona = self.persona.read().unwrap_or_else(|e| e.into_inner());
        let tone = analyze(&persona.lexicon, text);
        if cfg.adaptive {
            if let Some(n) = voice.and_then(|v| v.note_ru()) {
                out.push(n);
            }
            if let Some(n) = tone.note() {
                out.push(format!("речь: {n}"));
            }
        }
        let repeat = self.repeat_note(text, now);
        if let Some(r) = &repeat {
            out.push(r.clone());
        }
        let loud = voice.is_some_and(|v| v.calibrated && (v.loud || v.shouting));
        let shouting = voice.is_some_and(|v| v.calibrated && v.shouting);

        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.mirror.on_user_at(&tone, now);
        if cfg.mood {
            st.mood.on_tone_at(&tone, loud, now);
            if tone.apology {
                // «Проехали»: счётчики посыланий и оскорблений обнуляются.
                st.mood.on_apology_at(now);
            }
            if let Some(n) = st.mood.note_at(now) {
                out.push(format!("настроение: {n}"));
            }
        }
        let may_swear = st.mirror.may_swear_at(mode, now);
        if mode == PersonaMode::Uncensored {
            out.push(format!("{MAY_SWEAR_NOTE}: {}", if may_swear { "да" } else { "нет" }));
        }
        if cfg.adaptive {
            let offended = cfg.mood && st.mood.label_at(now) == MoodLabel::Offended;
            if let Some(sit) = style_situation(&tone, shouting, repeat.is_some(), offended) {
                let vars = Vars::new().address(cfg.address()).profanity(mode.allows_profanity() && may_swear);
                let samples = persona.replies.samples(mode, sit, STYLE_SAMPLES, &mut st.pick, &vars);
                if !samples.is_empty() {
                    let list = samples.iter().map(|s| format!("«{s}»")).collect::<Vec<_>>().join(" · ");
                    out.push(format!("{SAMPLES_NOTE} (это примеры манеры, не повторяй дословно): {list}"));
                }
            }
        }
        // Свои словечки пользователя: «Свой парень» — только приличные, «Без цензуры» — любые
        // (мат — если сейчас можно), дворецкий — никогда. Раз в несколько ходов или в «эмоциональный» ход.
        st.turns += 1;
        if cfg.adaptive && mode != PersonaMode::Butler && (st.turns.is_multiple_of(SLANG_EVERY) || !tone.is_neutral()) {
            let max_level = match mode {
                PersonaMode::Uncensored if may_swear => SwearLevel::Mat,
                PersonaMode::Uncensored => SwearLevel::Rude,
                _ => SwearLevel::Soft,
            };
            let slang = self.slang.read().unwrap_or_else(|e| e.into_inner());
            let allowed: Vec<&String> = slang
                .iter()
                .filter(|w| persona.lexicon.swear_level(w) <= max_level && persona.lexicon.never_say_hits(w).is_empty())
                .filter(|w| !cfg.banned_words.iter().any(|b| w.to_lowercase().contains(&b.to_lowercase())))
                .collect();
            if !allowed.is_empty() {
                // Свежие словечки — в конце списка; берём последние, по кругу от хода к ходу.
                let start = (st.turns as usize / SLANG_EVERY as usize) % allowed.len();
                let shown: Vec<String> =
                    (0..SLANG_SHOWN.min(allowed.len())).map(|i| allowed[(start + i) % allowed.len()].clone()).collect();
                out.push(format!("{SLANG_NOTE} (можешь иногда вставлять к месту, как свои): {}", shown.join(", ")));
            }
        }
        drop(persona);
        self.update_mood_badge(&mut st, now);
        out
    }

    /// «Повторяет команду»: та же команда (с ошибками распознавания) за последние 90 с.
    fn repeat_note(&self, text: &str, now: Instant) -> Option<String> {
        let key = normalize(text);
        let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        while recent.front().is_some_and(|(t, _)| now.saturating_duration_since(*t) > REPEAT_WINDOW) {
            recent.pop_front();
        }
        let repeats = recent.iter().filter(|(_, k)| similar(k, &key)).count();
        let note = (repeats > 0 && key.chars().count() >= 4).then(|| {
            if repeats == 1 { "повторяет команду".to_string() } else { format!("повторяет команду {} раз", repeats + 1) }
        });
        recent.push_back((now, key));
        if recent.len() > 20 {
            recent.pop_front();
        }
        note
    }

    // ───────────────────────────── настроение ─────────────────────────────

    /// Текущее настроение (для HUD после перезагрузки окна): ярлык и причина.
    pub fn mood_now(&self, now: Instant) -> (MoodLabel, Option<String>) {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !self.config().mood {
            return (MoodLabel::Calm, None);
        }
        let label = st.mood.label_at(now);
        (label, (label != MoodLabel::Calm).then(|| st.mood.reason_at(now)).flatten())
    }

    /// Проверить затухание настроения и сообщить HUD, если ярлык сменился (вызывается каждый ход и по таймеру).
    pub fn check_mood(&self, now: Instant) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.update_mood_badge(&mut st, now);
    }

    fn update_mood_badge(&self, st: &mut TurnState, now: Instant) {
        let label = if self.config().mood { st.mood.label_at(now) } else { MoodLabel::Calm };
        if label == st.shown {
            return;
        }
        st.shown = label;
        let reason = (label != MoodLabel::Calm).then(|| st.mood.reason_at(now)).flatten();
        tracing::info!(mood = label.as_str(), reason = ?reason, "характер: настроение");
        self.events.emit(Event::PersonaMood { mood: label, reason });
    }

    // ───────────────────────────── словечки ─────────────────────────────

    /// Любимые словечки пользователя от рефлектора → `config/persona/lexicon/user.json` (`words`), затем словарь
    /// перечитывается. Возвращает, сколько слов добавлено.
    pub fn add_slang(&self, words: &[String]) -> Result<usize, String> {
        let dirs = self.dirs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some((dir, _)) = dirs else { return Ok(0) };
        let path = dir.join("lexicon").join("user.json");
        let added = merge_slang(&path, words)?;
        if added > 0 {
            self.reload()?;
            tracing::info!(added, path = %path.display(), "характер: новые словечки пользователя");
        }
        Ok(added)
    }
}

impl Phrases for PersonaRuntime {
    fn phrase(&self, situation: Situation, vars: &Vars) -> Option<String> {
        self.pick(situation, vars.clone())
    }
}

/// Самая подходящая ситуация для примеров реплик; `None` — ход без эмоций.
fn style_situation(tone: &SpeechTone, shouting: bool, repeat: bool, offended: bool) -> Option<Situation> {
    Some(if tone.sendoff {
        Situation::SendoffReply
    } else if tone.insult_to_jarvis {
        Situation::InsultReply
    } else if shouting {
        Situation::Shouting
    } else if repeat {
        Situation::Repeat
    } else if tone.praise {
        Situation::PraiseReply
    } else if tone.apology {
        Situation::ApologyReply
    } else if offended {
        Situation::Offended
    } else {
        return None;
    })
}

/// Есть ли в фразе одно из слов (по токенам словаря: «Да-да?» → «да», «да»).
fn has_word(text: &str, words: &[&str]) -> bool {
    tokenize(text).iter().any(|t| words.contains(&t.as_str()))
}

/// Вопрос подтверждения всегда спрашивает «да или нет».
fn ensure_yes_no(phrase: String) -> String {
    if has_word(&phrase, &["да"]) && has_word(&phrase, &["нет"]) {
        phrase
    } else {
        format!("{} Да или нет?", phrase.trim_end())
    }
}

/// Дописать словечки в `user.json` (`words`), без дублей и без слов из `never_say`; прочие поля файла сохраняются.
pub fn merge_slang(path: &Path, words: &[String]) -> Result<usize, String> {
    let mut doc: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(s.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({ "words": [], "never_say": [] }),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let obj = doc.as_object_mut().ok_or_else(|| format!("{}: ожидался JSON-объект", path.display()))?;
    let key_of = |v: &serde_json::Value| -> Option<String> {
        let t = v.as_str().or_else(|| v.get("text").and_then(|t| t.as_str()))?;
        Some(lex_normalize(t)).filter(|k| !k.is_empty())
    };
    let never: Vec<String> =
        obj.get("never_say").and_then(|v| v.as_array()).into_iter().flatten().filter_map(key_of).collect();
    obj.entry("never_say").or_insert_with(|| serde_json::json!([]));
    let list = obj.entry("words").or_insert_with(|| serde_json::json!([]));
    if !list.is_array() {
        *list = serde_json::json!([]);
    }
    let arr = list.as_array_mut().expect("массив");
    let mut known: Vec<String> = arr.iter().filter_map(key_of).collect();
    let mut added = 0;
    for w in words {
        let w = w.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        let k = lex_normalize(&w);
        if k.is_empty() || w.chars().count() > 40 || k.split(' ').count() > 3 || known.contains(&k) || never.contains(&k) {
            continue;
        }
        arr.push(serde_json::Value::String(w));
        known.push(k);
        added += 1;
    }
    if added > 0 {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(added)
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

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::CollectSink;

    /// Настоящие `config/persona` и `data/persona` репозитория.
    fn repo_dirs() -> (PathBuf, PathBuf) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        (root.join("config/persona"), root.join("data/persona"))
    }

    fn real(mode: PersonaMode) -> (PersonaRuntime, Arc<CollectSink>) {
        let sink = Arc::new(CollectSink::default());
        let p = PersonaRuntime::new(&PersonaConfig { mode, ..Default::default() }, sink.clone());
        let (dir, local) = repo_dirs();
        p.load(&dir, Some(&local)).expect("config/persona загружается");
        (p, sink)
    }

    fn moods(sink: &CollectSink) -> Vec<MoodLabel> {
        sink.take()
            .into_iter()
            .filter_map(|e| match e {
                Event::PersonaMood { mood, .. } => Some(mood),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn repeat_detection_with_asr_noise() {
        let p = PersonaRuntime::default();
        let t0 = Instant::now();
        assert!(p.notes("открой блокнот", None, t0).is_empty());
        let n = p.notes("открой блокнод", None, t0 + Duration::from_secs(10));
        assert_eq!(n[0], "повторяет команду");
        // К повтору — пример реплики для этой ситуации (встроенный дворецкий).
        assert!(n[1].starts_with(SAMPLES_NOTE) && n[1].ends_with("«Уже выполняю, сэр.»"), "{n:?}");
        let n = p.notes("Открой блокнот!", None, t0 + Duration::from_secs(20));
        assert_eq!(n[0], "повторяет команду 3 раз");
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
        let hit = Arc::new(Mutex::new(None));
        let h = hit.clone();
        p.set_saver(Box::new(move |m| *h.lock().unwrap() = Some(m)));
        assert!(p.save(PersonaMode::Uncensored));
        assert_eq!(*hit.lock().unwrap(), Some(PersonaMode::Uncensored));
    }

    #[test]
    fn sendoff_gives_speech_note_mood_and_event() {
        let (p, sink) = real(PersonaMode::Buddy);
        let t0 = Instant::now();
        let n = p.notes("пошёл нахуй", None, t0);
        let speech = n.iter().find(|x| x.starts_with("речь: ")).expect("пометка речи");
        assert!(speech.starts_with("речь: мат") && speech.contains("посылание"), "{n:?}");
        // Пример ответок на посылание — без мата (режим «Свой парень»).
        let samples = n.iter().find(|x| x.starts_with(SAMPLES_NOTE)).expect("примеры реплик");
        assert!(samples.contains("не повторяй дословно") && samples.contains('«'), "{samples}");
        assert!(moods(&sink).contains(&MoodLabel::Irritated));
        let n = p.notes("иди нахуй", None, t0 + Duration::from_secs(5));
        let mood = n.iter().find(|x| x.starts_with("настроение: ")).expect("пометка настроения");
        assert!(mood.contains("обижен") && mood.contains("дважды"), "{mood}");
        let ev = sink.take();
        assert!(
            ev.contains(&Event::PersonaMood { mood: MoodLabel::Offended, reason: Some("его дважды послали".into()) }),
            "{ev:?}"
        );
        // Извинились — «проехали»: спокоен, событие о смене.
        let n = p.notes("ладно, извини", None, t0 + Duration::from_secs(10));
        assert!(!n.iter().any(|x| x.starts_with("настроение")), "{n:?}");
        assert_eq!(moods(&sink), vec![MoodLabel::Calm]);
        // Без смены ярлыка — без событий.
        p.notes("открой блокнот", None, t0 + Duration::from_secs(15));
        assert!(moods(&sink).is_empty());
    }

    #[test]
    fn mood_decays_and_badge_follows() {
        let (p, sink) = real(PersonaMode::Butler);
        let t0 = Instant::now();
        p.notes("отвали", None, t0);
        p.notes("заткнись", None, t0 + Duration::from_secs(1));
        assert_eq!(moods(&sink).last(), Some(&MoodLabel::Offended));
        p.check_mood(t0 + Duration::from_secs(3600));
        assert_eq!(moods(&sink), vec![MoodLabel::Calm]);
        // «Обижаться» выключено — настроения нет совсем.
        p.set_config(&PersonaConfig { mood: false, ..Default::default() });
        let n = p.notes("пошёл нахуй", None, t0 + Duration::from_secs(3700));
        assert!(!n.iter().any(|x| x.starts_with("настроение")), "{n:?}");
        assert!(moods(&sink).is_empty());
    }

    #[test]
    fn uncensored_may_swear_only_after_user_swore() {
        let (p, _) = real(PersonaMode::Uncensored);
        let t0 = Instant::now();
        let n = p.notes("открой блокнот", None, t0);
        assert!(n.contains(&"можно материться: нет".to_string()), "{n:?}");
        let n = p.notes("да блядь, опять не работает", None, t0 + Duration::from_secs(5));
        assert!(n.contains(&"можно материться: да".to_string()), "{n:?}");
        // Остальные режимы такой пометки не получают.
        let (b, _) = real(PersonaMode::Buddy);
        assert!(!b.notes("да блядь", None, t0).iter().any(|x| x.starts_with(MAY_SWEAR_NOTE)));
    }

    #[test]
    fn user_slang_used_by_mode() {
        let words = vec!["красава".to_string(), "жиза".to_string(), "ёпта".to_string(), "нахуй".to_string()];
        let slang_of = |n: &[String]| n.iter().find(|x| x.starts_with(SLANG_NOTE)).cloned();
        let t0 = Instant::now();
        // Дворецкий — никогда.
        let (b, _) = real(PersonaMode::Butler);
        *b.slang.write().unwrap() = words.clone();
        for i in 0..6 {
            assert!(slang_of(&b.notes(&format!("открой блокнот номер {i}"), None, t0 + Duration::from_secs(i * 100))).is_none());
        }
        // «Свой парень» — раз в 3 хода, без мата.
        let (y, _) = real(PersonaMode::Buddy);
        *y.slang.write().unwrap() = words.clone();
        let mut seen = None;
        for i in 0..3 {
            seen = seen.or(slang_of(&y.notes(&format!("какая погода {i}"), None, t0 + Duration::from_secs(i * 100))));
        }
        let s = seen.expect("за 3 хода словечки показаны");
        assert!(s.contains("красава") && !s.contains("нахуй"), "{s}");
        // «Без цензуры»: пока пользователь не матерился — без мата; после мата — можно.
        let (u, _) = real(PersonaMode::Uncensored);
        *u.slang.write().unwrap() = words;
        let n = u.notes("пошёл нахуй", None, t0);
        let s = slang_of(&n).expect("эмоциональный ход — словечки сразу");
        assert!(s.contains("нахуй"), "{s}");
        assert!(is_model_only_note(&s), "рефлектору словечки не нужны");
    }

    #[test]
    fn butler_never_gets_mat_samples() {
        let (p, _) = real(PersonaMode::Butler);
        let lex = Persona::load(&repo_dirs().0, None).unwrap().lexicon;
        let t0 = Instant::now();
        for (i, text) in ["пошёл нахуй", "ты дебил, блядь", "отъебись"].iter().enumerate() {
            let n = p.notes(text, None, t0 + Duration::from_secs(i as u64));
            let s = n.iter().find(|x| x.starts_with(SAMPLES_NOTE)).expect("примеры");
            let body = s.split_once(": ").unwrap().1;
            assert!(lex.swear_level(body) < jarvis_core::SwearLevel::Mat, "мат в примерах дворецкого: {body}");
        }
        // В «Без цензуры» после мата пользователя примеры могут быть с матом — и в них нет звёздочек.
        let (u, _) = real(PersonaMode::Uncensored);
        let n = u.notes("пошёл нахуй", None, t0);
        assert!(n.iter().any(|x| x.starts_with(SAMPLES_NOTE) && !x.contains('*')), "{n:?}");
    }

    #[test]
    fn not_adaptive_keeps_only_repeats_and_mood() {
        let (p, _) = real(PersonaMode::Buddy);
        p.set_config(&PersonaConfig { mode: PersonaMode::Buddy, adaptive: false, ..Default::default() });
        let t0 = Instant::now();
        let n = p.notes("пошёл нахуй", None, t0);
        assert!(!n.iter().any(|x| x.starts_with("речь") || x.starts_with(SAMPLES_NOTE)), "{n:?}");
        assert!(n.iter().any(|x| x.starts_with("настроение")), "{n:?}");
        assert_eq!(p.notes("пошёл нахуй", None, t0 + Duration::from_secs(1)).iter().filter(|x| x.starts_with("повторяет")).count(), 1);
    }

    #[test]
    fn fixed_phrases_per_mode() {
        for mode in PersonaMode::ALL {
            let (p, _) = real(mode);
            for _ in 0..20 {
                let w = p.wake_phrase();
                assert!(!has_word(&w, &["да"]), "{mode:?}: «{w}»");
                let c = p.confirm_phrase("удалить файл");
                assert!(has_word(&c, &["да"]) && has_word(&c, &["нет"]), "{mode:?}: «{c}»");
                assert!(!c.contains("{action}") && !c.contains('…'), "{mode:?}: «{c}»");
                let pp = p.protected_phrase("удалить файл");
                assert!(has_word(&pp, &["да"]) && has_word(&pp, &["нет"]), "{mode:?}: «{pp}»");
            }
            if mode != PersonaMode::Butler {
                let w = p.wake_phrase();
                assert!(!w.to_lowercase().contains("сэр"), "{mode:?}: «{w}»");
            }
            let switched = p.pick_in(mode, Situation::ModeSwitched, Vars::new()).expect("фраза смены режима");
            assert!(!switched.is_empty());
        }
        // Встроенный минимум (нет файлов): дворецкий — как раньше в коде.
        let p = PersonaRuntime::default();
        assert_eq!(p.wake_phrase(), "Слушаю, сэр.");
        assert_eq!(p.confirm_phrase("удалить файл"), "Сэр, удалить файл? Скажите «да» или «нет».");
        p.set_config(&PersonaConfig { mode: PersonaMode::Buddy, ..Default::default() });
        assert_eq!(p.wake_phrase(), "Слушаю.");
        assert_eq!(p.confirm_phrase("удалить файл"), "Удалить файл, точно? Да или нет?");
    }

    #[test]
    fn banned_words_filter_phrases() {
        let p = PersonaRuntime::default();
        p.set_config(&PersonaConfig { banned_words: vec!["сэр".into()], ..Default::default() });
        // Единственная встроенная фраза дворецкого с «сэр» запрещена → запасной текст.
        assert_eq!(p.pick(Situation::Done, Vars::new()), None);
        assert_eq!(p.say(Situation::Done, Vars::new(), "Готово."), "Готово.");
    }

    #[test]
    fn guard_alerts_by_mode() {
        use jarvis_sys::guard::AlertKind;
        let alert = jarvis_sys::GuardAlert {
            kind: AlertKind::Disk("D:\\".into()),
            value_pct: 3.2,
            top: None,
            free: Some("9.5 ГБ".into()),
            message: "Сэр, мало места.".into(),
        };
        let p = PersonaRuntime::default();
        assert_eq!(p.guard_phrase(&alert), "Сэр, на диске D: осталось всего 3 % свободного места.");
        p.set_config(&PersonaConfig { mode: PersonaMode::Buddy, ..Default::default() });
        let s = p.guard_phrase(&alert);
        assert!(s.contains("D:") && s.contains('3') && !s.contains("сэр"), "{s}");
        let ram = jarvis_sys::GuardAlert { kind: AlertKind::Ram, value_pct: 93.4, top: Some("chrome".into()), free: None, message: String::new() };
        for mode in PersonaMode::ALL {
            let (p, _) = real(mode);
            let s = p.guard_phrase(&ram);
            assert!(s.contains("93") && !s.contains('{'), "{mode:?}: {s}");
        }
    }

    #[test]
    fn slang_merges_into_user_json_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let lexdir = dir.path().join("lexicon");
        std::fs::create_dir_all(&lexdir).unwrap();
        let user = lexdir.join("user.json");
        std::fs::write(&user, r#"{ "words": ["жиза", {"text": "красава", "praise": true}], "never_say": ["ёпта"], "note": "моё" }"#).unwrap();
        assert_eq!(merge_slang(&user, &["Жиза".into(), "красава".into(), "ёпта".into(), "кринж".into(), " кринж ".into(), "".into()]).unwrap(), 1);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&user).unwrap()).unwrap();
        assert_eq!(v["words"], serde_json::json!(["жиза", {"text": "красава", "praise": true}, "кринж"]));
        assert_eq!(v["never_say"], serde_json::json!(["ёпта"]));
        assert_eq!(v["note"], "моё");
        // Нового нет — файл не трогаем.
        assert_eq!(merge_slang(&user, &["кринж".into()]).unwrap(), 0);
        // Через рантайм: слово попадает в словарь сразу (перезагрузка).
        let p = PersonaRuntime::default();
        p.load(dir.path(), None).unwrap();
        assert_eq!(p.add_slang(&["вайб".into()]).unwrap(), 1);
        let files = p.persona.read().unwrap().lexicon.files().to_vec();
        assert!(files.contains(&"user".to_string()), "{files:?}");
        assert!(std::fs::read_to_string(&user).unwrap().contains("вайб"));
        // Файла нет — создаётся.
        let fresh = dir.path().join("new/lexicon/user.json");
        assert_eq!(merge_slang(&fresh, &["имба".into()]).unwrap(), 1);
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&fresh).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"words": ["имба"], "never_say": []}));
    }

    #[test]
    fn model_only_notes() {
        assert!(is_model_only_note("[можно материться: да]"));
        assert!(is_model_only_note(&format!("[{SAMPLES_NOTE} (…): «x»]")));
        assert!(!is_model_only_note("[речь: мат, посылание]"));
    }
}
