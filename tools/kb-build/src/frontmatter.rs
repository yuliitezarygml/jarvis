//! YAML front matter документов Learn (`---` … `---`). Полный YAML не нужен: берём ключи верхнего уровня
//! (`title`, `description`, `ms.date`, `redirect_url`, `ROBOTS`…) — строки в кавычках и без, блочные скаляры
//! `>`/`|`, списки `- x` (склеиваются через запятую). Вложенные словари пропускаются.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrontMatter {
    pub fields: BTreeMap<String, String>,
}

impl FrontMatter {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(&key.to_ascii_lowercase()).map(String::as_str).filter(|s| !s.is_empty())
    }
    pub fn title(&self) -> Option<&str> {
        self.get("title")
    }
    pub fn description(&self) -> Option<&str> {
        self.get("description")
    }
    pub fn ms_date(&self) -> Option<&str> {
        self.get("ms.date")
    }
    /// Страница-перенаправление или скрытая от поиска — не индексируем.
    pub fn is_redirect_or_noindex(&self) -> bool {
        self.get("redirect_url").is_some() || self.get("robots").is_some_and(|r| r.to_ascii_lowercase().contains("noindex"))
    }
}

/// Разделить текст на front matter и тело. Без front matter — пустая структура и весь текст.
pub fn split(text: &str) -> (FrontMatter, &str) {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    // первая строка — ровно `---` (допускаем хвостовые пробелы: в Sysinternals встречается `--- \r\n`)
    let first_end = t.find('\n').unwrap_or(t.len());
    if t[..first_end].trim_end() != "---" || first_end == t.len() {
        return (FrontMatter::default(), t);
    }
    let rest = &t[first_end..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let l = line.trim_end();
        if offset > 0 && (l == "---" || l == "...") {
            let fm = parse(&rest[..offset]);
            return (fm, &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (FrontMatter::default(), t)
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        let inner = &v[1..v.len() - 1];
        let mut out = String::new();
        let mut it = inner.chars();
        while let Some(c) = it.next() {
            if c == '\\' {
                match it.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(o) => out.push(o),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        return out;
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    // комментарий в конце плоского значения
    match v.find(" #") {
        Some(i) => v[..i].trim_end().to_string(),
        None => v.to_string(),
    }
}

pub fn parse(yaml: &str) -> FrontMatter {
    let mut fields = BTreeMap::new();
    let lines: Vec<&str> = yaml.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') || line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        let indented = |l: &str| l.starts_with(' ') || l.starts_with('\t') || l.trim().is_empty();
        let v = if value.starts_with('>') || value.starts_with('|') {
            let folded = value.starts_with('>');
            let mut parts = Vec::new();
            while i < lines.len() && indented(lines[i]) {
                parts.push(lines[i].trim());
                i += 1;
            }
            let sep = if folded { " " } else { "\n" };
            parts.into_iter().filter(|p| !p.is_empty() || !folded).collect::<Vec<_>>().join(sep).trim().to_string()
        } else if value.is_empty() {
            // список или вложенный словарь
            let mut items = Vec::new();
            while i < lines.len() && (indented(lines[i]) || lines[i].starts_with("- ")) {
                if let Some(item) = lines[i].trim().strip_prefix("- ") {
                    items.push(unquote(item));
                }
                i += 1;
            }
            items.join(", ")
        } else {
            let mut v = value.to_string();
            // многострочное плоское значение (продолжение с отступом)
            while i < lines.len() && lines[i].starts_with(' ') && !lines[i].trim().is_empty() && !lines[i].trim_start().starts_with("- ") {
                v.push(' ');
                v.push_str(lines[i].trim());
                i += 1;
            }
            unquote(&v)
        };
        fields.insert(key, v);
    }
    FrontMatter { fields }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_parses() {
        let doc = "---\ntitle: robocopy\ndescription: \"Reference article for the \\\"robocopy\\\" command.\"\nms.topic: reference\nms.date: 05/16/2025\nauthor: x # comment\n---\n# robocopy\n\nBody";
        let (fm, body) = split(doc);
        assert_eq!(fm.title(), Some("robocopy"));
        assert_eq!(fm.description(), Some("Reference article for the \"robocopy\" command."));
        assert_eq!(fm.ms_date(), Some("05/16/2025"));
        assert_eq!(fm.get("author"), Some("x"));
        assert_eq!(body, "# robocopy\n\nBody");
    }

    #[test]
    fn block_scalars_lists_and_crlf() {
        let doc = "\u{feff}---\r\ntitle: 'It''s here'\r\ndescription: >-\r\n  Folded line one\r\n  line two\r\nms.custom:\r\n  - a\r\n  - \"b\"\r\nnested:\r\n  key: v\r\nROBOTS: NOINDEX\r\n---\r\ntext";
        let (fm, body) = split(doc);
        assert_eq!(fm.title(), Some("It's here"));
        assert_eq!(fm.description(), Some("Folded line one line two"));
        assert_eq!(fm.get("ms.custom"), Some("a, b"));
        assert!(fm.is_redirect_or_noindex());
        assert_eq!(body, "text");
    }

    #[test]
    fn opening_fence_with_trailing_space() {
        let (fm, body) = split("--- \r\nTOCTitle: Process Monitor\r\ntitle: Process Monitor\r\nms.date: 11/05/2026\r\n---\r\n# Process Monitor v4.13\r\n");
        assert_eq!(fm.title(), Some("Process Monitor"));
        assert_eq!(fm.get("toctitle"), Some("Process Monitor"));
        assert_eq!(body, "# Process Monitor v4.13\r\n");
    }

    #[test]
    fn literal_and_continuation() {
        let fm = parse("description: |\n  a\n  b\ntitle: Long title\n  continued here\nredirect_url: /x");
        assert_eq!(fm.description(), Some("a\nb"));
        assert_eq!(fm.title(), Some("Long title continued here"));
        assert!(fm.is_redirect_or_noindex());
    }

    #[test]
    fn no_front_matter() {
        let (fm, body) = split("# Title\n---\nmore");
        assert_eq!(fm, FrontMatter::default());
        assert_eq!(body, "# Title\n---\nmore");
        let (fm, body) = split("---\nunterminated: yes\n");
        assert!(fm.fields.is_empty());
        assert!(body.starts_with("---"));
    }
}
