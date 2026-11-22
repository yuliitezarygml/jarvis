//! Мелкие текстовые утилиты.

/// Схлопнуть пробельные символы в один пробел.
pub(crate) fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Обрезать до `max` символов с «…» (для сниппетов и сообщений об ошибках).
pub(crate) fn truncate_plain(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Текст HTML-фрагмента (`<b>Rust</b> &amp; Co` → `Rust & Co`) без лишних пробелов.
pub(crate) fn html_fragment_text(s: &str) -> String {
    if !s.contains(['<', '&']) {
        return collapse_ws(s);
    }
    let doc = dom_query::Document::fragment(s);
    collapse_ws(&doc.text())
}

/// Обрезать Markdown до `max` символов по границе абзаца/строки/слова с пометкой обрезки.
/// Возвращает (текст, обрезан ли).
pub(crate) fn truncate_markdown(s: &str, max: usize) -> (String, bool) {
    let total = s.chars().count();
    if total <= max {
        return (s.to_string(), false);
    }
    let byte_end = s.char_indices().nth(max).map_or(s.len(), |(i, _)| i);
    let head = &s[..byte_end];
    // Ищем красивую границу в последних 20 % куска.
    let min_cut = head.len() * 4 / 5;
    let cut = [head.rfind("\n\n"), head.rfind('\n'), head.rfind(' ')]
        .into_iter()
        .flatten()
        .find(|&i| i >= min_cut)
        .unwrap_or(head.len());
    let kept = head[..cut].trim_end();
    let shown = kept.chars().count();
    (
        format!("{kept}\n\n[…текст обрезан: показано {shown} из {total} символов]"),
        true,
    )
}

/// Привести Markdown/текст в порядок: без хвостовых пробелов, не больше одной пустой строки подряд.
pub(crate) fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0usize;
    let mut in_code = false;
    for line in s.lines() {
        let line = line.trim_end();
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if line.trim().is_empty() && !in_code {
            blank += 1;
            if blank > 1 || out.is_empty() {
                continue;
            }
            out.push('\n');
            continue;
        }
        blank = 0;
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Русское множественное число: 1 результат, 2 результата, 5 результатов.
pub(crate) fn plural_ru<'a>(n: usize, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let (n10, n100) = (n % 10, n % 100);
    if n10 == 1 && n100 != 11 {
        one
    } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
        few
    } else {
        many
    }
}

/// Сегодняшняя дата (UTC) `YYYY-MM-DD` — модели нужна для запросов «что нового».
pub(crate) fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Дни от 1970-01-01 → (год, месяц, день) по алгоритму Говарда Хиннанта.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_text() {
        assert_eq!(
            html_fragment_text("<b>Rust</b>  is &amp; was\n fast"),
            "Rust is & was fast"
        );
        assert_eq!(html_fragment_text("plain   text"), "plain text");
    }

    #[test]
    fn truncation_marks_and_respects_limit() {
        let s = "абзац один.\n\n".repeat(100);
        let (t, cut) = truncate_markdown(&s, 300);
        assert!(cut);
        assert!(t.contains("[…текст обрезан: показано"));
        let body = t.split("\n\n[…текст обрезан").next().unwrap();
        assert!(body.chars().count() <= 300);
        assert!(body.ends_with("один."));
        let (t2, cut2) = truncate_markdown("коротко", 300);
        assert_eq!((t2.as_str(), cut2), ("коротко", false));
    }

    #[test]
    fn tidy_collapses_blank_lines() {
        assert_eq!(
            tidy("\n\na  \n\n\n\nb\n```\n\n\ncode\n```\n"),
            "a\n\nb\n```\n\n\ncode\n```"
        );
    }

    #[test]
    fn plurals() {
        assert_eq!(
            plural_ru(1, "результат", "результата", "результатов"),
            "результат"
        );
        assert_eq!(
            plural_ru(3, "результат", "результата", "результатов"),
            "результата"
        );
        assert_eq!(
            plural_ru(8, "результат", "результата", "результатов"),
            "результатов"
        );
        assert_eq!(
            plural_ru(11, "результат", "результата", "результатов"),
            "результатов"
        );
        assert_eq!(
            plural_ru(22, "результат", "результата", "результатов"),
            "результата"
        );
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_414), (2025, 11, 22));
        assert_eq!(today_utc().len(), 10);
    }
}
