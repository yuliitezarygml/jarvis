//! Приближённый лексер PowerShell и cmd для классификатора команд.
//!
//! Цель — не исполнить команду, а надёжно найти в ней все «команды» (сегменты цепочек `;`, `|`, `&&`, `||`,
//! `&` в cmd, переводы строк), их аргументы, перенаправления вывода в файл и вложенный код
//! (`( )`, `$( )`, `@( )`, `{ }`, `@{ }`, `$( )` внутри строк в двойных кавычках).
//! Всё, что не удалось разобрать (незакрытые кавычки/скобки), — ошибка, а классификатор превращает её в T3.

use super::Shell;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GroupKind {
    /// `( … )`
    Paren,
    /// `$( … )`
    SubExpr,
    /// `@( … )`
    ArrayExpr,
    /// `{ … }` — блок скрипта
    Block,
    /// `@{ … }` — хэш-таблица
    Hash,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Word {
        /// Текст без кавычек и escape-символов.
        text: String,
        /// Есть ли части, где возможна подстановка переменных (`$x`, `%X%`): без кавычек или в двойных.
        expandable: bool,
        /// Слово целиком или частично было в кавычках.
        quoted: bool,
        /// Нет пробела перед токеном (метод `.Name(`, `(…).Prop`).
        attached: bool,
        /// Элемент массива через запятую с предыдущим токеном (`a,b`).
        comma: bool,
    },
    Group { kind: GroupKind, inner: String, attached: bool, comma: bool },
}

impl Tok {
    pub(crate) fn word(&self) -> Option<&str> {
        match self {
            Tok::Word { text, .. } => Some(text),
            Tok::Group { .. } => None,
        }
    }
    pub(crate) fn is_bare(&self) -> bool {
        matches!(self, Tok::Word { quoted: false, .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Redirect {
    pub target: String,
    pub expandable: bool,
    pub quoted: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Segment {
    pub toks: Vec<Tok>,
    /// Сегмент получает данные по конвейеру (`a | b` → у `b`).
    pub piped_in: bool,
    /// Перенаправления в файл (`> x`, `2>> y`); `2>&1`, `> $null`, `>nul` сюда не попадают.
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Default)]
pub(crate) struct Lexed {
    pub segments: Vec<Segment>,
    /// Код из `$( … )` внутри строк в двойных кавычках и here-strings (PowerShell).
    pub nested: Vec<String>,
}

pub(crate) fn lex(shell: Shell, src: &str) -> Result<Lexed, String> {
    let chars: Vec<char> = src.chars().collect();
    match shell {
        Shell::PowerShell => PsLexer::new(&chars).run(),
        Shell::Cmd => CmdLexer::new(&chars).run(),
    }
}

fn is_ws(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\r' || c == '\u{a0}'
}

/// Найти закрывающую скобку для `chars[open]`, учитывая кавычки и вложенность всех видов скобок.
/// Возвращает индекс закрывающей скобки.
fn scan_balanced(chars: &[char], open: usize, ps: bool) -> Result<usize, String> {
    let mut stack = vec![match chars[open] {
        '(' => ')',
        '{' => '}',
        '[' => ']',
        c => return Err(format!("неожиданная скобка {c}")),
    }];
    let mut i = open + 1;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if ps => {
                i = skip_single_quoted(chars, i)?;
                continue;
            }
            '"' => {
                i = if ps { skip_double_quoted_ps(chars, i)? } else { skip_double_quoted_cmd(chars, i) };
                continue;
            }
            '`' if ps => i += 1,
            '^' if !ps => i += 1,
            '#' if ps && (i == 0 || is_ws(chars[i - 1]) || chars[i - 1] == '\n') => {
                // комментарий до конца строки (скобки в нём не считаются)
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '(' => stack.push(')'),
            '{' if ps => stack.push('}'),
            '[' if ps => stack.push(']'),
            ')' | '}' | ']' => {
                if !ps && c != ')' {
                    i += 1;
                    continue;
                }
                match stack.last() {
                    Some(&want) if want == c => {
                        stack.pop();
                        if stack.is_empty() {
                            return Ok(i);
                        }
                    }
                    Some(_) if c == ']' => {} // `]` внутри обычного текста
                    _ => return Err(format!("несбалансированная скобка {c}")),
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err("незакрытая скобка".into())
}

/// `'…'` с удвоением `''`. Возвращает индекс после закрывающей кавычки.
fn skip_single_quoted(chars: &[char], start: usize) -> Result<usize, String> {
    let mut i = start + 1;
    while i < chars.len() {
        if is_squote(chars[i]) {
            if i + 1 < chars.len() && is_squote(chars[i + 1]) {
                i += 2;
                continue;
            }
            return Ok(i + 1);
        }
        i += 1;
    }
    Err("незакрытая одинарная кавычка".into())
}

fn skip_double_quoted_ps(chars: &[char], start: usize) -> Result<usize, String> {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '`' => i += 2,
            '$' if i + 1 < chars.len() && chars[i + 1] == '(' => {
                i = scan_balanced(chars, i + 1, true)? + 1;
            }
            c if is_dquote(c) => {
                if i + 1 < chars.len() && is_dquote(chars[i + 1]) {
                    i += 2;
                    continue;
                }
                return Ok(i + 1);
            }
            _ => i += 1,
        }
    }
    Err("незакрытая двойная кавычка".into())
}

fn skip_double_quoted_cmd(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    while i < chars.len() && chars[i] != '"' && chars[i] != '\n' {
        i += 1;
    }
    (i + 1).min(chars.len())
}

/// PowerShell принимает «умные» кавычки как обычные.
fn is_squote(c: char) -> bool {
    matches!(c, '\'' | '‘' | '’' | '‚' | '‛')
}
fn is_dquote(c: char) -> bool {
    matches!(c, '"' | '“' | '”' | '„')
}

// ───────────────────────────── PowerShell ─────────────────────────────

struct PsLexer<'a> {
    c: &'a [char],
    i: usize,
    out: Lexed,
    cur: Segment,
    next_piped: bool,
    last_end: Option<usize>,
    pending_comma: bool,
}

impl<'a> PsLexer<'a> {
    fn new(c: &'a [char]) -> Self {
        Self { c, i: 0, out: Lexed::default(), cur: Segment::default(), next_piped: false, last_end: None, pending_comma: false }
    }

    fn peek(&self, off: usize) -> Option<char> {
        self.c.get(self.i + off).copied()
    }

    fn end_segment(&mut self, piped_next: bool) {
        let seg = std::mem::take(&mut self.cur);
        if !seg.toks.is_empty() || !seg.redirects.is_empty() {
            self.out.segments.push(seg);
        }
        self.cur.piped_in = piped_next;
        self.next_piped = piped_next;
        self.pending_comma = false;
    }

    fn attached(&self) -> bool {
        self.last_end == Some(self.i)
    }

    fn push(&mut self, mut t: Tok) {
        let comma = std::mem::take(&mut self.pending_comma);
        match &mut t {
            Tok::Word { comma: c, .. } | Tok::Group { comma: c, .. } => *c = comma,
        }
        if self.cur.toks.is_empty() {
            self.cur.piped_in = self.next_piped;
        }
        self.cur.toks.push(t);
    }

    fn run(mut self) -> Result<Lexed, String> {
        while self.i < self.c.len() {
            let ch = self.c[self.i];
            match ch {
                c if is_ws(c) => self.i += 1,
                '\n' | ';' => {
                    self.i += 1;
                    self.end_segment(false);
                }
                '|' => {
                    if self.peek(1) == Some('|') {
                        self.i += 2;
                        self.end_segment(false);
                    } else {
                        self.i += 1;
                        self.end_segment(true);
                    }
                }
                '&' => {
                    if self.peek(1) == Some('&') {
                        self.i += 2;
                        self.end_segment(false);
                    } else {
                        // оператор вызова `& cmd` (или фоновое задание `cmd &` в PS 7)
                        let attached = self.attached();
                        self.i += 1;
                        self.push(Tok::Word { text: "&".into(), expandable: false, quoted: false, attached, comma: false });
                        self.last_end = Some(self.i);
                    }
                }
                '#' if !self.attached() => {
                    while self.i < self.c.len() && self.c[self.i] != '\n' {
                        self.i += 1;
                    }
                }
                '<' if self.peek(1) == Some('#') => {
                    let mut j = self.i + 2;
                    loop {
                        if j + 1 >= self.c.len() {
                            return Err("незакрытый блочный комментарий".into());
                        }
                        if self.c[j] == '#' && self.c[j + 1] == '>' {
                            break;
                        }
                        j += 1;
                    }
                    self.i = j + 2;
                }
                ',' => {
                    self.i += 1;
                    self.pending_comma = true;
                }
                '`' if self.peek(1) == Some('\n') || (self.peek(1) == Some('\r') && self.peek(2) == Some('\n')) => {
                    // продолжение строки
                    self.i += if self.peek(1) == Some('\r') { 3 } else { 2 };
                }
                '(' => self.group(GroupKind::Paren, 0)?,
                '{' => self.group(GroupKind::Block, 0)?,
                '$' if self.peek(1) == Some('(') => self.group(GroupKind::SubExpr, 1)?,
                '@' if self.peek(1) == Some('(') => self.group(GroupKind::ArrayExpr, 1)?,
                '@' if self.peek(1) == Some('{') => self.group(GroupKind::Hash, 1)?,
                '@' if self.peek(1).is_some_and(|c| is_squote(c) || is_dquote(c)) && self.is_here_string_start() => {
                    self.here_string()?
                }
                ')' | '}' => return Err(format!("лишняя закрывающая скобка {ch}")),
                '>' => self.redirect()?,
                c if (c.is_ascii_digit() || c == '*') && self.peek(1) == Some('>') && !self.attached() => {
                    self.i += 1;
                    self.redirect()?
                }
                '=' => {
                    let attached = self.attached();
                    self.i += 1;
                    self.push(Tok::Word { text: "=".into(), expandable: false, quoted: false, attached, comma: false });
                    self.last_end = Some(self.i);
                }
                _ => {
                    let attached = self.attached();
                    let (text, expandable, quoted) = self.word()?;
                    if text == "--%" && !quoted {
                        self.stop_parsing();
                        continue;
                    }
                    self.push(Tok::Word { text, expandable, quoted, attached, comma: false });
                    self.last_end = Some(self.i);
                }
            }
        }
        self.end_segment(false);
        Ok(self.out)
    }

    fn group(&mut self, kind: GroupKind, prefix: usize) -> Result<(), String> {
        let attached = self.attached();
        let open = self.i + prefix;
        let close = scan_balanced(self.c, open, true)?;
        let inner: String = self.c[open + 1..close].iter().collect();
        self.i = close + 1;
        self.push(Tok::Group { kind, inner, attached, comma: false });
        self.last_end = Some(self.i);
        Ok(())
    }

    fn is_here_string_start(&self) -> bool {
        let mut j = self.i + 2;
        while j < self.c.len() && is_ws(self.c[j]) {
            j += 1;
        }
        j < self.c.len() && self.c[j] == '\n'
    }

    fn here_string(&mut self) -> Result<(), String> {
        let attached = self.attached();
        let dq = is_dquote(self.c[self.i + 1]);
        let mut j = self.i + 2;
        while self.c[j] != '\n' {
            j += 1;
        }
        let body_start = j + 1;
        let mut k = body_start;
        let end = loop {
            if k + 1 >= self.c.len() {
                return Err("незакрытая here-string".into());
            }
            // закрытие: `'@` / `"@` в начале строки
            let at_line_start = k == body_start || self.c[k - 1] == '\n';
            let q = if dq { is_dquote(self.c[k]) } else { is_squote(self.c[k]) };
            if at_line_start && q && self.c[k + 1] == '@' {
                break k;
            }
            k += 1;
        };
        let body_end = if end > body_start { end - 1 } else { body_start };
        let body: String = self.c[body_start..body_end].iter().collect();
        if dq {
            self.collect_subexprs(&body.chars().collect::<Vec<_>>())?;
        }
        self.i = end + 2;
        self.push(Tok::Word { text: body, expandable: dq, quoted: true, attached, comma: false });
        self.last_end = Some(self.i);
        Ok(())
    }

    /// Все `$( … )` внутри текста строки — во вложенный код.
    fn collect_subexprs(&mut self, s: &[char]) -> Result<(), String> {
        let mut i = 0;
        while i < s.len() {
            if s[i] == '`' {
                i += 2;
                continue;
            }
            if s[i] == '$' && i + 1 < s.len() && s[i + 1] == '(' {
                let close = scan_balanced(s, i + 1, true)?;
                self.out.nested.push(s[i + 2..close].iter().collect());
                i = close + 1;
                continue;
            }
            i += 1;
        }
        Ok(())
    }

    fn redirect(&mut self) -> Result<(), String> {
        // self.c[self.i] == '>'
        self.i += 1;
        if self.peek(0) == Some('>') {
            self.i += 1;
        }
        if self.peek(0) == Some('&') {
            // 2>&1 — слияние потоков, файла нет
            self.i += 1;
            while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
            return Ok(());
        }
        while self.peek(0).is_some_and(is_ws) {
            self.i += 1;
        }
        if self.i >= self.c.len() || matches!(self.c[self.i], '\n' | ';' | '|') {
            return Err("перенаправление без цели".into());
        }
        let (target, expandable, quoted) = if self.c[self.i] == '(' || (self.c[self.i] == '$' && self.peek(1) == Some('(')) {
            let open = if self.c[self.i] == '$' { self.i + 1 } else { self.i };
            let close = scan_balanced(self.c, open, true)?;
            let inner: String = self.c[open + 1..close].iter().collect();
            self.out.nested.push(inner.clone());
            self.i = close + 1;
            (format!("$({inner})"), true, false)
        } else {
            self.word()?
        };
        let low = target.to_ascii_lowercase();
        if !(low == "$null" && !quoted) {
            self.cur.redirects.push(Redirect { target, expandable, quoted });
        }
        self.last_end = Some(self.i);
        Ok(())
    }

    /// Остаток строки после `--%` — как есть, по пробелам.
    fn stop_parsing(&mut self) {
        let start = self.i;
        while self.i < self.c.len() && self.c[self.i] != '\n' && self.c[self.i] != '|' {
            self.i += 1;
        }
        let rest: String = self.c[start..self.i].iter().collect();
        for w in rest.split_whitespace() {
            let quoted = w.contains('"');
            let text = w.replace('"', "");
            let expandable = text.contains('%');
            self.push(Tok::Word { expandable, text, quoted, attached: false, comma: false });
        }
    }

    /// Слово: голый текст + кусочки в кавычках, до пробела/разделителя.
    fn word(&mut self) -> Result<(String, bool, bool), String> {
        let mut text = String::new();
        let mut expandable = false;
        let mut quoted = false;
        let mut any_bare = false;
        while self.i < self.c.len() {
            let ch = self.c[self.i];
            match ch {
                c if is_ws(c) => break,
                '\n' | ';' | '|' | '&' | ',' | ')' | '}' | '>' | '=' | '{' => break,
                '(' => break, // `$x.Method(` / `cmd(…)` — группа отдельным токеном
                '`' => {
                    if let Some(n) = self.peek(1) {
                        if n == '\n' {
                            break;
                        }
                        text.push(n);
                    }
                    any_bare = true;
                    self.i += 2;
                }
                c if is_squote(c) => {
                    let end = skip_single_quoted(self.c, self.i)?;
                    let body: String = self.c[self.i + 1..end - 1].iter().collect();
                    text.push_str(&body.replace("''", "'"));
                    quoted = true;
                    self.i = end;
                }
                c if is_dquote(c) => {
                    let end = skip_double_quoted_ps(self.c, self.i)?;
                    let raw: Vec<char> = self.c[self.i + 1..end - 1].to_vec();
                    self.collect_subexprs(&raw)?;
                    let mut k = 0;
                    while k < raw.len() {
                        match raw[k] {
                            '`' if k + 1 < raw.len() => {
                                let n = raw[k + 1];
                                text.push(match n {
                                    'n' => '\n',
                                    't' => '\t',
                                    '0' => '\0',
                                    other => other,
                                });
                                k += 2;
                                continue;
                            }
                            '$' => {
                                expandable = true;
                                text.push('$');
                            }
                            c if is_dquote(c) && k + 1 < raw.len() && is_dquote(raw[k + 1]) => {
                                text.push('"');
                                k += 2;
                                continue;
                            }
                            c => text.push(c),
                        }
                        k += 1;
                    }
                    quoted = true;
                    self.i = end;
                }
                '[' => {
                    // тип `[IO.File]` или индекс `$a[0]` — до парной скобки
                    let close = scan_balanced(self.c, self.i, true)?;
                    text.extend(&self.c[self.i..=close]);
                    any_bare = true;
                    self.i = close + 1;
                }
                '$' => {
                    if self.peek(1) == Some('(') {
                        let close = scan_balanced(self.c, self.i + 1, true)?;
                        self.out.nested.push(self.c[self.i + 2..close].iter().collect());
                        text.extend(&self.c[self.i..=close]);
                        self.i = close + 1;
                    } else if self.peek(1) == Some('{') {
                        let close = self.c[self.i..].iter().position(|&c| c == '}').map(|p| p + self.i);
                        let Some(close) = close else { return Err("незакрытая ${…}".into()) };
                        text.extend(&self.c[self.i..=close]);
                        self.i = close + 1;
                    } else {
                        text.push('$');
                        self.i += 1;
                    }
                    expandable = true;
                    any_bare = true;
                }
                c => {
                    text.push(c);
                    any_bare = true;
                    self.i += 1;
                }
            }
        }
        if text.is_empty() && !quoted && !any_bare {
            // одиночный неожиданный символ — пропускаем, чтобы не зациклиться
            let c = self.c[self.i];
            self.i += 1;
            return Ok((c.to_string(), false, false));
        }
        let expandable = expandable || (any_bare && text.contains('$'));
        Ok((text, expandable, quoted))
    }
}

// ───────────────────────────── cmd.exe ─────────────────────────────

struct CmdLexer<'a> {
    c: &'a [char],
    i: usize,
    out: Lexed,
    cur: Segment,
    next_piped: bool,
}

impl<'a> CmdLexer<'a> {
    fn new(c: &'a [char]) -> Self {
        Self { c, i: 0, out: Lexed::default(), cur: Segment::default(), next_piped: false }
    }

    fn peek(&self, off: usize) -> Option<char> {
        self.c.get(self.i + off).copied()
    }

    fn end_segment(&mut self, piped_next: bool) {
        let seg = std::mem::take(&mut self.cur);
        if !seg.toks.is_empty() || !seg.redirects.is_empty() {
            self.out.segments.push(seg);
        }
        self.next_piped = piped_next;
    }

    fn push(&mut self, t: Tok) {
        if self.cur.toks.is_empty() {
            self.cur.piped_in = self.next_piped;
        }
        self.cur.toks.push(t);
    }

    fn run(mut self) -> Result<Lexed, String> {
        while self.i < self.c.len() {
            let ch = self.c[self.i];
            match ch {
                c if is_ws(c) || c == ',' || c == ';' || c == '=' => self.i += 1,
                '\n' => {
                    self.i += 1;
                    self.end_segment(false);
                }
                '&' => {
                    self.i += if self.peek(1) == Some('&') { 2 } else { 1 };
                    self.end_segment(false);
                }
                '|' => {
                    if self.peek(1) == Some('|') {
                        self.i += 2;
                        self.end_segment(false);
                    } else {
                        self.i += 1;
                        self.end_segment(true);
                    }
                }
                '(' => {
                    let close = scan_balanced(self.c, self.i, false)?;
                    let inner: String = self.c[self.i + 1..close].iter().collect();
                    self.i = close + 1;
                    self.push(Tok::Group { kind: GroupKind::Paren, inner, attached: false, comma: false });
                }
                '@' if self.cur.toks.is_empty() => self.i += 1, // @echo off
                '<' => {
                    // ввод из файла — чтение, цель пропускаем
                    self.i += 1;
                    while self.peek(0).is_some_and(is_ws) {
                        self.i += 1;
                    }
                    let _ = self.word();
                }
                '>' => self.redirect(),
                c if c.is_ascii_digit() && self.peek(1) == Some('>') => {
                    self.i += 1;
                    self.redirect()
                }
                _ => {
                    let (text, expandable, quoted) = self.word();
                    self.push(Tok::Word { text, expandable, quoted, attached: false, comma: false });
                }
            }
        }
        self.end_segment(false);
        Ok(self.out)
    }

    fn redirect(&mut self) {
        self.i += 1;
        if self.peek(0) == Some('>') {
            self.i += 1;
        }
        if self.peek(0) == Some('&') {
            self.i += 1;
            while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
            return;
        }
        while self.peek(0).is_some_and(is_ws) {
            self.i += 1;
        }
        let (target, expandable, quoted) = self.word();
        let low = target.to_ascii_lowercase();
        if target.is_empty() || (!quoted && matches!(low.as_str(), "nul" | "nul:" | "con" | "con:")) {
            return;
        }
        self.cur.redirects.push(Redirect { target, expandable, quoted });
    }

    fn word(&mut self) -> (String, bool, bool) {
        let mut text = String::new();
        let mut quoted = false;
        let mut in_q = false;
        while self.i < self.c.len() {
            let ch = self.c[self.i];
            if in_q {
                if ch == '"' {
                    in_q = false;
                } else if ch == '\n' {
                    break;
                } else {
                    text.push(ch);
                }
                self.i += 1;
                continue;
            }
            match ch {
                c if is_ws(c) => break,
                '\n' | '&' | '|' | '<' | '>' | ',' | ';' | '=' => break,
                '(' if text.is_empty() => break,
                '"' => {
                    in_q = true;
                    quoted = true;
                    self.i += 1;
                }
                '^' => {
                    if let Some(n) = self.peek(1)
                        && n != '\n'
                    {
                        text.push(n);
                    }
                    self.i += 2;
                }
                c => {
                    text.push(c);
                    self.i += 1;
                }
            }
        }
        let expandable = text.contains('%') || text.contains('!');
        (text, expandable, quoted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(seg: &Segment) -> Vec<String> {
        seg.toks
            .iter()
            .map(|t| match t {
                Tok::Word { text, .. } => text.clone(),
                Tok::Group { inner, .. } => format!("<{inner}>"),
            })
            .collect()
    }

    #[test]
    fn ps_chains_and_groups() {
        let l = lex(Shell::PowerShell, "Get-ChildItem C:\\ | Where-Object { $_.Length -gt 1 }; echo 'a;b' && x").unwrap();
        assert_eq!(l.segments.len(), 4);
        assert_eq!(words(&l.segments[0]), ["Get-ChildItem", "C:\\"]);
        assert!(l.segments[1].piped_in);
        assert_eq!(words(&l.segments[1]), ["Where-Object", "< $_.Length -gt 1 >"]);
        assert_eq!(words(&l.segments[2]), ["echo", "a;b"]);
    }

    #[test]
    fn ps_subexpr_in_string_is_nested() {
        let l = lex(Shell::PowerShell, "Write-Host \"x $(Remove-Item a) y\"").unwrap();
        assert_eq!(l.nested, ["Remove-Item a"]);
    }

    #[test]
    fn ps_redirect_and_method() {
        let l = lex(Shell::PowerShell, "echo hi > C:\\x.txt 2>&1; $p.Kill()").unwrap();
        assert_eq!(l.segments[0].redirects.len(), 1);
        assert_eq!(l.segments[0].redirects[0].target, "C:\\x.txt");
        let s = &l.segments[1];
        assert!(matches!(&s.toks[1], Tok::Group { attached: true, .. }));
    }

    #[test]
    fn ps_unbalanced_is_error() {
        assert!(lex(Shell::PowerShell, "echo 'abc").is_err());
        assert!(lex(Shell::PowerShell, "foo (bar").is_err());
        assert!(lex(Shell::PowerShell, "foo }").is_err());
    }

    #[test]
    fn cmd_basics() {
        let l = lex(Shell::Cmd, "@echo off & del /s /q \"C:\\a b\" >nul 2>&1 && dir").unwrap();
        assert_eq!(l.segments.len(), 3);
        assert_eq!(words(&l.segments[1]), ["del", "/s", "/q", "C:\\a b"]);
        assert!(l.segments[1].redirects.is_empty());
        let l = lex(Shell::Cmd, "if exist x (del x) else (echo no)").unwrap();
        assert_eq!(words(&l.segments[0]), ["if", "exist", "x", "<del x>", "else", "<echo no>"]);
    }
}
