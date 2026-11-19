//! Incremental, turn-local reader. The original Groq deltas never pass through
//! this module on their way to the HUD or SQLite.
mod markdown;
mod numbers;
mod words;

const TARGET_CHARS: usize = 260;
const MIN_CHARS: usize = 38;

#[derive(Default)]
pub struct SpeechNormalizer {
    concise_commands: bool,
    line: String,
    // Multiline markup, including formulas and tables, can span SSE messages.
    markup: String,
    table: Option<String>,
    table_has_rows: bool,
    fence: Option<(char, usize)>,
    pending: String,
    line_chars: usize,
    structured_line: bool,
}

fn fence(s: &str) -> Option<(char, usize)> {
    let c = s.chars().next()?;
    if !matches!(c, '`' | '~') {
        return None;
    }
    let count = s.chars().take_while(|&x| x == c).count();
    (count >= 3).then_some((c, count))
}

fn table_separator(s: &str) -> bool {
    s.contains('|')
        && s.trim_matches('|').split('|').all(|col| {
            let dashes = col.trim().trim_matches(':');
            !dashes.is_empty() && dashes.chars().all(|c| c == '-')
        })
}

fn open_markup(s: &str) -> bool {
    // Backslash delimiters must be paired before Markdown unescapes them.
    let mut dollars = None;
    let mut ticks = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && ticks.is_none() {
            chars.next();
            continue;
        }
        if !matches!(c, '`' | '$') {
            continue;
        }
        let mut n = 1;
        while chars.peek() == Some(&c) {
            chars.next();
            n += 1
        }
        let marker = if c == '`' && dollars.is_none() {
            &mut ticks
        } else if c == '$' && ticks.is_none() {
            &mut dollars
        } else {
            continue;
        };
        if *marker == Some(n) {
            *marker = None
        } else if marker.is_none() {
            *marker = Some(n)
        }
    }
    dollars.is_some()
        || ticks.is_some()
        || [("\\[", "\\]"), ("\\(", "\\)")]
            .iter()
            .any(|(l, r)| s.matches(l).count() > s.matches(r).count())
}

// A logical Markdown line can contain several complete sentences. Release them
// as soon as their inline delimiters close, leaving table and block syntax intact.
fn can_cut_inline(s: &str) -> bool {
    !s.contains('|')
        && !s.trim_start().starts_with(['#', '<', '~'])
        && !s.contains("<script")
        && !s.contains("<style")
        && s.matches('[').count() == s.matches(']').count()
        && s.matches('(').count() == s.matches(')').count()
        && s.matches("**").count().is_multiple_of(2)
        && s.matches('*').count().is_multiple_of(2)
        && s.matches('_').count().is_multiple_of(2)
        && !open_markup(s)
}

impl SpeechNormalizer {
    pub fn for_desktop() -> Self {
        Self {
            concise_commands: cfg!(windows),
            ..Self::default()
        }
    }
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        let mut ready = Vec::new();
        for c in delta.chars() {
            if c == '\n' {
                self.end_line(&mut ready);
            } else {
                // Look ahead by one separator so a period inside a decimal,
                // version, URL or list marker cannot end the spoken phrase.
                if c.is_whitespace()
                    && self.markup.is_empty()
                    && self.table.is_none()
                    && self.fence.is_none()
                {
                    let last = self.line.chars().next_back().unwrap_or(' ');
                    let sentence = self.line_chars > 2
                        && matches!(last, '.' | '!' | '?' | '…')
                        && !self
                            .line
                            .trim()
                            .trim_end_matches('.')
                            .bytes()
                            .all(|b| b.is_ascii_digit());
                    let long = self.line_chars >= TARGET_CHARS
                        && (!self.structured_line || self.line_chars % 64 < 8)
                        && (last == ',' || last.is_alphabetic() && !last.is_ascii());
                    if (sentence || long) && (!self.structured_line || can_cut_inline(&self.line)) {
                        let raw = std::mem::take(&mut self.line);
                        self.line_chars = 0;
                        self.structured_line = false;
                        self.enqueue(&markdown::render(&raw, self.concise_commands), &mut ready);
                    }
                }
                self.structured_line |= matches!(
                    c,
                    '|' | '$' | '`' | '*' | '_' | '[' | '<' | '\\' | '~' | '#'
                );
                self.line.push(c);
                self.line_chars += 1;
            }
        }
        ready
    }

    fn end_line(&mut self, ready: &mut Vec<String>) {
        self.structured_line = false;
        self.line_chars = 0;
        let line = std::mem::take(&mut self.line);
        let trimmed = line.trim();
        if let Some((marker, size)) = self.fence {
            if trimmed.len() >= size && trimmed.chars().all(|c| c == marker) {
                self.fence = None;
            } else {
                self.enqueue(&words::literal(&line), ready)
            }
            return;
        }
        if self.markup.is_empty() {
            if let Some(f) = fence(trimmed) {
                self.close_table(ready);
                self.fence = Some(f);
                return;
            }
        }
        if let Some(header) = &self.table {
            if trimmed.contains('|') && !trimmed.is_empty() {
                self.table_has_rows = true;
                let spoken = markdown::render(&format!("{header}\n{line}"), self.concise_commands);
                self.enqueue(&spoken, ready);
                return;
            }
            self.close_table(ready);
        }
        if !self.markup.is_empty() {
            if table_separator(trimmed) && self.markup.contains('|') && !self.markup.contains('\n')
            {
                self.table = Some(format!("{}\n{line}", std::mem::take(&mut self.markup)));
                self.table_has_rows = false;
                return;
            }
            // A non-table pipe line had been held for the header delimiter.
            if self.markup.contains('|')
                && !open_markup(&self.markup)
                && !self.markup.trim_start().starts_with('<')
            {
                self.emit_markup(ready);
            }
        }
        if !self.markup.is_empty() {
            self.markup.push('\n')
        }
        self.markup.push_str(&line);
        let html_block = self.markup.trim_start().starts_with('<') && !trimmed.is_empty();
        let unfinished_math = trimmed.ends_with(['^', '=', '×', '÷', '√', '≥', '≤', '−']);
        if !html_block && !unfinished_math && !open_markup(&self.markup) && !trimmed.contains('|') {
            self.emit_markup(ready)
        }
    }

    fn emit_markup(&mut self, ready: &mut Vec<String>) {
        let spoken = markdown::render(&std::mem::take(&mut self.markup), self.concise_commands);
        self.enqueue(&spoken, ready);
    }
    fn close_table(&mut self, ready: &mut Vec<String>) {
        if let Some(header) = self.table.take() {
            if !self.table_has_rows {
                self.enqueue(&markdown::render(&header, self.concise_commands), ready)
            }
        }
    }
    fn enqueue(&mut self, spoken: &str, ready: &mut Vec<String>) {
        if !spoken.chars().any(char::is_alphanumeric) {
            return;
        }
        let mut count = self.pending.chars().count();
        for word in spoken.split_whitespace() {
            let n = word.chars().count();
            if count > 0 && count + 1 + n > TARGET_CHARS {
                ready.push(std::mem::take(&mut self.pending));
                count = 0;
            }
            if count > 0 {
                self.pending.push(' ');
                count += 1
            }
            for c in word.chars() {
                if count == TARGET_CHARS {
                    ready.push(std::mem::take(&mut self.pending));
                    count = 0
                }
                self.pending.push(c);
                count += 1;
            }
        }
        if count >= MIN_CHARS {
            ready.push(std::mem::take(&mut self.pending))
        }
    }
    pub fn finish(&mut self) -> Vec<String> {
        let mut ready = Vec::new();
        self.end_line(&mut ready);
        self.close_table(&mut ready);
        self.emit_markup(&mut ready);
        if !self.pending.is_empty() {
            ready.push(std::mem::take(&mut self.pending))
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunks(text: &str, fragmented: bool) -> Vec<String> {
        let mut r = SpeechNormalizer::default();
        let mut result = Vec::new();
        if fragmented {
            for c in text.chars() {
                result.extend(r.push(c.encode_utf8(&mut [0; 4])))
            }
        } else {
            result.extend(r.push(text))
        }
        result.extend(r.finish());
        result
    }
    fn says(src: &str, expected: &str) {
        assert_eq!(chunks(src, true).join(" "), expected, "{src}");
    }
    #[test]
    fn math_agreement_and_values() {
        for (raw, spoken) in [
            ("3²", "три в квадрате"),
            ("3^2", "три во второй степени"),
            ("3³ = 27", "три в кубе равно двадцати семи"),
            (
                "2^10 = 1024",
                "два в десятой степени равно тысяче двадцати четырём",
            ),
            ("√16 = 4", "квадратный корень из шестнадцати равен четырём"),
            ("5 × 8 = 40", "пять умножить на восемь равно сорока"),
            ("12 ÷ 3 = 4", "двенадцать разделить на три равно четырём"),
            ("2/3", "две третьих"),
            ("25%", "двадцать пять процентов"),
            ("π и ∞", "пи и бесконечность"),
            ("3 > 2", "три больше двух"),
            ("2 - 5 = -3", "два минус пять равно минус трём"),
            (
                "−2345006",
                "минус два миллиона триста сорок пять тысяч шесть",
            ),
            ("3,05", "три целых пять сотых"),
            ("1.01", "одна целая одна сотая"),
            ("21%", "двадцать один процент"),
            ("104/125", "сто четыре сто двадцать пятых"),
            ("2^0", "два в нулевой степени"),
        ] {
            says(raw, spoken)
        }
    }

    #[test]
    fn preserves_meaning_at_format_boundaries() {
        says("2*3*4", "два умножить на три умножить на четыре");
        says(
            "2^\n10 = 1024",
            "два в десятой степени равно тысяче двадцати четырём",
        );
        says("1 024 000", "один миллион двадцать четыре тысячи");
        says("Раздел 2/3", "Раздел два слэш три");
        says("AES-256", "эй и эс двести пятьдесят шесть");
        says("На 2 устройствах", "На 2 устройствах");
        says(
            "[https://x.ru/api/2](https://x.ru/api/2)",
            "эйч-ти-ти-пи-эс двоеточие слэш слэш x точка ru слэш эй-пи-ай слэш два",
        );
        let mut reader = SpeechNormalizer::default();
        let early = reader.push("**Проверка сети** завершилась успешно, соединение работает. ");
        assert_eq!(
            early,
            ["Проверка сети завершилась успешно, соединение работает."]
        );
    }

    #[test]
    fn technical_reading_is_separate() {
        for name in ["Wi-Fi", "Wi‑Fi", "Wi–Fi", "Wi—Fi"] {
            says(name, "вай-фай")
        }
        says(
            "VPN-протоколы, WPA3 и USB.",
            "ви-пи-эн протоколы, дабл-ю пи эй три и ю-эс-би.",
        );
        says(
            "2026-11-19",
            "два ноль два шесть дефис один один дефис один девять",
        );
        says("v1.2.3", "v один точка два точка три");
        says("`2/3`", "два слэш три");
        says(
            "`mod_02 --flag`",
            "mod нижнее подчёркивание ноль два дефис дефис flag",
        );
        says("/api/v1/2", "слэш эй-пи-ай слэш v один слэш два");
        says("HTTP/2", "эйч-ти-ти-пи слэш два");
        says("Версия 2.5", "Версия два точка пять");
    }

    #[test]
    fn markup_and_tables() {
        says(
            "# Сеть\n\n1. **Wi-Fi**\n2. *VPN-протоколы*\n\n---\n[Справка](https://example.org/v2).",
            "Сеть. вай-фай. ви-пи-эн протоколы. Справка.",
        );
        says(
            "| Сеть | Режим |\n|---|---|\n| Wi-Fi | WPA3 |\n| VPN | TLS |",
            "Сеть: вай-фай, Режим: дабл-ю пи эй три. Сеть: ви-пи-эн, Режим: ти-эл-эс.",
        );
        says(
            "<div><b>Корень</b> из числа.</div><script>bad();</script>",
            "Корень из числа.",
        );
        says(
            "Текст <b>важен</b>, а <script>bad</script>код скрыт.",
            "Текст важен, а код скрыт.",
        );
        says("```sh\ncurl --version\n```", "curl дефис дефис version");
        says("Команда <code>2/3</code>.", "Команда два слэш три.");
        says("<table><tr><th>Сеть</th><th>Режим</th></tr><tr><td>Wi-Fi</td><td>WPA3</td></tr></table>","Сеть: вай-фай, Режим: дабл-ю пи эй три.");
        says(
            r"$\frac{2}{3}$ и \(\sqrt{16} = 4\)",
            "две третьих и квадратный корень из шестнадцати равен четырём",
        );
        says(
            r"$$2^{10} = 1024$$",
            "два в десятой степени равно тысяче двадцати четырём",
        );
    }
    #[test]
    fn fragments_stream_early_and_preserve_order() {
        let prefix = "Сначала проверим защищённое соединение с сервером. ";
        let mut r = SpeechNormalizer::default();
        assert_eq!(r.push(prefix), [prefix.trim()]);
        for part in ["Показатель: $2^", "{1", "0} = ", "10", "24"] {
            assert!(r.push(part).is_empty())
        }
        let mut rest = r.push("$. Следующее предложение.");
        rest.extend(r.finish());
        assert_eq!(rest.join(" "),"Показатель: два в десятой степени равно тысяче двадцати четырём. Следующее предложение.");
        let mixed=format!("{prefix}{}","\n### Сеть\nПроверка **Wi-Fi**: $3³ = 27$.\n\n| Вид | Ответ |\n|---|---|\n| Корень | √16 = 4 |\n\n```sh\ncurl --version\n```\n".repeat(65));
        let batch = chunks(&mixed, false);
        assert_eq!(batch, chunks(&mixed, true));
        assert!(batch.len() > 48);
        assert!(batch
            .iter()
            .all(|s| !s.is_empty() && s.chars().count() <= TARGET_CHARS));
        let speech = batch.join(" ");
        assert_eq!(speech.matches("двадцати семи").count(), 65);
        assert_eq!(speech.matches("равен четырём").count(), 65);
        assert_eq!(speech.matches("дефис дефис version").count(), 65);
    }
}
