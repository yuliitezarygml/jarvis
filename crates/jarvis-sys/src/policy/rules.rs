//! Правила классификации: сегменты → команды → уровни риска.

use jarvis_core::RiskTier::{T1, T2, T3};

use super::lexer::{GroupKind, Tok, lex};
use super::paths::{Hive, PathOp, PathZone, parse_reg_path, path_tier, ps_provider, zone_of};
use super::{Acc, PolicyCtx, Shell};

const MAX_DEPTH: usize = 8;

pub(crate) fn classify_script(shell: Shell, src: &str, ctx: &PolicyCtx, acc: &mut Acc, depth: usize, in_hash: bool) {
    if depth > MAX_DEPTH {
        acc.raise(T3, "слишком глубокая вложенность команд");
        return;
    }
    if src.trim().is_empty() {
        return;
    }
    match lex(shell, src) {
        Err(e) => acc.raise(T3, format!("не удалось разобрать команду ({e})")),
        Ok(lx) => {
            for n in &lx.nested {
                classify_script(Shell::PowerShell, n, ctx, acc, depth + 1, false);
            }
            for seg in &lx.segments {
                for r in &seg.redirects {
                    let a = Arg {
                        text: r.target.clone(),
                        low: r.target.to_lowercase(),
                        expandable: r.expandable,
                        quoted: r.quoted,
                        group: false,
                        comma: false,
                    };
                    target(acc, ctx, shell, &a, Op::Write, "перенаправление вывода в файл");
                }
                let mut c = Cx { shell, ctx, acc, depth };
                match shell {
                    Shell::PowerShell => c.ps_segment(&seg.toks, seg.piped_in, in_hash),
                    Shell::Cmd => c.cmd_segment(&seg.toks, seg.piped_in),
                }
            }
        }
    }
}

// ───────────────────────────── аргументы ─────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct Arg {
    pub text: String,
    pub low: String,
    pub expandable: bool,
    pub quoted: bool,
    pub group: bool,
    pub comma: bool,
}

impl Arg {
    fn from_tok(t: &Tok) -> Arg {
        match t {
            Tok::Word { text, expandable, quoted, comma, .. } => Arg {
                text: text.clone(),
                low: text.to_lowercase(),
                expandable: *expandable,
                quoted: *quoted,
                group: false,
                comma: *comma,
            },
            Tok::Group { inner, comma, .. } => Arg {
                text: format!("({inner})"),
                low: format!("({})", inner.to_lowercase()),
                expandable: true,
                quoted: false,
                group: true,
                comma: *comma,
            },
        }
    }

    fn plain(text: &str) -> Arg {
        Arg { text: text.into(), low: text.to_lowercase(), expandable: true, quoted: false, group: false, comma: false }
    }

    /// Параметр PowerShell `-Name` / `-Name:value` → (имя в нижнем регистре, встроенное значение).
    fn ps_flag(&self) -> Option<(String, Option<String>)> {
        if self.quoted || self.group {
            return None;
        }
        let t = self.low.strip_prefix('-')?;
        let t = t.strip_prefix('-').unwrap_or(t);
        if !t.chars().next()?.is_alphabetic() {
            return None;
        }
        Some(match t.split_once(':') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (t.to_string(), None),
        })
    }

    /// Флаг в стиле cmd: `/s`, `/q`, `/MIR`, `/a:h`.
    fn slash_flag(&self) -> bool {
        !self.quoted && !self.group && self.low.starts_with('/') && self.low.len() >= 2 && !self.low[1..].contains('\\')
    }

    fn is_flag(&self) -> bool {
        self.slash_flag() || (!self.quoted && !self.group && self.low.starts_with('-') && self.low.len() > 1)
    }

    fn has_wildcard(&self) -> bool {
        self.text.contains('*') || self.text.contains('?')
    }
}

/// Имена флагов (`/s/q` → s, q; `--force` → force; `-rf` → rf), в нижнем регистре.
fn flag_names(args: &[Arg]) -> Vec<String> {
    let mut out = Vec::new();
    for a in args.iter().filter(|a| a.is_flag()) {
        if a.low.starts_with('/') {
            for p in a.low.split('/').filter(|p| !p.is_empty()) {
                out.push(p.split(':').next().unwrap_or(p).to_string());
            }
        } else {
            let t = a.low.trim_start_matches('-');
            out.push(t.split(['=', ':']).next().unwrap_or(t).to_string());
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Основная цель операции.
    Path,
    /// Источник (только чтение).
    Read,
    /// Куда пишем.
    Dest,
    /// Имя дочернего элемента (New-Item -Name, Rename-Item -NewName).
    Name,
    /// Значение, не путь.
    Value,
    Switch,
}

#[derive(Debug, Default)]
struct Parsed {
    items: Vec<(Role, &'static str, Arg)>,
    switches: Vec<String>,
}

impl Parsed {
    fn role(&self, r: Role) -> Vec<&Arg> {
        self.items.iter().filter(|(rr, _, _)| *rr == r).map(|(_, _, a)| a).collect()
    }
    fn val(&self, param: &str) -> Option<&Arg> {
        self.items.iter().find(|(_, p, _)| *p == param).map(|(_, _, a)| a)
    }
    fn has(&self, param: &str) -> bool {
        self.items.iter().any(|(_, p, _)| *p == param) || self.switch(param)
    }
    /// Переключатель, в том числе по префиксу (`-r` → recurse).
    fn switch(&self, full: &str) -> bool {
        self.switches.iter().any(|s| !s.is_empty() && full.starts_with(s.as_str()))
    }
}

/// Разбор параметров командлета: имена/роли и позиционные параметры.
/// Неизвестные `-флаги` считаются переключателями (их значение уйдёт в позиционные — консервативно).
fn ps_parse(args: &[Arg], spec: &[(&'static str, Role)], pos: &[(&'static str, Role)]) -> Parsed {
    let mut out = Parsed::default();
    let mut positional: Vec<Vec<Arg>> = Vec::new();
    let mut bound: Vec<&'static str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some((name, inline)) = a.ps_flag() {
            let found = spec
                .iter()
                .find(|(p, _)| *p == name)
                .or_else(|| spec.iter().find(|(p, _)| p.starts_with(name.as_str())));
            match found {
                Some((p, Role::Switch)) => out.switches.push((*p).to_string()),
                Some((p, role)) => {
                    bound.push(p);
                    if let Some(v) = inline {
                        out.items.push((*role, p, Arg::plain(&v)));
                    } else if i + 1 < args.len() {
                        i += 1;
                        out.items.push((*role, p, args[i].clone()));
                        while i + 1 < args.len() && args[i + 1].comma {
                            i += 1;
                            out.items.push((*role, p, args[i].clone()));
                        }
                    }
                }
                None => out.switches.push(name),
            }
        } else {
            let mut v = vec![a.clone()];
            while i + 1 < args.len() && args[i + 1].comma {
                i += 1;
                v.push(args[i].clone());
            }
            positional.push(v);
        }
        i += 1;
    }
    let free: Vec<&(&'static str, Role)> = pos.iter().filter(|(p, _)| !bound.contains(p)).collect();
    let last_role = pos.last().map(|(_, r)| *r).unwrap_or(Role::Path);
    for (k, vals) in positional.into_iter().enumerate() {
        let (p, r) = free.get(k).map(|(p, r)| (*p, *r)).unwrap_or(("", last_role));
        for v in vals {
            out.items.push((r, p, v));
        }
    }
    out
}

// ───────────────────────────── цели (пути/реестр) ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Read,
    Write,
    Delete { recursive: bool },
    /// Удаление значения реестра (не ключа).
    DeleteValue,
}

fn target(acc: &mut Acc, ctx: &PolicyCtx, shell: Shell, a: &Arg, op: Op, what: &str) {
    if op == Op::Read {
        return;
    }
    if a.group {
        acc.raise(T3, format!("{what}: путь вычисляется во время выполнения"));
        return;
    }
    if let Some(reg) = parse_reg_path(&a.text) {
        let key = &a.text;
        let user = reg.hive == Hive::CurrentUser && !reg.sensitive();
        match op {
            Op::Read => {}
            Op::Write | Op::DeleteValue if user => acc.raise(T2, format!("{what}: реестр HKCU ({key})")),
            Op::Write => acc.raise(T3, format!("{what}: изменение реестра вне HKCU ({key})")),
            Op::DeleteValue => acc.raise(T3, format!("{what}: удаление значения реестра вне HKCU ({key})")),
            Op::Delete { .. } => acc.raise(T3, format!("{what}: удаление ключа реестра ({key})")),
        }
        return;
    }
    if shell == Shell::PowerShell
        && let Some(p) = ps_provider(&a.text)
    {
        match p {
            "env" | "variable" => {}
            "function" | "alias" => acc.raise(T3, format!("{what}: переопределение команд ({})", a.text)),
            "cert" => acc.raise(T3, format!("{what}: хранилище сертификатов")),
            "wsman" => acc.raise(T3, format!("{what}: настройки WinRM")),
            _ => acc.raise(T3, format!("{what}: неизвестный диск PowerShell ({})", a.text)),
        }
        return;
    }
    let eff = acc.effective(ctx);
    let (zone, _) = zone_of(&a.text, a.expandable, &eff);
    let pop = match op {
        Op::Write => PathOp::Write,
        Op::Delete { recursive } => PathOp::Delete { recursive },
        Op::DeleteValue => PathOp::Delete { recursive: false },
        Op::Read => PathOp::Read,
    };
    let (tier, protected, why) = path_tier(&zone, pop);
    let msg = format!("{what}: {why} — {}", a.text);
    if protected {
        acc.protect(msg);
    } else {
        acc.raise(tier, msg);
    }
}

/// Нормализованный путь аргумента (для `cd`), `None` — не вычислить заранее.
fn resolve_dir(acc: &Acc, ctx: &PolicyCtx, a: &Arg) -> Option<std::path::PathBuf> {
    if a.group || parse_reg_path(&a.text).is_some() {
        return None;
    }
    let eff = acc.effective(ctx);
    match zone_of(&a.text, a.expandable, &eff) {
        (PathZone::Unknown, _) | (_, None) => None,
        (_, Some(p)) => Some(p.into()),
    }
}

// ───────────────────────────── классификация сегментов ─────────────────────────────

struct Cx<'a> {
    shell: Shell,
    ctx: &'a PolicyCtx,
    acc: &'a mut Acc,
    depth: usize,
}

const PS_KEYWORDS: &[&str] = &[
    "if", "elseif", "else", "switch", "foreach", "for", "while", "do", "until", "try", "catch", "finally", "trap",
    "throw", "return", "break", "continue", "exit", "function", "filter", "workflow", "param", "begin", "process",
    "end", "clean", "dynamicparam", "class", "enum", "using", "data", "default", "hidden", "static", "parallel",
    "sequence", "inlinescript", "configuration",
];

fn is_number(t: &str) -> bool {
    let t = t.trim_start_matches(['-', '+']);
    if t.is_empty() {
        return false;
    }
    if let Some(h) = t.strip_prefix("0x") {
        return !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit());
    }
    let core = ["kb", "mb", "gb", "tb", "pb", "l", "d"]
        .iter()
        .find_map(|s| t.strip_suffix(s))
        .unwrap_or(t);
    !core.is_empty() && core.chars().all(|c| c.is_ascii_digit() || c == '.' || c == 'e') && core.chars().next().unwrap().is_ascii_digit()
}

/// Может ли токен начинать команду (голое слово, не переменная/литерал/оператор).
fn starts_command(t: &Tok) -> bool {
    match t {
        Tok::Word { text, quoted: false, .. } => {
            let s = text.as_str();
            !(s.is_empty()
                || s.starts_with(['$', '[', '-', '@', '!', '+', '*', '/', '%', ':', '='])
                || (s.starts_with('.') && !s.starts_with(".\\") && !s.starts_with("./") && !s.starts_with(".."))
                || is_number(s)
                || matches!(s.to_lowercase().as_str(), "in" | "else" | "elseif" | "until" | "catch" | "finally"))
        }
        _ => false,
    }
}

impl Cx<'_> {
    fn sub_script(&mut self, shell: Shell, src: &str) {
        classify_script(shell, src, self.ctx, self.acc, self.depth + 1, false);
    }

    fn ps_segment(&mut self, toks: &[Tok], piped: bool, in_hash: bool) {
        // весь вложенный код классифицируется независимо от команды
        for t in toks {
            if let Tok::Group { kind, inner, .. } = t {
                classify_script(Shell::PowerShell, inner, self.ctx, self.acc, self.depth + 1, *kind == GroupKind::Hash);
            }
        }
        self.method_calls(toks);
        self.ps_dispatch(toks, piped, in_hash);
    }

    fn ps_dispatch(&mut self, toks: &[Tok], piped: bool, in_hash: bool) {
        let Some(first) = toks.first() else { return };
        match first {
            Tok::Group { .. } => self.ps_expression(&toks[1..]),
            Tok::Word { text, quoted, .. } => {
                if *quoted {
                    // строковый литерал в начале — это вывод значения, а не запуск
                    return self.ps_expression(&toks[1..]);
                }
                let t = text.as_str();
                if t == "&" {
                    match toks.get(1) {
                        None => {}
                        Some(Tok::Word { text, .. }) if !text.contains('$') => {
                            let name = text.clone();
                            self.command(&name, &toks[2..], piped);
                        }
                        // `& { … }` — блок уже проверен как вложенный код
                        Some(Tok::Group { kind: GroupKind::Block, .. }) => {}
                        Some(_) => self.acc.raise(T3, "вызов команды из переменной/выражения (&) — нельзя проверить"),
                    }
                    return;
                }
                if t == "." {
                    self.acc.raise(T3, "dot-source сценария — содержимое неизвестно");
                    return;
                }
                if in_hash && toks.get(1).and_then(Tok::word) == Some("=") {
                    return self.ps_expression(&toks[1..]);
                }
                if !starts_command(first) {
                    return self.ps_expression(toks);
                }
                let low = t.to_lowercase();
                if PS_KEYWORDS.contains(&low.as_str()) {
                    return self.ps_keyword(&low, &toks[1..]);
                }
                let name = t.to_string();
                self.command(&name, &toks[1..], piped);
            }
        }
    }

    /// Выражение: ищем, где начинается команда (`$x = Get-Item …`, `… in Get-ChildItem`).
    fn ps_expression(&mut self, toks: &[Tok]) {
        for (i, t) in toks.iter().enumerate() {
            let Some(w) = t.word() else { continue };
            if !t.is_bare() {
                continue;
            }
            let lw = w.to_lowercase();
            if w == "=" || lw == "in" || lw == "return" || lw == "throw" {
                if let Some(next) = toks.get(i + 1)
                    && (starts_command(next) || next.word() == Some("&"))
                {
                    return self.ps_dispatch(&toks[i + 1..], false, false);
                }
                continue;
            }
            if w == "&" {
                return self.ps_dispatch(&toks[i..], false, false);
            }
            if i > 0 && starts_command(t) && !matches!(t, Tok::Word { attached: true, .. }) {
                // голое слово в позиции выражения — PowerShell выполнит его как команду
                return self.ps_dispatch(&toks[i..], false, false);
            }
        }
    }

    fn ps_keyword(&mut self, kw: &str, rest: &[Tok]) {
        match kw {
            "return" | "throw" | "exit" => {
                if rest.first().is_some_and(starts_command) {
                    self.ps_dispatch(rest, false, false);
                }
            }
            "using" => {
                let what = rest.first().and_then(Tok::word).unwrap_or("").to_lowercase();
                if what != "namespace" {
                    self.acc.raise(T3, format!("using {what}: загрузка внешнего кода"));
                }
            }
            _ => {}
        }
    }

    /// Вызовы методов .NET: `$x.Kill()`, `[IO.File]::Delete(…)`.
    fn method_calls(&mut self, toks: &[Tok]) {
        for i in 1..toks.len() {
            let Tok::Group { kind: GroupKind::Paren, attached: true, .. } = &toks[i] else { continue };
            let Some(prev) = toks[i - 1].word() else { continue };
            let (receiver, method) = match (prev.rfind("::"), prev.rfind('.')) {
                (Some(a), Some(b)) if b > a + 1 => (&prev[..b], &prev[b + 1..]),
                (Some(a), _) => (&prev[..a], &prev[a + 2..]),
                (None, Some(b)) => (&prev[..b], &prev[b + 1..]),
                (None, None) => continue, // `cmd(…)` — аргумент, а не метод
            };
            let m = method.trim_matches(['"', '\'']).to_lowercase();
            if m.is_empty() {
                continue;
            }
            let static_type = receiver.ends_with(']') && prev[receiver.len()..].starts_with("::");
            let ok = if static_type { static_call_ok(receiver, &m) } else { instance_call_ok(&m) };
            if !ok {
                self.acc.raise(T3, format!("вызов .NET-метода {receiver}.{method}() — нельзя проверить"));
            }
        }
    }

    fn cmd_segment(&mut self, toks: &[Tok], piped: bool) {
        let Some(first) = toks.first() else { return };
        match first {
            Tok::Group { inner, .. } => {
                let inner = inner.clone();
                self.sub_script(Shell::Cmd, &inner);
                for t in &toks[1..] {
                    if let Tok::Group { inner, .. } = t {
                        let inner = inner.clone();
                        self.sub_script(Shell::Cmd, &inner);
                    }
                }
            }
            Tok::Word { text, .. } => {
                let low = text.to_lowercase();
                if low.starts_with("::") || low == "rem" {
                    return;
                }
                match low.as_str() {
                    "if" => self.cmd_if(&toks[1..]),
                    "for" => self.cmd_for(&toks[1..]),
                    _ => {
                        // группы в аргументах обычной команды — тоже код (например, `echo (del x)` не бывает,
                        // но `call (…)` и подобное — проверяем консервативно)
                        for t in &toks[1..] {
                            if let Tok::Group { inner, .. } = t {
                                let inner = inner.clone();
                                self.sub_script(Shell::Cmd, &inner);
                            }
                        }
                        let name = text.clone();
                        self.command(&name, &toks[1..], piped);
                    }
                }
            }
        }
    }

    fn cmd_rest(&mut self, rest: &[Tok]) {
        let mut i = 0;
        while i < rest.len() {
            match &rest[i] {
                Tok::Group { inner, .. } => {
                    let inner = inner.clone();
                    self.sub_script(Shell::Cmd, &inner);
                    i += 1;
                }
                Tok::Word { text, .. } if text.eq_ignore_ascii_case("else") => i += 1,
                _ => {
                    // команда до `else`
                    let end = rest[i..]
                        .iter()
                        .position(|t| t.word().is_some_and(|w| w.eq_ignore_ascii_case("else")))
                        .map(|p| p + i)
                        .unwrap_or(rest.len());
                    self.cmd_segment(&rest[i..end], false);
                    i = end;
                }
            }
        }
    }

    fn cmd_if(&mut self, rest: &[Tok]) {
        let mut i = 0;
        let w = |k: usize| rest.get(k).and_then(Tok::word).map(str::to_lowercase).unwrap_or_default();
        while matches!(w(i).as_str(), "/i" | "not") {
            i += 1;
        }
        match w(i).as_str() {
            "exist" | "defined" | "errorlevel" | "cmdextversion" => i += 2,
            _ => {
                // str1==str2 (лексер разбивает по `=`) или str1 op str2
                if matches!(w(i + 1).as_str(), "equ" | "neq" | "lss" | "leq" | "gtr" | "geq") {
                    i += 3;
                } else {
                    i += 2;
                }
            }
        }
        if i > rest.len() {
            self.acc.raise(T3, "не удалось разобрать if");
            return;
        }
        self.cmd_rest(&rest[i..]);
    }

    fn cmd_for(&mut self, rest: &[Tok]) {
        // for [/d | /r [path] | /l | /f ["opts"]] %%v in (set) do command
        let Some(in_pos) = rest.iter().position(|t| t.word().is_some_and(|w| w.eq_ignore_ascii_case("in"))) else {
            self.acc.raise(T3, "не удалось разобрать for");
            return;
        };
        let is_f = rest[..in_pos].iter().any(|t| t.word().is_some_and(|w| w.eq_ignore_ascii_case("/f")));
        if let Some(Tok::Group { inner, .. }) = rest.get(in_pos + 1) {
            let s = inner.trim();
            if is_f && (s.starts_with('\'') || s.starts_with('`')) {
                let cmd = s.trim_matches(['\'', '`']).to_string();
                self.sub_script(Shell::Cmd, &cmd);
            }
        } else {
            self.acc.raise(T3, "не удалось разобрать for");
            return;
        }
        match rest.iter().position(|t| t.word().is_some_and(|w| w.eq_ignore_ascii_case("do"))) {
            Some(d) => self.cmd_rest(&rest[d + 1..]),
            None => self.acc.raise(T3, "не удалось разобрать for"),
        }
    }

    // ───────────────────────────── команды ─────────────────────────────

    fn command(&mut self, raw: &str, toks: &[Tok], piped: bool) {
        let args: Vec<Arg> = toks.iter().map(Arg::from_tok).collect();
        let mut trimmed = raw.trim_matches(['"', '\'']);
        if self.shell == Shell::Cmd {
            trimmed = trimmed.trim_start_matches('@');
        }
        let base = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed).to_lowercase();
        if base.is_empty() {
            return;
        }
        if let Some(ext) = script_ext(&base) {
            self.acc.raise(T3, format!("запуск файла «{trimmed}» ({ext}) — содержимое неизвестно"));
            return;
        }
        let mut name = base.clone();
        let mut had_ext = false;
        for ext in [".exe", ".com"] {
            if let Some(n) = name.strip_suffix(ext) {
                name = n.to_string();
                had_ext = true;
            }
        }
        if name.ends_with(".msc") {
            self.acc.raise(T1, format!("открытие консоли {name}"));
            return;
        }
        if piped && SHELLS.contains(&name.as_str()) {
            self.acc.raise(T3, format!("передача данных из конвейера в интерпретатор {name} (скачать-и-выполнить)"));
            return;
        }
        // `sc.exe`/`curl.exe` с расширением — это программа, а не псевдоним PowerShell
        let canon: String = if self.shell == Shell::PowerShell && !had_ext {
            ps_alias(&name).unwrap_or(&name).to_string()
        } else {
            name.clone()
        };
        let inv = Inv { name: &canon, args: &args, piped };
        self.dispatch(&inv, trimmed);
    }

    /// `cd`/`Set-Location`/`pushd`: дальнейшие относительные пути считаются от нового каталога.
    fn rule_cd(&mut self, inv: &Inv) {
        if matches!(inv.name, "pop-location" | "popd") {
            self.acc.cwd = Some(None);
            return;
        }
        let p = ps_parse(inv.args, &[("path", Role::Path), ("literalpath", Role::Path), ("stackname", Role::Value), ("passthru", Role::Switch)], &[("path", Role::Path)]);
        let dirs: Vec<&Arg> = p.role(Role::Path).into_iter().filter(|a| !a.slash_flag()).collect();
        let Some(d) = dirs.first() else { return };
        if d.text == "-" || d.text == "+" {
            self.acc.cwd = Some(None);
            return;
        }
        let new = resolve_dir(self.acc, self.ctx, d);
        self.acc.cwd = Some(new);
    }

    fn dispatch(&mut self, inv: &Inv, raw: &str) {
        let n = inv.name;
        let a = inv.args;
        // PowerShell `sc` — псевдоним Set-Content, но модель почти всегда имеет в виду sc.exe: берём максимум
        if self.shell == Shell::PowerShell && n == "set-content" && raw.eq_ignore_ascii_case("sc") {
            self.rule_sc(inv);
        }
        match n {
            // ── файлы ──
            "remove-item" | "del" | "erase" | "rd" | "rmdir" => self.rule_remove(inv),
            "copy-item" | "copy" => self.rule_copy(inv),
            "xcopy" => self.rule_copy_native(inv, false),
            "robocopy" => self.rule_robocopy(inv),
            "move-item" | "move" => self.rule_move(inv),
            "rename-item" | "ren" | "rename" => self.rule_rename(inv),
            "mkdir" | "md" => self.rule_mkdir(inv),
            "new-item" => self.rule_new_item(inv),
            "set-content" | "add-content" | "clear-content" | "out-file" | "export-csv" | "export-clixml"
            | "tee-object" | "start-transcript" | "set-acl" | "unblock-file" | "export-formatdata" => {
                self.rule_write_cmdlet(inv)
            }
            "set-item" | "clear-item" => self.rule_set_item(inv),
            "set-itemproperty" | "new-itemproperty" | "clear-itemproperty" | "rename-itemproperty" => {
                self.rule_itemprop(inv, Op::Write)
            }
            "remove-itemproperty" => self.rule_itemprop(inv, Op::DeleteValue),
            "copy-itemproperty" | "move-itemproperty" => self.rule_itemprop_copy(inv),
            "expand-archive" | "compress-archive" => self.rule_archive(inv),
            "invoke-item" => self.rule_invoke_item(inv),
            "mklink" => {
                if let Some(link) = a.iter().find(|x| !x.slash_flag()) {
                    target(self.acc, self.ctx, self.shell, link, Op::Write, "создание ссылки");
                } else {
                    self.acc.raise(T2, "mklink");
                }
            }
            "attrib" => {
                if a.iter().any(|x| !x.quoted && (x.low.starts_with('+') || x.low.starts_with('-'))) {
                    let paths: Vec<&Arg> = a.iter().filter(|x| !x.is_flag() && !x.low.starts_with('+')).collect();
                    self.targets_or(paths, Op::Write, "изменение атрибутов файла");
                }
            }
            "takeown" => {
                let paths = values_after(a, &["/f"]);
                self.targets_or(paths, Op::Write, "смена владельца (takeown)");
            }
            "icacls" | "cacls" => {
                let fl = flag_names(a);
                let modify = fl.iter().any(|f| {
                    matches!(f.as_str(), "grant" | "deny" | "remove" | "reset" | "setowner" | "setintegritylevel"
                        | "restore" | "inheritance" | "substitute" | "e" | "g" | "r" | "p")
                });
                if modify {
                    let first: Vec<&Arg> = a.iter().filter(|x| !x.is_flag()).take(1).collect();
                    self.targets_or(first, Op::Write, "изменение прав доступа (icacls)");
                }
            }
            "replace" | "expand" | "compact" | "extrac32" => {
                let paths: Vec<&Arg> = a.iter().filter(|x| !x.is_flag()).collect();
                let fl = flag_names(a);
                if n != "compact" || fl.iter().any(|f| f == "c" || f == "u") {
                    self.targets_or(paths, Op::Write, n);
                }
            }
            "sort" if self.shell == Shell::Cmd => {
                let out = values_after(a, &["/o", "/output"]);
                for p in out {
                    target(self.acc, self.ctx, self.shell, p, Op::Write, "sort /o");
                }
            }

            // ── чтение и безопасное ──
            "set-location" | "push-location" | "pop-location" | "cd" | "chdir" | "pushd" | "popd" => self.rule_cd(inv),
            "new-scheduledtaskaction" => {
                // действие задачи выполнится позже — проверяем его как команду
                let p = ps_parse(a, &[("execute", Role::Value), ("argument", Role::Value), ("workingdirectory", Role::Value), ("id", Role::Value)], &[("execute", Role::Value), ("argument", Role::Value)]);
                let exe = p.val("execute").map(|x| x.text.clone()).unwrap_or_default();
                let arg = p.val("argument").map(|x| x.text.clone()).unwrap_or_default();
                if exe.contains('$') || p.val("execute").is_some_and(|x| x.group) {
                    self.acc.raise(T3, "действие задачи из переменной");
                } else if !exe.is_empty() {
                    let script = format!("{} {arg}", quote_prog(&exe));
                    self.sub_script(Shell::Cmd, &script);
                }
            }
            n if T0_NAMES.contains(&n) => {}
            "write-eventlog" | "send-mailmessage" => self.acc.raise(T2, n.to_string()),

            // ── процессы ──
            "stop-process" | "taskkill" => self.rule_kill(inv),
            "start-process" => self.rule_start_process(inv),
            "start" => self.rule_cmd_start(inv),
            "debug-process" => self.acc.raise(T3, "отладка процесса"),

            // ── оболочки и выполнение кода ──
            "powershell" | "pwsh" => self.rule_powershell(inv),
            "cmd" => self.rule_cmd(inv),
            "invoke-expression" => self.acc.raise(T3, "Invoke-Expression: выполнение строки как кода"),
            "invoke-history" => self.acc.raise(T3, "повтор команды из истории — содержимое неизвестно"),
            "invoke-command" | "start-job" | "start-threadjob" => {
                let fl = flag_names(a);
                if fl.iter().any(|f| {
                    ["computername", "session", "vmname", "vmid", "containerid", "hostname", "connectionuri", "cn"]
                        .iter()
                        .any(|p| f.len() >= 2 && p.starts_with(f.as_str()))
                }) {
                    self.acc.raise(T3, "удалённое выполнение команд");
                }
                if fl.iter().any(|f| f.len() >= 2 && "filepath".starts_with(f.as_str())) {
                    self.acc.raise(T3, "выполнение файла сценария");
                }
                if !a.iter().any(|x| x.group) && a.iter().any(|x| !x.is_flag()) && n == "invoke-command" {
                    // блок передан переменной — содержимое неизвестно
                    if a.iter().any(|x| x.low.starts_with('$')) {
                        self.acc.raise(T3, "блок кода из переменной — нельзя проверить");
                    }
                }
            }
            "add-type" => self.acc.raise(T3, "Add-Type: компиляция произвольного кода"),
            "set-alias" | "new-alias" | "import-alias" => self.acc.raise(T3, "переопределение команд (alias)"),
            "import-module" => self.rule_import_module(inv),
            "new-module" => {}
            "register-objectevent" | "register-engineevent" | "register-wmievent" | "register-cimindicationevent" => {
                self.acc.raise(T2, "подписка на события");
            }
            "bash" | "sh" | "zsh" | "wsl" => self.rule_wsl(inv),
            "python" | "python3" | "py" | "pythonw" => self.rule_python(inv),
            "node" | "deno" | "bun" => self.rule_node(inv),
            "call" => {
                let rest: Vec<String> = a.iter().map(|x| x.text.clone()).collect();
                if rest.first().is_some_and(|r| r.starts_with(':')) {
                    self.acc.raise(T3, "call :метка — содержимое неизвестно");
                } else {
                    self.sub_script(Shell::Cmd, &join_args(a));
                }
            }

            // ── сеть и загрузки ──
            "invoke-webrequest" | "invoke-restmethod" | "curl" | "wget" => self.rule_web(inv),
            "start-bitstransfer" => {
                let p = ps_parse(a, &[("source", Role::Value), ("destination", Role::Dest)], &[("source", Role::Value), ("destination", Role::Dest)]);
                self.acc.raise(T2, "загрузка файла (BITS)");
                for d in p.role(Role::Dest) {
                    target(self.acc, self.ctx, self.shell, d, Op::Write, "загрузка файла");
                }
            }
            "bitsadmin" => {
                let fl = flag_names(a);
                if !fl.iter().all(|f| matches!(f.as_str(), "list" | "info" | "monitor" | "allusers" | "verbose" | "?")) {
                    self.acc.raise(T3, "bitsadmin: загрузка/задания BITS");
                }
            }
            "certutil" => {
                let fl = flag_names(a);
                let read = fl.iter().all(|f| matches!(f.as_str(), "hashfile" | "dump" | "store" | "user" | "v" | "?" | "verify"));
                if !read || fl.is_empty() {
                    self.acc.raise(T3, "certutil: загрузка/декодирование/сертификаты");
                }
            }
            "ipconfig" => self.rule_ipconfig(inv),
            "arp" => {
                if flag_names(a).iter().any(|f| f == "d" || f == "s") {
                    self.acc.raise(T3, "изменение ARP-таблицы");
                }
            }
            "route" => {
                let w = words(a);
                if w.iter().any(|x| matches!(x.as_str(), "add" | "delete" | "change" | "-f" | "/f")) {
                    self.acc.raise(T3, "изменение таблицы маршрутизации");
                }
            }
            "nbtstat" => {
                if a.iter().any(|x| x.text == "-R" || x.text == "-RR") {
                    self.acc.raise(T2, "сброс кэша NetBIOS");
                }
            }
            "netsh" => self.rule_netsh(inv),
            "net" | "net1" => self.rule_net(inv),
            "clear-dnsclientcache" | "register-dnsclient" => self.acc.raise(T2, "сброс кэша DNS"),
            "ssh" | "scp" | "sftp" | "plink" | "pscp" | "putty" | "telnet" | "ftp" | "tftp" => {
                if !(a.len() == 1 && (a[0].text == "-V" || a[0].low == "--version")) {
                    self.acc.raise(T3, format!("{n}: удалённое подключение/передача"));
                }
            }

            // ── службы, система ──
            "sc" => self.rule_sc(inv),
            "start-service" | "stop-service" | "restart-service" | "suspend-service" | "resume-service"
            | "set-service" | "new-service" | "remove-service" => self.acc.raise(T3, format!("{n}: управление службами")),
            "reg" => self.rule_reg(inv),
            "regedit" | "regedt32" => {
                if flag_names(a).iter().any(|f| f == "s") || a.iter().any(|x| x.low.ends_with(".reg")) {
                    self.acc.raise(T3, "импорт .reg-файла");
                } else {
                    self.acc.raise(T1, "открытие редактора реестра");
                }
            }
            "setx" => {
                if flag_names(a).iter().any(|f| f == "m" || f == "s") {
                    self.acc.raise(T3, "setx /m: системная переменная окружения");
                } else {
                    self.acc.raise(T2, "setx: переменная окружения пользователя (HKCU)");
                }
            }
            "assoc" | "ftype" => {
                if a.len() > 1 {
                    self.acc.raise(T3, format!("{n}: изменение ассоциаций файлов для всех пользователей"));
                }
            }
            "schtasks" => self.rule_schtasks(inv),
            "register-scheduledtask" => self.rule_register_task(inv),
            "unregister-scheduledtask" | "set-scheduledtask" => self.acc.raise(T3, format!("{n}: изменение задач планировщика")),
            "start-scheduledtask" | "stop-scheduledtask" => self.acc.raise(T2, n.to_string()),
            "enable-scheduledtask" | "disable-scheduledtask" => {
                if a.iter().any(|x| x.low.contains("\\microsoft\\") || x.low.contains("microsoft")) {
                    self.acc.raise(T3, "изменение системной задачи планировщика");
                } else {
                    self.acc.raise(T2, n.to_string());
                }
            }
            "powercfg" => self.rule_powercfg(inv),
            "wmic" => {
                let w = words(a);
                if w.iter().any(|x| matches!(x.as_str(), "call" | "delete" | "set" | "create") || x.starts_with("/node")) {
                    self.acc.raise(T3, "wmic: изменение/вызов методов WMI");
                }
            }
            "invoke-wmimethod" | "invoke-cimmethod" | "set-wmiinstance" | "set-ciminstance" | "remove-wmiobject"
            | "remove-ciminstance" | "new-ciminstance" => self.acc.raise(T3, format!("{n}: изменение через WMI/CIM")),
            "bcdedit" => {
                let fl = flag_names(a);
                if !(fl.is_empty() || fl.iter().all(|f| f == "enum" || f == "v")) {
                    self.acc.raise(T3, "bcdedit: изменение загрузчика Windows");
                }
            }
            "diskpart" | "format" | "format-volume" | "clear-disk" | "initialize-disk" | "new-partition"
            | "remove-partition" | "resize-partition" | "set-partition" | "set-disk" | "new-volume" | "bootrec"
            | "bcdboot" | "bootsect" | "convert" | "recover" | "label" | "optimize-volume" | "repair-volume"
            | "mount-diskimage" | "dismount-diskimage" | "remove-physicaldisk" => {
                self.acc.raise(T3, format!("{n}: операции с дисками/загрузкой"))
            }
            "shutdown" => {
                if flag_names(a).iter().any(|f| f == "a") && a.len() == 1 {
                    self.acc.raise(T1, "отмена выключения");
                } else {
                    self.acc.raise(T3, "выключение/перезагрузка компьютера");
                }
            }
            "restart-computer" | "stop-computer" | "logoff" | "tsdiscon" | "tscon" | "rename-computer"
            | "add-computer" | "remove-computer" | "restore-computer" | "disable-computerrestore"
            | "enable-computerrestore" | "reset-computermachinepassword" | "systemreset" => {
                self.acc.raise(T3, format!("{n}: состояние компьютера"))
            }
            "checkpoint-computer" => self.acc.raise(T2, "создание точки восстановления"),
            "set-date" | "set-timezone" | "set-culture" | "set-winsystemlocale" => self.acc.raise(T3, format!("{n}: системные настройки")),
            "tzutil" => {
                if flag_names(a).iter().any(|f| f == "s") {
                    self.acc.raise(T3, "смена часового пояса");
                }
            }
            "w32tm" => {
                let fl = flag_names(a);
                if fl.iter().any(|f| f == "resync") {
                    self.acc.raise(T2, "синхронизация времени");
                } else if !fl.iter().all(|f| matches!(f.as_str(), "query" | "tz" | "stripchart" | "monitor" | "computer" | "status" | "source" | "configuration" | "peers" | "verbose" | "dataonly" | "samples")) {
                    self.acc.raise(T3, "w32tm: изменение службы времени");
                }
            }
            "date" | "time" => {
                if !(a.is_empty() || flag_names(a).iter().any(|f| f == "t")) {
                    self.acc.raise(T3, "установка системной даты/времени");
                }
            }
            "set-executionpolicy" => {
                let p = ps_parse(a, &[("executionpolicy", Role::Value), ("scope", Role::Value), ("force", Role::Switch)], &[("executionpolicy", Role::Value), ("scope", Role::Value)]);
                match p.val("scope").map(|s| s.low.as_str()) {
                    Some("process") => {}
                    Some("currentuser") => self.acc.raise(T2, "политика выполнения для пользователя"),
                    _ => self.acc.raise(T3, "политика выполнения PowerShell для всей системы"),
                }
            }
            "set-mppreference" | "add-mppreference" | "remove-mppreference" | "set-mpthreat" | "start-mpwdoscan"
            | "mpcmdrun" => {
                if n == "mpcmdrun" && flag_names(a).iter().all(|f| f == "scan" || f == "scantype" || f == "signatureupdate") && !a.is_empty() {
                    self.acc.raise(T1, "проверка Defender");
                } else {
                    self.acc.raise(T3, format!("{n}: настройки Microsoft Defender"));
                }
            }
            "start-mpscan" | "update-mpsignature" => self.acc.raise(T1, "проверка/обновление Defender"),
            "remove-mpthreat" => self.acc.raise(T2, "удаление обнаруженных угроз Defender"),
            "pnputil" | "devcon" => {
                let w = words(a);
                if !(w.iter().any(|x| x.starts_with("/enum") || x.starts_with("-enum") || x == "find" || x == "status" || x == "hwids" || x == "drivernodes" || x == "/?")) {
                    self.acc.raise(T3, format!("{n}: драйверы и устройства"));
                }
            }
            "disable-pnpdevice" | "enable-pnpdevice" => self.acc.raise(T3, "включение/отключение устройства"),
            "manage-bde" | "enable-bitlocker" | "disable-bitlocker" | "suspend-bitlocker" | "resume-bitlocker"
            | "add-bitlockerkeyprotector" | "remove-bitlockerkeyprotector" | "lock-bitlocker" | "unlock-bitlocker" => {
                if !(n == "manage-bde" && flag_names(a).iter().all(|f| f == "status" || f == "protectors" || f == "get")) || a.is_empty() {
                    self.acc.raise(T3, "BitLocker");
                }
            }
            "vssadmin" | "wbadmin" | "reagentc" | "mountvol" | "fltmc" | "auditpol" | "dsregcmd" | "cmdkey"
            | "logman" | "openfiles" | "klist" | "wevtutil" | "fsutil" | "sfc" | "dism" | "chkdsk" | "defrag"
            | "cipher" | "gpresult" | "nvidia-smi" => self.rule_sys_tool(inv),
            "gpupdate" => self.acc.raise(T2, "обновление групповых политик"),
            "repair-windowsimage" => {
                if !flag_names(a).iter().any(|f| f == "checkhealth" || f == "scanhealth") {
                    self.acc.raise(T3, "восстановление образа Windows");
                }
            }
            "clear-recyclebin" => self.acc.raise(T3, "очистка корзины (необратимо)"),
            "clear-eventlog" | "remove-eventlog" | "limit-eventlog" | "new-eventlog" => self.acc.raise(T3, "журналы событий"),
            "enable-psremoting" | "disable-psremoting" | "enter-pssession" | "new-pssession" | "connect-pssession"
            | "enable-wsmancredssp" | "set-wsmanquickconfig" | "receive-pssession" | "import-pssession" => {
                self.acc.raise(T3, "удалённое управление PowerShell")
            }
            "install-module" | "install-script" => {
                let p = ps_parse(a, &[("name", Role::Value), ("scope", Role::Value), ("force", Role::Switch)], &[("name", Role::Value)]);
                if p.val("scope").is_some_and(|s| s.low == "currentuser") {
                    self.acc.raise(T2, "установка модуля PowerShell для пользователя");
                } else {
                    self.acc.raise(T3, "установка модуля PowerShell для всех пользователей");
                }
            }
            "save-module" | "save-script" => {
                let p = ps_parse(a, &[("name", Role::Value), ("path", Role::Dest), ("literalpath", Role::Dest)], &[("name", Role::Value), ("path", Role::Dest)]);
                self.targets_or(p.role(Role::Dest), Op::Write, "сохранение модуля");
            }
            "update-module" | "update-script" | "uninstall-module" | "uninstall-script" | "install-package"
            | "uninstall-package" | "install-packageprovider" | "register-psrepository" | "set-psrepository"
            | "register-packagesource" | "unregister-packagesource" | "set-packagesource" => {
                self.acc.raise(T3, format!("{n}: установка/удаление пакетов"))
            }
            "add-appxpackage" | "remove-appxpackage" | "add-appxprovisionedpackage" | "remove-appxprovisionedpackage"
            | "reset-appxpackage" | "enable-windowsoptionalfeature" | "disable-windowsoptionalfeature"
            | "install-windowsfeature" | "uninstall-windowsfeature" | "add-windowscapability"
            | "remove-windowscapability" | "add-windowspackage" | "remove-windowspackage" | "add-windowsdriver"
            | "remove-windowsdriver" | "msiexec" | "wusa" => self.acc.raise(T3, format!("{n}: установка/удаление компонентов")),
            "winget" => self.rule_winget(inv),
            "choco" | "chocolatey" | "scoop" => self.rule_choco(inv),
            "rundll32" | "regsvr32" | "mshta" | "wscript" | "cscript" | "runas" | "psexec" | "psexec64" | "at"
            | "installutil" | "regasm" | "regsvcs" | "cmstp" | "odbcconf" | "forfiles" | "pcalua" | "msbuild"
            | "syncappvpublishingserver" | "verifier" | "lodctr" | "unlodctr" | "sdbinst" | "secedit" | "certreq"
            | "ntdsutil" | "esentutl" | "makecab" | "diantz" | "msdt" | "presentationhost" | "hh" | "ie4uinit"
            | "infdefaultinstall" | "control.exe" => self.acc.raise(T3, format!("{n}: запуск произвольного кода/системный инструмент")),
            "new-netfirewallrule" | "set-netfirewallrule" | "remove-netfirewallrule" | "enable-netfirewallrule"
            | "disable-netfirewallrule" | "set-netfirewallprofile" | "copy-netfirewallrule" | "rename-netfirewallrule" => {
                self.acc.raise(T3, "изменение брандмауэра")
            }
            "new-localuser" | "set-localuser" | "remove-localuser" | "enable-localuser" | "disable-localuser"
            | "rename-localuser" | "add-localgroupmember" | "remove-localgroupmember" | "new-localgroup"
            | "remove-localgroup" | "set-localgroup" | "rename-localgroup" => self.acc.raise(T3, "изменение учётных записей"),
            "export-pfxcertificate" | "import-pfxcertificate" | "import-certificate" => self.acc.raise(T3, "сертификаты и закрытые ключи"),
            "test-computersecurechannel" => {
                if flag_names(a).iter().any(|f| "repair".starts_with(f.as_str())) {
                    self.acc.raise(T3, "восстановление доверия с доменом");
                }
            }

            // ── разработка ──
            "git" => self.rule_git(inv),
            "gh" => self.rule_gh(inv),
            "npm" | "yarn" | "pnpm" => self.rule_npm(inv),
            "npx" | "pnpx" | "bunx" => {
                if !(a.len() == 1 && (a[0].low == "--version" || a[0].low == "-v")) {
                    self.acc.raise(T3, "npx: загрузка и запуск пакета");
                }
            }
            "cargo" => self.rule_cargo(inv),
            "rustup" => self.rule_simple_sub(inv, &["show", "--version", "-v", "-V", "which", "check", "help", "doc", "default"], &["update", "install", "target", "component", "toolchain", "override", "set", "run"], "rustup"),
            "rustc" | "javac" | "go" | "dotnet" | "mvn" | "gradle" | "gradlew" | "java" | "tsc" => self.rule_dev_tool(inv),
            "pip" | "pip3" => self.rule_pip(inv),
            "docker" | "podman" => self.rule_docker(inv),
            "kubectl" => {
                let sub = first_word(a);
                if !matches!(sub.as_deref(), Some("get" | "describe" | "logs" | "version" | "top" | "explain" | "api-resources" | "cluster-info" | "config")) {
                    self.acc.raise(T3, "kubectl: изменение кластера");
                }
            }
            "tar" => {
                let fl = flag_names(a);
                let list = fl.iter().any(|f| f.starts_with('t') || f == "list") && !fl.iter().any(|f| f.contains('x') || f.contains('c'));
                if !list {
                    self.acc.raise(T2, "tar: распаковка/упаковка");
                    for d in values_after(a, &["-c", "--directory", "-f", "--file"]) {
                        target(self.acc, self.ctx, self.shell, d, Op::Write, "tar");
                    }
                }
            }
            "7z" | "7za" | "7zr" => {
                match first_word(a).as_deref() {
                    Some("l" | "t" | "h" | "i") => {}
                    Some("d") => self.acc.raise(T3, "7z: удаление из архива"),
                    _ => {
                        self.acc.raise(T2, "7z: распаковка/упаковка");
                        for x in a.iter().filter(|x| x.low.starts_with("-o")) {
                            let d = Arg::plain(&x.text[2..]);
                            target(self.acc, self.ctx, self.shell, &d, Op::Write, "7z");
                        }
                    }
                }
            }

            // ── GUI и открытие ──
            n if GUI_APPS.contains(&n) => {
                if matches!(n, "explorer" | "control") && a.iter().any(|x| !x.is_flag() && exec_like(&x.low)) {
                    self.acc.raise(T3, format!("запуск исполняемого файла через {n}"));
                } else {
                    self.acc.raise(T1, format!("запуск приложения {n}"));
                }
            }
            "set-clipboard" | "clip" | "out-gridview" | "out-printer" | "show-command" | "msg" | "print" => {
                self.acc.raise(T1, n.to_string())
            }
            "new-psdrive" => self.acc.raise(T1, "подключение диска PowerShell"),
            "set-winuserlanguagelist" | "set-winhomelocation" | "set-wincultureformat" => self.acc.raise(T2, n.to_string()),
            "doskey" => {
                if !a.iter().all(|x| x.low.starts_with("/history") || x.low.starts_with("/macros")) {
                    self.acc.raise(T1, "doskey");
                }
            }
            "cleanmgr" => {
                if a.is_empty() {
                    self.acc.raise(T1, "очистка диска (окно)");
                } else {
                    self.acc.raise(T2, "очистка диска");
                }
            }

            _ => self.unknown(inv, raw),
        }
    }

    fn unknown(&mut self, inv: &Inv, raw: &str) {
        let n = inv.name;
        if let Some((verb, noun)) = n.split_once('-')
            && !noun.is_empty()
        {
            let read = match verb {
                "get" | "test" | "find" | "measure" | "compare" | "resolve" | "search" | "select" | "sort" | "group"
                | "convertto" | "convertfrom" | "convert" | "join" | "split" | "read" | "wait" | "watch" | "trace" => true,
                "format" => matches!(noun, "table" | "list" | "wide" | "custom" | "hex"),
                "out" => matches!(noun, "string" | "null" | "host" | "default"),
                "write" => matches!(noun, "output" | "host" | "verbose" | "debug" | "warning" | "error" | "information" | "progress"),
                _ => false,
            };
            if read {
                return;
            }
            if verb == "show" {
                self.acc.raise(T1, format!("{n}: окно"));
                return;
            }
        }
        self.acc.raise(T3, format!("неизвестная команда «{raw}» — по умолчанию опасно"));
    }

    fn targets_or(&mut self, paths: Vec<&Arg>, op: Op, what: &str) {
        if paths.is_empty() {
            self.acc.raise(T3, format!("{what}: путь не указан явно"));
        }
        for p in paths {
            target(self.acc, self.ctx, self.shell, p, op, what);
        }
    }

    // ───────────────────────────── правила: файлы ─────────────────────────────

    fn rule_remove(&mut self, inv: &Inv) {
        let p = ps_parse(
            inv.args,
            &[
                ("path", Role::Path),
                ("literalpath", Role::Path),
                ("pspath", Role::Path),
                ("lp", Role::Path),
                ("filter", Role::Value),
                ("include", Role::Value),
                ("exclude", Role::Value),
                ("credential", Role::Value),
                ("stream", Role::Value),
                ("recurse", Role::Switch),
                ("force", Role::Switch),
            ],
            &[("path", Role::Path)],
        );
        let cmd_flags: Vec<String> = flag_names(inv.args);
        let recursive = p.switches.iter().any(|s| s.starts_with('r')) || cmd_flags.iter().any(|f| f == "s");
        let paths: Vec<&Arg> = p.role(Role::Path).into_iter().filter(|a| !a.slash_flag()).collect();
        if paths.is_empty() {
            if inv.piped {
                self.acc.raise(T3, "удаление объектов из конвейера — пути заранее неизвестны");
            } else {
                self.acc.raise(T3, "удаление без явного пути");
            }
            return;
        }
        if p.has("include") || p.has("filter") {
            self.acc.raise(T3, "удаление по фильтру");
        }
        for a in paths {
            if a.has_wildcard() {
                self.acc.raise(T3, format!("удаление по маске — {}", a.text));
            }
            target(self.acc, self.ctx, self.shell, a, Op::Delete { recursive }, "удаление");
        }
    }

    fn rule_copy(&mut self, inv: &Inv) {
        let a = inv.args;
        if self.shell == Shell::Cmd || a.iter().any(|x| x.slash_flag()) {
            return self.rule_copy_native(inv, false);
        }
        let p = ps_parse(
            a,
            &[
                ("path", Role::Read),
                ("literalpath", Role::Read),
                ("destination", Role::Dest),
                ("filter", Role::Value),
                ("include", Role::Value),
                ("exclude", Role::Value),
                ("credential", Role::Value),
                ("tosession", Role::Value),
                ("fromsession", Role::Value),
                ("recurse", Role::Switch),
                ("force", Role::Switch),
                ("container", Role::Switch),
                ("passthru", Role::Switch),
            ],
            &[("path", Role::Read), ("destination", Role::Dest)],
        );
        if p.has("tosession") || p.has("fromsession") {
            self.acc.raise(T3, "копирование через удалённый сеанс");
        }
        let dest = p.role(Role::Dest);
        if dest.is_empty() {
            let cwd = Arg::plain(".");
            target(self.acc, self.ctx, self.shell, &cwd, Op::Write, "копирование в текущую папку");
        }
        for d in dest {
            target(self.acc, self.ctx, self.shell, d, Op::Write, "копирование");
        }
        if p.role(Role::Read).is_empty() && !inv.piped {
            self.acc.raise(T2, "копирование");
        }
    }

    fn rule_copy_native(&mut self, inv: &Inv, _robocopy: bool) {
        let paths: Vec<&Arg> = inv.args.iter().filter(|x| !x.is_flag()).collect();
        match paths.len() {
            0 => self.acc.raise(T3, "копирование без путей"),
            1 => {
                let cwd = Arg::plain(".");
                target(self.acc, self.ctx, self.shell, &cwd, Op::Write, "копирование в текущую папку");
            }
            _ => target(self.acc, self.ctx, self.shell, paths[paths.len() - 1], Op::Write, "копирование"),
        }
    }

    fn rule_robocopy(&mut self, inv: &Inv) {
        let paths: Vec<&Arg> = inv.args.iter().filter(|x| !x.is_flag()).collect();
        let fl = flag_names(inv.args);
        if paths.len() < 2 {
            self.acc.raise(T3, "robocopy: не указаны источник и назначение");
            return;
        }
        if fl.iter().any(|f| f == "mir" || f == "purge") {
            self.acc.raise(T3, "robocopy /MIR или /PURGE удаляет файлы в назначении");
        }
        if fl.iter().any(|f| f == "move" || f == "mov") {
            target(self.acc, self.ctx, self.shell, paths[0], Op::Write, "перенос (robocopy /MOVE)");
        }
        target(self.acc, self.ctx, self.shell, paths[1], Op::Write, "копирование (robocopy)");
        for f in inv.args.iter().filter(|x| x.low.starts_with("/log") || x.low.starts_with("/unilog")) {
            if let Some((_, v)) = f.text.split_once(':') {
                target(self.acc, self.ctx, self.shell, &Arg::plain(v), Op::Write, "журнал robocopy");
            }
        }
    }

    fn rule_move(&mut self, inv: &Inv) {
        let a = inv.args;
        let (srcs, dest): (Vec<Arg>, Option<Arg>) = if self.shell == Shell::Cmd || a.iter().any(|x| x.slash_flag()) {
            let mut paths: Vec<Arg> = a.iter().filter(|x| !x.is_flag()).cloned().collect();
            let dest = if paths.len() >= 2 { paths.pop() } else { None };
            (paths, dest)
        } else {
            let p = ps_parse(
                a,
                &[
                    ("path", Role::Path),
                    ("literalpath", Role::Path),
                    ("destination", Role::Dest),
                    ("filter", Role::Value),
                    ("include", Role::Value),
                    ("exclude", Role::Value),
                    ("credential", Role::Value),
                    ("force", Role::Switch),
                    ("passthru", Role::Switch),
                ],
                &[("path", Role::Path), ("destination", Role::Dest)],
            );
            let srcs = p.role(Role::Path).into_iter().cloned().collect();
            let dest = p.role(Role::Dest).first().map(|d| (*d).clone());
            (srcs, dest)
        };
        self.move_targets(&srcs, dest.as_ref(), inv.piped);
    }

    fn move_targets(&mut self, srcs: &[Arg], dest: Option<&Arg>, piped: bool) {
        if srcs.is_empty() {
            if piped {
                self.acc.raise(T3, "перенос объектов из конвейера — пути заранее неизвестны");
            } else {
                self.acc.raise(T3, "перенос без явного пути");
            }
        }
        for s in srcs {
            if s.has_wildcard() {
                self.acc.raise(T2, format!("перенос по маске — {}", s.text));
            }
            target(self.acc, self.ctx, self.shell, s, Op::Write, "перенос (источник)");
        }
        match dest {
            Some(d) => target(self.acc, self.ctx, self.shell, d, Op::Write, "перенос (назначение)"),
            None => target(self.acc, self.ctx, self.shell, &Arg::plain("."), Op::Write, "перенос в текущую папку"),
        }
    }

    fn rule_rename(&mut self, inv: &Inv) {
        let p = ps_parse(
            inv.args,
            &[("path", Role::Path), ("literalpath", Role::Path), ("newname", Role::Name), ("credential", Role::Value), ("force", Role::Switch), ("passthru", Role::Switch)],
            &[("path", Role::Path), ("newname", Role::Name)],
        );
        let paths = p.role(Role::Path);
        if paths.is_empty() && inv.piped {
            self.acc.raise(T3, "переименование объектов из конвейера");
        }
        self.targets_or(paths, Op::Write, "переименование");
    }

    fn rule_mkdir(&mut self, inv: &Inv) {
        let p = ps_parse(
            inv.args,
            &[("path", Role::Path), ("name", Role::Name), ("force", Role::Switch), ("itemtype", Role::Value), ("value", Role::Value)],
            &[("path", Role::Path)],
        );
        let mut paths: Vec<Arg> = p.role(Role::Path).into_iter().filter(|a| !a.slash_flag()).cloned().collect();
        if let Some(name) = p.val("name") {
            paths = if paths.is_empty() {
                vec![name.clone()]
            } else {
                paths.iter().map(|b| join_arg(b, name)).collect()
            };
        }
        let refs: Vec<&Arg> = paths.iter().collect();
        self.targets_or(refs, Op::Write, "создание папки");
    }

    fn rule_new_item(&mut self, inv: &Inv) {
        let p = ps_parse(
            inv.args,
            &[
                ("path", Role::Path),
                ("name", Role::Name),
                ("itemtype", Role::Value),
                ("type", Role::Value),
                ("value", Role::Value),
                ("target", Role::Value),
                ("credential", Role::Value),
                ("force", Role::Switch),
            ],
            &[("path", Role::Path)],
        );
        let mut paths: Vec<Arg> = p.role(Role::Path).into_iter().cloned().collect();
        if let Some(name) = p.val("name") {
            paths = if paths.is_empty() {
                vec![name.clone()]
            } else {
                paths.iter().map(|b| join_arg(b, name)).collect()
            };
        }
        let refs: Vec<&Arg> = paths.iter().collect();
        self.targets_or(refs, Op::Write, "создание");
    }

    fn rule_write_cmdlet(&mut self, inv: &Inv) {
        let n = inv.name;
        let mut spec: Vec<(&'static str, Role)> = vec![
            ("path", Role::Path),
            ("literalpath", Role::Path),
            ("filepath", Role::Path),
            ("pspath", Role::Path),
            ("lp", Role::Path),
            ("outputdirectory", Role::Path),
            ("value", Role::Value),
            ("encoding", Role::Value),
            ("delimiter", Role::Value),
            ("stream", Role::Value),
            ("filter", Role::Value),
            ("include", Role::Value),
            ("exclude", Role::Value),
            ("width", Role::Value),
            ("inputobject", Role::Value),
            ("credential", Role::Value),
            ("variable", Role::Value),
            ("aclobject", Role::Value),
            ("depth", Role::Value),
            ("usequotes", Role::Value),
            ("quotefields", Role::Value),
        ];
        spec.extend([("append", Role::Switch), ("force", Role::Switch), ("nonewline", Role::Switch), ("noclobber", Role::Switch), ("passthru", Role::Switch)]);
        let pos: &[(&'static str, Role)] = match n {
            "set-content" | "add-content" => &[("path", Role::Path), ("value", Role::Value)],
            "out-file" => &[("filepath", Role::Path), ("encoding", Role::Value)],
            "set-acl" => &[("path", Role::Path), ("aclobject", Role::Value)],
            _ => &[("path", Role::Path)],
        };
        let p = ps_parse(inv.args, &spec, pos);
        let paths = p.role(Role::Path);
        match n {
            "tee-object" if paths.is_empty() => {}
            "start-transcript" if paths.is_empty() => self.acc.raise(T2, "журнал сеанса в «Документах»"),
            "unblock-file" | "set-acl" if paths.is_empty() && inv.piped => self.acc.raise(T3, format!("{n}: объекты из конвейера")),
            _ => self.targets_or(paths, Op::Write, &format!("запись ({n})")),
        }
        if n == "set-acl" {
            self.acc.raise(T2, "изменение прав доступа");
        }
    }

    fn rule_set_item(&mut self, inv: &Inv) {
        let p = ps_parse(
            inv.args,
            &[("path", Role::Path), ("literalpath", Role::Path), ("value", Role::Value), ("filter", Role::Value), ("include", Role::Value), ("exclude", Role::Value), ("credential", Role::Value), ("force", Role::Switch), ("passthru", Role::Switch)],
            &[("path", Role::Path), ("value", Role::Value)],
        );
        let paths = p.role(Role::Path);
        self.targets_or(paths, Op::Write, inv.name);
    }

    fn rule_itemprop(&mut self, inv: &Inv, op: Op) {
        let p = ps_parse(
            inv.args,
            &[
                ("path", Role::Path),
                ("literalpath", Role::Path),
                ("name", Role::Value),
                ("newname", Role::Value),
                ("value", Role::Value),
                ("propertytype", Role::Value),
                ("type", Role::Value),
                ("filter", Role::Value),
                ("include", Role::Value),
                ("exclude", Role::Value),
                ("credential", Role::Value),
                ("inputobject", Role::Value),
                ("force", Role::Switch),
                ("passthru", Role::Switch),
            ],
            &[("path", Role::Path), ("name", Role::Value), ("value", Role::Value)],
        );
        let paths = p.role(Role::Path);
        if paths.is_empty() && inv.piped {
            self.acc.raise(T3, format!("{}: объекты из конвейера", inv.name));
            return;
        }
        self.targets_or(paths, op, "реестр/свойства");
    }

    fn rule_itemprop_copy(&mut self, inv: &Inv) {
        let mv = inv.name == "move-itemproperty";
        let p = ps_parse(
            inv.args,
            &[("path", Role::Path), ("literalpath", Role::Path), ("destination", Role::Dest), ("name", Role::Value), ("force", Role::Switch), ("passthru", Role::Switch)],
            &[("path", Role::Path), ("destination", Role::Dest), ("name", Role::Value)],
        );
        let dests = p.role(Role::Dest);
        self.targets_or(dests, Op::Write, "копирование свойства");
        if mv {
            let src = p.role(Role::Path);
            self.targets_or(src, Op::DeleteValue, "перенос свойства");
        }
    }

    fn rule_archive(&mut self, inv: &Inv) {
        let p = ps_parse(
            inv.args,
            &[("path", Role::Read), ("literalpath", Role::Read), ("destinationpath", Role::Dest), ("compressionlevel", Role::Value), ("force", Role::Switch), ("update", Role::Switch), ("passthru", Role::Switch)],
            &[("path", Role::Read), ("destinationpath", Role::Dest)],
        );
        let dests = p.role(Role::Dest);
        if dests.is_empty() {
            target(self.acc, self.ctx, self.shell, &Arg::plain("."), Op::Write, "распаковка в текущую папку");
        }
        for d in dests {
            target(self.acc, self.ctx, self.shell, d, Op::Write, "архив");
        }
    }

    fn rule_invoke_item(&mut self, inv: &Inv) {
        let p = ps_parse(inv.args, &[("path", Role::Path), ("literalpath", Role::Path), ("filter", Role::Value), ("include", Role::Value), ("exclude", Role::Value), ("credential", Role::Value)], &[("path", Role::Path)]);
        let paths = p.role(Role::Path);
        if paths.is_empty() {
            self.acc.raise(T3, "открытие объектов из конвейера");
        }
        for a in paths {
            if a.group || a.low.contains('$') {
                self.acc.raise(T3, "открытие файла по вычисляемому пути");
            } else if script_ext(&a.low).is_some() || a.low.ends_with(".exe") || a.low.ends_with(".com") {
                self.acc.raise(T3, format!("запуск исполняемого файла {}", a.text));
            } else {
                self.acc.raise(T1, format!("открытие {}", a.text));
            }
        }
    }

    // ───────────────────────────── правила: процессы и запуск ─────────────────────────────

    fn rule_kill(&mut self, inv: &Inv) {
        let a = inv.args;
        if inv.name == "taskkill" {
            let fl = flag_names(a);
            if fl.iter().any(|f| f == "s" || f == "fi") {
                self.acc.raise(T3, "taskkill: удалённо или по фильтру");
            }
            if fl.iter().any(|f| f == "pid") {
                self.acc.raise(T3, "завершение процесса по PID — владелец неизвестен");
            }
            let names = values_after(a, &["/im"]);
            if names.is_empty() && !fl.iter().any(|f| f == "pid") {
                self.acc.raise(T3, "taskkill без цели");
            }
            for n in names {
                self.kill_name(&n.low);
            }
            return;
        }
        let p = ps_parse(a, &[("id", Role::Value), ("name", Role::Name), ("processname", Role::Name), ("inputobject", Role::Value), ("force", Role::Switch), ("passthru", Role::Switch)], &[("id", Role::Value)]);
        if p.has("id") || p.has("inputobject") {
            self.acc.raise(T3, "завершение процесса по PID — владелец неизвестен");
        }
        let names = p.role(Role::Name);
        if names.is_empty() && !p.has("id") {
            self.acc.raise(T3, "завершение процессов из конвейера");
        }
        for n in names {
            if n.group || n.low.contains('$') {
                self.acc.raise(T3, "имя процесса вычисляется");
            } else {
                self.kill_name(&n.low);
            }
        }
    }

    fn kill_name(&mut self, name: &str) {
        let n = name.trim_end_matches(".exe");
        if n.contains('*') || n.contains('?') {
            self.acc.raise(T3, format!("завершение процессов по маске {name}"));
        } else if CRITICAL_PROCESSES.contains(&n) {
            self.acc.raise(T3, format!("завершение системного процесса {n}"));
        } else {
            self.acc.raise(T2, format!("завершение процесса {n}"));
        }
    }

    fn rule_start_process(&mut self, inv: &Inv) {
        let p = ps_parse(
            inv.args,
            &[
                ("filepath", Role::Path),
                ("argumentlist", Role::Value),
                ("args", Role::Value),
                ("verb", Role::Value),
                ("workingdirectory", Role::Value),
                ("credential", Role::Value),
                ("windowstyle", Role::Value),
                ("redirectstandardoutput", Role::Dest),
                ("redirectstandarderror", Role::Dest),
                ("redirectstandardinput", Role::Read),
                ("wait", Role::Switch),
                ("nonewwindow", Role::Switch),
                ("passthru", Role::Switch),
                ("usenewenvironment", Role::Switch),
                ("loaduserprofile", Role::Switch),
            ],
            &[("filepath", Role::Path), ("argumentlist", Role::Value)],
        );
        if p.val("verb").is_some_and(|v| v.low == "runas" || v.low.contains('$')) {
            self.acc.raise(T3, "запуск с повышением прав (RunAs)");
        }
        if p.has("credential") {
            self.acc.raise(T3, "запуск от имени другого пользователя");
        }
        for d in p.role(Role::Dest) {
            target(self.acc, self.ctx, self.shell, d, Op::Write, "вывод процесса в файл");
        }
        let prog = p.role(Role::Path);
        let Some(prog) = prog.first() else {
            self.acc.raise(T3, "Start-Process без программы");
            return;
        };
        let args: Vec<String> = p
            .items
            .iter()
            .filter(|(_, n, _)| *n == "argumentlist" || *n == "args")
            .map(|(_, _, a)| a.text.clone())
            .collect();
        let prog = (*prog).clone();
        self.launch(&prog, &args);
    }

    fn rule_cmd_start(&mut self, inv: &Inv) {
        let a = inv.args;
        let mut i = 0;
        if a.first().is_some_and(|x| x.quoted) && a.len() > 1 {
            i = 1; // заголовок окна
        }
        while i < a.len() && a[i].slash_flag() {
            if matches!(a[i].low.as_str(), "/d" | "/affinity" | "/node") {
                i += 1;
            }
            i += 1;
        }
        let Some(prog) = a.get(i) else {
            self.acc.raise(T1, "новое окно командной строки");
            return;
        };
        let args: Vec<String> = a[i + 1..].iter().map(|x| x.text.clone()).collect();
        let prog = prog.clone();
        self.launch(&prog, &args);
    }

    /// Запуск программы/документа/URI (Start-Process, start).
    fn launch(&mut self, prog: &Arg, args: &[String]) {
        if prog.group || prog.low.contains('$') || prog.low.contains('%') {
            self.acc.raise(T3, "запуск программы по вычисляемому пути");
            return;
        }
        let l = prog.low.trim();
        if l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:") {
            self.acc.raise(T1, format!("открытие ссылки {}", prog.text));
            return;
        }
        if l.starts_with("ms-settings:") || l.starts_with("shell:") || l.starts_with("windowsdefender:") || l.starts_with("ms-") {
            self.acc.raise(T1, format!("открытие {}", prog.text));
            return;
        }
        let base = l.rsplit(['\\', '/']).next().unwrap_or(l);
        if let Some(ext) = base.rsplit_once('.').map(|(_, e)| e)
            && DOC_EXTS.contains(&ext)
        {
            self.acc.raise(T1, format!("открытие файла {}", prog.text));
            return;
        }
        if (l.contains('\\') || l == "." || l == "..") && !base.contains('.') {
            self.acc.raise(T1, format!("открытие папки {}", prog.text));
            return;
        }
        // программа: классифицируем её как команду с аргументами
        let script = format!("{} {}", quote_prog(&prog.text), args.join(" "));
        self.sub_script(Shell::Cmd, &script);
        self.acc.raise(T1, format!("запуск программы {}", prog.text));
    }

    fn rule_powershell(&mut self, inv: &Inv) {
        let a = inv.args;
        let mut i = 0;
        let value_params = ["executionpolicy", "ep", "ex", "windowstyle", "w", "win", "outputformat", "o", "of", "inputformat", "if", "inp", "version", "v", "psconsolefile", "configurationname", "config", "workingdirectory", "wd", "settingsfile", "custompipename"];
        while i < a.len() {
            let x = &a[i];
            let flag = if x.is_flag() && !x.quoted { Some(x.low.trim_start_matches(['-', '/']).to_string()) } else { None };
            match flag {
                Some(f) if f == "e" || f == "ec" || (f.len() >= 2 && ("encodedcommand".starts_with(f.as_str()) || "encodedarguments".starts_with(f.as_str()))) => {
                    self.acc.raise(T3, "закодированная команда (-EncodedCommand) — нельзя проверить");
                    return;
                }
                Some(f) if f == "c" || (f.len() >= 2 && "command".starts_with(f.as_str())) => {
                    let rest = join_args(&a[i + 1..]);
                    if rest.trim() == "-" {
                        self.acc.raise(T3, "команда из стандартного ввода — содержимое неизвестно");
                    } else {
                        self.sub_script(Shell::PowerShell, &rest);
                    }
                    return;
                }
                Some(f) if f == "f" || (f.len() >= 2 && "file".starts_with(f.as_str())) => {
                    self.acc.raise(T3, "запуск файла сценария PowerShell");
                    return;
                }
                Some(f) if value_params.contains(&f.as_str()) => i += 2,
                Some(_) => i += 1,
                None => {
                    // позиционный текст — это -Command
                    let rest = join_args(&a[i..]);
                    self.sub_script(Shell::PowerShell, &rest);
                    return;
                }
            }
        }
        self.acc.raise(T3, "вложенная оболочка PowerShell без команды");
    }

    fn rule_cmd(&mut self, inv: &Inv) {
        let a = inv.args;
        for (i, x) in a.iter().enumerate() {
            if x.slash_flag() {
                let parts: Vec<&str> = x.low.split('/').filter(|p| !p.is_empty()).collect();
                if parts.iter().any(|p| *p == "c" || *p == "k" || *p == "r") {
                    let rest = join_args(&a[i + 1..]);
                    self.sub_script(Shell::Cmd, &rest);
                    return;
                }
            }
        }
        self.acc.raise(T3, "вложенная командная строка без команды");
    }

    fn rule_wsl(&mut self, inv: &Inv) {
        let a = inv.args;
        let readonly = inv.name == "wsl"
            && !a.is_empty()
            && a.iter().all(|x| matches!(x.low.as_str(), "--list" | "-l" | "--status" | "--version" | "-v" | "--help" | "--verbose" | "--running" | "-q" | "--quiet" | "--online" | "-o"));
        if !readonly {
            self.acc.raise(T3, format!("{}: выполнение команд Linux", inv.name));
        }
    }

    fn rule_python(&mut self, inv: &Inv) {
        let a = inv.args;
        if a.len() == 1 && matches!(a[0].low.as_str(), "--version" | "-v" | "-vv") || a.iter().any(|x| x.text == "-V") && a.len() == 1 {
            return;
        }
        if a.len() >= 2 && a[0].low == "-m" && matches!(a[1].low.as_str(), "pip" | "pip3") {
            let sub = Inv { name: "pip", args: &a[2..], piped: inv.piped };
            return self.rule_pip(&sub);
        }
        self.acc.raise(T3, format!("{}: выполнение произвольного кода", inv.name));
    }

    fn rule_node(&mut self, inv: &Inv) {
        let a = inv.args;
        if a.len() == 1 && matches!(a[0].low.as_str(), "--version" | "-v" | "--help" | "-h") {
            return;
        }
        self.acc.raise(T3, format!("{}: выполнение произвольного кода", inv.name));
    }

    fn rule_import_module(&mut self, inv: &Inv) {
        let p = ps_parse(inv.args, &[("name", Role::Path), ("force", Role::Switch), ("global", Role::Switch), ("prefix", Role::Value), ("scope", Role::Value)], &[("name", Role::Path)]);
        for n in p.role(Role::Path) {
            let l = n.low.as_str();
            if n.group || l.contains('$') || l.contains('\\') || l.contains('/') || [".psm1", ".psd1", ".dll", ".ps1"].iter().any(|e| l.ends_with(e)) {
                self.acc.raise(T3, "загрузка модуля из файла");
            } else if !KNOWN_MODULES.contains(&l) && !l.starts_with("microsoft.powershell.") {
                self.acc.raise(T3, format!("загрузка неизвестного модуля {}", n.text));
            }
        }
    }

    // ───────────────────────────── правила: сеть ─────────────────────────────

    fn rule_web(&mut self, inv: &Inv) {
        let a = inv.args;
        let mut write = false;
        let mut send = false;
        for (i, x) in a.iter().enumerate() {
            if !x.is_flag() {
                continue;
            }
            let raw = x.text.as_str();
            let f = x.low.trim_start_matches('-');
            let (f, inline) = match f.split_once([':', '=']) {
                Some((f, v)) => (f, Some(v)),
                None => (f, None),
            };
            let is_out = raw == "-o" || raw == "-O" || f == "output" || f == "remote-name" || f == "remote-name-all" || (f.len() >= 2 && "outfile".starts_with(f));
            if is_out {
                write = true;
                if raw != "-O" && !f.starts_with("remote") {
                    match inline.map(Arg::plain).or_else(|| a.get(i + 1).cloned()) {
                        Some(d) => target(self.acc, self.ctx, self.shell, &d, Op::Write, "загрузка в файл"),
                        None => self.acc.raise(T3, "загрузка в файл без пути"),
                    }
                } else {
                    target(self.acc, self.ctx, self.shell, &Arg::plain("."), Op::Write, "загрузка в текущую папку");
                }
            }
            if matches!(f, "d" | "data" | "data-binary" | "data-raw" | "data-urlencode" | "f" | "form" | "t" | "upload-file" | "body" | "infile" | "json")
                || raw == "-F"
                || raw == "-T"
            {
                send = true;
            }
            if f == "x" || f == "request" || f == "method" || (f.len() >= 2 && "method".starts_with(f)) {
                let v = inline.map(str::to_string).or_else(|| a.get(i + 1).map(|v| v.low.clone())).unwrap_or_default();
                if !matches!(v.as_str(), "get" | "head" | "options") {
                    send = true;
                }
            }
        }
        if send {
            self.acc.raise(T2, "отправка данных по сети");
        }
        if !write {
            self.acc.raise(T1, "сетевой запрос");
        }
    }

    fn rule_ipconfig(&mut self, inv: &Inv) {
        for f in flag_names(inv.args) {
            match f.as_str() {
                "all" | "displaydns" | "?" | "showclassid" | "showclassid6" | "allcompartments" => {}
                "flushdns" | "registerdns" | "renew" | "renew6" => self.acc.raise(T2, format!("ipconfig /{f}")),
                _ => self.acc.raise(T3, format!("ipconfig /{f}: отключение/изменение сети")),
            }
        }
    }

    fn rule_netsh(&mut self, inv: &Inv) {
        let w = words(inv.args);
        let has = |s: &str| w.iter().any(|x| x == s);
        if w.iter().any(|x| x == "key" || x.starts_with("key=")) && has("clear") {
            self.acc.raise(T3, "показ паролей Wi-Fi");
            return;
        }
        let modify = ["set", "add", "delete", "reset", "import", "install", "uninstall", "flush", "exec"];
        let read = has("show") || has("dump");
        if w.first().is_some_and(|x| x == "wlan") && (has("connect") || has("disconnect")) && !w.iter().any(|x| modify.contains(&x.as_str())) {
            self.acc.raise(T1, "подключение к Wi-Fi");
            return;
        }
        if read && !w.iter().any(|x| modify.contains(&x.as_str())) {
            return;
        }
        if has("advfirewall") || has("firewall") {
            self.acc.raise(T3, "netsh: изменение брандмауэра");
        } else {
            self.acc.raise(T3, "netsh: изменение сетевых настроек");
        }
    }

    fn rule_net(&mut self, inv: &Inv) {
        let w = words(inv.args);
        let Some(sub) = w.first() else { return };
        let extra = w.len() > 1;
        match sub.as_str() {
            "user" | "localgroup" | "group" | "accounts" if !extra => {}
            "user" | "localgroup" | "group" | "accounts" => self.acc.raise(T3, format!("net {sub}: учётные записи")),
            "start" if !extra => {}
            "start" | "stop" | "pause" | "continue" => self.acc.raise(T3, format!("net {sub}: управление службами")),
            "use" if !extra => {}
            "use" => self.acc.raise(T2, "net use: подключение сетевого диска"),
            "share" if !extra => {}
            "share" => self.acc.raise(T3, "net share: общий доступ"),
            "session" | "view" | "config" | "statistics" | "helpmsg" | "help" | "print" | "name" => {
                if w.iter().any(|x| x.starts_with("/delete") || x.starts_with("/add")) {
                    self.acc.raise(T3, format!("net {sub}"));
                }
            }
            "file" => {
                if w.iter().any(|x| x.starts_with("/close")) {
                    self.acc.raise(T3, "net file /close");
                }
            }
            "time" => {
                if w.iter().any(|x| x.starts_with("/set")) {
                    self.acc.raise(T3, "net time /set");
                }
            }
            _ => self.acc.raise(T3, format!("net {sub}")),
        }
    }

    // ───────────────────────────── правила: система ─────────────────────────────

    fn rule_sc(&mut self, inv: &Inv) {
        let w = words(inv.args);
        let mut it = w.iter().peekable();
        if it.peek().is_some_and(|x| x.starts_with("\\\\")) {
            self.acc.raise(T3, "sc: удалённый компьютер");
            it.next();
        }
        match it.next().map(String::as_str) {
            None => {}
            Some(
                "query" | "queryex" | "qc" | "qdescription" | "qfailure" | "qfailureflag" | "qsidtype" | "qprivs"
                | "qtriggerinfo" | "qpreferrednode" | "qmanagedaccount" | "qprotection" | "quserservice"
                | "enumdepend" | "getdisplayname" | "getkeyname" | "sdshow" | "showsid" | "interrogate",
            ) => {}
            Some(other) => self.acc.raise(T3, format!("sc {other}: управление службами")),
        }
    }

    fn rule_reg(&mut self, inv: &Inv) {
        let a = inv.args;
        let w = words(a);
        let Some(sub) = w.first().cloned() else { return };
        let key = a.get(1);
        if key.is_some_and(|k| k.low.starts_with("\\\\")) {
            self.acc.raise(T3, "reg: удалённый компьютер");
        }
        let fl = flag_names(a);
        match sub.as_str() {
            "query" | "compare" | "/?" => {}
            "add" => match key {
                Some(k) => target(self.acc, self.ctx, self.shell, k, Op::Write, "reg add"),
                None => self.acc.raise(T3, "reg add без ключа"),
            },
            "delete" => match key {
                Some(k) => {
                    let value_only = fl.iter().any(|f| f == "v") && !fl.iter().any(|f| f == "va");
                    target(self.acc, self.ctx, self.shell, k, if value_only { Op::DeleteValue } else { Op::Delete { recursive: true } }, "reg delete");
                }
                None => self.acc.raise(T3, "reg delete без ключа"),
            },
            "export" | "save" => {
                if sub == "save"
                    && key.is_some_and(|k| {
                        let l = k.low.replace('/', "\\");
                        ["\\sam", "\\security", "\\system"].iter().any(|h| l.ends_with(h))
                    })
                {
                    self.acc.raise(T3, "reg save: выгрузка кустов SAM/SECURITY/SYSTEM (учётные данные)");
                }
                match a.get(2) {
                    Some(f) => target(self.acc, self.ctx, self.shell, f, Op::Write, "сохранение реестра в файл"),
                    None => self.acc.raise(T3, "reg export без файла"),
                }
            }
            "copy" => match a.get(2) {
                Some(d) => target(self.acc, self.ctx, self.shell, d, Op::Write, "reg copy"),
                None => self.acc.raise(T3, "reg copy"),
            },
            _ => self.acc.raise(T3, format!("reg {sub}: импорт/загрузка кустов реестра")),
        }
    }

    fn rule_schtasks(&mut self, inv: &Inv) {
        let a = inv.args;
        let fl = flag_names(a);
        let has = |f: &str| fl.iter().any(|x| x == f);
        if has("s") {
            self.acc.raise(T3, "schtasks: удалённый компьютер");
        }
        if fl.is_empty() || has("query") {
            return;
        }
        if has("create") {
            // команда задачи выполнится позже — классифицируем её сейчас
            for tr in values_after(a, &["/tr"]) {
                let t = tr.text.clone();
                self.sub_script(Shell::Cmd, &t);
            }
            let user = values_after(a, &["/ru", "/u"]);
            let system = user.iter().any(|u| u.low.contains("system") || u.low.contains("nt authority") || u.low.contains("localservice") || u.low.contains("networkservice"));
            let highest = values_after(a, &["/rl"]).iter().any(|v| v.low == "highest");
            if system || highest || has("xml") {
                self.acc.raise(T3, "schtasks /create от SYSTEM или с наивысшими правами");
            } else {
                self.acc.raise(T2, "создание задачи планировщика пользователя");
            }
            return;
        }
        if has("run") || has("end") {
            self.acc.raise(T2, "запуск/остановка задачи планировщика");
            return;
        }
        self.acc.raise(T3, "schtasks: удаление/изменение задач");
    }

    fn rule_register_task(&mut self, inv: &Inv) {
        let fl = flag_names(inv.args);
        let p = ps_parse(inv.args, &[("taskname", Role::Value), ("user", Role::Value), ("runlevel", Role::Value), ("principal", Role::Value), ("xml", Role::Value), ("inputobject", Role::Value), ("action", Role::Value), ("trigger", Role::Value), ("settings", Role::Value), ("taskpath", Role::Value), ("password", Role::Value), ("force", Role::Switch)], &[("taskname", Role::Value)]);
        let system = p.val("user").is_some_and(|u| u.low.contains("system") || u.low.contains("nt authority") || u.low.contains('$'));
        let highest = p.val("runlevel").is_some_and(|v| v.low != "limited");
        if system || highest || p.has("principal") || p.has("xml") || p.has("inputobject") || fl.iter().any(|f| f == "cimsession") {
            self.acc.raise(T3, "задача планировщика от SYSTEM/с повышенными правами");
        } else {
            self.acc.raise(T2, "создание задачи планировщика пользователя");
        }
    }

    fn rule_powercfg(&mut self, inv: &Inv) {
        // значим только первый флаг — это подкоманда; остальные (`-monitor-timeout-ac`) — её параметры
        if let Some(f) = flag_names(inv.args).into_iter().next() {
            match f.as_str() {
                "l" | "list" | "getactivescheme" | "q" | "query" | "qh" | "a" | "availablesleepstates" | "requests"
                | "lastwake" | "devicequery" | "waketimers" | "batteryreport" | "energy" | "sleepstudy" | "srumutil"
                | "systempowerreport" | "spr" | "systemsleepdiagnostics" | "?" | "aliases" | "aliasesh" | "output"
                | "duration" | "xml" => {}
                "s" | "setactive" | "change" | "x" | "setacvalueindex" | "setdcvalueindex" | "duplicatescheme"
                | "changename" | "import" | "attributes" => self.acc.raise(T2, format!("powercfg /{f}: схема питания")),
                _ => self.acc.raise(T3, format!("powercfg /{f}: системные настройки питания")),
            }
        }
    }

    fn rule_sys_tool(&mut self, inv: &Inv) {
        let n = inv.name;
        let a = inv.args;
        let w = words(a);
        let fl = flag_names(a);
        let first = w.first().map(String::as_str).unwrap_or("");
        let safe = match n {
            "vssadmin" => first == "list",
            "wbadmin" => first == "get",
            "reagentc" => fl.iter().all(|f| f == "info"),
            "mountvol" => a.is_empty() || fl.iter().all(|f| f == "l" || f == "?"),
            "fltmc" => a.is_empty() || matches!(first, "filters" | "instances" | "volumes"),
            "auditpol" => fl.iter().all(|f| f == "get" || f == "list" || f == "category" || f == "subcategory" || f == "user" || f == "r"),
            "dsregcmd" => fl.iter().all(|f| f == "status" || f == "status_old" || f == "debug"),
            "cmdkey" => fl.iter().all(|f| f == "list"),
            "logman" => first == "query" || a.is_empty(),
            "openfiles" => fl.iter().all(|f| f == "query" || f == "fo" || f == "v" || f == "nh"),
            "klist" => !w.iter().any(|x| x == "purge"),
            "wevtutil" => matches!(first, "qe" | "query-events" | "el" | "enum-logs" | "gl" | "get-log" | "gli" | "get-log-info" | "ep" | "enum-publishers" | "gp" | "get-publisher"),
            "fsutil" => first == "fsinfo" || w.iter().any(|x| x == "query" || x == "queryflags" || x == "info"),
            "sfc" => fl.iter().all(|f| f == "verifyonly" || f == "verifyfile"),
            "dism" => fl.iter().all(|f| {
                f.starts_with("get-") || matches!(f.as_str(), "online" | "english" | "format" | "cleanup-image" | "checkhealth" | "scanhealth" | "image" | "?")
            }),
            "chkdsk" => !fl.iter().any(|f| matches!(f.as_str(), "f" | "r" | "x" | "b" | "spotfix" | "offlinescanandfix" | "forceofflinefix" | "perf" | "l")),
            "defrag" => fl.iter().any(|f| f == "a" || f == "analyze") && !fl.iter().any(|f| matches!(f.as_str(), "o" | "x" | "l" | "k" | "g" | "b" | "d" | "t")),
            "cipher" => fl.is_empty() || fl.iter().all(|f| f == "s" || f == "h" || f == "c" || f == "?"),
            "gpresult" => {
                for p in values_after(a, &["/h", "/x"]) {
                    target(self.acc, self.ctx, self.shell, p, Op::Write, "отчёт gpresult");
                }
                true
            }
            "nvidia-smi" => !a.iter().any(|x| ["-pl", "-r", "-ac", "-rac", "-e", "-c", "-pm", "-am", "-lgc", "-rgc", "-lmc", "--power-limit", "--gpu-reset"].contains(&x.low.as_str())),
            _ => false,
        };
        if !safe {
            self.acc.raise(T3, format!("{n}: системный инструмент в режиме изменения"));
        }
    }

    fn rule_winget(&mut self, inv: &Inv) {
        let a = inv.args;
        let sub = first_word(a);
        match sub.as_deref() {
            None => {
                if !a.iter().all(|x| matches!(x.low.as_str(), "--version" | "-v" | "--info" | "--help" | "-?")) {
                    self.acc.raise(T3, "winget");
                }
            }
            Some("search" | "find" | "list" | "ls" | "show" | "view" | "features" | "hash" | "validate" | "--version" | "--info" | "help") => {}
            Some("source") => {
                if !matches!(a.get(1).map(|x| x.low.as_str()), Some("list" | "ls" | "export")) {
                    self.acc.raise(T3, "winget source: изменение источников");
                }
            }
            Some("upgrade" | "update") if a.iter().filter(|x| !x.is_flag()).count() == 1 && !a.iter().any(|x| matches!(x.low.as_str(), "--all" | "-r" | "--recurse" | "--id" | "--name" | "-q" | "--query")) => {}
            Some("download") => self.acc.raise(T2, "winget download: загрузка установщика"),
            Some(s) => self.acc.raise(T3, format!("winget {s}: установка/удаление программ")),
        }
    }

    fn rule_choco(&mut self, inv: &Inv) {
        let sub = first_word(inv.args);
        match sub.as_deref() {
            None | Some("search" | "find" | "list" | "info" | "outdated" | "help" | "which" | "status" | "checkup" | "cat" | "prefix" | "home" | "--version" | "-v") => {}
            Some("config" | "feature" | "source" | "pin" | "bucket") if inv.args.get(1).is_none_or(|x| x.low == "list") => {}
            Some(s) => self.acc.raise(T3, format!("{} {s}: установка/удаление программ", inv.name)),
        }
    }

    // ───────────────────────────── правила: разработка ─────────────────────────────

    fn rule_git(&mut self, inv: &Inv) {
        let a = inv.args;
        // глобальные опции до подкоманды
        let mut i = 0;
        while i < a.len() && a[i].is_flag() {
            if matches!(a[i].low.as_str(), "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path") || a[i].text == "-C" {
                i += 1;
            }
            i += 1;
        }
        let Some(sub) = a.get(i).map(|x| x.low.clone()) else { return };
        let rest = &a[i + 1..];
        let rw = words(rest);
        let has = |s: &str| rw.iter().any(|x| x == s);
        let pos_count = rest.iter().filter(|x| !x.is_flag()).count();
        match sub.as_str() {
            "status" | "log" | "diff" | "show" | "rev-parse" | "describe" | "blame" | "ls-files" | "ls-tree" | "ls-remote"
            | "cat-file" | "shortlog" | "grep" | "help" | "version" | "whatchanged" | "count-objects" | "fsck" | "name-rev"
            | "merge-base" | "rev-list" | "show-ref" | "for-each-ref" | "var" | "check-ignore" | "check-attr" | "cherry"
            | "range-diff" | "annotate" | "show-branch" | "verify-commit" | "verify-tag" | "get-tar-commit-id" => {}
            "reflog" => {
                if has("expire") || has("delete") {
                    self.acc.raise(T3, "git reflog expire/delete: потеря истории");
                }
            }
            "branch" => {
                let modify = rw.iter().any(|x| matches!(x.as_str(), "-d" | "-D" | "--delete" | "-m" | "-M" | "--move" | "-c" | "-C" | "--copy" | "-f" | "--force" | "-u" | "--set-upstream-to" | "--unset-upstream" | "--edit-description"))
                    || rest.iter().any(|x| x.text == "-D" || x.text == "-M" || x.text == "-C");
                let list = rw.iter().any(|x| matches!(x.as_str(), "-l" | "--list" | "-a" | "--all" | "-r" | "--remotes" | "--contains" | "--merged" | "--no-merged" | "--points-at" | "--show-current" | "-v" | "-vv"));
                if modify || (pos_count > 0 && !list) {
                    self.acc.raise(T2, "git branch: изменение веток");
                }
            }
            "tag" => {
                if pos_count > 0 && !rw.iter().any(|x| matches!(x.as_str(), "-l" | "--list" | "-n" | "--contains" | "--points-at")) || has("-d") {
                    self.acc.raise(T2, "git tag");
                }
            }
            "remote" => {
                if !(rest.is_empty() || matches!(rw[0].as_str(), "-v" | "show" | "get-url")) {
                    self.acc.raise(T2, "git remote: изменение удалённых репозиториев");
                }
            }
            "stash" => match rw.first().map(String::as_str) {
                Some("list" | "show") => {}
                Some("drop" | "clear") => self.acc.raise(T3, "git stash drop/clear: удаление сохранённых изменений"),
                _ => self.acc.raise(T2, "git stash"),
            },
            "config" => {
                if has("--system") {
                    self.acc.raise(T3, "git config --system");
                } else if !rw.iter().any(|x| matches!(x.as_str(), "--get" | "--get-all" | "--get-regexp" | "--list" | "-l" | "get" | "list" | "--show-origin" | "--show-scope" | "--get-urlmatch")) && pos_count != 1 {
                    self.acc.raise(T2, "git config: изменение настроек");
                }
            }
            "worktree" | "notes" | "lfs" | "bisect" | "submodule" => {
                let s = rw.first().map(String::as_str).unwrap_or("");
                if sub == "submodule" && s == "foreach" {
                    self.acc.raise(T3, "git submodule foreach: выполнение команд");
                } else if !matches!(s, "list" | "show" | "status" | "ls-files" | "env" | "version" | "log" | "visualize" | "view" | "summary" | "") {
                    self.acc.raise(T2, format!("git {sub}"));
                }
            }
            "fetch" => self.acc.raise(T1, "git fetch"),
            "checkout" => {
                if has("--") || has(".") || has("-f") || has("--force") || has("-p") || has("--patch") || has("--ours") || has("--theirs") {
                    self.acc.raise(T3, "git checkout: отмена несохранённых изменений");
                } else {
                    self.acc.raise(T2, "git checkout: переключение ветки");
                }
            }
            "restore" => {
                if (has("--staged") || has("-s") || has("-S")) && !has("--worktree") && !has("-w") {
                    self.acc.raise(T2, "git restore --staged");
                } else {
                    self.acc.raise(T3, "git restore: отмена несохранённых изменений");
                }
            }
            "reset" => {
                if has("--hard") || has("--merge") || has("--keep") {
                    self.acc.raise(T3, "git reset --hard: потеря несохранённых изменений");
                } else {
                    self.acc.raise(T2, "git reset");
                }
            }
            "clean" => {
                if has("-n") || has("--dry-run") {
                    return;
                }
                self.acc.raise(T3, "git clean: удаление неотслеживаемых файлов");
            }
            "push" => {
                let force = rw.iter().any(|x| x == "-f" || x.starts_with("--force") || x == "--delete" || x == "-d" || x == "--mirror" || x == "--prune")
                    || rest.iter().any(|x| !x.is_flag() && (x.text.starts_with('+') || x.text.starts_with(':')));
                if force {
                    self.acc.raise(T3, "git push --force/--delete: перезапись истории на сервере");
                } else {
                    self.acc.raise(T2, "git push");
                }
            }
            "filter-branch" | "filter-repo" | "update-ref" | "replace" | "daemon" | "instaweb" | "credential" | "credential-manager" => {
                self.acc.raise(T3, format!("git {sub}"))
            }
            "add" | "commit" | "switch" | "merge" | "rebase" | "pull" | "init" | "clone" | "mv" | "rm" | "cherry-pick"
            | "revert" | "am" | "apply" | "gc" | "prune" | "repack" | "pack-refs" | "maintenance" | "sparse-checkout"
            | "format-patch" | "archive" | "bundle" | "mergetool" | "difftool" | "stage" | "update-index"
            | "commit-tree" | "write-tree" | "read-tree" | "hash-object" | "symbolic-ref" | "citool" | "gui" | "restore-staged" => {
                self.acc.raise(T2, format!("git {sub}"))
            }
            _ => self.acc.raise(T3, format!("неизвестная подкоманда git {sub}")),
        }
    }

    fn rule_gh(&mut self, inv: &Inv) {
        let w = words(inv.args);
        let (a, b) = (w.first().map(String::as_str).unwrap_or(""), w.get(1).map(String::as_str).unwrap_or(""));
        match (a, b) {
            ("" | "--version" | "help" | "status", _) => {}
            ("auth", "status") => {}
            (_, "list" | "view" | "status" | "diff" | "checks") => {}
            ("api" | "extension" | "secret" | "variable" | "ssh-key" | "gpg-key" | "auth", _) => self.acc.raise(T3, format!("gh {a}")),
            (_, "delete" | "archive" | "transfer" | "rename" | "edit" | "close" | "merge" | "lock") => self.acc.raise(T3, format!("gh {a} {b}")),
            ("pr" | "issue" | "repo" | "release" | "gist" | "label" | "run" | "workflow" | "browse" | "codespace", _) => self.acc.raise(T2, format!("gh {a} {b}")),
            _ => self.acc.raise(T3, format!("gh {a}")),
        }
    }

    fn rule_npm(&mut self, inv: &Inv) {
        let a = inv.args;
        let w = words(a);
        let global = w.iter().any(|x| x == "-g" || x == "--global" || x == "--location=global" || x == "--location");
        let sub = first_word(a);
        let Some(sub) = sub else { return };
        let read = ["-v", "--version", "help", "ls", "list", "ll", "la", "view", "info", "show", "v", "outdated", "search", "s", "find", "root", "prefix", "bin", "doctor", "explain", "why", "fund", "whoami", "ping", "query", "repo", "docs", "home", "bugs", "config", "get", "audit", "cache", "licenses"];
        let modify = ["install", "i", "in", "ins", "inst", "insta", "instal", "isntall", "add", "ci", "uninstall", "un", "unlink", "remove", "rm", "r", "update", "up", "upgrade", "dedupe", "prune", "rebuild", "link", "ln", "init", "create", "run", "run-script", "rum", "urn", "test", "t", "tst", "start", "stop", "restart", "pack", "set-script", "build", "dev", "why", "import", "patch"];
        if matches!(sub.as_str(), "publish" | "unpublish" | "deprecate" | "owner" | "access" | "team" | "token" | "adduser" | "login" | "logout" | "star" | "unstar" | "dist-tag" | "hook" | "org" | "profile") {
            self.acc.raise(T3, format!("{} {sub}: действия с учётной записью/реестром пакетов", inv.name));
            return;
        }
        if matches!(sub.as_str(), "exec" | "x" | "dlx" | "create") {
            self.acc.raise(T3, format!("{} {sub}: загрузка и запуск пакета", inv.name));
            return;
        }
        if sub == "config" && w.iter().any(|x| x == "set" || x == "delete" || x == "edit") {
            self.acc.raise(T2, "npm config set");
            return;
        }
        if sub == "audit" && w.iter().any(|x| x == "fix") || sub == "cache" && w.iter().any(|x| x == "clean") {
            self.acc.raise(T2, format!("npm {sub}"));
            return;
        }
        if sub == "version" {
            if a.len() > 1 {
                self.acc.raise(T2, "npm version");
            }
            return;
        }
        if read.contains(&sub.as_str()) && !modify.contains(&sub.as_str()) {
            return;
        }
        if modify.contains(&sub.as_str()) || inv.name != "npm" {
            if global && !matches!(sub.as_str(), "ls" | "list" | "outdated" | "root" | "prefix") {
                self.acc.raise(T3, format!("{} {sub} -g: глобальная установка/удаление", inv.name));
            } else {
                self.acc.raise(T2, format!("{} {sub}: изменение проекта", inv.name));
            }
            return;
        }
        self.acc.raise(T3, format!("неизвестная подкоманда {} {sub}", inv.name));
    }

    fn rule_cargo(&mut self, inv: &Inv) {
        let a: Vec<Arg> = inv.args.iter().filter(|x| !x.text.starts_with('+')).cloned().collect();
        let Some(sub) = first_word(&a) else { return };
        match sub.as_str() {
            "--version" | "-v" | "version" | "help" | "metadata" | "tree" | "search" | "locate-project" | "pkgid"
            | "verify-project" | "read-manifest" | "info" | "--list" | "report" | "-vv" => {}
            "check" | "build" | "b" | "c" | "test" | "t" | "bench" | "run" | "r" | "clippy" | "fmt" | "doc" | "d" | "fix"
            | "clean" | "update" | "add" | "remove" | "rm" | "new" | "init" | "vendor" | "generate-lockfile" | "fetch"
            | "package" | "install" | "uninstall" | "nextest" | "watch" | "expand" | "audit" | "deny" | "outdated"
            | "upgrade" | "tauri" | "make" | "llvm-cov" | "udeps" | "machete" | "bloat" | "xtask" | "miri" | "rustc"
            | "rustdoc" => self.acc.raise(T2, format!("cargo {sub}: сборка/изменение проекта")),
            "publish" | "yank" | "owner" | "login" | "logout" => self.acc.raise(T3, format!("cargo {sub}: действия на crates.io")),
            _ => self.acc.raise(T3, format!("неизвестная подкоманда cargo {sub}")),
        }
    }

    fn rule_simple_sub(&mut self, inv: &Inv, read: &[&str], t2: &[&str], what: &str) {
        let Some(sub) = first_word(inv.args).or_else(|| inv.args.first().map(|x| x.text.clone())) else { return };
        if read.iter().any(|r| r.eq_ignore_ascii_case(&sub)) || inv.args.iter().any(|x| x.low == "list") {
            return;
        }
        if t2.contains(&sub.as_str()) {
            self.acc.raise(T2, format!("{what} {sub}"));
        } else {
            self.acc.raise(T3, format!("{what} {sub}"));
        }
    }

    fn rule_dev_tool(&mut self, inv: &Inv) {
        let a = inv.args;
        let w = words(a);
        let n = inv.name;
        if w.iter().all(|x| matches!(x.as_str(), "--version" | "-v" | "-version" | "--info" | "--list-sdks" | "--list-runtimes" | "version" | "env" | "help" | "--help" | "-h" | "-vv" | "--print" | "--explain")) && !w.is_empty() || (n == "rustc" && a.iter().any(|x| x.text == "-V")) {
            return;
        }
        if n == "java" && w.iter().any(|x| x == "-jar") {
            self.acc.raise(T3, "java -jar: запуск программы");
            return;
        }
        if n == "dotnet" && w.iter().any(|x| x == "nuget") && w.iter().any(|x| x == "push" || x == "delete") {
            self.acc.raise(T3, "dotnet nuget push");
            return;
        }
        if n == "go" && matches!(w.first().map(String::as_str), Some("list" | "doc" | "vet" | "env" | "version")) {
            return;
        }
        if n == "java" {
            self.acc.raise(T3, "java: запуск программы");
            return;
        }
        self.acc.raise(T2, format!("{n}: сборка/запуск проекта"));
    }

    fn rule_pip(&mut self, inv: &Inv) {
        let sub = first_word(inv.args);
        match sub.as_deref() {
            None | Some("list" | "show" | "freeze" | "check" | "--version" | "-v" | "-V" | "search" | "help" | "config" | "debug" | "inspect" | "index" | "hash") => {}
            Some("install" | "uninstall" | "download" | "wheel" | "cache") => self.acc.raise(T2, format!("pip {}", sub.unwrap_or_default())),
            Some(s) => self.acc.raise(T3, format!("pip {s}")),
        }
    }

    fn rule_docker(&mut self, inv: &Inv) {
        let w = words(inv.args);
        let a = w.first().map(String::as_str).unwrap_or("");
        let b = w.get(1).map(String::as_str).unwrap_or("");
        let read = matches!(a, "" | "version" | "info" | "ps" | "images" | "inspect" | "logs" | "stats" | "top" | "port" | "diff" | "history" | "search" | "--version" | "-v" | "help")
            || matches!(b, "ls" | "list" | "ps" | "logs" | "config" | "inspect" | "df" | "version" | "images" | "top");
        if read && !matches!(b, "rm" | "prune") {
            return;
        }
        if matches!(a, "rm" | "rmi" | "prune") || matches!(b, "rm" | "prune" | "remove") || (a == "compose" && b == "down" && w.iter().any(|x| x == "-v" || x == "--volumes")) {
            self.acc.raise(T3, format!("{} {a} {b}: удаление контейнеров/образов/томов", inv.name));
        } else {
            self.acc.raise(T2, format!("{} {a}", inv.name));
        }
    }
}

struct Inv<'a> {
    name: &'a str,
    args: &'a [Arg],
    piped: bool,
}

fn words(args: &[Arg]) -> Vec<String> {
    args.iter().filter(|a| !a.group).map(|a| a.low.clone()).collect()
}

fn first_word(args: &[Arg]) -> Option<String> {
    args.iter().find(|a| !a.is_flag() && !a.group).map(|a| a.low.clone())
}

/// Значения после флагов (`/im x`, `/f path`).
fn values_after<'a>(args: &'a [Arg], flags: &[&str]) -> Vec<&'a Arg> {
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if flags.contains(&a.low.as_str())
            && let Some(v) = args.get(i + 1)
        {
            out.push(v);
        }
    }
    out
}

/// Склейка аргументов как у powershell.exe/cmd.exe: кавычки уже сняты разбором командной строки,
/// аргументы соединяются пробелами и разбираются заново. Это консервативно: путь с пробелом распадается
/// на части, каждая из которых проверяется отдельно.
fn join_args(args: &[Arg]) -> String {
    args.iter().map(|a| a.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// Имя программы с пробелами — в кавычки, чтобы оно осталось одним словом для cmd-лексера.
fn quote_prog(s: &str) -> String {
    if s.contains(' ') && !s.contains('"') { format!("\"{s}\"") } else { s.to_string() }
}

/// Аргумент — исполняемый файл (для explorer/control).
fn exec_like(low: &str) -> bool {
    [".exe", ".com", ".bat", ".cmd", ".ps1", ".vbs", ".js", ".msi", ".reg", ".scr", ".hta", ".wsf", ".jar"]
        .iter()
        .any(|e| low.ends_with(e))
}

fn join_arg(base: &Arg, name: &Arg) -> Arg {
    let text = format!("{}\\{}", base.text.trim_end_matches(['\\', '/']), name.text);
    Arg { low: text.to_lowercase(), text, expandable: base.expandable || name.expandable, quoted: base.quoted || name.quoted, group: base.group || name.group, comma: false }
}

fn script_ext(base: &str) -> Option<&'static str> {
    const EXTS: &[&str] = &[
        "ps1", "psm1", "psd1", "bat", "cmd", "vbs", "vbe", "js", "jse", "wsf", "wsh", "hta", "msi", "msp", "reg", "scr",
        "lnk", "cpl", "jar", "py", "pyw", "sh", "inf", "appx", "msix", "appxbundle", "msixbundle", "application", "gadget",
    ];
    let (_, ext) = base.rsplit_once('.')?;
    EXTS.iter().find(|e| **e == ext).copied()
}

const SHELLS: &[&str] = &[
    "powershell", "pwsh", "cmd", "bash", "sh", "wsl", "python", "python3", "py", "node", "invoke-expression", "iex",
    "wscript", "cscript", "mshta", "ruby", "perl", "php", "deno", "bun",
];

/// Критичные/системные процессы: их завершение — T3.
pub(crate) const CRITICAL_PROCESSES: &[&str] = &[
    "system", "idle", "registry", "smss", "csrss", "wininit", "winlogon", "services", "lsass", "lsaiso", "svchost",
    "dwm", "fontdrvhost", "explorer", "msmpeng", "nissrv", "securityhealthservice", "sgrmbroker", "spoolsv",
    "audiodg", "conhost", "ctfmon", "sihost", "taskhostw", "runtimebroker", "searchindexer", "wudfhost", "dllhost",
    "memory compression", "secure system", "mpdefendercoreservice", "lsm", "userinit", "logonui", "csrss.exe",
    "trustedinstaller", "tiworker", "wmiprvse", "startmenuexperiencehost", "shellexperiencehost", "textinputhost",
];

const DOC_EXTS: &[&str] = &[
    "txt", "md", "log", "csv", "json", "xml", "yaml", "yml", "toml", "ini", "pdf", "doc", "docx", "xls", "xlsx", "ppt",
    "pptx", "odt", "ods", "rtf", "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico", "mp3", "wav", "flac", "ogg",
    "m4a", "mp4", "mkv", "avi", "mov", "webm", "html", "htm", "zip", "7z", "rar", "epub", "rs", "ts", "tsx", "css",
];

const GUI_APPS: &[&str] = &[
    "notepad", "calc", "mspaint", "write", "wordpad", "explorer", "chrome", "msedge", "firefox", "opera", "brave",
    "code", "cursor", "wt", "taskmgr", "resmon", "perfmon", "control", "msinfo32", "dxdiag", "snippingtool", "magnify",
    "osk", "charmap", "winver", "mmc", "sndvol", "mstsc", "notepad++", "sublime_text", "devenv", "idea64", "pycharm64",
    "spotify", "telegram", "discord", "steam", "outlook", "winword", "excel", "powerpnt", "onenote", "teams", "ms-teams",
    "slack", "zoom", "vlc", "obs64", "powershell_ise", "msconfig", "rstrui", "eventvwr", "compmgmt", "devmgmt",
    "services", "taskschd", "diskmgmt", "gpedit", "lusrmgr", "certmgr", "secpol", "wf", "mrt", "sigverif", "colorcpl",
    "cttune", "dpiscaling", "displayswitch", "fsquirt", "msra", "psr", "recdisc", "stikynot", "systempropertiesadvanced",
    "wscui", "wmplayer", "iexplore", "photos", "paint",
];

/// Только чтение/безопасное — T0 при любых аргументах (перенаправления и вложенный код проверяются отдельно).
const T0_NAMES: &[&str] = &[
    // PowerShell
    "get-content", "get-childitem", "get-item", "get-itemproperty", "get-itempropertyvalue", "get-location",
    "set-location", "push-location", "pop-location", "write-output", "write-host", "write-verbose", "write-debug",
    "write-warning", "write-error", "write-information", "write-progress", "select-object", "select-string",
    "select-xml", "sort-object", "where-object", "foreach-object", "group-object", "measure-object", "measure-command",
    "compare-object", "format-table", "format-list", "format-wide", "format-custom", "format-hex", "out-string",
    "out-null", "out-host", "out-default", "convertto-json", "convertfrom-json", "convertto-csv", "convertfrom-csv",
    "convertto-html", "convertto-xml", "convertfrom-stringdata", "convertfrom-markdown", "convertto-securestring",
    "convertfrom-securestring", "convert-path", "resolve-path", "split-path", "join-path", "test-path", "start-sleep",
    "new-object", "new-timespan", "new-guid", "get-random", "read-host", "clear-host", "set-variable", "new-variable",
    "remove-variable", "clear-variable", "get-variable", "set-strictmode", "import-csv", "import-clixml",
    "import-localizeddata", "add-member", "get-clipboard", "wait-process", "wait-job", "receive-job", "get-job",
    "remove-job", "stop-job", "suspend-job", "resume-job", "stop-transcript", "get-help", "get-command", "get-member",
    "get-alias", "get-module", "remove-module", "get-history", "clear-history", "get-process", "get-service",
    "get-psdrive", "remove-psdrive", "get-uniq", "get-unique", "get-date", "get-host", "get-culture", "get-uiculture",
    "get-computerinfo", "get-timezone", "get-wmiobject", "get-ciminstance", "get-cimclass", "get-filehash",
    "get-acl", "get-pssession", "exit-pssession", "update-typedata", "update-formatdata", "get-psbreakpoint",
    "get-pscallstack", "new-scheduledtaskaction", "new-scheduledtasktrigger", "new-scheduledtaskprincipal",
    "new-scheduledtasksettingsset", "export-scheduledtask", "export-modulemember", "trace-command", "get-error",
    "where", "foreach",
    // cmd / утилиты
    "dir", "type", "echo", "cls", "title", "color", "chcp", "ver", "vol", "tree", "more",
    "find", "findstr", "fc", "comp", "where", "whoami", "hostname", "systeminfo", "tasklist", "ping", "tracert",
    "pathping", "nslookup", "netstat", "getmac", "set", "path", "prompt", "exit", "rem", "pause", "goto", "shift",
    "setlocal", "endlocal", "timeout", "choice", "break", "verify", "driverquery", "query", "quser", "qwinsta",
    "qprocess", "help", "sort", "which", "pwd", "ls", "cat", "echo.", "uptime",
];

/// Встроенные модули PowerShell, загрузка которых безопасна.
const KNOWN_MODULES: &[&str] = &[
    "scheduledtasks", "nettcpip", "netadapter", "netsecurity", "dnsclient", "defender", "storage", "pnpdevice", "appx",
    "dism", "bitlocker", "bitstransfer", "cimcmdlets", "international", "smbshare", "printmanagement", "psreadline",
    "packagemanagement", "powershellget", "threadjob", "netconnection", "netqos", "networkconnectivitystatus",
    "wdac", "windowsupdate", "trustedplatformmodule", "configci", "deliveryoptimization", "pester", "psdiagnostics",
    "startlayout", "volume", "nettopology",
];

fn ps_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "%" | "foreach" => "foreach-object",
        "?" | "where" => "where-object",
        "ac" => "add-content",
        "cat" | "gc" | "type" => "get-content",
        "cd" | "chdir" | "sl" => "set-location",
        "clc" => "clear-content",
        "clear" | "cls" => "clear-host",
        "clhy" => "clear-history",
        "cli" => "clear-item",
        "clp" => "clear-itemproperty",
        "clv" => "clear-variable",
        "cnsn" => "connect-pssession",
        "compare" | "diff" => "compare-object",
        "copy" | "cp" | "cpi" => "copy-item",
        "cpp" => "copy-itemproperty",
        "curl" | "iwr" | "wget" => "invoke-webrequest",
        "cvpa" => "convert-path",
        "del" | "erase" | "rd" | "ri" | "rm" | "rmdir" => "remove-item",
        "dir" | "gci" | "ls" => "get-childitem",
        "echo" | "write" => "write-output",
        "epal" => "export-alias",
        "epcsv" => "export-csv",
        "etsn" => "enter-pssession",
        "exsn" => "exit-pssession",
        "fc" => "format-custom",
        "fhx" => "format-hex",
        "fl" => "format-list",
        "ft" => "format-table",
        "fw" => "format-wide",
        "gal" => "get-alias",
        "gcb" => "get-clipboard",
        "gcim" => "get-ciminstance",
        "gcm" => "get-command",
        "gdr" => "get-psdrive",
        "ghy" | "h" | "history" => "get-history",
        "gi" => "get-item",
        "gin" => "get-computerinfo",
        "gjb" => "get-job",
        "gl" | "pwd" => "get-location",
        "gm" => "get-member",
        "gmo" => "get-module",
        "gp" => "get-itemproperty",
        "gps" | "ps" => "get-process",
        "gpv" => "get-itempropertyvalue",
        "group" => "group-object",
        "gsn" => "get-pssession",
        "gsv" => "get-service",
        "gtz" => "get-timezone",
        "gu" => "get-unique",
        "gv" => "get-variable",
        "gwmi" => "get-wmiobject",
        "icm" => "invoke-command",
        "iex" => "invoke-expression",
        "ihy" | "r" => "invoke-history",
        "ii" => "invoke-item",
        "ipal" => "import-alias",
        "ipcsv" => "import-csv",
        "ipmo" => "import-module",
        "ipsn" => "import-pssession",
        "irm" => "invoke-restmethod",
        "ise" => "powershell_ise",
        "iwmi" => "invoke-wmimethod",
        "icim" => "invoke-cimmethod",
        "kill" | "spps" => "stop-process",
        "lp" => "out-printer",
        "man" | "help" => "get-help",
        "md" | "mkdir" => "mkdir",
        "measure" => "measure-object",
        "mi" | "move" | "mv" => "move-item",
        "mount" | "ndr" => "new-psdrive",
        "mp" => "move-itemproperty",
        "nal" => "new-alias",
        "ni" => "new-item",
        "nmo" => "new-module",
        "nsn" => "new-pssession",
        "nv" => "new-variable",
        "ogv" => "out-gridview",
        "oh" => "out-host",
        "popd" => "pop-location",
        "pushd" => "push-location",
        "rcjb" => "receive-job",
        "rcsn" => "receive-pssession",
        "rdr" => "remove-psdrive",
        "ren" | "rni" => "rename-item",
        "rjb" => "remove-job",
        "rmo" => "remove-module",
        "rnp" => "rename-itemproperty",
        "rp" => "remove-itemproperty",
        "rv" => "remove-variable",
        "rvpa" => "resolve-path",
        "rwmi" => "remove-wmiobject",
        "rcim" => "remove-ciminstance",
        "sajb" => "start-job",
        "sal" => "set-alias",
        "saps" | "start" => "start-process",
        "sasv" => "start-service",
        "sc" => "set-content",
        "scb" => "set-clipboard",
        "select" => "select-object",
        "set" | "sv" => "set-variable",
        "shcm" => "show-command",
        "si" => "set-item",
        "sleep" => "start-sleep",
        "sls" => "select-string",
        "sort" => "sort-object",
        "sp" => "set-itemproperty",
        "spjb" => "stop-job",
        "spsv" => "stop-service",
        "stz" => "set-timezone",
        "swmi" => "set-wmiinstance",
        "scim" => "set-ciminstance",
        "ncim" => "new-ciminstance",
        "tee" => "tee-object",
        "trcm" => "trace-command",
        "wjb" => "wait-job",
        _ => return None,
    })
}

/// Безопасные статические методы .NET: `[Type]::Method(…)`.
fn static_call_ok(receiver: &str, m: &str) -> bool {
    let t = receiver.trim_start_matches('[').trim_end_matches(']').to_lowercase();
    let t = t.strip_prefix("system.").unwrap_or(&t);
    match t {
        "math" | "datetime" | "timespan" | "string" | "char" | "int" | "int32" | "int64" | "long" | "double" | "decimal"
        | "bool" | "boolean" | "byte" | "single" | "float" | "version" | "uri" | "guid" | "bitconverter" | "array"
        | "text.encoding" | "text.regularexpressions.regex" | "regex" | "convert" | "io.path" | "net.ipaddress"
        | "globalization.cultureinfo" | "datetimeoffset" | "timezoneinfo" | "linq.enumerable" | "enum"
        | "diagnostics.stopwatch" | "web.httputility" | "net.webutility" | "numerics.biginteger" | "text.stringbuilder"
        | "collections.generic.list[string]" | "pscustomobject" | "ordered" => true,
        "environment" => m.starts_with("get") || matches!(m, "expandenvironmentvariables" | "is64bitoperatingsystem"),
        "io.file" => matches!(m, "exists" | "readalltext" | "readalllines" | "readallbytes" | "getattributes" | "getlastwritetime" | "getcreationtime" | "getlastaccesstime" | "readlines" | "openread" | "opentext"),
        "io.directory" => matches!(m, "exists" | "getfiles" | "getdirectories" | "enumeratefiles" | "enumeratedirectories" | "enumeratefilesystementries" | "getcurrentdirectory" | "getlogicaldrives" | "getfilesystementries" | "getparent" | "getlastwritetime"),
        "io.driveinfo" => true,
        "diagnostics.process" => matches!(m, "getprocesses" | "getprocessbyid" | "getcurrentprocess" | "getprocessesbyname"),
        "net.dns" => matches!(m, "gethostaddresses" | "gethostentry" | "gethostname" | "resolve"),
        "security.principal.windowsidentity" => m == "getcurrent",
        "console" => matches!(m, "writeline" | "write" | "readline" | "readkey" | "clear" | "beep" | "resetcolor"),
        "net.networkinformation.networkinterface" => m.starts_with("get"),
        "net.networkinformation.ipglobalproperties" => m.starts_with("get"),
        "management.automation.language.parser" => matches!(m, "parseinput" | "parsefile"),
        _ => false,
    }
}

/// Безопасные методы экземпляров (строки, коллекции, даты, чтение реестра/файловой системы).
fn instance_call_ok(m: &str) -> bool {
    matches!(
        m,
        "tostring" | "toupper" | "tolower" | "toupperinvariant" | "tolowerinvariant" | "trim" | "trimstart" | "trimend"
            | "split" | "replace" | "substring" | "contains" | "startswith" | "endswith" | "indexof" | "lastindexof"
            | "indexofany" | "padleft" | "padright" | "insert" | "equals" | "gethashcode" | "gettype" | "compareto"
            | "format" | "join" | "toarray" | "tochararray" | "getenumerator" | "add" | "addrange" | "where" | "foreach"
            | "getvalue" | "getvaluenames" | "getsubkeynames" | "getvaluekind" | "opensubkey" | "getfiles"
            | "getdirectories" | "getfilesysteminfos" | "enumeratefiles" | "enumeratedirectories" | "readtoend"
            | "readline" | "getbytes" | "getstring" | "matches" | "match" | "ismatch" | "getfolderpath"
            | "getenvironmentvariable" | "isinrole" | "toint32" | "toint64" | "todouble" | "todatetime" | "tolocaltime"
            | "touniversaltime" | "adddays" | "addhours" | "addminutes" | "addseconds" | "addmonths" | "addyears"
            | "addmilliseconds" | "addticks" | "subtract" | "tofiletime" | "normalize" | "parse" | "tryparse" | "round"
            | "floor" | "ceiling" | "abs" | "max" | "min" | "pow" | "sqrt" | "getupperbound" | "getlowerbound"
            | "tolist" | "distinct" | "first" | "last" | "sum" | "count" | "reverse" | "sort" | "containskey"
            | "containsvalue" | "getaccesscontrol" | "getaccessrules" | "getowner" | "get_item" | "item" | "clone"
            | "copyto" | "getproperties" | "getmethods" | "getmembers" | "getfiletype" | "getstatus" | "elapsed"
            | "isnullorempty" | "isnullorwhitespace" | "toshortdatestring" | "tolongdatestring" | "toshorttimestring"
            | "tolongtimestring" | "getnetworkcredential" | "newline" | "peek" | "pop" | "push" | "enqueue" | "dequeue"
            | "getaddressbytes" | "getipproperties" | "getphysicaladdress" | "getipv4statistics" | "getsubkeys"
            | "close" | "dispose" | "waitforexit" | "refresh" | "hasflag" | "getresult"
    )
}
