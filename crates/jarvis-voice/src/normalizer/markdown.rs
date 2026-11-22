//! Markdown → текст для речи: заголовки и пункты списков становятся предложениями, блоки кода — «пример кода»,
//! таблицы — «столбец: значение», ссылки — подписью, выделение снимается, инлайн-код читается.

use regex::Regex;
use std::sync::LazyLock;

static RE_HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}\s+(.*?)\s*#*\s*$").unwrap());
static RE_LIST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[-*+•]|\d{1,3}[.)])\s+(?:\[[ xX]\]\s*)?").unwrap());
static RE_QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:>\s?)+").unwrap());
static RE_HR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[-*_]\s*){3,}$").unwrap());
static RE_TABLE_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\|?\s*:?-{2,}:?\s*(?:\|\s*:?-{2,}:?\s*)*\|?$").unwrap());
static RE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`+([^`]+?)`+").unwrap());
static RE_IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap());
static RE_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
static RE_REF_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\[[^\]]*\]").unwrap());
static RE_AUTOLINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<((?:https?://|mailto:)[^>\s]+)>").unwrap());
static RE_FOOTNOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\^[^\]]+\]").unwrap());
static RE_BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*]+?)\*\*|__([^_]+?)__").unwrap());
static RE_ITALIC_STAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*([^*\s][^*]*?)\*").unwrap());
static RE_ITALIC_UNDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|[^\p{L}\p{N}_])_([^_\n]+?)_($|[^\p{L}\p{N}_])").unwrap());
static RE_STRIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"~~([^~]+?)~~").unwrap());
static RE_HTML: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</?[a-zA-Z][^>\n]*>").unwrap());
static RE_ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\([\\`*_{}\[\]()#+\-.!|])").unwrap());

fn is_fence(t: &str) -> Option<&'static str> {
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn is_table_row(t: &str) -> bool {
    t.starts_with('|') && t.len() > 1
}

fn cells(t: &str) -> Vec<String> {
    let t = t.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| inline(c.trim())).collect()
}

/// Есть ли в конце знак, завершающий фразу (или вводящий перечисление).
fn has_terminal(s: &str) -> bool {
    s.trim_end_matches(['"', '»', ')', '\'', ' ']).ends_with(['.', '!', '?', '…', ':', ';'])
}

fn sentence(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() || has_terminal(s) { s.to_string() } else { format!("{s}.") }
}

/// Инлайн-разметка → текст.
pub fn inline(s: &str) -> String {
    // Экранированные символы прячем в Private Use Area, чтобы их не съело выделение, и возвращаем в конце.
    const ESCAPABLE: &str = "\\`*_{}[]()#+-.!|";
    let s = RE_ESCAPE.replace_all(s, |c: &regex::Captures| {
        let i = ESCAPABLE.find(&c[1]).unwrap_or(0) as u32;
        char::from_u32(0xE000 + i).unwrap().to_string()
    });
    // Код — первым, чтобы `*`/`_` внутри него не считались выделением.
    let s = RE_CODE.replace_all(&s, |c: &regex::Captures| {
        c[1].replace(['_', '*'], " ").replace("()", "").trim().to_string()
    });
    let s = RE_IMAGE.replace_all(&s, "$1");
    let s = RE_LINK.replace_all(&s, "$1");
    let s = RE_REF_LINK.replace_all(&s, "$1");
    let s = RE_AUTOLINK.replace_all(&s, "$1");
    let s = RE_FOOTNOTE.replace_all(&s, "");
    let s = RE_BOLD.replace_all(&s, "$1$2");
    let s = RE_ITALIC_STAR.replace_all(&s, "$1");
    let s = RE_ITALIC_UNDER.replace_all(&s, "$1$2$3");
    let s = RE_STRIKE.replace_all(&s, "$1");
    let s = RE_HTML.replace_all(&s, " ");
    s.chars()
        .map(|c| match c as u32 {
            v @ 0xE000..=0xE00F => ESCAPABLE.chars().nth((v - 0xE000) as usize).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// Таблица (заголовок + строки) → «столбец: значение, …» по строке.
fn table_rows(header: &[String], rows: &[Vec<String>]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let parts: Vec<String> = row
                .iter()
                .enumerate()
                .filter(|(_, v)| !v.is_empty())
                .map(|(i, v)| match header.get(i).filter(|h| !h.is_empty() && *h != v) {
                    Some(h) => format!("{h}: {v}"),
                    None => v.clone(),
                })
                .collect();
            sentence(&parts.join(", "))
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Весь Markdown-текст → строки-предложения, соединённые пробелом.
pub fn to_plain(text: &str, code_placeholder: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(fence) = is_fence(t) {
            out.push(sentence(code_placeholder));
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with(fence) {
                i += 1;
            }
            i += 1; // закрывающая ограда (или конец текста)
            continue;
        }
        if is_table_row(t) && lines.get(i + 1).is_some_and(|n| RE_TABLE_SEP.is_match(n.trim())) {
            let header = cells(t);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && is_table_row(lines[i].trim()) {
                rows.push(cells(lines[i]));
                i += 1;
            }
            out.extend(table_rows(&header, &rows));
            continue;
        }
        i += 1;
        if t.is_empty() || RE_HR.is_match(t) {
            continue;
        }
        let t = RE_QUOTE.replace(t, "");
        let content = if let Some(c) = RE_HEADING.captures(&t) {
            c[1].to_string()
        } else {
            RE_LIST.replace(&t, "").into_owned()
        };
        let s = sentence(&inline(&content));
        if !s.is_empty() && s != "." {
            out.push(s);
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks() {
        let md = "# Итоги\n\n- первый пункт\n- **второй** пункт\n\n1. Шаг [ссылка](http://x.y)\n\n```rust\nfn main() {}\n```\nГотово";
        assert_eq!(
            to_plain(md, "пример кода"),
            "Итоги. первый пункт. второй пункт. Шаг ссылка. пример кода. Готово."
        );
    }

    #[test]
    fn tables() {
        let md = "| Диск | Свободно |\n|---|:---:|\n| C | мало |\n| D | много |";
        assert_eq!(to_plain(md, "x"), "Диск: C, Свободно: мало. Диск: D, Свободно: много.");
        // Без строки-разделителя это не таблица.
        assert_eq!(to_plain("| a | b |", "x"), "| a | b |.");
    }

    #[test]
    fn inline_marks() {
        assert_eq!(inline("*курсив* и _тоже_ и snake_case_name"), "курсив и тоже и snake_case_name");
        assert_eq!(inline("выполни `git_status()` сейчас"), "выполни git status сейчас");
        assert_eq!(inline("~~старое~~ <b>новое</b>"), "старое  новое ");
        assert_eq!(inline("![схема](a.png) и <https://ya.ru>"), "схема и https://ya.ru");
        assert_eq!(inline(r"цена \*5\*"), "цена *5*");
    }
}
