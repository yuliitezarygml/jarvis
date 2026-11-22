//! Извлечение команд: основная команда документа (по заголовку/имени файла) и команды фрагмента
//! (командлеты `Verb-Noun` в коде и прозе, имена exe — первое слово строк в блоках кода и в `inline code`).

use jarvis_kb::commands::{find_cmdlets, is_cmdlet, looks_like_exe, normalize_command};
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

use crate::config::{CommandsFrom, SourceCfg};

/// Частые консольные программы Windows — известны даже без справочника windows-commands.
pub const BUILTIN_EXES: &[&str] = &[
    "arp", "assoc", "attrib", "auditpol", "bcdboot", "bcdedit", "bitsadmin", "bootrec", "cacls", "certutil",
    "chkdsk", "chkntfs", "cipher", "cleanmgr", "clip", "cmd", "cmdkey", "compact", "control", "cscript", "curl",
    "defrag", "diskpart", "dism", "dsregcmd", "driverquery", "eventvwr", "explorer", "findstr", "fsutil", "ftype",
    "getmac", "git", "gpresult", "gpupdate", "hostname", "icacls", "ipconfig", "logman", "manage-bde", "mklink",
    "mmc", "mountvol", "msconfig", "msiexec", "msinfo32", "mstsc", "nbtstat", "net", "netsh", "netstat", "nslookup",
    "pathping", "ping", "pnputil", "powercfg", "powershell", "pwsh", "quser", "reagentc", "reg", "regedit",
    "regsvr32", "robocopy", "route", "rundll32", "runas", "sc", "schtasks", "secedit", "setx", "sfc", "shutdown",
    "systeminfo", "takeown", "taskkill", "tasklist", "tracert", "tzutil", "vssadmin", "w32tm", "wbadmin",
    "wevtutil", "whoami", "winget", "winver", "wmic", "wsl", "wsreset", "wusa", "xcopy",
];

/// Языки блоков кода, в которых первое слово строки — команда оболочки.
const SHELL_LANGS: &[&str] = &["", "cmd", "console", "powershell", "ps", "pwsh", "ps1", "bat", "batch", "dos", "shell", "terminal", "azurepowershell"];

static PROMPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:PS\s*)?(?:[A-Za-z]:\\[^>]*)?>\s*|^\$\s+").unwrap());
static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`\n]+)`").unwrap());

/// Основные команды документа (страница самой команды).
pub fn primary_commands(src: &SourceCfg, rel_path: &str, title: &str) -> Vec<String> {
    let rel = rel_path.replace('\\', "/");
    let mut out: Vec<String> = Vec::new();
    let in_scope = src.command_scope.is_empty() || src.command_scope.iter().any(|p| rel.starts_with(p.as_str()));
    let stem = rel.rsplit('/').next().unwrap_or("").trim_end_matches(".md").to_lowercase();
    if in_scope {
        match src.commands {
            CommandsFrom::None => {}
            CommandsFrom::CmdletTitle => {
                let t = title.trim();
                if is_cmdlet(t, false) || (t.to_ascii_lowercase().starts_with("about_") && !t.contains(' ')) {
                    out.push(t.to_string());
                }
            }
            CommandsFrom::ExeTitle => {
                let n = normalize_command(title);
                let words: Vec<&str> = n.split_whitespace().collect();
                if let Some(first) = words.first() {
                    let stem_words = stem.replace('-', " ");
                    if words.len() <= 3 && looks_like_exe(first) && (stem_words == n || stem == *first) {
                        out.push(n.clone());
                        if words.len() > 1 && stem == *first {
                            out.push(first.to_string());
                        }
                    }
                }
            }
            CommandsFrom::Filename => {
                if !stem.contains('-') && stem != "index" && looks_like_exe(&stem) {
                    out.push(stem.clone());
                }
            }
        }
    }
    if let Some(aliases) = src.command_aliases.get(&rel) {
        out.extend(aliases.iter().cloned());
    }
    let mut seen = HashSet::new();
    out.retain(|c| seen.insert(c.to_lowercase()));
    out
}

fn first_command_of_line(line: &str, known: &HashSet<String>) -> Option<String> {
    let l = PROMPT.replace(line.trim(), "");
    let l = l.trim_start_matches(['&', '.', ' ']).trim();
    let tok = l.split_whitespace().next()?.trim_matches(|c| matches!(c, '"' | '\'' | '(' | ')'));
    let lower = tok.to_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    (looks_like_exe(name) && known.contains(name)).then(|| name.to_string())
}

/// Команды фрагмента в порядке появления, без повторов (без учёта регистра).
pub fn chunk_commands(code: &[(String, String)], prose: &str, known_exes: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |c: String, out: &mut Vec<String>| {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&c)) {
            out.push(c);
        }
    };
    for (lang, c) in code {
        if !SHELL_LANGS.contains(&lang.as_str()) {
            continue;
        }
        for cm in find_cmdlets(c, false) {
            push(cm, &mut out);
        }
        for line in c.lines() {
            if let Some(e) = first_command_of_line(line, known_exes) {
                push(e, &mut out);
            }
        }
    }
    for cm in find_cmdlets(prose, true) {
        push(cm, &mut out);
    }
    for cap in INLINE_CODE.captures_iter(prose) {
        if let Some(e) = first_command_of_line(&cap[1], known_exes) {
            push(e, &mut out);
        }
        for cm in find_cmdlets(&cap[1], false) {
            push(cm, &mut out);
        }
    }
    out
}

pub fn builtin_exes() -> HashSet<String> {
    BUILTIN_EXES.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UrlCfg;

    fn src(commands: CommandsFrom) -> SourceCfg {
        SourceCfg {
            id: "t".into(),
            name: "t".into(),
            kind: Default::default(),
            repo: String::new(),
            mirrors: vec![],
            branch: String::new(),
            sparse: vec![],
            path: String::new(),
            include: vec![],
            exclude: vec![],
            url: UrlCfg::default(),
            commands,
            command_scope: vec![],
            command_aliases: Default::default(),
            dedupe_versions: vec![],
            note: String::new(),
            enabled: true,
        }
    }

    #[test]
    fn primary_from_titles_and_filenames() {
        let s = src(CommandsFrom::ExeTitle);
        let p = "WindowsServerDocs/administration/windows-commands/";
        assert_eq!(primary_commands(&s, &format!("{p}robocopy.md"), "robocopy"), ["robocopy"]);
        assert_eq!(primary_commands(&s, &format!("{p}sc-config.md"), "sc.exe config"), ["sc config"]);
        assert_eq!(primary_commands(&s, &format!("{p}netsh-winsock.md"), "Netsh winsock"), ["netsh winsock"]);
        assert!(primary_commands(&s, &format!("{p}command-line-syntax-key.md"), "Command-line syntax key").is_empty());
        assert!(primary_commands(&s, &format!("{p}foo.md"), "Something else entirely").is_empty());

        let s = src(CommandsFrom::CmdletTitle);
        assert_eq!(primary_commands(&s, "reference/7.6/M/Get-Service.md", "Get-Service"), ["Get-Service"]);
        assert_eq!(primary_commands(&s, "reference/7.6/M/About/about_Execution_Policies.md", "about_Execution_Policies"), ["about_Execution_Policies"]);
        assert!(primary_commands(&s, "reference/docs-conceptual/x.md", "Installing PowerShell on Windows").is_empty());

        let mut s = src(CommandsFrom::Filename);
        s.command_scope = vec!["sysinternals/downloads/".into()];
        s.command_aliases.insert("sysinternals/downloads/process-explorer.md".into(), vec!["procexp".into()]);
        assert_eq!(primary_commands(&s, "sysinternals/downloads/procmon.md", "Process Monitor v4.01"), ["procmon"]);
        assert_eq!(primary_commands(&s, "sysinternals/downloads/process-explorer.md", "Process Explorer"), ["procexp"]);
        assert!(primary_commands(&s, "sysinternals/resources/x.md", "x").is_empty());
        assert!(primary_commands(&s, "sysinternals/downloads/index.md", "Index").is_empty());
    }

    #[test]
    fn commands_from_code_and_prose() {
        let known = builtin_exes();
        let code = vec![
            ("cmd".to_string(), "C:\\> ipconfig /flushdns\nnetsh winsock reset\nrem comment\nsc.exe query spooler".to_string()),
            ("powershell".to_string(), "PS C:\\> Get-Service | where-object Status -eq Running\n& \"C:\\Windows\\System32\\robocopy.exe\" a b".to_string()),
            ("json".to_string(), "{\"cmd\": \"Get-Item\"}".to_string()),
        ];
        let prose = "Run `sfc /scannow` or use Restart-Computer. A read-only set-up.";
        let c = chunk_commands(&code, prose, &known);
        assert_eq!(c, ["ipconfig", "netsh", "sc", "Get-Service", "where-object", "robocopy", "Restart-Computer", "sfc"]);
    }
}
