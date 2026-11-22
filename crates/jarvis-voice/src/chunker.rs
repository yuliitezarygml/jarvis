//! `PhraseChunker`: дельты потока LLM → законченные фразы для TTS (≤ `max_len` символов) по границам предложений.
//!
//! Не рвёт «3.14», «т. е.», «и т. д.» (в середине фразы), инициалы, URL и числа с пробелами-разделителями тысяч.
//! Понимает Markdown на уровне строк: строка = граница фразы, блок кода и таблица выдаются целиком
//! (нормализатор превратит их в «пример кода» и «столбец: значение»).

pub const MAX_PHRASE_CHARS: usize = 260;

/// Сокращения, после точки которых фраза не заканчивается (сравнение без точки, в нижнем регистре).
const NEVER_END: &[&str] = &[
    "т", "т.е", "т.к", "т.н", "т.ч", "е", "к", "н", "им", "ул", "см", "напр", "рис", "стр", "тыс", "млн", "млрд",
    "руб", "коп", "св", "проф", "акад", "доц", "ст", "англ", "лат", "нем", "франц", "mr", "mrs", "ms", "dr", "prof",
    "vs", "e.g", "i.e", "no", "№",
];

#[derive(Debug)]
enum Block {
    None,
    Code { fence: &'static str, text: String },
    Table { lines: Vec<String> },
}

#[derive(Debug)]
pub struct PhraseChunker {
    max_len: usize,
    line: String,
    /// Байтовое смещение в `line`, до которого текст уже выдан.
    start: usize,
    block: Block,
}

impl Default for PhraseChunker {
    fn default() -> Self {
        Self::new()
    }
}

fn fence_of(t: &str) -> Option<&'static str> {
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

impl PhraseChunker {
    pub fn new() -> Self {
        Self::with_max_len(MAX_PHRASE_CHARS)
    }

    pub fn with_max_len(max_len: usize) -> Self {
        Self { max_len: max_len.max(16), line: String::new(), start: 0, block: Block::None }
    }

    /// Принять дельту; вернуть фразы, которые уже точно закончились.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        let mut out = Vec::new();
        for ch in delta.chars() {
            if ch == '\n' {
                self.end_line(&mut out);
            } else if ch != '\r' {
                self.line.push(ch);
            }
        }
        self.scan_partial(&mut out);
        out
    }

    /// Конец потока: выдать всё, что осталось.
    pub fn flush(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.line.is_empty() {
            self.end_line(&mut out);
        }
        match std::mem::replace(&mut self.block, Block::None) {
            Block::None => {}
            Block::Code { text, .. } => push_nonempty(&mut out, text.trim_end().to_string()),
            Block::Table { lines } => self.emit_table(lines, &mut out),
        }
        out
    }

    /// Сбросить без выдачи (барж-ин).
    pub fn reset(&mut self) {
        self.line.clear();
        self.start = 0;
        self.block = Block::None;
    }

    fn end_line(&mut self, out: &mut Vec<String>) {
        let line = std::mem::take(&mut self.line);
        let start = std::mem::replace(&mut self.start, 0);
        let trimmed = line.trim_start();
        match &mut self.block {
            Block::Code { fence, text } => {
                text.push_str(&line);
                if trimmed.starts_with(*fence) {
                    let t = std::mem::take(text);
                    self.block = Block::None;
                    push_nonempty(out, t.trim_end().to_string());
                } else {
                    text.push('\n');
                }
                return;
            }
            Block::Table { lines } => {
                if trimmed.starts_with('|') {
                    lines.push(line);
                    return;
                }
                let lines = std::mem::take(lines);
                self.block = Block::None;
                self.emit_table(lines, out);
            }
            Block::None => {}
        }
        if start == 0 {
            if let Some(fence) = fence_of(trimmed) {
                self.block = Block::Code { fence, text: format!("{line}\n") };
                return;
            }
            if trimmed.starts_with('|') {
                self.block = Block::Table { lines: vec![line] };
                return;
            }
        }
        self.line = line;
        self.start = start;
        self.split_sentences(true, out);
        self.line.clear();
        self.start = 0;
    }

    fn scan_partial(&mut self, out: &mut Vec<String>) {
        let trimmed = self.line.trim_start();
        match &mut self.block {
            Block::Code { .. } => return,
            Block::Table { lines } => {
                if trimmed.is_empty() || trimmed.starts_with('|') {
                    return;
                }
                let lines = std::mem::take(lines);
                self.block = Block::None;
                self.emit_table(lines, out);
            }
            Block::None => {}
        }
        let trimmed = self.line.trim_start();
        if self.start == 0 {
            // Строка может оказаться оградой кода или строкой таблицы — ждём её конца.
            let maybe_fence = !trimmed.is_empty() && ("```".starts_with(trimmed) || "~~~".starts_with(trimmed));
            if fence_of(trimmed).is_some() || maybe_fence || trimmed.starts_with('|') {
                return;
            }
        }
        self.split_sentences(false, out);
    }

    /// Таблица: заголовок + разделитель повторяются в каждом куске, чтобы нормализатор знал столбцы.
    fn emit_table(&self, lines: Vec<String>, out: &mut Vec<String>) {
        let is_sep = |l: &str| l.contains("--") && l.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'));
        if lines.len() < 3 || !is_sep(&lines[1]) {
            for l in lines {
                for p in split_long(&l, self.max_len) {
                    push_nonempty(out, p);
                }
            }
            return;
        }
        let head = format!("{}\n{}", lines[0].trim(), lines[1].trim());
        let mut chunk = head.clone();
        let mut rows = 0;
        for row in &lines[2..] {
            let row = row.trim();
            if rows > 0 && chunk.chars().count() + row.chars().count() + 1 > self.max_len {
                out.push(std::mem::replace(&mut chunk, head.clone()));
                rows = 0;
            }
            chunk.push('\n');
            chunk.push_str(row);
            rows += 1;
        }
        if rows > 0 {
            out.push(chunk);
        }
    }

    fn split_sentences(&mut self, eol: bool, out: &mut Vec<String>) {
        loop {
            let text = &self.line[self.start..];
            if text.trim().is_empty() {
                if eol {
                    self.start = self.line.len();
                }
                return;
            }
            if let Some(end) = find_boundary(text, eol) {
                let sentence = text[..end].trim().to_string();
                self.start += end;
                for p in split_long(&sentence, self.max_len) {
                    push_nonempty(out, p);
                }
                continue;
            }
            // Границы нет: длинный хвост режем по мягкой границе, не дожидаясь конца предложения.
            if text.chars().count() > self.max_len {
                match soft_split_point(text, self.max_len) {
                    Some(cut) => {
                        let piece = text[..cut].trim().to_string();
                        self.start += cut;
                        push_nonempty(out, piece);
                        continue;
                    }
                    None if !eol => return, // один гигантский токен ещё не закончился
                    None => {}
                }
            }
            if eol {
                let rest = text.trim().to_string();
                self.start = self.line.len();
                for p in split_long(&rest, self.max_len) {
                    push_nonempty(out, p);
                }
            }
            return;
        }
    }
}

fn push_nonempty(out: &mut Vec<String>, s: String) {
    if !s.trim().is_empty() {
        out.push(s);
    }
}

fn is_terminal(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

fn is_closer(c: char) -> bool {
    matches!(c, '"' | '»' | ')' | '\'' | '”' | ']')
}

/// Слово перед позицией `pos` (до пробела), без открывающих скобок и кавычек.
fn token_before(text: &str, pos: usize) -> &str {
    let head = &text[..pos];
    let start = head.rfind(char::is_whitespace).map(|i| i + head[i..].chars().next().unwrap().len_utf8()).unwrap_or(0);
    head[start..].trim_start_matches(['(', '«', '"', '\'', '[', '“'])
}

/// Байтовый индекс конца первого предложения в `text` (после знаков и закрывающих кавычек).
/// `None` — граница не найдена или ещё не ясна (нужен следующий символ).
fn find_boundary(text: &str, eol: bool) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let (pos, c) = chars[i];
        if !is_terminal(c) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && is_terminal(chars[j].1) {
            j += 1;
        }
        let strong = chars[i..j].iter().any(|(_, c)| matches!(c, '!' | '?' | '…')) || j - i >= 3;
        while j < n && is_closer(chars[j].1) {
            j += 1;
        }
        let end = chars.get(j).map(|(p, _)| *p).unwrap_or(text.len());
        if j >= n {
            return eol.then_some(end);
        }
        if !chars[j].1.is_whitespace() {
            // «3.14», «т.е.», URL — знак внутри токена.
            i = j;
            continue;
        }
        let mut k = j;
        while k < n && chars[k].1.is_whitespace() {
            k += 1;
        }
        if k >= n {
            return eol.then_some(end);
        }
        if strong {
            return Some(end);
        }
        let next = chars[k].1;
        let token = token_before(text, pos);
        let lower = token.to_lowercase();
        let first_token = text[..pos].trim_start() == token;
        let single_upper = token.chars().count() == 1 && token.chars().all(char::is_uppercase);
        let list_marker = first_token && !token.is_empty() && token.chars().all(|c| c.is_ascii_digit());
        if NEVER_END.contains(&lower.as_str()) || single_upper || list_marker || next.is_lowercase() {
            i = j;
            continue;
        }
        return Some(end);
    }
    None
}

/// Где резать слишком длинный текст: после «, ; : —», иначе по пробелу (не между группами цифр «1 000»).
fn soft_split_point(text: &str, max: usize) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let limit = chars.len().min(max);
    let min = max / 3;
    let mut soft = None;
    let mut space = None;
    for idx in 1..limit {
        let (pos, c) = chars[idx];
        if !c.is_whitespace() {
            continue;
        }
        let prev = chars[idx - 1].1;
        let next = chars.get(idx + 1).map(|x| x.1).unwrap_or(' ');
        if prev.is_ascii_digit() && next.is_ascii_digit() {
            continue; // «10 000»
        }
        if idx >= min && matches!(prev, ',' | ';' | ':' | '—' | '–') {
            soft = Some(pos);
        }
        if idx >= min / 2 {
            space = Some(pos);
        }
    }
    if let Some(p) = soft.or(space) {
        return Some(p);
    }
    // Один токен длиннее лимита (URL): режем после него, если он уже закончился.
    chars.iter().skip(limit).find(|(_, c)| c.is_whitespace()).map(|(p, _)| *p)
}

/// Разбить готовый текст на куски ≤ `max` символов (по предложениям, потом по мягким границам).
pub fn split_long(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while rest.chars().count() > max {
        let cut = find_boundary(rest, true)
            .filter(|&e| e < rest.len() && rest[..e].chars().count() <= max)
            .or_else(|| soft_split_point(rest, max));
        match cut {
            Some(c) if c > 0 && c < rest.len() => {
                push_nonempty(&mut out, rest[..c].trim().to_string());
                rest = rest[c..].trim_start();
            }
            _ => break,
        }
    }
    push_nonempty(&mut out, rest.to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Подать текст дельтами по `step` символов и собрать все фразы.
    fn chunk(text: &str, step: usize) -> Vec<String> {
        let mut c = PhraseChunker::new();
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        for part in chars.chunks(step) {
            out.extend(c.push(&part.iter().collect::<String>()));
        }
        out.extend(c.flush());
        out
    }

    fn all_steps(text: &str, expected: &[&str]) {
        for step in [1, 2, 3, 7, 1000] {
            assert_eq!(chunk(text, step), expected, "шаг {step}");
        }
    }

    #[test]
    fn sentences() {
        all_steps("Привет. Как дела? Всё хорошо!", &["Привет.", "Как дела?", "Всё хорошо!"]);
        all_steps("Ну... ладно. «Стоп!» Он ушёл.", &["Ну...", "ладно.", "«Стоп!»", "Он ушёл."]);
    }

    #[test]
    fn early_emit_before_flush() {
        let mut c = PhraseChunker::new();
        assert!(c.push("Первая фраза.").is_empty(), "без следующего символа граница неясна");
        assert_eq!(c.push(" Вто"), vec!["Первая фраза."]);
        assert_eq!(c.flush(), vec!["Вто"]);
    }

    #[test]
    fn does_not_break_numbers_abbrevs_urls() {
        all_steps("Число 3.14 важно. Далее", &["Число 3.14 важно.", "Далее"]);
        all_steps("Купи хлеб, т. е. батон. Потом", &["Купи хлеб, т. е. батон.", "Потом"]);
        all_steps("Хлеб, молоко и т. д. Потом позвони.", &["Хлеб, молоко и т. д.", "Потом позвони."]);
        all_steps("Хлеб и т. д. и т. п. всё.", &["Хлеб и т. д. и т. п. всё."]);
        all_steps("Открой https://example.com/a.b?c=d. Готово.", &["Открой https://example.com/a.b?c=d.", "Готово."]);
        all_steps("А. С. Пушкин написал. Да.", &["А. С. Пушкин написал.", "Да."]);
        all_steps("Версия 2.0.1 вышла. Ура", &["Версия 2.0.1 вышла.", "Ура"]);
    }

    #[test]
    fn lines_lists_code_tables() {
        all_steps("1. Первое. Подробно\n2. Второе\n", &["1. Первое.", "Подробно", "2. Второе"]);
        all_steps(
            "Пример:\n```\ncode. More. code\n```\nКонец.",
            &["Пример:", "```\ncode. More. code\n```", "Конец."],
        );
        all_steps("| a | b |\n|---|---|\n| 1 | 2 |\nТекст.", &["| a | b |\n|---|---|\n| 1 | 2 |", "Текст."]);
        // Незакрытый блок кода выдаётся на flush.
        all_steps("```\nfn x()", &["```\nfn x()"]);
    }

    #[test]
    fn long_text_split_softly() {
        let word = "слово ";
        let long: String = word.repeat(100); // 600 символов без точек
        let parts = chunk(&long, 5);
        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|p| p.chars().count() <= MAX_PHRASE_CHARS), "{parts:?}");
        assert_eq!(parts.join(" "), long.trim());

        let s = format!(
            "{}, а потом ещё {} и сумма 1 000 000 рублей.",
            "раз ".repeat(20).trim(),
            "два ".repeat(30).trim()
        );
        let parts = split_long(&s, 120);
        assert!(parts.iter().all(|p| p.chars().count() <= 120), "{parts:?}");
        assert!(parts.iter().any(|p| p.contains("1 000 000")), "группы цифр не рвутся: {parts:?}");
        assert!(parts[0].ends_with(','), "режем после запятой: {parts:?}");
    }

    #[test]
    fn giant_token_kept_whole() {
        let url = format!("https://example.com/{}", "a".repeat(300));
        let parts = chunk(&format!("Ссылка {url} конец"), 9);
        assert!(parts.iter().any(|p| p.ends_with(&url)), "{parts:?}");
    }
}
