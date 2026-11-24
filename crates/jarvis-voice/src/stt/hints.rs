//! Словарь-подсказка для облачного распознавания: названия программ и сайтов, которые хозяин произносит
//! (VS Code, GitHub, Discord…), плюс свои слова из `config.stt.hints` и то, что передаст приложение
//! (например, имена из памяти). Уходит в параметр `prompt` Whisper — меньше «вэс код» и «комид».
//!
//! Здесь же — разговорные написания этих названий (как их пишет GigaAM кириллицей): их использует замер
//! точности (`stt::bench`), чтобы «вс код» и «VS Code» считались одним и тем же словом.

/// (как пишется, как ещё может записать распознаватель). Порядок — по важности: при нехватке места
/// в подсказке Whisper отрезаются последние.
pub const NAME_HINTS: &[(&str, &[&str])] = &[
    ("Джарвис", &["jarvis", "джервис"]),
    ("VS Code", &["вс код", "вэс код", "вскод", "ви эс код", "вс-код", "vscode"]),
    ("GitHub", &["гитхаб", "гит хаб"]),
    ("Discord", &["дискорд"]),
    ("Steam", &["стим"]),
    ("Telegram", &["телеграм", "телеграмм"]),
    ("Chrome", &["хром"]),
    ("YouTube", &["ютуб", "ютюб", "ютубе", "ютьюб"]),
    ("Блокнот", &["notepad"]),
    ("PowerShell", &["павершелл", "пауэршелл", "повершелл", "пауэр шелл", "павер шелл", "power shell"]),
    ("Windows", &["виндовс", "виндоус"]),
    ("Wi-Fi", &["вайфай", "вай фай", "wifi", "вай-фай"]),
    ("Bluetooth", &["блютуз", "блютус", "блютуc"]),
    ("Spotify", &["спотифай"]),
    ("Firefox", &["файрфокс", "фаерфокс"]),
    ("Google", &["гугл"]),
    ("Яндекс", &["yandex"]),
    ("ChatGPT", &["чат джипити", "чатгпт", "чат гпт"]),
    ("Word", &["ворд"]),
    ("Excel", &["эксель"]),
    ("OBS", &["обс"]),
    ("Twitch", &["твич"]),
    ("WhatsApp", &["ватсап", "вотсап"]),
    ("Zoom", &["зум"]),
    ("Photoshop", &["фотошоп"]),
    ("Minecraft", &["майнкрафт"]),
    ("Gmail", &["джимейл"]),
    ("Rust", &["раст"]),
    ("Downloads", &["даунлоадс", "даунлоудс"]),
    ("диск C", &["диск с", "диск си"]),
    ("диск D", &["диск д", "диск ди"]),
];

/// Короткие слова-подсказки без вариантов написания (буквы дисков и частые термины).
pub const EXTRA_HINTS: &[&str] = &["диспетчер задач", "командная строка", "проводник"];

/// Список подсказок: свои слова (`config.stt.hints`) → переданные приложением → встроенный словарь.
/// Без повторов (без учёта регистра), пустые строки отброшены.
pub fn stt_hints(config_hints: &[String], extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let builtin = NAME_HINTS.iter().map(|(n, _)| *n).chain(EXTRA_HINTS.iter().copied());
    for h in config_hints.iter().map(String::as_str).chain(extra.iter().map(String::as_str)).chain(builtin) {
        let h = h.trim();
        if !h.is_empty() && seen.insert(h.to_lowercase()) {
            out.push(h.to_string());
        }
    }
    out
}

/// Грубая оценка числа токенов Whisper (с запасом): кириллица — ~2 символа на токен, латиница/цифры — ~3.
pub fn estimate_tokens(s: &str) -> usize {
    let mut t = 0.0f32;
    for c in s.chars() {
        t += if c.is_alphabetic() && !c.is_ascii() {
            0.6
        } else if c.is_ascii_alphanumeric() {
            0.35
        } else if c == ' ' {
            0.1
        } else {
            0.5
        };
    }
    t.ceil() as usize
}

/// Текст `prompt` для Whisper: подсказка стиля + словарь через запятую, не длиннее `max_tokens`
/// (у Groq предел 224 токена; лишние подсказки с конца отбрасываются целиком).
pub fn build_prompt(style: &str, hints: &[String], max_tokens: usize) -> String {
    let mut out = style.trim().to_string();
    let mut used = estimate_tokens(&out);
    let mut first = true;
    for h in hints {
        let piece = if first { format!(" {h}") } else { format!(", {h}") };
        let cost = estimate_tokens(&piece);
        if used + cost + 1 > max_tokens {
            continue; // короткая подсказка дальше по списку ещё может влезть
        }
        used += cost;
        out.push_str(&piece);
        first = false;
    }
    if !first {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_dedup_and_order() {
        let h = stt_hints(&["Кузя".into(), " github ".into(), "".into()], &["Мама Лена".into()]);
        assert_eq!(&h[..3], ["Кузя", "github", "Мама Лена"]);
        assert!(!h.iter().any(|x| x == "GitHub"), "встроенный GitHub не дублирует свой «github»");
        assert!(h.iter().any(|x| x == "VS Code") && h.iter().any(|x| x == "диск C"));
    }

    #[test]
    fn prompt_fits_groq_limit() {
        let many: Vec<String> = (0..400).map(|i| format!("Программа номер {i}")).collect();
        let hints = stt_hints(&many, &[]);
        let p = build_prompt("Джарвис, открой браузер.", &hints, 200);
        assert!(estimate_tokens(&p) <= 200, "{}", estimate_tokens(&p));
        assert!(p.starts_with("Джарвис, открой браузер. Программа номер 0, Программа номер 1"));
        assert!(p.ends_with('.'));
        // Встроенный словарь целиком влезает.
        let p = build_prompt("Джарвис, открой браузер.", &stt_hints(&[], &[]), 200);
        assert!(p.contains("VS Code, GitHub") && p.contains("Downloads"), "{p}");
        assert_eq!(build_prompt("Стиль.", &[], 200), "Стиль.");
    }
}
