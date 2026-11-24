//! Разбор викитекста статей ru.wiktionary: русская секция, шаблоны морфологии (основы и явные формы),
//! первое значение без разметки. Полные парадигмы Зализняка не строим: распознавание ловит по основам.

/// Убрать знаки ударения (U+0301, U+0300) и привести к нижнему регистру, ё → е.
pub fn norm_word(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(*c, '\u{0301}' | '\u{0300}' | '\u{00AD}'))
        .flat_map(|c| c.to_lowercase())
        .map(|c| if matches!(c, 'ё' | 'ѐ') { 'е' } else { c })
        .collect()
}

/// Секция `= {{-ru-}} =` до следующей языковой секции первого уровня.
pub fn russian_section(text: &str) -> Option<&str> {
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        let is_lang = t.starts_with('=') && !t.starts_with("==") && t.contains("{{-");
        if is_lang {
            if start.is_some() {
                return start.map(|s| &text[s..offset]);
            }
            if t.contains("{{-ru-}}") {
                start = Some(offset + line.len());
            }
        }
        offset += line.len();
    }
    start.map(|s| &text[s..])
}

/// Шаблон верхнего уровня: имя и параметры (позиционные получают ключи "1", "2", …).
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub name: String,
    pub params: Vec<(String, String)>,
}

/// Все шаблоны верхнего уровня (вложенные остаются внутри значений параметров).
pub fn templates(text: &str) -> Vec<Template> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && b[i + 1] == b'{' {
            if let Some(end) = match_braces(b, i) {
                out.push(parse_template(&text[i + 2..end - 2]));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Конец `{{…}}`, начинающегося в `i` (индекс после закрывающих скобок).
fn match_braces(b: &[u8], i: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = i;
    while j + 1 < b.len() {
        if b[j] == b'{' && b[j + 1] == b'{' {
            depth += 1;
            j += 2;
        } else if b[j] == b'}' && b[j + 1] == b'}' {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return Some(j);
            }
        } else {
            j += 1;
        }
    }
    None
}

/// Разделить тело шаблона по `|` вне вложенных `{{ }}` и `[[ ]]`.
fn split_top(body: &str) -> Vec<&str> {
    let b = body.as_bytes();
    let (mut depth_t, mut depth_l) = (0i32, 0i32);
    let mut parts = Vec::new();
    let mut last = 0;
    let mut j = 0;
    while j < b.len() {
        let two = if j + 1 < b.len() { &b[j..j + 2] } else { &b[j..j + 1] };
        match two {
            b"{{" => {
                depth_t += 1;
                j += 2;
                continue;
            }
            b"}}" => {
                depth_t -= 1;
                j += 2;
                continue;
            }
            b"[[" => {
                depth_l += 1;
                j += 2;
                continue;
            }
            b"]]" => {
                depth_l -= 1;
                j += 2;
                continue;
            }
            _ => {}
        }
        if b[j] == b'|' && depth_t == 0 && depth_l == 0 {
            parts.push(&body[last..j]);
            last = j + 1;
        }
        j += 1;
    }
    parts.push(&body[last..]);
    parts
}

fn parse_template(body: &str) -> Template {
    let parts = split_top(body);
    let name = parts[0].trim().to_string();
    let mut params = Vec::new();
    let mut pos = 0;
    for p in &parts[1..] {
        match p.find('=') {
            // `=` внутри вложенного шаблона — это не ключ.
            Some(eq) if !p[..eq].contains("{{") && !p[..eq].contains("[[") => {
                params.push((p[..eq].trim().to_string(), p[eq + 1..].trim().to_string()))
            }
            _ => {
                pos += 1;
                params.push((pos.to_string(), p.trim().to_string()));
            }
        }
    }
    Template { name, params }
}

/// Шаблон морфологии русского слова: `сущ ru …`, `гл ru …`, `прил ru …`, `мест ru`, `прич ru`, `числ ru`.
pub fn is_morph(name: &str) -> bool {
    let n = name.trim();
    ["сущ ru", "гл ru", "прил ru", "мест ru", "прич ru", "числ ru", "мест-прил ru", "Гл-блок", "Прил-блок"]
        .iter()
        .any(|p| n.starts_with(p))
}

/// Похоже ли значение параметра на одну словоформу (кириллица, дефис, ударения).
fn is_word_value(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty()
        && v.chars().count() <= 40
        && v.chars().all(|c| c.is_alphabetic() || matches!(c, '-' | '\u{0301}' | '\u{0300}' | '△' | '*'))
        && v.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
}

/// Ключи падежей/лиц, в которых шаблоны иногда дают явные формы.
fn is_form_key(k: &str) -> bool {
    let k = k.trim();
    let case = ["Им.", "Р.", "Д.", "В.", "Тв.", "Пр.", "Им-", "Р-", "Д-", "В-", "Т-", "П-"];
    let verb = [
        "Я", "Ты", "Он", "Мы", "Вы", "Они", "ПрМ", "ПрЖ", "ПрС", "ПрМн", "Пов", "ПовМн", "Деепр", "ПричНаст", "ПричПрош",
        "ПричСтрад", "Будущее",
    ];
    ((k.contains("ед") || k.contains("мн")) && case.iter().any(|c| k.starts_with(c))) || verb.contains(&k)
}

/// Основы (`основа`, `основа1`, …) и явные формы из шаблонов морфологии.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Morph {
    pub stems: Vec<String>,
    pub forms: Vec<String>,
    pub pos: Option<String>,
}

pub fn morph(section: &str) -> Morph {
    let mut m = Morph::default();
    for t in templates(section) {
        if !is_morph(&t.name) {
            continue;
        }
        if m.pos.is_none() {
            m.pos = t.name.split_whitespace().next().map(str::to_string);
        }
        for (k, v) in &t.params {
            let v = v.trim();
            if k.starts_with("основа") {
                // Основа без окончания: «ху́», «ебану́т»; бывают альтернативы через запятую.
                for s in v.split([',', '/']) {
                    let s = norm_word(s.trim());
                    if !s.is_empty() && s.chars().all(|c| c.is_alphabetic() || c == '-') && !m.stems.contains(&s) {
                        m.stems.push(s);
                    }
                }
            } else if is_form_key(k) {
                for f in v.split([',', '/']).map(str::trim).filter(|f| is_word_value(f)) {
                    let f = norm_word(f).replace(['△', '*'], "");
                    if !f.is_empty() && !m.forms.contains(&f) {
                        m.forms.push(f);
                    }
                }
            }
        }
    }
    m
}

/// Текст шаблона в значении: несколько частых шаблонов превращаем в слова, остальные выбрасываем.
fn template_text(t: &Template) -> String {
    let p1 = t.params.iter().find(|(k, _)| k == "1").map(|(_, v)| strip_markup(v)).unwrap_or_default();
    match t.name.trim() {
        "=" => format!("то же, что {p1}"),
        "помета" | "помета." | "курсив" => p1,
        "действие" => format!("действие по значению гл. {p1}"),
        "свойство" => format!("свойство по значению прил. {p1}"),
        "состояние" => format!("состояние по значению гл. {p1}"),
        "женск." => format!("женск. к {p1}"),
        "уменьш." => format!("уменьш. к {p1}"),
        "увелич." => format!("увелич. к {p1}"),
        "страд." => format!("страд. к {p1}"),
        "возвр." => format!("возвр. к {p1}"),
        "сов." => format!("сов. к {p1}"),
        "несов." => format!("несов. к {p1}"),
        "итп" | "и т. п." => "и т. п.".into(),
        "т. е." => "т. е.".into(),
        _ => String::new(),
    }
}

/// Убрать вики-разметку: шаблоны, ссылки, <ref>, курсив, HTML.
pub fn strip_markup(s: &str) -> String {
    let mut s = s.to_string();
    // комментарии и сноски
    while let Some(a) = s.find("<!--") {
        let b = s[a..].find("-->").map(|b| a + b + 3).unwrap_or(s.len());
        s.replace_range(a..b, "");
    }
    loop {
        let Some(a) = s.find("<ref") else { break };
        let rest = &s[a..];
        let end = if let Some(sc) = rest.find("/>").filter(|sc| !rest[..*sc].contains('>')) {
            a + sc + 2
        } else if let Some(e) = rest.find("</ref>") {
            a + e + 6
        } else {
            s.len()
        };
        s.replace_range(a..end, "");
    }
    // шаблоны (с вложенностью)
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    let mut last = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && b[i + 1] == b'{' {
            if let Some(end) = match_braces(b, i) {
                out.push_str(&s[last..i]);
                out.push_str(&template_text(&parse_template(&s[i + 2..end - 2])));
                i = end;
                last = end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[last..]);
    let mut s = out;
    // ссылки [[a|b]] → b, [[a]] → a
    while let Some(a) = s.find("[[") {
        let Some(e) = s[a..].find("]]").map(|e| a + e) else { break };
        let inner = s[a + 2..e].to_string();
        let shown = inner.rsplit('|').next().unwrap_or("").to_string();
        s.replace_range(a..e + 2, &shown);
    }
    // HTML-теги
    let mut t = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => t.push(c),
            _ => {}
        }
    }
    let t = t.replace("'''", "").replace("''", "").replace("&nbsp;", " ").replace('\u{0301}', "");
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.replace(" ,", ",").replace(" .", ".").replace(" ;", ";").replace("( ", "(").replace(" )", ")");
    t.trim_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '—' | '-' | '◆')).to_string()
}

/// Первое значение (`# …` после заголовка «Значение»), без разметки, ≤ `max` символов.
pub fn first_meaning(section: &str, max: usize) -> String {
    let mut after = false;
    for line in section.lines() {
        let t = line.trim();
        if t.starts_with('=') {
            after = t.trim_matches('=').trim() == "Значение";
            continue;
        }
        if after && t.starts_with('#') && !t.starts_with("#:") && !t.starts_with("#*") {
            let m = strip_markup(t.trim_start_matches('#'));
            if m.is_empty() {
                continue;
            }
            return truncate(&m, max);
        }
    }
    String::new()
}

/// Сырая строка первого значения (`# …` после «Значение»).
pub fn first_meaning_raw(section: &str) -> Option<&str> {
    let mut after = false;
    for line in section.lines() {
        let t = line.trim();
        if t.starts_with('=') {
            after = t.trim_matches('=').trim() == "Значение";
            continue;
        }
        if after && t.starts_with('#') && !t.starts_with("#:") && !t.starts_with("#*") && !strip_markup(t.trim_start_matches('#')).is_empty() {
            return Some(t);
        }
    }
    None
}

/// Пометки грубости в первом значении: имена шаблонов (`обсц.`, `груб.`, …) и значения `{{помета|…}}`.
pub fn first_meaning_labels(section: &str) -> Vec<String> {
    let Some(line) = first_meaning_raw(section) else { return Vec::new() };
    let mut out = Vec::new();
    for t in templates(line) {
        let name = t.name.trim().to_string();
        if name.starts_with("помета") {
            for (_, v) in &t.params {
                out.push(v.trim().to_string());
            }
        }
        out.push(name);
    }
    out
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max - 1).collect();
    let cut = match cut.rfind(' ') {
        Some(p) if p > cut.len() / 2 => cut[..p].to_string(),
        _ => cut,
    };
    format!("{}…", cut.trim_end_matches([',', ';', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "= {{-ru-}} =\n=== Морфологические и синтаксические свойства ===\n{{сущ ru m a 3a\n|основа=ху́\n|основа1=хуй\n|слоги={{по-слогам|хуй}}\n|Р.мн=хуёв\n}}\n=== Семантические свойства ===\n==== Значение ====\n# {{обсц.|ru}} [[мужской|мужской]] [[половой член]] {{пример|текст|автор=X}}\n# второе\n= {{-uk-}} =\n{{сущ uk m|основа=другое}}\n";

    #[test]
    fn section_morph_meaning() {
        let ru = russian_section(PAGE).unwrap();
        assert!(!ru.contains("другое"));
        let m = morph(ru);
        assert_eq!(m.stems, vec!["ху", "хуй"]);
        assert_eq!(m.forms, vec!["хуев"]);
        assert_eq!(m.pos.as_deref(), Some("сущ"));
        assert_eq!(first_meaning(ru, 120), "мужской половой член");
    }

    #[test]
    fn markup_and_truncate() {
        assert_eq!(strip_markup("{{=|хуй|ru}}"), "то же, что хуй");
        assert_eq!(strip_markup("''груб.'' [[a|бэ]]<ref name=x>сноска</ref> [[вэ]]<ref name=y/>"), "груб. бэ вэ");
        let t = truncate(&"слово ".repeat(40), 30);
        assert!(t.chars().count() <= 30 && t.ends_with('…'));
    }
}
