//! Английские термины в распознанном тексте — в исходном написании: «открой пауэршелл» → «открой PowerShell»,
//! «конфиг джейсон» → «config.json», «зайди на гитхаб» → «зайди на GitHub».
//!
//! GigaAM пишет только кириллицей, Whisper — как получится, поэтому каждый финальный (и живой) текст
//! проходит обратный словарь: чтение по-русски → английское написание. Источники:
//! 1. словарь произношения голоса (`normalizer::english::dictionary()`: «powershell» → «пауэршэл») — без
//!    слов, совпадающих с обычными русскими («код», «чат», «сайт», «офис»…, см. `SKIP`);
//! 2. разговорные написания из `hints::NAME_HINTS` («вс код», «блютуз»);
//! 3. варианты распознавания (`VARIANTS`: «джсон», «конфик», «пул реквест»…).
//!
//! Сравнение — по «свёрнутой» форме слова (ё/э → е, ы → и, без ь/ъ, двойные буквы — одна), поэтому
//! «пауэршелл» и «пауэршэл» совпадают. Слова — целиком, самые длинные фразы первыми, слитно и раздельно
//! («гит хаб» = «гитхаб»), падежные окончания отбрасываются («в дискорде» → «в Discord»). Для длинных слов
//! (от 6 букв) — ещё нечёткое совпадение: одна правка или «как слышится» (а/о, е/и, оглушение на конце).
//! Остальной русский текст не трогается; «Джарвис» не меняется никогда.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};

use super::hints::NAME_HINTS;
use crate::normalizer::english;

/// Английские слова словаря произношения, которые НЕ возвращаются в латиницу: их чтение — обычное русское
/// слово или имя («код», «чат», «офис», «тони»), они слишком частые («file», «start») или это обращение.
const SKIP: &[&str] = &[
    "jarvis", "git", "code", "visual", "studio", "opera", "yandex", "ozon", "avito", "wikipedia", "stack", "overflow",
    "llama", "office", "terminal", "next", "obsidian", "mistral", "counter", "strike", "task manager", "iron man",
    "tony stark", "pull", "file", "files", "folder", "settings", "update", "download", "store", "game", "games",
    "server", "home", "page", "online", "smart", "pro", "max", "plus", "mini", "ultra", "sir", "hello", "ok", "okay",
    "yes", "no", "the", "and", "of", "to", "for", "with", "is", "in", "on", "new", "news", "free", "start", "stop",
    "play", "player", "music", "video", "browser", "cloud", "login", "password", "email", "chat", "bot", "app", "apps",
    "web", "site", "blog", "release", "notes", "week", "blue", "screen", "task", "manager", "control", "panel", "mark",
    "stark", "tony", "iron", "man", "service", "node", "build", "bug", "all", "com", "process", "error", "warning",
    "dota", "android", "amazon",
];

/// Варианты распознавания (кириллица, слова через пробел) → написание. Приоритетнее словаря.
const VARIANTS: &[(&str, &str)] = &[
    ("джсон", "JSON"),
    ("джейсан", "JSON"),
    ("джисон", "JSON"),
    ("джейсон", "JSON"),
    ("конфик", "config"),
    ("конфиг", "config"),
    ("пайтон", "Python"),
    ("повершелл", "PowerShell"),
    ("пауэр шелл", "PowerShell"),
    ("вс код", "VS Code"),
    ("вэс код", "VS Code"),
    ("виэс код", "VS Code"),
    ("ви эс код", "VS Code"),
    ("визуал студио код", "Visual Studio Code"),
    ("вижуал студио код", "Visual Studio Code"),
    ("гит хаб", "GitHub"),
    ("гит лаб", "GitLab"),
    ("гит пуш", "git push"),
    ("гит пулл", "git pull"),
    ("гит коммит", "git commit"),
    ("гит клон", "git clone"),
    ("пул реквест", "pull request"),
    ("пулл реквест", "pull request"),
    ("пул риквест", "pull request"),
    ("эн пи эм", "npm"),
    ("локал хост", "localhost"),
    ("вай фай", "Wi-Fi"),
    ("вайфай", "Wi-Fi"),
    ("блютуз", "Bluetooth"),
    ("чат гпт", "ChatGPT"),
    ("чат джипити", "ChatGPT"),
    ("ютюб", "YouTube"),
    ("гугл хром", "Google Chrome"),
    ("докер", "Docker"),
    ("карго", "cargo"),
    ("раст", "Rust"),
];

/// Написание с нужными заглавными (ключ — нижний регистр словаря).
const CASING: &[(&str, &str)] = &[
    ("powershell", "PowerShell"), ("github", "GitHub"), ("gitlab", "GitLab"), ("youtube", "YouTube"), ("json", "JSON"),
    ("yaml", "YAML"), ("toml", "TOML"), ("api", "API"), ("vscode", "VS Code"), ("vs code", "VS Code"),
    ("chatgpt", "ChatGPT"), ("chat gpt", "ChatGPT"), ("openai", "OpenAI"), ("open ai", "OpenAI"), ("iphone", "iPhone"),
    ("whatsapp", "WhatsApp"), ("tiktok", "TikTok"), ("onedrive", "OneDrive"), ("teamviewer", "TeamViewer"),
    ("anydesk", "AnyDesk"), ("javascript", "JavaScript"), ("typescript", "TypeScript"), ("geforce", "GeForce"),
    ("nvidia", "NVIDIA"), ("deepseek", "DeepSeek"), ("openrouter", "OpenRouter"), ("readme", "README"),
    ("powerpoint", "PowerPoint"), ("winrar", "WinRAR"), ("visual studio code", "Visual Studio Code"),
    ("google chrome", "Google Chrome"), ("microsoft edge", "Microsoft Edge"),
];

/// Пишутся строчными (команды и технические слова, не названия).
const LOWER: &[&str] = &[
    "commit", "push", "merge", "branch", "config", "localhost", "npm", "cargo", "pull request", "frontend", "backend",
    "deploy", "debug", "ipconfig", "spooler",
];

fn display(en: &str) -> String {
    if let Some((_, d)) = CASING.iter().find(|(k, _)| *k == en) {
        return d.to_string();
    }
    if LOWER.contains(&en) {
        return en.to_string();
    }
    en.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Свёрнутая форма: нижний регистр, ё/э → е, ы → и, без ь/ъ/ударения/пробелов/дефисов, двойные буквы — одна.
fn fold(w: &str) -> String {
    let mut out = String::with_capacity(w.len());
    let mut last = None;
    for c in w.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'ё' | 'э' => 'е',
            'ы' => 'и',
            'ь' | 'ъ' | '\u{301}' | '+' | '-' | ' ' => continue,
            c => c,
        };
        if last != Some(c) {
            out.push(c);
        }
        last = Some(c);
    }
    out
}

/// «Как слышится»: свёрнутая форма + о → а, е → и, звонкая на конце → глухая («дискорд» ≈ «дискарт»).
fn phonetic(folded: &str) -> String {
    let mut cs: Vec<char> = folded
        .chars()
        .map(|c| match c {
            'о' => 'а',
            'е' => 'и',
            c => c,
        })
        .collect();
    if let Some(l) = cs.last_mut() {
        *l = match *l {
            'д' => 'т',
            'г' => 'к',
            'б' => 'п',
            'з' => 'с',
            'в' => 'ф',
            'ж' => 'ш',
            c => c,
        };
    }
    cs.into_iter().collect()
}

fn lev(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(x != y)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

struct Dict {
    /// Свёрнутое чтение (слова слитно) → написание.
    exact: HashMap<String, String>,
    /// Чтения от 6 букв для нечёткого сравнения: (свёрнутое, «как слышится», написание).
    long: Vec<(Vec<char>, String, String)>,
    /// Наибольшее число слов в чтении.
    max_words: usize,
}

static DICT: LazyLock<Dict> = LazyLock::new(|| {
    let mut exact = HashMap::new();
    let mut max_words = 1;
    let mut add = |ru: &str, out: String| {
        max_words = max_words.max(ru.split([' ', '-']).filter(|w| !w.is_empty()).count());
        exact.entry(fold(ru)).or_insert(out);
    };
    for (ru, en) in VARIANTS {
        add(ru, en.to_string());
    }
    for (en, ru) in english::dictionary() {
        if !SKIP.contains(&en) {
            add(&ru, display(en));
        }
    }
    for (canon, aliases) in NAME_HINTS {
        if !canon.is_ascii() {
            continue; // «Джарвис», «Блокнот», «диск C» — не латиница
        }
        for a in aliases.iter().filter(|a| !a.is_ascii()) {
            add(a, canon.to_string());
        }
    }
    let long = exact
        .iter()
        .filter(|(k, _)| k.chars().count() >= 6)
        .map(|(k, v)| (k.chars().collect(), phonetic(k), v.clone()))
        .collect();
    Dict { exact, long, max_words }
});

/// Падежные окончания, которые отбрасываются у названия («в дискорде», «на ютубе», «с гитхаба»).
const ENDINGS: &[&str] = &["ами", "ах", "ам", "ом", "ой", "ов", "е", "а", "у", "и"];

fn is_wake(folded: &str) -> bool {
    folded.starts_with("джарв") || folded.starts_with("джерв")
}

fn lookup_word(w: &str) -> Option<String> {
    let d = &*DICT;
    let f = fold(w);
    if is_wake(&f) {
        return None;
    }
    if let Some(v) = d.exact.get(&f) {
        return Some(v.clone());
    }
    for e in ENDINGS {
        if let Some(stem) = f.strip_suffix(e)
            && stem.chars().count() >= 4
            && let Some(v) = d.exact.get(stem)
        {
            return Some(v.clone());
        }
    }
    let fc: Vec<char> = f.chars().collect();
    if fc.len() < 6 {
        return None;
    }
    let ph = phonetic(&f);
    let mut found: Option<&String> = None;
    for (k, kph, v) in &d.long {
        if lev(&fc, k) <= 1 || *kph == ph {
            match found {
                Some(prev) if prev != v => return None, // неоднозначно — не трогать
                _ => found = Some(v),
            }
        }
    }
    found.cloned()
}

fn is_cyr_word(w: &str) -> bool {
    w.chars().all(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё'))
}

static RE_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9_-]*)\s+(?:точка\s+)?(json|toml|yaml)\b").unwrap());
static RE_DOT_EXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(^|\s)точка\s+(json|toml|yaml)\b").unwrap());
static RE_DOMAIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9-]*)\s+(?:(?:точка\s+)?(ком|орг)|точка\s+(ру|нет))\b").unwrap());
static RE_DRIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(диск|диска|диске|диску|диском)\s+(си|ди|це|цэ)\b").unwrap());
/// «диск д»/«диск с» — только в конце фразы, перед знаком или «и» (иначе «с» — предлог: «диск с фильмами»).
static RE_DRIVE_SHORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(диск|диска|диске|диску|диском)\s+(с|д)(\s*$|\s*[,.!?;:]|\s+и\s)").unwrap());

/// Склеиваются в имя файла только термины-имена (config.json, Cargo.toml), а не любое латинское слово.
const FILE_STEMS: &[&str] = &["config", "cargo", "package", "tsconfig", "settings", "tauri"];

fn post(s: &str) -> String {
    let s = RE_FILE.replace_all(s, |c: &Captures| {
        let (stem, ext) = (&c[1], c[2].to_lowercase());
        if !FILE_STEMS.contains(&stem.to_lowercase().as_str()) {
            return c[0].to_string();
        }
        let stem = if stem.eq_ignore_ascii_case("cargo") && ext == "toml" { "Cargo".to_string() } else { stem.to_lowercase() };
        format!("{stem}.{ext}")
    });
    let s = RE_DOT_EXT.replace_all(&s, |c: &Captures| format!("{}.{}", &c[1], c[2].to_lowercase()));
    let s = RE_DOMAIN.replace_all(&s, |c: &Captures| {
        let tld = match c.get(2).or(c.get(3)).map(|m| m.as_str().to_lowercase()).as_deref() {
            Some("ком") => "com",
            Some("орг") => "org",
            Some("ру") => "ru",
            _ => "net",
        };
        format!("{}.{tld}", c[1].to_lowercase())
    });
    let letter = |l: &str| match l.to_lowercase().as_str() {
        "си" | "це" | "цэ" | "с" => "C",
        _ => "D",
    };
    let s = RE_DRIVE.replace_all(&s, |c: &Captures| format!("{} {}", &c[1], letter(&c[2])));
    let s = RE_DRIVE_SHORT.replace_all(&s, |c: &Captures| format!("{} {}{}", &c[1], letter(&c[2]), &c[3]));
    s.into_owned()
}

/// Вернуть английским терминам исходное написание. Текст без кириллических терминов не меняется.
pub fn apply(text: &str) -> String {
    // Слова (буквы/цифры) и их байтовые границы.
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            words.push((s, i));
        }
    }
    if let Some(s) = start {
        words.push((s, text.len()));
    }
    let d = &*DICT;
    let mut out = String::with_capacity(text.len() + 8);
    let mut last = 0;
    let mut i = 0;
    while i < words.len() {
        let mut matched = None;
        // Самые длинные фразы первыми; внутри фразы между словами — только пробелы/дефисы.
        for n in (1..=d.max_words.min(words.len() - i)).rev() {
            let span = &words[i..i + n];
            if !span.iter().all(|&(s, e)| is_cyr_word(&text[s..e])) {
                continue;
            }
            if !span.windows(2).all(|w| text[w[0].1..w[1].0].chars().all(|c| c.is_whitespace() || c == '-')) {
                continue;
            }
            let hit = if n == 1 {
                lookup_word(&text[span[0].0..span[0].1])
            } else {
                let joined: String = span.iter().map(|&(s, e)| &text[s..e]).collect();
                let f = fold(&joined);
                if is_wake(&f) { None } else { d.exact.get(&f).cloned() }
            };
            if let Some(h) = hit {
                matched = Some((n, h));
                break;
            }
        }
        match matched {
            Some((n, h)) => {
                out.push_str(&text[last..words[i].0]);
                out.push_str(&h);
                last = words[i + n - 1].1;
                i += n;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[last..]);
    post(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_examples() {
        assert_eq!(apply("открой пауэршелл"), "открой PowerShell");
        assert_eq!(apply("открой конфиг джейсон"), "открой config.json");
        assert_eq!(apply("открой конфиг точка джейсон"), "открой config.json");
        assert_eq!(apply("запусти вэс код"), "запусти VS Code");
        assert_eq!(apply("зайди на гитхаб"), "зайди на GitHub");
        assert_eq!(apply("зайди на гит хаб"), "зайди на GitHub");
    }

    #[test]
    fn variants_endings_and_fuzzy() {
        assert_eq!(apply("Джарвис, напиши в дискорде"), "Джарвис, напиши в Discord");
        assert_eq!(apply("джарвис включи дискарт"), "джарвис включи Discord");
        assert_eq!(apply("найди на ютубе обзор"), "найди на YouTube обзор");
        assert_eq!(apply("запусти павер шелл"), "запусти PowerShell");
        assert_eq!(apply("выключи вай-фай и блютуз"), "выключи Wi-Fi и Bluetooth");
        assert_eq!(apply("проверь виндоус"), "проверь Windows");
        assert_eq!(apply("сделай гит пуш"), "сделай git push");
        assert_eq!(apply("создай пул реквест"), "создай pull request");
        assert_eq!(apply("сохрани как точка джейсон"), "сохрани как .json");
        assert_eq!(apply("открой сайт гитхаб ком"), "открой сайт github.com");
        assert_eq!(apply("открой диск ди и папку даунлоудс"), "открой диск D и папку Downloads");
        assert_eq!(apply("сколько места на диске си"), "сколько места на диске C");
        assert_eq!(apply("открой диск д"), "открой диск D");
        assert_eq!(apply("запусти эн пи эм"), "запусти npm");
        assert_eq!(apply("Джарвис, запусти стим."), "Джарвис, запусти Steam.");
    }

    #[test]
    fn russian_text_untouched() {
        for s in [
            "Джарвис, открой браузер",
            "джарвиз открой блокнот",
            "шелест листьев за окном",
            "купи конфету",
            "гит",
            "напиши код",
            "открой чат",
            "это мой сайт",
            "диск с фильмами",
            "поставь таймер на пятнадцать минут",
            "питон ползёт по траве",
            "сходи в храм",
            "он сказал нет",
            "покер с друзьями",
            "Джарвис, запусти Steam",
            "проверь config.json",
            "",
        ] {
            assert_eq!(apply(s), s, "«{s}» не должна меняться");
        }
    }

    #[test]
    fn wake_word_never_changes() {
        for s in ["Джарвис", "джарвис,", "Джарвиса", "ДЖАРВИС открой", "джервис"] {
            assert_eq!(apply(s), s);
        }
    }
}
