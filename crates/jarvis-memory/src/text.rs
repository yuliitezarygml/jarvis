//! Нормализация и стемминг текста (русский + английский).
//!
//! Одна и та же функция [`stems`] используется и для индекса (стеммированные колонки FTS5),
//! и для запроса — поэтому «люблю» в факте и «любил» в запросе сводятся к одной основе.
//! Правила: lowercase, `ё → е`, слова с дефисом/подчёркиванием дают и склеенную форму, и части
//! (`wi-fi` → `wifi`, `wi`, `fi`), стоп-слова и однобуквенные слова отбрасываются.

use rust_stemmers::{Algorithm, Stemmer};
use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

static RU: LazyLock<Stemmer> = LazyLock::new(|| Stemmer::create(Algorithm::Russian));
static EN: LazyLock<Stemmer> = LazyLock::new(|| Stemmer::create(Algorithm::English));

/// Стоп-слова уже в нормализованном виде (lowercase, `ё → е`).
const STOP_WORDS: &[&str] = &[
    // русские
    "и", "в", "во", "не", "что", "он", "на", "я", "с", "со", "как", "а", "то", "все", "она", "так", "его", "но",
    "да", "ты", "к", "у", "же", "вы", "за", "бы", "по", "только", "ее", "мне", "было", "вот", "от", "меня",
    "еще", "нет", "о", "из", "ему", "теперь", "когда", "даже", "ну", "ли", "если", "уже", "или", "ни", "быть",
    "был", "него", "до", "вас", "нибудь", "опять", "уж", "вам", "ведь", "там", "потом", "себя", "ничего", "ей",
    "может", "они", "тут", "где", "есть", "надо", "ней", "для", "мы", "тебя", "их", "чем", "была", "сам",
    "чтоб", "без", "будто", "чего", "раз", "тоже", "себе", "под", "будет", "ж", "тогда", "кто", "этот", "того",
    "потому", "этого", "какой", "какая", "какое", "какие", "каким", "какую", "ним", "здесь", "этом", "почти",
    "мой", "моя", "мое", "мои", "моего", "моей", "моих", "мою", "тем", "чтобы", "нее", "сейчас", "были",
    "куда", "зачем", "всех", "можно", "при", "об", "обо", "хоть", "после", "над", "тот", "через", "эти", "нас",
    "про", "всего", "них", "разве", "эту", "впрочем", "свою", "свой", "своя", "свои", "этой", "перед",
    "том", "такой", "им", "более", "всю", "между", "это", "эта", "мной", "тебе", "твой", "твоя", "ваш",
    "наш", "который", "которая", "которое", "которые", "очень", "пожалуйста", "джарвис", "запомни",
    "помнишь", "знаешь",
    // английские
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "to", "of", "and", "or", "in", "on", "at",
    "for", "with", "it", "its", "this", "that", "these", "those", "i", "you", "my", "me", "we", "our",
    "your", "he", "she", "they", "them", "do", "does", "did", "so", "as", "by", "from", "what", "which",
    "who", "if", "then", "than", "there", "here", "please",
];

static STOP: LazyLock<HashSet<&'static str>> = LazyLock::new(|| STOP_WORDS.iter().copied().collect());

/// Lowercase + `ё → е`.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        for l in c.to_lowercase() {
            out.push(if l == 'ё' { 'е' } else { l });
        }
    }
    out
}

/// Обрезать пробелы по краям и схлопнуть все пробельные последовательности в один пробел.
pub fn clean_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Обрезать до `max` символов (по символам, не байтам) с «…» в конце.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    // не оставлять висящий пробел перед многоточием
    while out.ends_with(' ') {
        out.pop();
    }
    out.push('…');
    out
}

fn is_joiner(c: char) -> bool {
    matches!(c, '-' | '_' | '\u{2010}' | '\u{2011}')
}

/// Сырые слова (после `fold`): дефисные слова дают склейку и части.
fn raw_words(folded: &str, out: &mut Vec<String>) {
    let mut chunk = String::new();
    for c in folded.chars() {
        if c.is_alphanumeric() || is_joiner(c) {
            chunk.push(c);
        } else if !chunk.is_empty() {
            flush_chunk(&mut chunk, out);
        }
    }
    if !chunk.is_empty() {
        flush_chunk(&mut chunk, out);
    }
}

fn flush_chunk(chunk: &mut String, out: &mut Vec<String>) {
    let parts: Vec<&str> = chunk.split(is_joiner).filter(|p| !p.is_empty()).collect();
    if parts.len() > 1 {
        out.push(parts.concat());
    }
    out.extend(parts.into_iter().map(str::to_string));
    chunk.clear();
}

fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04FF}')
}

fn stem_word(w: &str) -> Option<String> {
    let n = w.chars().count();
    if n > 48 {
        return None;
    }
    let all_digits = w.chars().all(|c| c.is_ascii_digit());
    if n < 2 && !all_digits {
        return None;
    }
    if STOP.contains(w) {
        return None;
    }
    let s = if w.chars().any(is_cyrillic) {
        RU.stem(w).into_owned()
    } else if w.chars().any(|c| c.is_ascii_alphabetic()) {
        EN.stem(w).into_owned()
    } else {
        w.to_string()
    };
    (!s.is_empty()).then_some(s)
}

/// Стеммированные токены текста в порядке появления (с повторами).
pub fn stems(text: &str) -> Vec<String> {
    let folded = fold(text);
    let mut words = Vec::new();
    raw_words(&folded, &mut words);
    words.iter().filter_map(|w| stem_word(w)).collect()
}

/// Стеммированный текст для FTS-колонки: токены через пробел.
pub fn stem_text(text: &str) -> String {
    stems(text).join(" ")
}

/// Множество основ — для сравнения похожести.
pub fn stem_set(text: &str) -> BTreeSet<String> {
    stems(text).into_iter().collect()
}

/// Множество из уже стеммированной строки (значение FTS-колонки).
pub fn set_of(stemmed: &str) -> BTreeSet<String> {
    stemmed.split_whitespace().map(str::to_string).collect()
}

const MAX_QUERY_TERMS: usize = 24;

fn unique_in_order(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|s| seen.insert(s.clone())).take(MAX_QUERY_TERMS).collect()
}

/// FTS5-выражение для поиска: основы через OR, основы длиной ≥3 — префиксные
/// (`"люб"*` находит и «любл» от «люблю»). `None`, если в запросе нет значимых слов.
pub fn fts_query(text: &str) -> Option<String> {
    let terms = unique_in_order(stems(text));
    if terms.is_empty() {
        return None;
    }
    let parts: Vec<String> = terms
        .iter()
        .map(|t| {
            let digits = t.chars().all(|c| c.is_ascii_digit());
            if t.chars().count() >= 3 && !digits { format!("\"{t}\"*") } else { format!("\"{t}\"") }
        })
        .collect();
    Some(parts.join(" OR "))
}

/// FTS5-выражение по точным основам (для поиска дубликатов).
pub(crate) fn fts_query_exact<'a>(terms: impl IntoIterator<Item = &'a String>) -> Option<String> {
    let parts: Vec<String> = terms.into_iter().take(MAX_QUERY_TERMS).map(|t| format!("\"{t}\"")).collect();
    (!parts.is_empty()).then(|| parts.join(" OR "))
}

/// Коэффициент Дайса: 2|A∩B| / (|A|+|B|), 0..1.
pub fn dice(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    2.0 * inter / (a.len() + b.len()) as f64
}

/// Доля основ запроса, присутствующих в документе, 0..1.
pub fn coverage(query: &BTreeSet<String>, doc: &BTreeSet<String>) -> f64 {
    if query.is_empty() {
        return 0.0;
    }
    query.intersection(doc).count() as f64 / query.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_lowercases_and_replaces_yo() {
        assert_eq!(fold("ЧЁРНЫЙ Ёж"), "черный еж");
    }

    #[test]
    fn russian_morphology_shares_stems() {
        let a = stem_set("я люблю чёрный кофе");
        let b = stem_set("какой кофе я любил");
        assert!(a.contains("коф"), "{a:?}");
        assert!(a.intersection(&b).count() >= 1, "{a:?} vs {b:?}");
        assert_eq!(stem_set("ошибки"), stem_set("ошибка"));
        assert_eq!(stem_set("файлов"), stem_set("файлы"));
    }

    #[test]
    fn english_is_stemmed() {
        assert_eq!(stem_set("running tests"), stem_set("run test"));
    }

    #[test]
    fn hyphenated_words_produce_joined_and_parts() {
        let s = stems("Wi-Fi роутер");
        assert!(s.contains(&"wifi".to_string()), "{s:?}");
        assert!(s.contains(&"wi".to_string()));
        assert!(s.contains(&"fi".to_string()));
        // «WiFi» без дефиса совпадает со склейкой
        assert!(stem_set("WiFi").contains("wifi"));
        // имена инструментов: склейка + части
        let t = stem_set("powershell_run");
        assert!(t.contains("run"), "{t:?}");
        assert_eq!(t.len(), 3, "{t:?}");
    }

    #[test]
    fn stop_words_and_short_tokens_are_dropped() {
        assert!(stems("я и ты, а он — в").is_empty());
        assert_eq!(stems("x 5"), vec!["5".to_string()]);
    }

    #[test]
    fn query_uses_prefix_for_long_stems() {
        let q = fts_query("какой кофе я любил").unwrap();
        assert!(q.contains("\"коф\"*"), "{q}");
        assert!(q.contains(" OR "));
        assert!(fts_query("я и ты").is_none());
        assert_eq!(fts_query("404").unwrap(), "\"404\"");
    }

    #[test]
    fn similarity_metrics() {
        let a = stem_set("пьёт чёрный кофе");
        let b = stem_set("пьет черный кофе по утрам");
        assert!(dice(&a, &b) > 0.6);
        assert_eq!(coverage(&a, &b), 1.0);
        assert_eq!(dice(&a, &BTreeSet::new()), 0.0);
    }

    #[test]
    fn truncation_is_char_safe() {
        assert_eq!(truncate_chars("привет мир", 7), "привет…");
        assert_eq!(truncate_chars("абв", 3), "абв");
        assert_eq!(truncate_chars("абвг", 1), "…");
    }
}
