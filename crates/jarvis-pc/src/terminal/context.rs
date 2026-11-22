//! Контекст «посмотри ошибку»: последняя команда, cwd, оболочка, код возврата, текст ошибки и источник.

use serde::{Deserialize, Serialize};

use super::hook::HookRecord;
use super::parse::{MAX_ERROR_LINES, last_block, looks_like_error};
use super::pty::PtyRun;
use super::vt::tail_lines;
use crate::args::clip;
use crate::win::WindowInfo;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TerminalContext {
    pub shell: Option<String>,
    pub cwd: Option<String>,
    pub last_command: Option<String>,
    pub exit_code: Option<i32>,
    /// Вывод/ошибка последней команды, не больше 200 строк.
    pub error_text: String,
    /// `pty` | `hook` | `hook+uia` | `hook+ocr` | `uia` | `ocr`.
    pub source: String,
    pub window_title: Option<String>,
    pub time_unix_ms: Option<u64>,
    /// Похоже на ошибку (по коду возврата или по тексту).
    pub error_detected: bool,
    /// Набрано в приглашении, но не выполнено.
    pub pending_input: Option<String>,
}

/// Классы окон терминалов: Windows Terminal, conhost, mintty (Git Bash), ConEmu, PuTTY.
pub const TERMINAL_CLASSES: [&str; 5] =
    ["CASCADIA_HOSTING_WINDOW_CLASS", "ConsoleWindowClass", "mintty", "VirtualConsoleClass", "PuTTY"];

/// Редакторы со встроенным терминалом (xterm.js: UIA отдаёт мало — дальше OCR).
pub fn is_editor_with_terminal(w: &WindowInfo) -> bool {
    let p = w.process.to_lowercase();
    w.class == "Chrome_WidgetWin_1"
        && matches!(p.as_str(), "code.exe" | "code - insiders.exe" | "cursor.exe" | "vscodium.exe" | "windsurf.exe")
}

pub fn is_terminal_window(w: &WindowInfo) -> bool {
    TERMINAL_CLASSES.iter().any(|c| w.class.eq_ignore_ascii_case(c)) || is_editor_with_terminal(w)
}

pub fn from_pty(run: &PtyRun) -> TerminalContext {
    let mut lines: Vec<String> = run.output.lines().map(str::to_string).collect();
    let skip = lines.len().saturating_sub(MAX_ERROR_LINES);
    lines.drain(..skip);
    let mut error_text = lines.join("\n");
    if run.timed_out {
        error_text.push_str("\n[остановлено по таймауту]");
    }
    TerminalContext {
        shell: Some(run.shell.name().into()),
        cwd: Some(run.cwd.clone()),
        last_command: Some(run.command.clone()),
        exit_code: run.exit_code,
        error_detected: run.exit_code != Some(0),
        error_text,
        source: "pty".into(),
        window_title: None,
        time_unix_ms: Some(run.finished_unix_ms()),
        pending_input: None,
    }
}

pub fn from_hook(rec: &HookRecord) -> TerminalContext {
    let error_text = rec.error_text.as_deref().map(|t| tail_lines(t, MAX_ERROR_LINES)).unwrap_or_default();
    TerminalContext {
        shell: Some(rec.shell.clone()).filter(|s| !s.is_empty()),
        cwd: rec.cwd.clone(),
        last_command: Some(rec.command.clone()).filter(|c| !c.is_empty()),
        exit_code: rec.exit_code,
        error_detected: rec.success == Some(false) || rec.exit_code.is_some_and(|c| c != 0),
        error_text,
        source: "hook".into(),
        window_title: None,
        time_unix_ms: rec.time_unix_ms,
        pending_input: None,
    }
}

/// Из текста окна (UIA TextPattern или OCR).
pub fn from_text(text: &str, source: &str, window_title: Option<String>) -> TerminalContext {
    let b = last_block(text);
    TerminalContext {
        shell: b.shell.map(|s| s.name().to_string()),
        cwd: b.cwd,
        last_command: b.command,
        exit_code: None,
        error_detected: looks_like_error(&b.output),
        error_text: b.output.join("\n"),
        source: source.into(),
        window_title,
        time_unix_ms: None,
        pending_input: b.pending_input,
    }
}

fn norm_cmd(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Хук знает команду и код, но не stderr внешних программ — добираем вывод с экрана, если команда та же.
pub fn merge_hook_with_screen(mut hook: TerminalContext, screen: &TerminalContext) -> TerminalContext {
    let same = match (&hook.last_command, &screen.last_command) {
        (Some(h), Some(s)) => {
            let (h, s) = (norm_cmd(h), norm_cmd(s));
            !h.is_empty() && (h == s || s.ends_with(&h) || h.ends_with(&s))
        }
        _ => false,
    };
    if same && !screen.error_text.trim().is_empty() {
        if hook.error_text.trim().is_empty() {
            hook.error_text = screen.error_text.clone();
        } else if !hook.error_text.contains(screen.error_text.trim()) {
            hook.error_text = format!("{}\n{}", screen.error_text, hook.error_text);
        }
        hook.source = format!("hook+{}", screen.source);
        hook.window_title = screen.window_title.clone();
        hook.pending_input = screen.pending_input.clone();
        hook.error_detected |= screen.error_detected;
    }
    hook
}

/// Какой из точных источников свежее (оба — не старше `max_age_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fresh {
    Pty,
    Hook,
    None,
}

pub fn choose_fresh(pty_finished_ms: Option<u64>, hook_ms: Option<u64>, now_ms: u64, max_age_ms: u64) -> Fresh {
    let fresh = |t: Option<u64>| t.filter(|&t| now_ms.saturating_sub(t) <= max_age_ms);
    match (fresh(pty_finished_ms), fresh(hook_ms)) {
        (Some(p), Some(h)) => {
            if p >= h {
                Fresh::Pty
            } else {
                Fresh::Hook
            }
        }
        (Some(_), None) => Fresh::Pty,
        (None, Some(_)) => Fresh::Hook,
        (None, None) => Fresh::None,
    }
}

/// Текст для LLM.
pub fn format_context(c: &TerminalContext) -> String {
    let mut s = format!("Источник: {}", c.source);
    if let Some(sh) = &c.shell {
        s.push_str(&format!(" · оболочка: {sh}"));
    }
    if let Some(cwd) = &c.cwd {
        s.push_str(&format!(" · каталог: {cwd}"));
    }
    if let Some(t) = &c.window_title {
        s.push_str(&format!(" · окно: «{}»", clip(t, 60)));
    }
    s.push('\n');
    match &c.last_command {
        Some(cmd) => s.push_str(&format!("Команда: {cmd}\n")),
        None => s.push_str("Команда: не распознана\n"),
    }
    match c.exit_code {
        Some(code) => s.push_str(&format!("Код возврата: {code}\n")),
        None => s.push_str("Код возврата: неизвестен\n"),
    }
    if c.error_detected {
        s.push_str("Похоже на ошибку.\n");
    }
    if let Some(p) = &c.pending_input {
        s.push_str(&format!("В приглашении набрано (не выполнено): {p}\n"));
    }
    let n = c.error_text.lines().count();
    if n == 0 {
        s.push_str("Вывода нет.\n");
    } else {
        s.push_str(&format!("Вывод/ошибка ({n} строк):\n{}\n", c.error_text));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::pty::RunShell;

    #[test]
    fn hook_plus_screen_fills_stderr() {
        let hook = from_hook(&HookRecord {
            version: 1,
            shell: "powershell".into(),
            command: "git comit -m x".into(),
            exit_code: Some(1),
            success: Some(false),
            cwd: Some("C:\\proj".into()),
            time_unix_ms: Some(1000),
            ..Default::default()
        });
        assert!(hook.error_detected);
        assert!(hook.error_text.is_empty());
        let screen = from_text(
            "PS C:\\proj> git comit -m x\ngit: 'comit' is not a git command. See 'git --help'.\nPS C:\\proj> ",
            "uia",
            Some("Windows PowerShell".into()),
        );
        let m = merge_hook_with_screen(hook.clone(), &screen);
        assert_eq!(m.source, "hook+uia");
        assert_eq!(m.exit_code, Some(1));
        assert!(m.error_text.contains("is not a git command"));
        // другая команда на экране — не смешиваем
        let other = from_text("PS C:\\> dir\nfoo\nPS C:\\> ", "uia", None);
        assert_eq!(merge_hook_with_screen(hook, &other).source, "hook");
    }

    #[test]
    fn freshness() {
        let now = 10_000_000;
        let max = 60_000;
        assert_eq!(choose_fresh(Some(now - 10), Some(now - 20), now, max), Fresh::Pty);
        assert_eq!(choose_fresh(Some(now - 30), Some(now - 20), now, max), Fresh::Hook);
        assert_eq!(choose_fresh(Some(now - 120_000), Some(now - 20), now, max), Fresh::Hook);
        assert_eq!(choose_fresh(None, None, now, max), Fresh::None);
        assert_eq!(choose_fresh(Some(now - 120_000), None, now, max), Fresh::None);
    }

    #[test]
    fn pty_context_and_format() {
        let run = PtyRun {
            id: 1,
            command: "git comit".into(),
            shell: RunShell::Powershell,
            cwd: "D:\\jarvis".into(),
            exit_code: Some(1),
            output: (0..250).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n"),
            timed_out: false,
            cancelled: false,
            started_unix_ms: 5,
            duration_ms: 10,
        };
        let c = from_pty(&run);
        assert_eq!(c.error_text.lines().count(), MAX_ERROR_LINES);
        assert!(c.error_text.ends_with("l249"));
        assert_eq!(c.time_unix_ms, Some(15));
        let t = format_context(&c);
        assert!(t.starts_with("Источник: pty · оболочка: powershell · каталог: D:\\jarvis\n"));
        assert!(t.contains("Код возврата: 1"));
        assert!(t.contains("Команда: git comit"));
    }

    #[test]
    fn terminal_window_classes() {
        let mut w = WindowInfo {
            hwnd: 1,
            title: "PowerShell".into(),
            class: "CASCADIA_HOSTING_WINDOW_CLASS".into(),
            pid: 1,
            process: "WindowsTerminal.exe".into(),
            rect: Default::default(),
            minimized: false,
            foreground: true,
        };
        assert!(is_terminal_window(&w));
        w.class = "Chrome_WidgetWin_1".into();
        w.process = "Code.exe".into();
        assert!(is_terminal_window(&w) && is_editor_with_terminal(&w));
        w.process = "chrome.exe".into();
        assert!(!is_terminal_window(&w));
    }
}
