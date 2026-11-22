//! Разбор свободного запроса (англ./рус.) в выражения FTS5.
//!
//! Индекс токенизирован `unicode61 remove_diacritics 2` без стемминга, поэтому стемминг делаем на стороне запроса:
//! слово → основа Snowball (англ. или рус.) → префиксный терм `"основа"*` (`disabling` → `"disabl"*` найдёт
//! disable/disabled). Командлеты `Verb-Noun` становятся фразой `"get service"` (в индексе дефис — разделитель).

use rust_stemmers::{Algorithm, Stemmer};

use crate::commands;

const STOP_EN: &[&str] = &[
    "a", "about", "after", "all", "also", "am", "an", "and", "any", "are", "as", "at", "be", "been", "being", "but",
    "by", "can", "cannot", "could", "do", "does", "doing", "done", "for", "from", "get", "gets", "getting", "had",
    "has", "have", "how", "i", "if", "in", "into", "is", "it", "its", "just", "me", "my", "need", "of", "on", "or",
    "our", "please", "should", "so", "some", "than", "that", "the", "their", "them", "then", "there", "these",
    "they", "this", "those", "to", "too", "use", "used", "using", "via", "want", "was", "way", "ways", "we", "were",
    "what", "when", "where", "which", "while", "who", "why", "will", "with", "would", "you", "your",
];

const STOP_RU: &[&str] = &[
    "а", "без", "более", "бы", "был", "была", "были", "было", "быть", "в", "вам", "вас", "весь", "во", "вот",
    "все", "всё", "всех", "вы", "где", "да", "даже", "для", "до", "его", "ее", "её", "если", "есть", "еще", "ещё",
    "же", "за", "здесь", "и", "из", "или", "им", "их", "к", "как", "какой", "какая", "какие", "когда", "кто", "ли",
    "либо", "мне", "меня", "мой", "моя", "мои", "можно", "мы", "на", "надо", "над", "не", "нет", "ни", "но",
    "нужно", "о", "об", "он", "она", "они", "оно", "от", "по", "под", "почему", "при", "про", "с", "сделать", "со",
    "так", "также", "там", "то", "тот", "у", "уже", "хочу", "чем", "через", "что", "чтобы", "это", "этот", "эта",
    "эти", "я",
];

/// Частые английские слова, совпадающие с именами команд Windows (`start`, `set`, `type`, `find`…). Для них
/// буст «точная команда» из запроса не применяется — иначе «start menu» поднимет страницу команды `start`.
const COMMON_WORDS: &[&str] = &[
    "add", "assign", "attach", "attributes", "break", "call", "change", "clean", "color", "compact", "convert",
    "copy", "create", "date", "delete", "detach", "echo", "edit", "erase", "exit", "expand", "export", "extend",
    "find", "format", "help", "import", "label", "list", "manage", "mode", "more", "move", "offline", "online",
    "open", "path", "pause", "print", "query", "recover", "remove", "rename", "repair", "replace", "reset",
    "select", "set", "shift", "shrink", "sort", "start", "time", "title", "tree", "type", "verify", "where",
    "windows", "user", "users", "file", "files", "service", "services", "network", "disk", "power", "driver",
    "update", "install", "uninstall", "enable", "disable", "show", "test", "stop", "restart", "error", "event",
    "task", "tasks", "process", "registry",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// Нормализованное слово (нижний регистр) — для сниппета.
    pub word: String,
    /// Основа для подсветки/сниппета (то, что ищется префиксом).
    pub stem: String,
    /// Готовый элемент выражения FTS5: `"stem"*`, `"word"` или фраза `"get service"`.
    pub fts: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedQuery {
    pub terms: Vec<Term>,
    /// Явные команды: `Verb-Noun`, `x.exe`, слово перед ключом (`sfc /scannow`).
    pub strong_commands: Vec<String>,
    /// Прочие слова, которые могут оказаться именем команды (`robocopy`), кроме частых английских.
    pub weak_commands: Vec<String>,
}

impl ParsedQuery {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.strong_commands.is_empty() && self.weak_commands.is_empty()
    }
    /// `t1 OR t2 …` (или `None`, если термов нет).
    pub fn or_expr(&self) -> Option<String> {
        (!self.terms.is_empty()).then(|| self.terms.iter().map(|t| t.fts.as_str()).collect::<Vec<_>>().join(" OR "))
    }
    /// `t1 AND t2 …` — только если термов ≥ 2 (иначе совпадает с OR).
    pub fn and_expr(&self) -> Option<String> {
        (self.terms.len() >= 2).then(|| self.terms.iter().map(|t| t.fts.as_str()).collect::<Vec<_>>().join(" AND "))
    }
}

fn is_cyrillic(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\u{0400}'..='\u{04FF}'))
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Слова запроса так же, как их режет `unicode61`: всё, что не буква/цифра, — разделитель.
pub fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

fn stem_word(w: &str) -> String {
    let alg = if is_cyrillic(w) { Algorithm::Russian } else { Algorithm::English };
    Stemmer::create(alg).stem(w).into_owned()
}

fn make_term(w: &str) -> Term {
    let alpha = w.chars().all(char::is_alphabetic);
    let min = if is_cyrillic(w) { 4 } else { 3 };
    if alpha && w.chars().count() >= 4 {
        let st = stem_word(w);
        if st.chars().count() >= min && w.starts_with(st.as_str()) {
            return Term { word: w.to_string(), fts: format!("{}*", quote(&st)), stem: st };
        }
        // directory → directori: основа не префикс слова — берём общий префикс («director»*), если он не короткий
        let common: String = w.chars().zip(st.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect();
        if common.chars().count() >= 5 {
            return Term { word: w.to_string(), fts: format!("{}*", quote(&common)), stem: common };
        }
    }
    Term { word: w.to_string(), stem: w.to_string(), fts: quote(w) }
}

pub fn parse(q: &str) -> ParsedQuery {
    let mut out = ParsedQuery::default();
    let mut seen_fts: Vec<String> = Vec::new();
    let mut push_term = |t: Term, out: &mut ParsedQuery| {
        if !seen_fts.contains(&t.fts) {
            seen_fts.push(t.fts.clone());
            out.terms.push(t);
        }
    };

    // 1. Командлеты целиком (фраза) и явные exe.
    let raw_tokens: Vec<&str> = q.split_whitespace().collect();
    let mut cmdlet_words: Vec<String> = Vec::new();
    for (i, tok) in raw_tokens.iter().enumerate() {
        let clean = tok.trim_matches(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')));
        let clean = clean.trim_end_matches('.');
        if commands::is_cmdlet(clean, false) {
            let ws = words(clean);
            cmdlet_words.extend(ws.iter().cloned());
            let phrase = ws.join(" ");
            push_term(Term { word: clean.to_lowercase(), stem: phrase.clone(), fts: quote(&phrase) }, &mut out);
            push_unique(&mut out.strong_commands, clean.to_string());
            continue;
        }
        let lower = clean.to_lowercase();
        let next_is_switch = raw_tokens
            .get(i + 1)
            .is_some_and(|n| (n.starts_with('/') || n.starts_with('-')) && n.len() > 1 && !n[1..].starts_with('-'));
        if lower.ends_with(".exe") && commands::looks_like_exe(&lower) {
            push_unique(&mut out.strong_commands, commands::normalize_command(&lower));
        } else if next_is_switch && commands::looks_like_exe(&lower) && !is_stop(&lower) {
            push_unique(&mut out.strong_commands, lower.clone());
        }
    }

    // 2. Обычные слова.
    for w in words(q) {
        if w.chars().count() < 2 && !w.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if is_stop(&w) {
            continue;
        }
        // слово уже вошло во фразу командлета
        if cmdlet_words.contains(&w) && !q.to_lowercase().split_whitespace().any(|t| t == w) {
            continue;
        }
        if w == "exe" {
            continue;
        }
        push_term(make_term(&w), &mut out);
        if commands::looks_like_exe(&w)
            && !COMMON_WORDS.contains(&w.as_str())
            && !out.strong_commands.iter().any(|c| c.eq_ignore_ascii_case(&w))
            && w.len() >= 2
            && !is_cyrillic(&w)
        {
            push_unique(&mut out.weak_commands, w.clone());
        }
    }
    out
}

fn is_stop(w: &str) -> bool {
    STOP_EN.binary_search(&w).is_ok() || STOP_RU.contains(&w)
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
        v.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_en_sorted() {
        let mut v = STOP_EN.to_vec();
        v.sort_unstable();
        assert_eq!(v, STOP_EN);
    }

    #[test]
    fn english_stems_to_prefix() {
        let p = parse("How to disable startup programs in Windows 11");
        let f: Vec<_> = p.terms.iter().map(|t| t.fts.as_str()).collect();
        assert_eq!(f, ["\"disabl\"*", "\"startup\"*", "\"program\"*", "\"window\"*", "\"11\""]);
        assert_eq!(p.or_expr().unwrap(), f.join(" OR "));
        assert_eq!(p.and_expr().unwrap(), f.join(" AND "));
    }

    #[test]
    fn russian_stems() {
        let p = parse("как отключить автозагрузку программ");
        let stems: Vec<_> = p.terms.iter().map(|t| t.stem.as_str()).collect();
        assert_eq!(stems, ["отключ", "автозагрузк", "программ"]);
        assert!(p.terms.iter().all(|t| t.fts.ends_with('*')));
        assert!(p.weak_commands.is_empty());
    }

    #[test]
    fn cmdlets_become_phrases_and_commands() {
        let p = parse("Get-Service filter running services");
        assert_eq!(p.terms[0].fts, "\"get service\"");
        assert_eq!(p.strong_commands, ["Get-Service"]);
        // «service» как отдельное слово тоже остаётся (встречается в запросе отдельно — «services»)
        assert!(p.terms.iter().any(|t| t.stem == "servic"));
    }

    #[test]
    fn exe_and_switch_detection() {
        let p = parse("sfc /scannow repair system files");
        assert_eq!(p.strong_commands, ["sfc"]);
        let p = parse("what does robocopy.exe /mir do");
        assert_eq!(p.strong_commands, ["robocopy"]);
        let p = parse("robocopy mirror folders");
        assert_eq!(p.weak_commands, ["robocopy", "mirror", "folders"]);
        let p = parse("start menu layout");
        assert!(!p.weak_commands.contains(&"start".to_string()));
    }

    #[test]
    fn short_and_numbers() {
        let p = parse("error 0x80070005 a b");
        let f: Vec<_> = p.terms.iter().map(|t| t.fts.as_str()).collect();
        assert_eq!(f, ["\"error\"*", "\"0x80070005\""]);
        assert!(parse("the of and").is_empty());
        assert!(parse("").or_expr().is_none());
    }

    #[test]
    fn non_prefix_stem_uses_common_prefix() {
        assert_eq!(make_term("directory").fts, "\"director\"*");
        assert_eq!(make_term("copies").fts, "\"copies\"");
    }

    #[test]
    fn quotes_are_escaped() {
        let t = make_term("ab\"c");
        assert_eq!(t.fts, "\"ab\"\"c\"");
    }
}
