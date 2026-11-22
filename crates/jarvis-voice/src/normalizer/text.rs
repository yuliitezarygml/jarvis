//! Эмодзи, ссылки/почта, служебные символы и финальная чистка пробелов и пунктуации.

use regex::{Captures, Regex};
use std::sync::LazyLock;

/// Эмодзи и пиктограммы (включая флаги, модификаторы тона, ZWJ, вариационные селекторы, keycap).
pub fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF     // эмодзи, флаги, символы
        | 0x2600..=0x27BF     // разные символы, dingbats (✓ ✔ ✨ ❤)
        | 0x2B00..=0x2BFF     // звёзды, стрелки-эмодзи ⭐ ⬆
        | 0x2300..=0x23FF     // ⌚ ⏰ ⏳
        | 0xFE00..=0xFE0F     // вариационные селекторы
        | 0x200D | 0x20E3     // ZWJ, keycap
        | 0xE0020..=0xE007F   // теги флагов
        | 0x3030 | 0x303D | 0x3297 | 0x3299
        | 0x00A9 | 0x00AE | 0x2122 // © ® ™
    )
}

pub fn strip_emoji(s: &str) -> String {
    s.chars().filter(|c| !is_emoji(*c)).collect()
}

static RE_EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([\w.+-]+)@([\w-]+(?:\.[\w-]+)+)\b").unwrap());
static RE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:https?://|ftp://|www\.)[^\s<>()\[\]"'«»]+"#).unwrap()
});

/// Ссылка → её хост словами: `https://www.github.com/a/b` → «github точка com».
fn speak_url(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    // Порт не читаем, а у IPv4 оставляем точки — их прочитает разбор чисел.
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return host.to_string();
    }
    host.split('.').filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" точка ")
}

pub fn links_to_words(s: &str) -> String {
    let s = RE_EMAIL.replace_all(s, |c: &Captures| {
        format!("{} собака {}", c[1].replace(['.', '_'], " "), c[2].split('.').collect::<Vec<_>>().join(" точка "))
    });
    RE_URL
        .replace_all(&s, |c: &Captures| {
            let raw = &c[0];
            // Пунктуация в конце — не часть ссылки.
            let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            format!("{}{}", speak_url(trimmed), &raw[trimmed.len()..])
        })
        .into_owned()
}

static RE_PRE: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"[№#]\s?(\d)", "номер $1"),
        (r"~\s?(\d)", "около $1"),
        (r"≈\s?", "примерно "),
        (r"≥\s?", "не меньше "),
        (r"≤\s?", "не больше "),
        (r"(^|\s)>\s?(\d)", "${1}больше $2"),
        (r"(^|\s)<\s?(\d)", "${1}меньше $2"),
        (r"±\s?", "плюс-минус "),
        (r"(\d)\s?[×x]\s?(\d)", "$1 на $2"),
        (r"(\d)\s?\*\s?(\d)", "$1 умножить на $2"),
    ]
    .into_iter()
    .map(|(p, r)| (Regex::new(p).unwrap(), r))
    .collect()
});

/// Символы, которые меняют чтение чисел, — до разбора чисел («~5 мин» → «около 5 мин»).
pub fn symbols_pre(s: &str) -> String {
    let mut s = s.to_string();
    for (re, rep) in RE_PRE.iter() {
        s = re.replace_all(&s, *rep).into_owned();
    }
    s
}

static RE_ARROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*(?:->|=>|[→⇒⟶➜←↔⇄])\s*").unwrap());
static RE_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_SPACE_PUNCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+([,.!?;:…)»])").unwrap());
static RE_OPEN_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([(«])\s+").unwrap());
static RE_DUP_PUNCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([,;:])(?:\s*[,;:.])+|\.(?:\s*\.)+|!{2,}|\?{2,}").unwrap());
static RE_EMPTY_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(\s*\)|«\s*»").unwrap());
static RE_LEADING_PUNCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\s,.;:!?—–-]+").unwrap());

/// Служебные символы после разбора чисел.
pub fn symbols_post(s: &str) -> String {
    let s = s.replace("...", "…");
    let s = RE_ARROW.replace_all(&s, " — ");
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str(" и "),
            '@' => out.push_str(" собака "),
            '+' => out.push_str(" плюс "),
            '=' => out.push_str(" равно "),
            '*' | '_' | '#' | '`' | '|' | '^' | '\\' | '{' | '}' | '[' | ']' | '<' | '>' | '~' | '/' | '%' | '"' => {
                out.push(' ')
            }
            '\u{00A0}' | '\u{202F}' | '\t' | '\r' | '\n' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// Пробелы и пунктуация: без пробелов перед знаками, без повторов, без висящих скобок.
pub fn cleanup(s: &str) -> String {
    let s = RE_SPACES.replace_all(s, " ");
    let s = RE_EMPTY_PARENS.replace_all(&s, "");
    let s = RE_SPACE_PUNCT.replace_all(&s, "$1");
    let s = RE_OPEN_SPACE.replace_all(&s, "$1");
    let s = RE_DUP_PUNCT.replace_all(&s, |c: &Captures| {
        let m = &c[0];
        if let Some(first) = c.get(1) {
            return first.as_str().to_string();
        }
        m.chars().next().map(String::from).unwrap_or_default()
    });
    let s = RE_LEADING_PUNCT.replace(&s, "");
    RE_SPACES.replace_all(s.trim(), " ").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_removed_keep_symbols() {
        assert_eq!(strip_emoji("Готово ✅ 🚀👍🏽 ❤️ 🇷🇺 °C № €"), "Готово     °C № €");
    }

    #[test]
    fn links() {
        assert_eq!(links_to_words("см. https://www.github.com/a/b?x=1."), "см. github точка com.");
        assert_eq!(links_to_words("пиши на ivan.petrov@mail.ru"), "пиши на ivan petrov собака mail точка ru");
        assert_eq!(links_to_words("http://192.168.50.38:8443/ready"), "192.168.50.38");
    }

    #[test]
    fn cleanup_spaces_and_punct() {
        assert_eq!(cleanup("  Привет ,  мир !!  ( ) Да.. . "), "Привет, мир! Да.");
        assert_eq!(cleanup(", начало"), "начало");
        assert_eq!(symbols_post("C++ & Rust -> круто"), "C плюс  плюс   и  Rust — круто");
    }
}
