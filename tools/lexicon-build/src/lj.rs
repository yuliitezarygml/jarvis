//! Разбор поста ЖЖ «Подъёбки» (101.livejournal.com/1230369.html): 4 раздела, строки через `<br />`.
//! Лицензии у поста нет — результат пишется только в `data/persona/` (не в git).

use serde::Serialize;

pub const URL: &str = "https://101.livejournal.com/1230369.html";

pub const SECTIONS: [&str; 4] = ["Культурные подколы", "Жёсткие подъёбки", "Подколки", "Подъёбки подонков"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LjLine {
    pub text: String,
    pub section: String,
    /// Жёсткая (раздел «Жёсткие подъёбки» / «Подъёбки подонков»).
    pub harsh: bool,
    /// Режимы, где строку можно использовать.
    pub modes: Vec<&'static str>,
    /// Ситуации банка ответов.
    pub situations: Vec<&'static str>,
    /// false — строка не годится для Джарвиса (про внешность/одежду, которой он не видит).
    pub usable: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&hellip;", "…")
        .replace("&quot;", "\"")
        .replace("&laquo;", "«")
        .replace("&raquo;", "»")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Строки про внешность, одежду и тело — Джарвис их не видит, а по правилам о внешности не шутит.
const APPEARANCE: [&str; 9] = ["джинс", "кепк", "стрижк", "кофточк", "безобразие на голове", "лицом", "губами", "носом", "зенки"];

pub fn parse(html: &str) -> Vec<LjLine> {
    let Some(start) = html.find("aentry-post__text") else { return Vec::new() };
    let body = &html[start..];
    let body = body.find('>').map(|p| &body[p + 1..]).unwrap_or(body);
    let end = ["cutid1-end", "<form", "</article"].iter().filter_map(|m| body.find(m)).min().unwrap_or(body.len());
    let body = &body[..end];
    let mut section: Option<&'static str> = None;
    let mut out = Vec::new();
    for (i, raw) in body.split("<br").enumerate() {
        // остаток тега `<br />` (у первого куска его нет)
        let raw = if i == 0 { raw } else { raw.split_once('>').map(|(_, r)| r).unwrap_or("") };
        let line = decode_entities(&strip_tags(raw));
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        if let Some(s) = SECTIONS.iter().find(|s| line.trim() == **s) {
            section = Some(s);
            continue;
        }
        let Some(sec) = section else { continue };
        let text = line.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches('.').trim().to_string();
        if text.is_empty() {
            continue;
        }
        let harsh = sec == SECTIONS[1] || sec == SECTIONS[3];
        let low = text.to_lowercase();
        let appearance = APPEARANCE.iter().any(|a| low.contains(a));
        out.push(LjLine {
            text,
            section: sec.to_string(),
            harsh,
            modes: if harsh { vec!["uncensored"] } else { vec!["buddy", "uncensored"] },
            situations: if harsh { vec!["insult_reply"] } else { vec!["insult_reply", "user_mistake_tease"] },
            usable: !appearance,
            reason: if appearance { "про внешность/одежду".into() } else { String::new() },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections() {
        let html = r#"<div class="aentry-post__text aentry-post__text--view"> Культурные подколы <br />1. Клёвая стрижка! Проспорил? <a name="cutid1"></a><br />2. Ты что какой нервный сегодня? <br /><br />Жёсткие подъёбки <br />1. Я тебе &hellip; <br />Подколки <br />Ты такой умный, тебе череп не жмёт? <br /><a name='cutid1-end'></a><br /><form>"#;
        let l = parse(html);
        assert_eq!(l.len(), 4);
        assert_eq!(l[0].text, "Клёвая стрижка! Проспорил?");
        assert!(!l[0].usable);
        assert_eq!(l[1].text, "Ты что какой нервный сегодня?");
        assert!(l[2].harsh && l[2].modes == vec!["uncensored"]);
        assert_eq!(l[3].section, "Подколки");
        assert!(!l[3].harsh);
    }
}
