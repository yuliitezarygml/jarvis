//! Характер Джарвиса (см. `PERSONALITY-PLAN.md`): словарь ругани/похвалы/извинений, разбор тона реплики,
//! банк готовых фраз по режимам и ситуациям, настроение и «зеркало» мата.
//!
//! Всё здесь — чистые функции и маленькие состояния; ввод-вывод только в загрузчиках
//! ([`Lexicon::load`], [`Replies::load`], [`Persona::load`]). Время передаётся явно (`*_at(now)`),
//! чтобы тесты не зависели от часов.
//!
//! Файлы (`config/persona/`):
//! - `lexicon/*.json` — словари распознавания (формат — в `lexicon/roots-ru.json`, поле `note`);
//!   `lexicon/user.json` — свои слова владельца: `{ "words": [...], "never_say": [...] }`;
//! - `replies/{butler,buddy,uncensored}.json` — `{ "mode": "...", "situations": { "<ключ>": ["…"] } }`;
//!   необязательный `replies/user.json` — `{ "<режим>": { "<ключ>": ["…"] } }` (свои фразы владельца);
//! - локальный `data/persona/replies-lj.json` (не в git) — подколки из ЖЖ, см. [`Replies::add_local_file`].
//!
//! Быстрый путь на каждую реплику пользователя:
//! ```ignore
//! let tone = analyze(&persona.lexicon, text);          // мат? послал? похвалил?
//! mood.on_tone(&tone, voice_is_loud);                  // настроение
//! mirror.on_user(&tone);                               // «зеркало» мата
//! let vars = Vars::new().address(cfg.address()).profanity(mirror.may_swear(cfg.mode));
//! let line = persona.replies.pick(cfg.mode, Situation::SendoffReply, &mut pick_state, &vars);
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::PersonaMode;

// ───────────────────────────── ошибки ─────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum PersonaError {
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path}: {source}")]
    Json { path: String, source: serde_json::Error },
}

pub type Result<T> = std::result::Result<T, PersonaError>;

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let s = fs::read_to_string(path).map_err(|source| PersonaError::Io { path: path.display().to_string(), source })?;
    serde_json::from_str(s.trim_start_matches('\u{feff}'))
        .map_err(|source| PersonaError::Json { path: path.display().to_string(), source })
}

/// `dir/<sub>`, если такая папка есть, иначе сам `dir` (можно передать и `config/persona`, и `config/persona/lexicon`).
fn subdir(dir: &Path, sub: &str) -> PathBuf {
    let d = dir.join(sub);
    if d.is_dir() { d } else { dir.to_path_buf() }
}

// ───────────────────────────── нормализация ─────────────────────────────

/// Нижний регистр, ё→е, румынская диакритика без знаков, ударения выброшены; всё, что не буква/цифра, — пробел.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'ё' | 'ѐ' => 'е',
            'ă' | 'â' | 'á' | 'à' | 'ä' => 'a',
            'î' | 'í' | 'ì' => 'i',
            'ș' | 'ş' => 's',
            'ț' | 'ţ' => 't',
            'é' | 'è' => 'e',
            'ó' | 'ö' => 'o',
            'ú' | 'ü' => 'u',
            c => c,
        };
        // Ударения и прочие комбинирующие знаки, апострофы (you're → youre) — просто выбросить.
        if ('\u{0300}'..='\u{036F}').contains(&c) || matches!(c, '\'' | '’' | '\u{00AD}') {
            continue;
        }
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Слова текста после [`normalize`].
pub fn tokenize(text: &str) -> Vec<String> {
    normalize(text).split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect()
}

/// Ключ фразы: слова без пробелов («на хуй» = «нахуй»).
fn phrase_key(text: &str) -> String {
    tokenize(text).concat()
}

// ───────────────────────────── словарь ─────────────────────────────

/// Насколько грубо сказано. Порядок важен: `None < Soft < Rude < Mat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwearLevel {
    /// Без ругани.
    #[default]
    None,
    /// «блин», «капец», «чёрт».
    Soft,
    /// Грубо без мата: «дебил», «заткнись», «херня».
    Rude,
    /// Мат.
    Mat,
}

impl SwearLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            SwearLevel::None => "none",
            SwearLevel::Soft => "soft",
            SwearLevel::Rude => "rude",
            SwearLevel::Mat => "mat",
        }
    }

    /// Для пометки хода модели.
    pub fn title_ru(self) -> &'static str {
        match self {
            SwearLevel::None => "без ругани",
            SwearLevel::Soft => "мягкая ругань",
            SwearLevel::Rude => "грубость",
            SwearLevel::Mat => "мат",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RawEntry {
    text: String,
    level: SwearLevel,
    forms: Vec<String>,
    prefix: Vec<String>,
    contains: Vec<String>,
    exclude: Vec<String>,
    sendoff: bool,
    insult: bool,
    praise: bool,
    apology: bool,
    never_say: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawWord {
    Text(String),
    Entry(RawEntry),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RawSendoff {
    go: Vec<String>,
    go_plural: Vec<String>,
    dest: Vec<String>,
    dest_weak: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RawLexFile {
    name: String,
    safe: Vec<String>,
    second_person: Vec<String>,
    third_person: Vec<String>,
    sendoff: Option<RawSendoff>,
    entries: Vec<RawEntry>,
    /// user.json: свои слова (строка — просто «понимаю это словечко», объект — как запись словаря).
    words: Vec<RawWord>,
    /// user.json: слова, которые Джарвис не говорит никогда.
    never_say: Vec<String>,
}

/// Запись словаря (после загрузки).
#[derive(Debug, Clone, PartialEq)]
pub struct LexEntry {
    pub text: String,
    pub level: SwearLevel,
    pub sendoff: bool,
    pub insult: bool,
    pub praise: bool,
    pub apology: bool,
    /// Распознаём, но Джарвис не произносит никогда (оскорбления по национальности, ориентации, здоровью…).
    pub never_say: bool,
    /// Файл-источник (`roots-ru`, `wiktionary-ru`, `user`…).
    pub source: String,
    exclude: Vec<String>,
}

impl LexEntry {
    fn active(&self) -> bool {
        self.level > SwearLevel::None || self.sendoff || self.insult || self.praise || self.apology || self.never_say
    }
}

/// Одно совпадение словаря в тексте.
#[derive(Debug, Clone, PartialEq)]
pub struct LexHit {
    /// Как написано в тексте (нормализовано): «заебал», «на хуй».
    pub surface: String,
    /// Запись словаря: «еб/ёб», «пошёл нахуй»…
    pub entry: String,
    pub level: SwearLevel,
    pub sendoff: bool,
    pub insult: bool,
    pub praise: bool,
    pub apology: bool,
    pub never_say: bool,
    /// Индексы слов `[start, end)`.
    pub start: usize,
    pub end: usize,
}

/// Словарь распознавания: мат и ругань по корням, посылания, похвала, извинения.
#[derive(Debug, Clone, Default)]
pub struct Lexicon {
    entries: Vec<LexEntry>,
    exact: HashMap<String, Vec<u32>>,
    prefix: HashMap<String, Vec<u32>>,
    contains: HashMap<String, Vec<u32>>,
    phrases: HashMap<String, Vec<u32>>,
    phrase_max: usize,
    prefix_max: usize,
    contains_max: usize,
    safe: Vec<String>,
    second_person: HashSet<String>,
    third_person: HashSet<String>,
    go: HashSet<String>,
    go_plural: HashSet<String>,
    dest: HashSet<String>,
    dest_weak: HashSet<String>,
    dest_max: usize,
    never_say_words: Vec<String>,
    files: Vec<String>,
}

fn push_id(map: &mut HashMap<String, Vec<u32>>, key: String, id: u32) {
    let v = map.entry(key).or_default();
    if !v.contains(&id) {
        v.push(id);
    }
}

impl Lexicon {
    /// Все `*.json` из `dir` (или `dir/lexicon`), включая `user.json`. Файлы читаются по алфавиту.
    pub fn load(dir: &Path) -> Result<Lexicon> {
        let dir = subdir(dir, "lexicon");
        let rd = fs::read_dir(&dir).map_err(|source| PersonaError::Io { path: dir.display().to_string(), source })?;
        let mut paths: Vec<PathBuf> =
            rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        paths.sort();
        let mut lex = Lexicon::default();
        for p in paths {
            let raw: RawLexFile = read_json(&p)?;
            let name = if raw.name.is_empty() {
                p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                raw.name.clone()
            };
            lex.add_file(&name, raw);
        }
        Ok(lex)
    }

    /// Встроенный минимальный словарь: основные корни мата, посылания, похвала, извинения.
    /// Для тестов и на случай, если `config/persona` нет.
    pub fn builtin_minimal() -> Lexicon {
        let raw: RawLexFile = serde_json::from_str(BUILTIN_LEXICON).expect("встроенный словарь");
        let mut lex = Lexicon::default();
        lex.add_file("builtin", raw);
        lex
    }

    /// Добавить словарь из JSON-строки (формат как у `lexicon/*.json`).
    pub fn add_json(&mut self, name: &str, json: &str) -> std::result::Result<(), serde_json::Error> {
        let raw: RawLexFile = serde_json::from_str(json)?;
        self.add_file(name, raw);
        Ok(())
    }

    fn add_file(&mut self, name: &str, raw: RawLexFile) {
        self.files.push(name.to_string());
        self.safe.extend(raw.safe.iter().map(|s| normalize(s).replace(' ', "")).filter(|s| !s.is_empty()));
        self.second_person.extend(raw.second_person.iter().map(|s| normalize(s)));
        self.third_person.extend(raw.third_person.iter().map(|s| normalize(s)));
        if let Some(s) = raw.sendoff {
            self.go.extend(s.go.iter().map(|w| normalize(w)));
            self.go_plural.extend(s.go_plural.iter().map(|w| normalize(w)));
            for d in &s.dest {
                self.dest_max = self.dest_max.max(tokenize(d).len());
                self.dest.insert(phrase_key(d));
            }
            for d in &s.dest_weak {
                self.dest_max = self.dest_max.max(tokenize(d).len());
                self.dest_weak.insert(phrase_key(d));
            }
        }
        self.never_say_words.extend(raw.never_say.iter().map(|w| normalize(w)).filter(|w| !w.is_empty()));
        let words = raw.words.into_iter().map(|w| match w {
            RawWord::Text(t) => RawEntry { text: t.clone(), forms: vec![t], ..Default::default() },
            RawWord::Entry(e) => e,
        });
        let user_file = name == "user";
        for e in raw.entries.into_iter().chain(words) {
            self.add_entry(name, e, user_file);
        }
    }

    fn add_entry(&mut self, source: &str, e: RawEntry, keep_inert: bool) {
        let entry = LexEntry {
            text: e.text.clone(),
            level: e.level,
            sendoff: e.sendoff,
            insult: e.insult,
            praise: e.praise,
            apology: e.apology,
            never_say: e.never_say,
            source: source.to_string(),
            exclude: e.exclude.iter().map(|x| normalize(x).replace(' ', "")).filter(|x| !x.is_empty()).collect(),
        };
        // Справочные записи Викисловаря (`level: none` без флагов) не распознаём; свои слова владельца — да.
        if !entry.active() && !keep_inert {
            return;
        }
        let id = self.entries.len() as u32;
        let mut forms = e.forms;
        if forms.is_empty() && e.prefix.is_empty() && e.contains.is_empty() {
            forms.push(e.text.clone());
        }
        for f in &forms {
            let toks = tokenize(f);
            match toks.len() {
                0 => {}
                1 => push_id(&mut self.exact, toks[0].clone(), id),
                n => {
                    self.phrase_max = self.phrase_max.max(n);
                    push_id(&mut self.phrases, toks.concat(), id);
                }
            }
        }
        for p in &e.prefix {
            let k = phrase_key(p);
            if !k.is_empty() {
                self.prefix_max = self.prefix_max.max(k.chars().count());
                push_id(&mut self.prefix, k, id);
            }
        }
        for c in &e.contains {
            let k = phrase_key(c);
            if !k.is_empty() {
                self.contains_max = self.contains_max.max(k.chars().count());
                push_id(&mut self.contains, k, id);
            }
        }
        self.entries.push(entry);
    }

    /// Число записей, участвующих в распознавании.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Загруженные файлы (имена).
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Слова из `user.json → never_say` (нормализованные).
    pub fn never_say_words(&self) -> &[String] {
        &self.never_say_words
    }

    fn vetoed(&self, tok: &str, id: u32) -> bool {
        self.entries[id as usize].exclude.iter().any(|x| tok.contains(x.as_str()))
            || self.safe.iter().any(|s| tok.contains(s.as_str()))
    }

    fn scan(&self, toks: &[String]) -> Vec<(usize, usize, u32)> {
        let mut hits: Vec<(usize, usize, u32)> = Vec::new();
        let add = |hits: &mut Vec<(usize, usize, u32)>, h: (usize, usize, u32)| {
            if !hits.contains(&h) {
                hits.push(h);
            }
        };
        for (i, tok) in toks.iter().enumerate() {
            if let Some(ids) = self.exact.get(tok) {
                for &id in ids {
                    add(&mut hits, (i, i + 1, id));
                }
            }
            let bounds: Vec<usize> = tok.char_indices().map(|(b, _)| b).chain([tok.len()]).collect();
            for k in 1..bounds.len().min(self.prefix_max + 1) {
                if let Some(ids) = self.prefix.get(&tok[..bounds[k]]) {
                    for &id in ids {
                        if !self.vetoed(tok, id) {
                            add(&mut hits, (i, i + 1, id));
                        }
                    }
                }
            }
            if !self.contains.is_empty() {
                for s in 0..bounds.len() - 1 {
                    for e in s + 1..bounds.len().min(s + self.contains_max + 1) {
                        if let Some(ids) = self.contains.get(&tok[bounds[s]..bounds[e]]) {
                            for &id in ids {
                                if !self.vetoed(tok, id) {
                                    add(&mut hits, (i, i + 1, id));
                                }
                            }
                        }
                    }
                }
            }
        }
        // Фразы: окна по 1..phrase_max слов, склеенные без пробелов (ловит и «нахуй», и «на хуй»).
        if self.phrase_max > 0 {
            for i in 0..toks.len() {
                let mut key = String::new();
                for (len, t) in toks[i..].iter().enumerate().take(self.phrase_max) {
                    key.push_str(t);
                    if let Some(ids) = self.phrases.get(&key) {
                        for &id in ids {
                            add(&mut hits, (i, i + len + 1, id));
                        }
                    }
                }
            }
        }
        hits.sort();
        hits
    }

    /// Все совпадения словаря в тексте.
    pub fn hits(&self, text: &str) -> Vec<LexHit> {
        let toks = tokenize(text);
        self.hits_tokens(&toks)
    }

    fn hits_tokens(&self, toks: &[String]) -> Vec<LexHit> {
        self.scan(toks)
            .into_iter()
            .map(|(s, e, id)| {
                let en = &self.entries[id as usize];
                LexHit {
                    surface: toks[s..e].join(" "),
                    entry: en.text.clone(),
                    level: en.level,
                    sendoff: en.sendoff,
                    insult: en.insult,
                    praise: en.praise,
                    apology: en.apology,
                    never_say: en.never_say,
                    start: s,
                    end: e,
                }
            })
            .collect()
    }

    /// Самый грубый уровень в тексте.
    pub fn swear_level(&self, text: &str) -> SwearLevel {
        self.hits(text).iter().map(|h| h.level).max().unwrap_or_default()
    }

    /// Слова из «никогда не говорить» (словарные `never_say` и `user.json → never_say`), найденные в тексте.
    pub fn never_say_hits(&self, text: &str) -> Vec<String> {
        let mut out: Vec<String> = self.hits(text).into_iter().filter(|h| h.never_say).map(|h| h.surface).collect();
        let toks = tokenize(text);
        out.extend(banned_in(&toks, &self.never_say_words));
        out.dedup();
        out
    }

    /// Есть ли с позиции `j` назначение посылания («нахуй», «в жопу», …): длина в словах и «слабое» ли оно.
    fn dest_at(&self, toks: &[String], j: usize) -> Option<(usize, bool)> {
        let mut key = String::new();
        for (len, t) in toks.iter().skip(j).enumerate().take(self.dest_max) {
            key.push_str(t);
            if self.dest.contains(&key) {
                return Some((len + 1, false));
            }
            if self.dest_weak.contains(&key) {
                return Some((len + 1, true));
            }
        }
        None
    }

    /// Посылание по схеме «глагол движения + куда»: «иди нахуй», «пошёл ты в жопу», «нахуй иди», «вали отсюда»,
    /// и «пошёл ты» / «иди ты» в конце фразы.
    fn go_dest(&self, toks: &[String], swear_at: &HashSet<usize>) -> bool {
        for (i, t) in toks.iter().enumerate() {
            if !self.go.contains(t) {
                continue;
            }
            let plural = self.go_plural.contains(t);
            for j in i + 1..toks.len().min(i + 4) {
                if let Some((_, weak)) = self.dest_at(toks, j) {
                    if !weak || !plural {
                        return true;
                    }
                }
            }
            // «нахуй иди», «в жопу пошёл»
            for j in i.saturating_sub(3)..i {
                if let Some((len, weak)) = self.dest_at(toks, j) {
                    if j + len == i && !weak {
                        return true;
                    }
                }
            }
            // «да пошёл ты», «иди ты, бля»
            if toks.get(i + 1).is_some_and(|n| n == "ты" || n == "вы") {
                let next = i + 2;
                if next == toks.len() || swear_at.contains(&next) || self.dest_at(toks, next).is_some() {
                    return true;
                }
            }
        }
        false
    }
}

/// Запрещённые слова/фразы, найденные в словах текста (слово — по началу при длине ≥ 4, фраза — подряд).
fn banned_in(toks: &[String], banned: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if banned.is_empty() {
        return out;
    }
    let joined = format!(" {} ", toks.join(" "));
    for b in banned {
        let hit = if b.contains(' ') {
            joined.contains(&format!(" {b} "))
        } else {
            toks.iter().any(|t| t == b || (b.chars().count() >= 4 && t.starts_with(b.as_str())))
        };
        if hit {
            out.push(b.clone());
        }
    }
    out
}

// ───────────────────────────── тон реплики ─────────────────────────────

/// Что словарь понял о реплике пользователя.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeechTone {
    pub swear_level: SwearLevel,
    /// Послал («пошёл нахуй», «отвали», «заткнись»).
    pub sendoff: bool,
    /// Оскорбил именно Джарвиса (best effort): «ты дебил», «тупая железяка», «пошёл ты».
    pub insult_to_jarvis: bool,
    pub praise: bool,
    pub apology: bool,
    /// Совпавшие слова и фразы (нормализованные), в порядке появления.
    pub matched: Vec<String>,
}

impl SpeechTone {
    /// Ничего примечательного.
    pub fn is_neutral(&self) -> bool {
        self.swear_level == SwearLevel::None && !self.sendoff && !self.insult_to_jarvis && !self.praise && !self.apology
    }

    /// Текст для служебной пометки хода модели (без скобок): «мат (заебал), посылание».
    /// `None`, если реплика нейтральная.
    pub fn note(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.swear_level > SwearLevel::None {
            let words: Vec<&str> = self.matched.iter().take(3).map(String::as_str).collect();
            if words.is_empty() {
                parts.push(self.swear_level.title_ru().to_string());
            } else {
                parts.push(format!("{} ({})", self.swear_level.title_ru(), words.join(", ")));
            }
        }
        if self.sendoff {
            parts.push("посылание".into());
        }
        if self.insult_to_jarvis && !self.sendoff {
            parts.push("оскорбление в твой адрес".into());
        }
        if self.praise {
            parts.push("похвала".into());
        }
        if self.apology {
            parts.push("извинение".into());
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// Разобрать реплику: уровень ругани, посылание, оскорбление, похвала, извинение. Быстро: для каждой фразы.
pub fn analyze(lex: &Lexicon, text: &str) -> SpeechTone {
    let toks = tokenize(text);
    let hits = lex.hits_tokens(&toks);
    let mut tone = SpeechTone::default();
    let negated = |h: &LexHit| h.start > 0 && toks[h.start - 1] == "не";
    let swear_at: HashSet<usize> =
        hits.iter().filter(|h| h.level >= SwearLevel::Rude).flat_map(|h| h.start..h.end).collect();
    let near_you = |h: &LexHit| {
        let from = h.start.saturating_sub(3);
        let to = (h.end + 3).min(toks.len());
        (from..to).any(|k| (k < h.start || k >= h.end) && lex.second_person.contains(&toks[k]))
    };
    let third = toks.iter().any(|t| lex.third_person.contains(t));
    for h in &hits {
        tone.swear_level = tone.swear_level.max(h.level);
        if h.sendoff {
            tone.sendoff = true;
        }
        if h.praise && !negated(h) {
            tone.praise = true;
        }
        if h.apology && !negated(h) {
            tone.apology = true;
        }
        if h.insult && !negated(h) && (near_you(h) || (toks.len() <= 2 && !third)) {
            tone.insult_to_jarvis = true;
        }
        if (h.level > SwearLevel::None || h.sendoff || h.insult) && !tone.matched.contains(&h.surface) {
            tone.matched.push(h.surface.clone());
        }
    }
    if !tone.sendoff && lex.go_dest(&toks, &swear_at) {
        tone.sendoff = true;
    }
    if tone.sendoff {
        tone.insult_to_jarvis = true;
    }
    tone
}

// ───────────────────────────── ситуации и шаблоны ─────────────────────────────

/// Ситуация банка фраз. Ключи совпадают с ключами `replies/*.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Situation {
    /// Отклик на голое «Джарвис».
    Wake,
    Greeting,
    Goodbye,
    /// Его послали.
    SendoffReply,
    /// Его оскорбили («ты дебил»).
    InsultReply,
    /// Пользователь ошибся / сказал фигню — подкол.
    UserMistakeTease,
    /// Джарвис сам ошибся — признаёт.
    SelfMistake,
    /// Пользователь кричит.
    Shouting,
    /// Пользователь повторяет команду.
    Repeat,
    PraiseReply,
    ApologyReply,
    /// Настроение «раздражён/обижен».
    Offended,
    /// Примирение.
    Makeup,
    LateNight,
    IdleLong,
    /// Подтверждение опасного действия: `{action}`, в конце — ясный вопрос «да или нет».
    ConfirmDangerous,
    /// Повторное подтверждение для защищённой системной папки (`{action}` — необязательно).
    ProtectedPath,
    Cancelled,
    Done,
    /// Ошибка модели (`{err}` — необязательно).
    ModelError,
    /// Модель не дала ответа.
    NoAnswer,
    /// Исчерпан лимит шагов (`{n}` — необязательно).
    LimitReached,
    /// Нет ключа к языковой модели.
    NoKey,
    /// Модель недоступна (`{problem}` — необязательно).
    ModelUnavailable,
    /// Память заполнена: `{pct}`, `{proc}` — необязательно.
    RamHigh,
    /// Процессор загружен: `{pct}`, `{proc}` — необязательно.
    CpuHigh,
    /// Мало места: `{disk}`, `{pct}` (свободно, %), `{free}` — необязательно (например «12 ГБ»).
    DiskLow,
    SoundTest,
    /// Джарвис объявляет, что перешёл в этот режим.
    ModeSwitched,
}

impl Situation {
    pub const ALL: [Situation; 29] = [
        Situation::Wake,
        Situation::Greeting,
        Situation::Goodbye,
        Situation::SendoffReply,
        Situation::InsultReply,
        Situation::UserMistakeTease,
        Situation::SelfMistake,
        Situation::Shouting,
        Situation::Repeat,
        Situation::PraiseReply,
        Situation::ApologyReply,
        Situation::Offended,
        Situation::Makeup,
        Situation::LateNight,
        Situation::IdleLong,
        Situation::ConfirmDangerous,
        Situation::ProtectedPath,
        Situation::Cancelled,
        Situation::Done,
        Situation::ModelError,
        Situation::NoAnswer,
        Situation::LimitReached,
        Situation::NoKey,
        Situation::ModelUnavailable,
        Situation::RamHigh,
        Situation::CpuHigh,
        Situation::DiskLow,
        Situation::SoundTest,
        Situation::ModeSwitched,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Situation::Wake => "wake",
            Situation::Greeting => "greeting",
            Situation::Goodbye => "goodbye",
            Situation::SendoffReply => "sendoff_reply",
            Situation::InsultReply => "insult_reply",
            Situation::UserMistakeTease => "user_mistake_tease",
            Situation::SelfMistake => "self_mistake",
            Situation::Shouting => "shouting",
            Situation::Repeat => "repeat",
            Situation::PraiseReply => "praise_reply",
            Situation::ApologyReply => "apology_reply",
            Situation::Offended => "offended",
            Situation::Makeup => "makeup",
            Situation::LateNight => "late_night",
            Situation::IdleLong => "idle_long",
            Situation::ConfirmDangerous => "confirm_dangerous",
            Situation::ProtectedPath => "protected_path",
            Situation::Cancelled => "cancelled",
            Situation::Done => "done",
            Situation::ModelError => "model_error",
            Situation::NoAnswer => "no_answer",
            Situation::LimitReached => "limit_reached",
            Situation::NoKey => "no_key",
            Situation::ModelUnavailable => "model_unavailable",
            Situation::RamHigh => "ram_high",
            Situation::CpuHigh => "cpu_high",
            Situation::DiskLow => "disk_low",
            Situation::SoundTest => "sound_test",
            Situation::ModeSwitched => "mode_switched",
        }
    }

    pub fn from_key(key: &str) -> Option<Situation> {
        Situation::ALL.into_iter().find(|s| s.key() == key)
    }

    /// Подстановки, которые обязаны быть в каждой фразе ситуации.
    pub fn required(self) -> &'static [&'static str] {
        match self {
            Situation::ConfirmDangerous => &["action"],
            Situation::RamHigh | Situation::CpuHigh => &["pct"],
            Situation::DiskLow => &["disk", "pct"],
            _ => &[],
        }
    }

    /// Подстановки, которые фраза может использовать (фраза выбирается, только если значение передано).
    pub fn optional(self) -> &'static [&'static str] {
        match self {
            Situation::RamHigh | Situation::CpuHigh => &["proc"],
            Situation::DiskLow => &["free"],
            Situation::ModelError => &["err"],
            Situation::LimitReached => &["n"],
            Situation::ModelUnavailable => &["problem"],
            Situation::ProtectedPath => &["action"],
            _ => &[],
        }
    }

    /// Допустима ли подстановка `{name}` в этой ситуации (`address` — везде).
    pub fn allows(self, name: &str) -> bool {
        name == "address" || self.required().contains(&name) || self.optional().contains(&name)
    }
}

/// Имена подстановок `{name}` в шаблоне (латиница, цифры, `_`).
pub fn placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(a) = rest.find('{') {
        let after = &rest[a + 1..];
        match after.find('}') {
            Some(b) if b > 0 && after[..b].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                let name = after[..b].to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
                rest = &after[b + 1..];
            }
            _ => rest = after,
        }
    }
    out
}

/// Значения подстановок и условия выбора фразы.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vars {
    /// Обращение («сэр», имя); `None` — без обращения (запятые вокруг аккуратно убираются).
    pub address: Option<String>,
    /// Можно ли фразы с матом и жёсткие подколки (режим «Без цензуры» + [`MirrorState::may_swear`]).
    pub profanity: bool,
    values: Vec<(String, String)>,
}

impl Vars {
    pub fn new() -> Self {
        Self::default()
    }

    /// Обращение: обычно `PersonaConfig::address()`.
    pub fn address(mut self, a: Option<&str>) -> Self {
        self.address = a.map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
        self
    }

    pub fn profanity(mut self, on: bool) -> Self {
        self.profanity = on;
        self
    }

    /// Значение подстановки `{key}` (`action`, `err`, `pct`, `proc`, `disk`, `free`, `n`, `problem`…).
    pub fn set(mut self, key: &str, value: impl Into<String>) -> Self {
        let value = value.into();
        match self.values.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.values.push((key.to_string(), value)),
        }
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        if key == "address" {
            return self.address.as_deref();
        }
        self.values.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Начало предложения: начало строки или после `. ! ? …` и пробела.
fn sentence_start(s: &str, idx: usize) -> bool {
    let before = s[..idx].trim_end();
    before.is_empty() || before.ends_with(['.', '!', '?', '…'])
}

/// Подставить значения в шаблон. `{address}` без значения убирается вместе с запятой
/// («Слушаю, {address}.» → «Слушаю.», «{address}, удалить?» → «Удалить?»); неизвестные подстановки
/// без значения заменяются на «…». Первая буква предложения — заглавная.
pub fn render(template: &str, vars: &Vars) -> String {
    let mut s = template.to_string();
    const A: &str = "{address}";
    match vars.address.as_deref() {
        Some(a) => {
            while let Some(i) = s.find(A) {
                let v = if sentence_start(&s, i) { upper_first(a) } else { a.to_string() };
                s.replace_range(i..i + A.len(), &v);
            }
        }
        None => {
            s = s.replace(&format!(", {A},"), ",").replace(&format!(", {A}"), "");
            while let Some(i) = s.find(&format!("{A}, ")) {
                s.replace_range(i..i + A.len() + 2, "");
                if sentence_start(&s, i) {
                    let rest = upper_first(&s[i..]);
                    s.replace_range(i.., &rest);
                }
            }
            s = s.replace(&format!(" {A}"), "").replace(&format!("{A} "), "").replace(A, "");
        }
    }
    for name in placeholders(&s) {
        let ph = format!("{{{name}}}");
        let v = vars.get(&name).unwrap_or("…").to_string();
        s = s.replace(&ph, &v);
    }
    let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    for (a, b) in [(" ,", ","), (" .", "."), (" !", "!"), (" ?", "?"), (",.", "."), (",!", "!"), (",?", "?"), (",,", ",")] {
        s = s.replace(a, b);
    }
    upper_first(s.trim())
}

// ───────────────────────────── выбор фраз ─────────────────────────────

/// Состояние выбора: генератор случайных чисел и последние фразы по ситуациям (чтобы не повторяться подряд).
#[derive(Debug, Clone)]
pub struct PickState {
    rng: u64,
    memory: usize,
    recent: HashMap<(PersonaMode, Situation), VecDeque<u64>>,
}

impl Default for PickState {
    fn default() -> Self {
        Self::new()
    }
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl PickState {
    /// Случайное зерно от часов; помнит 4 последние фразы каждой ситуации.
    pub fn new() -> Self {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Self::with_seed(t ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// Детерминированный выбор (тесты).
    pub fn with_seed(seed: u64) -> Self {
        Self { rng: seed.max(1), memory: 4, recent: HashMap::new() }
    }

    /// Сколько последних фраз ситуации не повторять (если вариантов хватает).
    pub fn with_memory(mut self, n: usize) -> Self {
        self.memory = n.max(1);
        self
    }

    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Фраза банка (после загрузки и проверки словарём).
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyLine {
    pub text: String,
    /// Грубость фразы по словарю.
    pub level: SwearLevel,
    /// Жёсткая подколка (только «Без цензуры» и только при разрешённом мате).
    pub harsh: bool,
    pub source: String,
    placeholders: Vec<String>,
}

impl ReplyLine {
    /// Нужен ли разрешённый мат, чтобы сказать эту фразу.
    pub fn needs_profanity(&self) -> bool {
        self.level == SwearLevel::Mat || self.harsh
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RawReplies {
    #[allow(dead_code)]
    mode: String,
    situations: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawLjLine {
    text: String,
    #[serde(default)]
    harsh: bool,
    #[serde(default)]
    modes: Vec<String>,
    #[serde(default)]
    situations: Vec<String>,
    #[serde(default = "yes")]
    usable: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
struct RawLjFile {
    lines: Vec<RawLjLine>,
}

fn mode_from_str(s: &str) -> Option<PersonaMode> {
    PersonaMode::ALL.into_iter().find(|m| m.as_str() == s)
}

/// Банк готовых фраз по режимам и ситуациям.
#[derive(Debug, Clone, Default)]
pub struct Replies {
    lines: HashMap<(PersonaMode, Situation), Vec<ReplyLine>>,
    fallback: HashMap<(PersonaMode, Situation), Vec<ReplyLine>>,
    banned: Vec<String>,
    never_say: Vec<String>,
    warnings: Vec<String>,
}

impl Replies {
    /// `dir` (или `dir/replies`): `butler.json`, `buddy.json`, `uncensored.json` и необязательный `user.json`.
    /// Словарь нужен, чтобы оценить грубость каждой фразы и выбросить фразы со словами `never_say`.
    /// Встроенные минимальные фразы подключаются как запасные для пустых ситуаций.
    pub fn load(dir: &Path, lex: &Lexicon) -> Result<Replies> {
        let dir = subdir(dir, "replies");
        let mut r = Replies::builtin_minimal(lex);
        for mode in PersonaMode::ALL {
            let p = dir.join(format!("{}.json", mode.as_str()));
            if !p.exists() {
                r.warnings.push(format!("нет файла {}", p.display()));
                continue;
            }
            let raw: RawReplies = read_json(&p)?;
            for (key, list) in raw.situations {
                let Some(sit) = Situation::from_key(&key) else {
                    r.warnings.push(format!("{}: неизвестная ситуация «{key}»", p.display()));
                    continue;
                };
                for t in list {
                    r.add_line(lex, mode, sit, &t, false, mode.as_str());
                }
            }
        }
        let user = dir.join("user.json");
        if user.exists() {
            let raw: HashMap<String, HashMap<String, Vec<String>>> = read_json(&user)?;
            for (m, sits) in raw {
                let Some(mode) = mode_from_str(&m) else { continue };
                for (key, list) in sits {
                    if let Some(sit) = Situation::from_key(&key) {
                        for t in list {
                            r.add_line(lex, mode, sit, &t, false, "user");
                        }
                    }
                }
            }
        }
        Ok(r)
    }

    /// Только встроенные фразы (по одной-две на ситуацию; дворецкий — как раньше в коде).
    pub fn builtin_minimal(lex: &Lexicon) -> Replies {
        let mut r = Replies { never_say: lex.never_say_words().to_vec(), ..Default::default() };
        for &(sit, butler, casual) in BUILTIN_REPLIES {
            for mode in PersonaMode::ALL {
                let text = if mode == PersonaMode::Butler { butler } else { casual };
                if let Some(line) = r.make_line(lex, sit, text, false, "builtin") {
                    r.fallback.entry((mode, sit)).or_default().push(line);
                }
            }
        }
        r
    }

    /// Подключить локальный файл подколок (`data/persona/replies-lj.json`, формат `lexicon-build`).
    /// Возвращает число добавленных фраз. Непригодные (`usable: false`) и с `never_say` пропускаются.
    pub fn add_local_file(&mut self, path: &Path, lex: &Lexicon) -> Result<usize> {
        let raw: RawLjFile = read_json(path)?;
        let mut n = 0;
        for l in raw.lines.into_iter().filter(|l| l.usable) {
            for m in &l.modes {
                let Some(mode) = mode_from_str(m) else { continue };
                // Жёсткие — только в «Без цензуры».
                if l.harsh && mode != PersonaMode::Uncensored {
                    continue;
                }
                for s in &l.situations {
                    if let Some(sit) = Situation::from_key(s) {
                        n += usize::from(self.add_line(lex, mode, sit, &l.text, l.harsh, "local"));
                    }
                }
            }
        }
        Ok(n)
    }

    fn make_line(&mut self, lex: &Lexicon, sit: Situation, text: &str, harsh: bool, source: &str) -> Option<ReplyLine> {
        let mut text = text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        if text.contains('*') {
            self.warnings.push(format!("{source}/{}: звёздочка в «{text}» убрана", sit.key()));
            text = text.replace('*', "");
        }
        let bad = lex.never_say_hits(&text);
        if !bad.is_empty() {
            self.warnings.push(format!("{source}/{}: «{text}» содержит never_say ({}) — пропущено", sit.key(), bad.join(", ")));
            return None;
        }
        let level = lex.swear_level(&text);
        let placeholders = placeholders(&text);
        if let Some(p) = placeholders.iter().find(|p| !sit.allows(p)) {
            self.warnings.push(format!("{source}/{}: недопустимая подстановка {{{p}}} в «{text}»", sit.key()));
        }
        Some(ReplyLine { text, level, harsh, source: source.to_string(), placeholders })
    }

    fn add_line(&mut self, lex: &Lexicon, mode: PersonaMode, sit: Situation, text: &str, harsh: bool, source: &str) -> bool {
        match self.make_line(lex, sit, text, harsh, source) {
            Some(l) => {
                let v = self.lines.entry((mode, sit)).or_default();
                if v.iter().any(|x| x.text == l.text) {
                    return false;
                }
                v.push(l);
                true
            }
            None => false,
        }
    }

    /// Слова, которые Джарвис не говорит никогда (`PersonaConfig::banned_words`).
    pub fn set_banned(&mut self, words: &[String]) {
        self.banned = words.iter().map(|w| normalize(w)).filter(|w| !w.is_empty()).collect();
    }

    /// Предупреждения загрузки (пропущенные фразы, неизвестные ключи).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Фразы ситуации из файлов (без встроенных запасных).
    pub fn lines(&self, mode: PersonaMode, situation: Situation) -> &[ReplyLine] {
        self.lines.get(&(mode, situation)).map(Vec::as_slice).unwrap_or(&[])
    }

    fn is_banned(&self, rendered: &str) -> bool {
        let toks = tokenize(rendered);
        !banned_in(&toks, &self.banned).is_empty() || !banned_in(&toks, &self.never_say).is_empty()
    }

    /// Подходящие фразы с подстановками: из файлов, а если их нет — встроенные.
    fn candidates(&self, mode: PersonaMode, situation: Situation, vars: &Vars, need_values: bool) -> Vec<String> {
        let ok = |l: &&ReplyLine| {
            (vars.profanity || !l.needs_profanity())
                && (!need_values || l.placeholders.iter().all(|p| p == "address" || vars.get(p).is_some()))
        };
        for src in [&self.lines, &self.fallback] {
            let list: Vec<String> = src
                .get(&(mode, situation))
                .into_iter()
                .flatten()
                .filter(ok)
                .map(|l| render(&l.text, vars))
                .filter(|t| !self.is_banned(t))
                .collect();
            if !list.is_empty() {
                return list;
            }
        }
        Vec::new()
    }

    /// Случайная фраза ситуации без повторов подряд, с подстановками. Фразы с матом и жёсткие — только
    /// при `vars.profanity`; фразы с подстановкой, значения которой нет в `vars`, не выбираются.
    /// `None` — подходящих фраз нет совсем (даже встроенных).
    pub fn pick(&self, mode: PersonaMode, situation: Situation, st: &mut PickState, vars: &Vars) -> Option<String> {
        let list = self.candidates(mode, situation, vars, true);
        if list.is_empty() {
            return None;
        }
        let memory = st.memory;
        let recent = st.recent.entry((mode, situation)).or_default().clone();
        let mut fresh: Vec<usize> = (0..list.len()).filter(|&i| !recent.contains(&fnv(&list[i]))).collect();
        if fresh.is_empty() {
            let last = recent.back().copied();
            fresh = (0..list.len()).filter(|&i| Some(fnv(&list[i])) != last || list.len() == 1).collect();
        }
        let i = fresh[st.below(fresh.len())];
        let q = st.recent.entry((mode, situation)).or_default();
        q.push_back(fnv(&list[i]));
        while q.len() > memory {
            q.pop_front();
        }
        Some(list[i].clone())
    }

    /// До `n` разных случайных фраз ситуации — «примеры стиля» для модели. Подстановки без значения
    /// заменяются на «…». Историю повторов не трогает.
    pub fn samples(&self, mode: PersonaMode, situation: Situation, n: usize, st: &mut PickState, vars: &Vars) -> Vec<String> {
        let mut list = self.candidates(mode, situation, vars, false);
        list.dedup();
        // Фишер — Йейтс на первых n.
        let n = n.min(list.len());
        for i in 0..n {
            let j = i + st.below(list.len() - i);
            list.swap(i, j);
        }
        list.truncate(n);
        list
    }
}

/// Словарь и банк фраз вместе.
#[derive(Debug, Clone)]
pub struct Persona {
    pub lexicon: Lexicon,
    pub replies: Replies,
}

impl Persona {
    /// `persona_dir` — `config/persona`; `local_dir` — `data/persona` (необязательные локальные подколки
    /// `replies-lj.json`). Ошибка локального файла не роняет загрузку — уходит в `replies.warnings()`.
    pub fn load(persona_dir: &Path, local_dir: Option<&Path>) -> Result<Persona> {
        let lexicon = Lexicon::load(persona_dir)?;
        let mut replies = Replies::load(persona_dir, &lexicon)?;
        if let Some(d) = local_dir {
            let p = d.join("replies-lj.json");
            if p.exists() {
                if let Err(e) = replies.add_local_file(&p, &lexicon) {
                    replies.warnings.push(e.to_string());
                }
            }
        }
        Ok(Persona { lexicon, replies })
    }

    /// Встроенный минимум (нет `config/persona`).
    pub fn builtin() -> Persona {
        let lexicon = Lexicon::builtin_minimal();
        let replies = Replies::builtin_minimal(&lexicon);
        Persona { lexicon, replies }
    }
}

// ───────────────────────────── настроение ─────────────────────────────

/// Настроение для HUD и пометки модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoodLabel {
    Calm,
    Irritated,
    Offended,
}

impl MoodLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            MoodLabel::Calm => "calm",
            MoodLabel::Irritated => "irritated",
            MoodLabel::Offended => "offended",
        }
    }

    pub fn title_ru(self) -> &'static str {
        match self {
            MoodLabel::Calm => "спокоен",
            MoodLabel::Irritated => "раздражён",
            MoodLabel::Offended => "обижен",
        }
    }
}

/// Порог «раздражён».
pub const MOOD_IRRITATED: f32 = 0.3;
/// Порог «обижен».
pub const MOOD_OFFENDED: f32 = 0.6;

/// Раздражение Джарвиса 0…1: растёт от посыланий, оскорблений и крика, падает от похвалы и извинений,
/// само затухает экспоненциально (полураспад ~10 минут).
#[derive(Debug, Clone)]
pub struct Mood {
    irritation: f32,
    at: Option<Instant>,
    half_life: Duration,
    window: Duration,
    sendoffs: VecDeque<Instant>,
    insults: VecDeque<Instant>,
    shouts: VecDeque<Instant>,
}

impl Default for Mood {
    fn default() -> Self {
        Self::new()
    }
}

fn count_ru(n: usize, one: &str, many: &str) -> String {
    match n {
        0 | 1 => one.to_string(),
        2 => format!("дважды {many}"),
        3 => format!("трижды {many}"),
        n => format!("{many} {n} раз"),
    }
}

impl Mood {
    pub fn new() -> Self {
        Self::with_half_life(Duration::from_secs(600))
    }

    /// Свой полураспад; окно подсчёта причин («дважды послали») равно ему же.
    pub fn with_half_life(half_life: Duration) -> Self {
        Self {
            irritation: 0.0,
            at: None,
            half_life,
            window: half_life,
            sendoffs: VecDeque::new(),
            insults: VecDeque::new(),
            shouts: VecDeque::new(),
        }
    }

    fn decay(&mut self, now: Instant) {
        if let Some(t) = self.at {
            let dt = now.saturating_duration_since(t).as_secs_f32();
            self.irritation *= 0.5f32.powf(dt / self.half_life.as_secs_f32().max(1.0));
        }
        self.at = Some(now);
        for q in [&mut self.sendoffs, &mut self.insults, &mut self.shouts] {
            while q.front().is_some_and(|t| now.saturating_duration_since(*t) > self.window) {
                q.pop_front();
            }
        }
    }

    fn add(&mut self, delta: f32, now: Instant) {
        self.decay(now);
        self.irritation = (self.irritation + delta).clamp(0.0, 1.0);
    }

    /// Учесть реплику пользователя. `loud` — голос заметно громче обычного (крик).
    /// Возвращает раздражение после изменения.
    pub fn on_tone_at(&mut self, tone: &SpeechTone, loud: bool, now: Instant) -> f32 {
        let mut d = 0.0;
        if tone.sendoff {
            d += 0.35;
            self.sendoffs.push_back(now);
        } else if tone.insult_to_jarvis {
            d += 0.25;
            self.insults.push_back(now);
        } else {
            d += match tone.swear_level {
                SwearLevel::Mat => 0.04,
                SwearLevel::Rude => 0.03,
                _ => 0.0,
            };
        }
        if loud {
            d += 0.12;
            self.shouts.push_back(now);
        }
        if tone.praise {
            d -= 0.25;
        }
        if tone.apology {
            d -= 0.5;
        }
        self.add(d, now);
        self.irritation
    }

    pub fn on_tone(&mut self, tone: &SpeechTone, loud: bool) -> f32 {
        self.on_tone_at(tone, loud, Instant::now())
    }

    pub fn on_praise_at(&mut self, now: Instant) {
        self.add(-0.25, now);
    }

    pub fn on_praise(&mut self) {
        self.on_praise_at(Instant::now());
    }

    /// Извинение: раздражение сильно падает, счётчик посыланий обнуляется («проехали»).
    pub fn on_apology_at(&mut self, now: Instant) {
        self.add(-0.5, now);
        self.sendoffs.clear();
        self.insults.clear();
    }

    pub fn on_apology(&mut self) {
        self.on_apology_at(Instant::now());
    }

    /// Раздражение на момент `now` (с учётом затухания), 0…1.
    pub fn irritation_at(&self, now: Instant) -> f32 {
        match self.at {
            Some(t) => {
                let dt = now.saturating_duration_since(t).as_secs_f32();
                self.irritation * 0.5f32.powf(dt / self.half_life.as_secs_f32().max(1.0))
            }
            None => self.irritation,
        }
    }

    pub fn irritation(&self) -> f32 {
        self.irritation_at(Instant::now())
    }

    pub fn label_at(&self, now: Instant) -> MoodLabel {
        let i = self.irritation_at(now);
        if i >= MOOD_OFFENDED {
            MoodLabel::Offended
        } else if i >= MOOD_IRRITATED {
            MoodLabel::Irritated
        } else {
            MoodLabel::Calm
        }
    }

    pub fn label(&self) -> MoodLabel {
        self.label_at(Instant::now())
    }

    fn recent(q: &VecDeque<Instant>, now: Instant, window: Duration) -> usize {
        q.iter().filter(|t| now.saturating_duration_since(**t) <= window).count()
    }

    /// Сколько раз послали за последнее окно (~10 минут).
    pub fn sendoffs_at(&self, now: Instant) -> usize {
        Self::recent(&self.sendoffs, now, self.window)
    }

    /// Причина настроения по-русски: «его дважды послали», «на него кричали»…
    pub fn reason_at(&self, now: Instant) -> Option<String> {
        let s = self.sendoffs_at(now);
        let i = Self::recent(&self.insults, now, self.window);
        let l = Self::recent(&self.shouts, now, self.window);
        if s > 0 {
            Some(format!("его {}", count_ru(s, "послали", "послали")))
        } else if i > 0 {
            Some(format!("его {}", count_ru(i, "оскорбили", "оскорбили")))
        } else if l > 0 {
            Some(if l == 1 { "на него накричали".into() } else { "на него несколько раз кричали".into() })
        } else {
            None
        }
    }

    /// Текст пометки для модели: «обижен — его дважды послали». `None`, когда спокоен.
    pub fn note_at(&self, now: Instant) -> Option<String> {
        let label = self.label_at(now);
        if label == MoodLabel::Calm {
            return None;
        }
        Some(match self.reason_at(now) {
            Some(r) => format!("{} — {r}", label.title_ru()),
            None => format!("{} — ему грубили", label.title_ru()),
        })
    }

    pub fn note(&self) -> Option<String> {
        self.note_at(Instant::now())
    }

    pub fn reset(&mut self) {
        *self = Self::with_half_life(self.half_life);
    }
}

// ───────────────────────────── зеркало мата ─────────────────────────────

/// «Зеркало» режима «Без цензуры»: Джарвис матерится, только если пользователь сам матерился недавно
/// (в последних `turns` репликах и не дольше `window` назад) или послал его.
#[derive(Debug, Clone)]
pub struct MirrorState {
    turns: VecDeque<(Instant, bool)>,
    max_turns: usize,
    window: Duration,
}

impl Default for MirrorState {
    fn default() -> Self {
        Self::new()
    }
}

impl MirrorState {
    /// 6 реплик / 15 минут.
    pub fn new() -> Self {
        Self::with_limits(6, Duration::from_secs(15 * 60))
    }

    pub fn with_limits(turns: usize, window: Duration) -> Self {
        Self { turns: VecDeque::new(), max_turns: turns.max(1), window }
    }

    /// Учесть реплику пользователя.
    pub fn on_user_at(&mut self, tone: &SpeechTone, now: Instant) {
        let swore = tone.swear_level == SwearLevel::Mat || tone.sendoff;
        self.turns.push_back((now, swore));
        while self.turns.len() > self.max_turns {
            self.turns.pop_front();
        }
    }

    pub fn on_user(&mut self, tone: &SpeechTone) {
        self.on_user_at(tone, Instant::now());
    }

    /// Можно ли сейчас мат в ответах: только «Без цензуры» и пользователь недавно матерился/послал.
    pub fn may_swear_at(&self, mode: PersonaMode, now: Instant) -> bool {
        mode.allows_profanity()
            && self.turns.iter().any(|(t, swore)| *swore && now.saturating_duration_since(*t) <= self.window)
    }

    pub fn may_swear(&self, mode: PersonaMode) -> bool {
        self.may_swear_at(mode, Instant::now())
    }

    pub fn reset(&mut self) {
        self.turns.clear();
    }
}

// ───────────────────────────── встроенный минимум ─────────────────────────────

const BUILTIN_LEXICON: &str = r#"{
  "safe": ["хлеб", "ребен", "ребят", "учеб", "небо", "тебе", "тебя", "себе", "себя", "колеба", "колебл", "требов", "употребл", "оскорбл",
           "погреб", "глеб", "рубля", "корабл", "сабл", "мудр", "психу", "страху", "скипидар", "команд", "мандар", "мандат", "спидр", "педикюр", "посрам"],
  "second_person": ["ты", "тебя", "тебе", "тобой", "вы", "вас", "джарвис", "железяка", "машина", "бот"],
  "third_person": ["он", "она", "они", "его", "ее", "их"],
  "sendoff": {
    "go": ["иди", "идите", "пошел", "пошла", "пошли", "вали", "катись", "проваливай"],
    "go_plural": ["пошли", "идите"],
    "dest": ["нахуй", "на хуй", "нах", "нахер", "на хер", "нахрен", "нафиг", "в жопу", "в пизду", "к черту", "лесом"],
    "dest_weak": ["вон", "отсюда"]
  },
  "entries": [
    { "text": "хуй", "level": "mat", "contains": ["хуй", "хуе", "хуя", "хуи", "хую"], "forms": ["нах", "хули"], "exclude": ["психу", "страху"] },
    { "text": "пизд", "level": "mat", "contains": ["пизд", "пизж"] },
    { "text": "еб", "level": "mat", "prefix": ["еб"], "contains": ["заеб", "наеб", "уеб", "выеб", "доеб", "ъеб", "проеб", "поеб", "приеб", "долбоеб"] },
    { "text": "бля", "level": "mat", "prefix": ["бля"], "contains": ["бляд"], "forms": ["бл"], "exclude": ["блях", "бляш"] },
    { "text": "муд", "level": "mat", "insult": true, "contains": ["мудак", "мудил", "мудач"] },
    { "text": "сука", "level": "mat", "forms": ["сука", "суки", "суку", "сукой", "сучка", "сучара"] },
    { "text": "говн", "level": "mat", "contains": ["говн"] },
    { "text": "жоп", "level": "mat", "contains": ["жоп"] },
    { "text": "пидор", "level": "mat", "insult": true, "never_say": true, "contains": ["пидор", "пидар", "пидр"], "exclude": ["спидр", "скипидар"] },
    { "text": "дебил", "level": "rude", "insult": true, "prefix": ["дебил", "идиот", "кретин", "придур"], "forms": ["тупой", "тупая", "лох", "урод"] },
    { "text": "блин", "level": "soft", "forms": ["блин", "капец", "жесть", "черт", "офигеть", "елки"] },
    { "text": "отвали", "level": "rude", "sendoff": true, "forms": ["отвали", "отстань", "заткнись", "отъебись", "иди ты", "пошел ты"] },
    { "text": "похвала", "praise": true, "forms": ["молодец", "красава", "умница", "спасибо", "спс", "ты лучший"] },
    { "text": "извинение", "apology": true, "forms": ["извини", "прости", "сорри", "не обижайся", "был не прав"] }
  ]
}"#;

/// (ситуация, дворецкий, «свой парень»/«без цензуры»).
const BUILTIN_REPLIES: &[(Situation, &str, &str)] = &[
    (Situation::Wake, "Слушаю, {address}.", "Да, слушаю."),
    (Situation::Greeting, "Здравствуйте, {address}.", "Привет!"),
    (Situation::Goodbye, "До свидания, {address}.", "Давай, до связи."),
    (Situation::SendoffReply, "Как скажете, {address}. Я всё равно на месте.", "Грубо. Но я всё равно сделаю."),
    (Situation::InsultReply, "Принято к сведению, {address}.", "Обидно, вообще-то."),
    (Situation::UserMistakeTease, "Смелое решение, {address}.", "Смело. Неправильно, но смело."),
    (Situation::SelfMistake, "Прошу прощения, {address}, моя ошибка. Исправляю.", "Ок, косяк мой, переделываю."),
    (Situation::Shouting, "Я вас прекрасно слышу, {address}.", "Тише, тише, я слышу."),
    (Situation::Repeat, "Уже выполняю, {address}.", "Делаю, делаю."),
    (Situation::PraiseReply, "Благодарю, {address}.", "Спасибо, стараюсь."),
    (Situation::ApologyReply, "Всё в порядке, {address}.", "Да ладно, проехали."),
    (Situation::Offended, "Как вам угодно, {address}.", "Ну-ну."),
    (Situation::Makeup, "Забыто, {address}.", "Ок, мир."),
    (Situation::LateNight, "{address}, уже поздно. Может быть, пора отдохнуть?", "Ночь на дворе, может, спать?"),
    (Situation::IdleLong, "{address}, я здесь, если понадоблюсь.", "Давно не болтали."),
    (Situation::ConfirmDangerous, "{address}, {action}? Скажите «да» или «нет».", "{action}, точно? Да или нет?"),
    (Situation::ProtectedPath, "{address}, это защищённая системная папка. Подтвердите ещё раз: да или нет?", "Это системная папка. Точно? Да или нет?"),
    (Situation::Cancelled, "Остановлено, {address}.", "Ок, стоп."),
    (Situation::Done, "Готово, {address}.", "Готово."),
    (Situation::ModelError, "Прошу прощения, {address}: {err}.", "Что-то сломалось: {err}."),
    (Situation::ModelError, "Прошу прощения, {address}, произошла ошибка.", "Что-то сломалось, попробуй ещё раз."),
    (Situation::NoAnswer, "Прошу прощения, {address}, модель не дала ответа.", "Модель промолчала, попробуй ещё раз."),
    (Situation::LimitReached, "{address}, я остановился: исчерпан лимит шагов, задача не завершена.", "Упёрся в лимит шагов, задача не доделана."),
    (Situation::NoKey, "{address}, у меня нет ключа к языковой модели. Добавьте его в настройках.", "Нет ключа к модели, добавь его в настройках."),
    (Situation::ModelUnavailable, "{address}, языковая модель недоступна.", "Модель сейчас недоступна."),
    (Situation::RamHigh, "{address}, оперативная память заполнена на {pct} %.", "Память забита на {pct} %."),
    (Situation::CpuHigh, "{address}, процессор загружен на {pct} % уже больше минуты.", "Процессор загружен на {pct} %."),
    (Situation::DiskLow, "{address}, на диске {disk} осталось всего {pct} % свободного места.", "На диске {disk} осталось {pct} % места."),
    (Situation::SoundTest, "Проверка звука. Вы меня хорошо слышите, {address}?", "Проверка звука. Слышно меня?"),
    (Situation::ModeSwitched, "Слушаюсь, {address}. Перехожу на прежний вежливый тон.", "Окей, теперь по-простому."),
];

#[cfg(test)]
mod tests;
