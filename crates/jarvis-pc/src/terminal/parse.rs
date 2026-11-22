//! Разбор текста окна терминала: приглашения PowerShell/cmd/bash и последний блок «приглашение → команда → вывод».

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Сколько строк вывода/ошибки отдавать модели (план, раздел 3б).
pub const MAX_ERROR_LINES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Powershell,
    Cmd,
    Bash,
}

impl ShellKind {
    pub fn name(self) -> &'static str {
        match self {
            ShellKind::Powershell => "powershell",
            ShellKind::Cmd => "cmd",
            ShellKind::Bash => "bash",
        }
    }
}

static PS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^PS (?P<cwd>[^>]+?)>+ ?(?P<cmd>.*)$").expect("regex"));
static CMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(?P<cwd>[A-Za-z]:\\[^<>|"*?]*?)>(?P<cmd>.*)$"#).expect("regex"));
static BASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\([^)]*\)\s*)?(?P<user>[\w.\-]+)@(?P<host>[\w.\-]+):(?P<cwd>[^$#]*?)[$#] ?(?P<cmd>.*)$").expect("regex")
});
/// Git Bash (MSYS2): `user@HOST MINGW64 ~/proj (main)` и на следующей строке `$ команда`.
static MSYS_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\([^)]*\)\s*)?[\w.\-]+@[\w.\-]+ (?:MINGW64|MINGW32|MSYS|UCRT64|CLANG64|CLANGARM64) (?P<cwd>.+?)(?: \([^)]*\))?$")
        .expect("regex")
});
static DOLLAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\$(?: (?P<cmd>.*))?$").expect("regex"));

/// Найденное приглашение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// Строка с командой.
    pub line: usize,
    /// Первая строка приглашения (для двухстрочного Git Bash — строка заголовка).
    pub first_line: usize,
    pub shell: ShellKind,
    pub cwd: String,
    pub command: String,
}

/// Разобрать одну строку как приглашение.
pub fn parse_prompt_line(line: &str) -> Option<(ShellKind, String, String)> {
    let line = line.trim_end();
    if let Some(c) = PS.captures(line) {
        return Some((ShellKind::Powershell, c["cwd"].to_string(), c["cmd"].trim().to_string()));
    }
    if let Some(c) = CMD.captures(line) {
        return Some((ShellKind::Cmd, c["cwd"].to_string(), c["cmd"].trim().to_string()));
    }
    if let Some(c) = BASH.captures(line) {
        return Some((ShellKind::Bash, c["cwd"].trim().to_string(), c["cmd"].trim().to_string()));
    }
    None
}

pub fn find_prompts(lines: &[&str]) -> Vec<Prompt> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].trim_end();
        if let Some(h) = MSYS_HEADER.captures(l) {
            if let Some(d) = lines.get(i + 1).and_then(|n| DOLLAR.captures(n.trim_end())) {
                out.push(Prompt {
                    line: i + 1,
                    first_line: i,
                    shell: ShellKind::Bash,
                    cwd: h["cwd"].trim().to_string(),
                    command: d.name("cmd").map(|m| m.as_str().trim().to_string()).unwrap_or_default(),
                });
                i += 2;
                continue;
            }
        }
        if let Some((shell, cwd, command)) = parse_prompt_line(l) {
            out.push(Prompt { line: i, first_line: i, shell, cwd, command });
        }
        i += 1;
    }
    out
}

/// Последний выполненный блок.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Block {
    pub shell: Option<ShellKind>,
    pub cwd: Option<String>,
    pub command: Option<String>,
    /// Вывод команды (≤ MAX_ERROR_LINES последних строк).
    pub output: Vec<String>,
    /// Текст, набранный в текущем приглашении, но не выполненный.
    pub pending_input: Option<String>,
    pub prompt_found: bool,
}

fn trim_empty_tail(v: &mut Vec<String>) {
    while v.last().is_some_and(|l| l.trim().is_empty()) {
        v.pop();
    }
}

fn tail(v: Vec<String>, n: usize) -> Vec<String> {
    let skip = v.len().saturating_sub(n);
    v.into_iter().skip(skip).collect()
}

/// Выделить последний блок «приглашение → команда → вывод». Без распознанных приглашений — хвост текста.
pub fn last_block(text: &str) -> Block {
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let prompts = find_prompts(&lines);
    if prompts.is_empty() {
        let mut output: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        trim_empty_tail(&mut output);
        return Block { output: tail(output, MAX_ERROR_LINES), ..Default::default() };
    }
    let mut idx = prompts.len() - 1;
    let mut pending = None;
    let current = &prompts[idx];
    if current.line + 1 == lines.len() {
        // Последняя строка — текущее приглашение (команда ещё не выполнена или пусто).
        if !current.command.is_empty() {
            pending = Some(current.command.clone());
        }
        if idx == 0 {
            return Block {
                shell: Some(current.shell),
                cwd: Some(current.cwd.clone()),
                pending_input: pending,
                prompt_found: true,
                ..Default::default()
            };
        }
        idx -= 1;
    }
    // Пропускаем пустые приглашения (просто Enter).
    while idx > 0 && prompts[idx].command.is_empty() {
        idx -= 1;
    }
    let p = &prompts[idx];
    let end = prompts.get(idx + 1).map(|n| n.first_line).unwrap_or(lines.len());
    let mut output: Vec<String> = lines[(p.line + 1).min(end)..end].iter().map(|s| s.to_string()).collect();
    trim_empty_tail(&mut output);
    Block {
        shell: Some(p.shell),
        cwd: Some(p.cwd.clone()),
        command: (!p.command.is_empty()).then(|| p.command.clone()),
        output: tail(output, MAX_ERROR_LINES),
        pending_input: pending,
        prompt_found: true,
    }
}

/// Похоже ли на ошибку (для случаев, когда кода возврата нет — UIA/OCR).
pub fn looks_like_error(lines: &[String]) -> bool {
    const MARKERS: [&str; 26] = [
        "error",
        "ошибка",
        "не является внутренней",
        "не распознано",
        "not recognized",
        "is not a git command",
        "command not found",
        "not found",
        "не найден",
        "fatal:",
        "failed",
        "exception",
        "access is denied",
        "отказано в доступе",
        "permission denied",
        "cannot",
        "не удается",
        "невозможно",
        "categoryinfo",
        "fullyqualifiederrorid",
        "traceback",
        "panicked",
        "no such file",
        "нет такого файла",
        "unexpected token",
        "syntax error",
    ];
    lines.iter().any(|l| {
        let l = l.to_lowercase();
        MARKERS.iter().any(|m| l.contains(m))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS_GIT: &str = "\
Windows PowerShell
(C) Корпорация Майкрософт (Microsoft Corporation). Все права защищены.

PS C:\\Users\\yulii\\proj> git status
On branch main
nothing to commit, working tree clean
PS C:\\Users\\yulii\\proj> git comit -m x
git: 'comit' is not a git command. See 'git --help'.

The most similar command is
        commit
PS C:\\Users\\yulii\\proj>

";

    const PS_CMDLET: &str = "\
PS D:\\jarvis> Get-Foo
Get-Foo : Имя \"Get-Foo\" не распознано как имя командлета, функции, файла сценария или выполняемой программы.
строка:1 знак:1
+ Get-Foo
+ ~~~~~~~
    + CategoryInfo          : ObjectNotFound: (Get-Foo:String) [], CommandNotFoundException
    + FullyQualifiedErrorId : CommandNotFoundException

PS D:\\jarvis>
PS D:\\jarvis> git pu";

    const CMD_ERR: &str = "\
Microsoft Windows [Version 10.0.26300.1]
(c) Корпорация Майкрософт (Microsoft Corporation). Все права защищены.

C:\\Users\\yulii>pyton --version
\"pyton\" не является внутренней или внешней
командой, исполняемой программой или пакетным файлом.

C:\\Users\\yulii>";

    const BASH_ERR: &str = "\
yulii@xeon:~/proj$ ls
Cargo.toml  src
yulii@xeon:~/proj$ git comit -m x
git: 'comit' is not a git command. See 'git --help'.

The most similar command is
	commit
yulii@xeon:~/proj$ ";

    const GITBASH_ERR: &str = "\
yulii@DESKTOP MINGW64 ~/proj (main)
$ npm rn build
Unknown command: \"rn\"

yulii@DESKTOP MINGW64 ~/proj (main)
$ ";

    #[test]
    fn prompt_lines() {
        assert_eq!(
            parse_prompt_line("PS C:\\Users\\yulii> git status"),
            Some((ShellKind::Powershell, "C:\\Users\\yulii".into(), "git status".into()))
        );
        assert_eq!(parse_prompt_line("PS C:\\>"), Some((ShellKind::Powershell, "C:\\".into(), "".into())));
        assert_eq!(parse_prompt_line("C:\\Windows\\System32>dir /b"), Some((ShellKind::Cmd, "C:\\Windows\\System32".into(), "dir /b".into())));
        assert_eq!(
            parse_prompt_line("(venv) root@host:/etc# cat x"),
            Some((ShellKind::Bash, "/etc".into(), "cat x".into()))
        );
        assert_eq!(parse_prompt_line("At C:\\script.ps1:3 char:1"), None);
        assert_eq!(parse_prompt_line("+ CategoryInfo : x"), None);
    }

    #[test]
    fn powershell_git_typo() {
        let b = last_block(PS_GIT);
        assert_eq!(b.shell, Some(ShellKind::Powershell));
        assert_eq!(b.cwd.as_deref(), Some("C:\\Users\\yulii\\proj"));
        assert_eq!(b.command.as_deref(), Some("git comit -m x"));
        assert_eq!(b.output.first().map(String::as_str), Some("git: 'comit' is not a git command. See 'git --help'."));
        assert_eq!(b.output.last().map(|s| s.trim()), Some("commit"));
        assert_eq!(b.pending_input, None);
        assert!(looks_like_error(&b.output));
    }

    #[test]
    fn powershell_cmdlet_error_with_pending_input_and_empty_prompts() {
        let b = last_block(PS_CMDLET);
        assert_eq!(b.command.as_deref(), Some("Get-Foo"));
        assert_eq!(b.pending_input.as_deref(), Some("git pu"));
        assert_eq!(b.output.len(), 6, "{:?}", b.output);
        assert!(b.output[0].contains("не распознано"));
        assert!(looks_like_error(&b.output));
    }

    #[test]
    fn cmd_error() {
        let b = last_block(CMD_ERR);
        assert_eq!(b.shell, Some(ShellKind::Cmd));
        assert_eq!(b.cwd.as_deref(), Some("C:\\Users\\yulii"));
        assert_eq!(b.command.as_deref(), Some("pyton --version"));
        assert_eq!(b.output.len(), 2);
        assert!(looks_like_error(&b.output));
    }

    #[test]
    fn bash_error() {
        let b = last_block(BASH_ERR);
        assert_eq!(b.shell, Some(ShellKind::Bash));
        assert_eq!(b.cwd.as_deref(), Some("~/proj"));
        assert_eq!(b.command.as_deref(), Some("git comit -m x"));
        assert!(b.output[0].contains("is not a git command"));
    }

    #[test]
    fn git_bash_two_line_prompt() {
        let b = last_block(GITBASH_ERR);
        assert_eq!(b.shell, Some(ShellKind::Bash));
        assert_eq!(b.cwd.as_deref(), Some("~/proj"));
        assert_eq!(b.command.as_deref(), Some("npm rn build"));
        assert_eq!(b.output, vec!["Unknown command: \"rn\"".to_string()]);
    }

    #[test]
    fn no_prompt_falls_back_to_tail() {
        let text: String = (0..300).map(|i| format!("строка {i}\n")).collect();
        let b = last_block(&text);
        assert!(!b.prompt_found);
        assert_eq!(b.output.len(), MAX_ERROR_LINES);
        assert_eq!(b.output.last().map(String::as_str), Some("строка 299"));
        assert!(!looks_like_error(&b.output));
    }

    #[test]
    fn only_current_prompt() {
        let b = last_block("PS C:\\> ");
        assert!(b.prompt_found);
        assert_eq!(b.command, None);
        assert_eq!(b.cwd.as_deref(), Some("C:\\"));
    }
}
