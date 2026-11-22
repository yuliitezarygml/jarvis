//! Распознавание имён команд: командлеты PowerShell `Verb-Noun` (по списку одобренных глаголов `Get-Verb`
//! плюс несколько ядровых исключений вроде `Where-Object`) и нормализация имён exe (`sc.exe` → `sc`).
//! Используется и при сборке базы (`kb-build`), и при разборе запросов.

/// Одобренные глаголы PowerShell (`Get-Verb`) + глаголы ядровых командлетов вне списка (Sort, Tee, Where, ForEach).
const VERBS: &[&str] = &[
    "add", "approve", "assert", "backup", "block", "build", "checkpoint", "clear", "close", "compare", "complete",
    "compress", "confirm", "connect", "convert", "convertfrom", "convertto", "copy", "debug", "deny", "deploy",
    "disable", "disconnect", "dismount", "edit", "enable", "enter", "exit", "expand", "export", "find", "foreach",
    "format", "get", "grant", "group", "hide", "import", "initialize", "install", "invoke", "join", "limit", "lock",
    "measure", "merge", "mount", "move", "new", "open", "optimize", "out", "ping", "pop", "protect", "publish",
    "push", "read", "receive", "redo", "register", "remove", "rename", "repair", "request", "reset", "resize",
    "resolve", "restart", "restore", "resume", "revoke", "save", "search", "select", "send", "set", "show", "skip",
    "sort", "split", "start", "step", "stop", "submit", "suspend", "switch", "sync", "tee", "test", "trace",
    "unblock", "undo", "uninstall", "unlock", "unprotect", "unpublish", "unregister", "update", "use", "wait",
    "watch", "where", "write",
];

/// Существительные, при которых «Verb-Noun» в прозе — это обычное слово через дефис (read-only, set-up…).
const FALSE_NOUNS: &[&str] = &[
    "only", "up", "in", "out", "of", "on", "off", "to", "by", "side", "level", "based", "write", "time", "back",
    "down", "aside", "through", "over", "the", "and", "ins", "outs", "free", "safe", "box", "line",
];

pub fn is_verb(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    VERBS.binary_search(&w.as_str()).is_ok()
}

/// Является ли слово командлетом `Verb-Noun`. `strict` (для прозы) требует заглавные буквы у глагола и
/// существительного (`Get-Service`), без него (для блоков кода) регистр любой (`get-service`).
pub fn is_cmdlet(word: &str, strict: bool) -> bool {
    let Some((verb, noun)) = word.split_once('-') else { return false };
    if !is_verb(verb) || noun.len() < 2 || !noun.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    if !noun.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if FALSE_NOUNS.contains(&noun.to_ascii_lowercase().as_str()) {
        return false;
    }
    if strict {
        let v0 = verb.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        let n0 = noun.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if !(v0 && n0) {
            return false;
        }
    }
    true
}

/// Все командлеты в тексте (в порядке появления, без повторов; сравнение без учёта регистра).
pub fn find_cmdlets(text: &str, strict: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        let word = word.trim_matches('-');
        if is_cmdlet(word, strict) && !out.iter().any(|o| o.eq_ignore_ascii_case(word)) {
            out.push(word.to_string());
        }
    }
    out
}

/// Нормализовать имя команды для поиска: обрезать пробелы и кавычки, убрать `.exe`/`.com`, для exe — нижний регистр.
/// Командлеты сохраняют регистр (`Get-Service`), сравнение в базе всё равно без учёта регистра.
pub fn normalize_command(name: &str) -> String {
    let n = name.trim().trim_matches(|c| matches!(c, '`' | '"' | '\'' | '(' | ')'));
    let first = n.split_whitespace().next().unwrap_or("");
    if is_cmdlet(first, false) {
        return first.to_string();
    }
    let lower = n.to_lowercase();
    let mut words: Vec<String> = lower.split_whitespace().map(str::to_string).collect();
    if let Some(w0) = words.first_mut() {
        for ext in [".exe", ".com"] {
            if let Some(s) = w0.strip_suffix(ext) {
                *w0 = s.to_string();
            }
        }
    }
    words.join(" ")
}

/// Похоже ли слово на имя консольной программы (`robocopy`, `sc`, `bcdedit`, `pnputil`).
pub fn looks_like_exe(word: &str) -> bool {
    let w = word.strip_suffix(".exe").unwrap_or(word);
    !w.is_empty()
        && w.len() <= 32
        && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_sorted_for_binary_search() {
        let mut v = VERBS.to_vec();
        v.sort_unstable();
        assert_eq!(v, VERBS);
    }

    #[test]
    fn cmdlet_detection() {
        assert!(is_cmdlet("Get-Service", true));
        assert!(is_cmdlet("get-service", false));
        assert!(!is_cmdlet("get-service", true));
        assert!(is_cmdlet("ConvertTo-Json", true));
        assert!(is_cmdlet("Get-NetIPv4Protocol", true));
        assert!(!is_cmdlet("Read-Only", true));
        assert!(!is_cmdlet("Set-Up", true));
        assert!(!is_cmdlet("Foo-Bar", false));
        assert!(!is_cmdlet("Get-", false));
        assert!(!is_cmdlet("Get-Service-Extra", false));
        assert!(!is_cmdlet("Get-1abc", false));
    }

    #[test]
    fn finds_cmdlets_in_text() {
        let t = "Use Get-Service, then `Stop-Service -Name Spooler`; a read-only set-up. Get-Service again.";
        assert_eq!(find_cmdlets(t, true), ["Get-Service", "Stop-Service"]);
        assert_eq!(find_cmdlets("get-process | where-object cpu -gt 5", false), ["get-process", "where-object"]);
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize_command(" SC.EXE "), "sc");
        assert_eq!(normalize_command("robocopy.exe /mir"), "robocopy /mir");
        assert_eq!(normalize_command("`Get-Service -Name x`"), "Get-Service");
        assert_eq!(normalize_command("Netsh AdvFirewall"), "netsh advfirewall");
        assert!(looks_like_exe("bcdedit"));
        assert!(looks_like_exe("pnputil.exe"));
        assert!(!looks_like_exe("C:\\x"));
        assert!(!looks_like_exe("1abc"));
    }
}
