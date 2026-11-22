//! Сигнатура ошибки (план, 3б «Самообучение на ошибках»).
//!
//! Текст сообщения об ошибке нормализуется: убираются ANSI-коды, пути Windows/Unix, URL, e-mail,
//! GUID, IP, даты/время, hex-адреса, числа, имена файлов и всё в кавычках; служебные строки
//! PowerShell (`+ gti status`, `+ ~~~`) и кадры кода rustc (`3 | let x`) отбрасываются;
//! пробелы схлопываются, текст в lowercase. От нормализованного текста берётся SHA-256 → 16 hex.
//!
//! Две версии:
//! * **общая** (`signature`) — кавычки заменяются на `<q>`: «The term 'gti' …» и «The term 'pyhton' …» —
//!   одна ошибка «команда не найдена»;
//! * **точная** (`exact`) — содержимое кавычек сохраняется: рецепт «gti → git» не применяется к «pyhton».
//!
//! Коды ошибок, которые сами являются идентификатором (`E0425`, `CS0103`, HRESULT `0x80070005`,
//! NTSTATUS `0xC0000005`), сохраняются.

use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

use crate::text::fold;

/// Сколько значимых строк ошибки учитывать (дальше обычно стек и повторы).
const MAX_LINES: usize = 6;
/// Предел длины нормализованного текста (символы).
const MAX_CHARS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorSig {
    /// Общая сигнатура, 16 hex.
    pub signature: String,
    /// Точная сигнатура (с содержимым кавычек), 16 hex.
    pub exact: String,
    /// Нормализованный текст общей сигнатуры (хранится в уроке, показывается в UI).
    pub normalized: String,
}

impl ErrorSig {
    pub fn of(text: &str) -> Self {
        let normalized = normalize(text, false);
        let exact_text = normalize(text, true);
        Self { signature: hash16(&normalized), exact: hash16(&exact_text), normalized }
    }
}

/// Общая сигнатура ошибки: 16 hex-символов.
pub fn error_signature(text: &str) -> String {
    hash16(&normalize(text, false))
}

/// Нормализованный текст ошибки (основа общей сигнатуры).
pub fn normalize_error(text: &str) -> String {
    normalize(text, false)
}

fn hash16(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect(stringify!($name)));
    };
}

re!(ANSI, r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)");
re!(PS_CMD_PREFIX, r"^[\w.\-]+ : ");
re!(URL, r"\b(?:https?|ftp|file|wss?)://[^\s'\x22<>]+");
re!(EMAIL, r"\b[\w.+\-]+@[\w\-]+(?:\.[\w\-]+)+\b");
re!(GUID, r"\{?\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b\}?");
re!(PS_STRING_REF, r"\([^()\s]*:string\)");
re!(Q_DOUBLE, r#""[^"\n]*""#);
re!(Q_BACKTICK, r"`[^`\n]*`");
re!(Q_GUILLEMET, r"«[^»\n]*»");
re!(Q_CURLY, r"[‘“][^’”\n]*[’”]");
// одинарные кавычки — только если открывающая стоит не внутри слова (don't, can't)
re!(Q_SINGLE, r"(^|[\s(\[{:=,;])'[^'\n]*'");
re!(BACKSLASH_TOKEN, r"[^\s'\x22<>|()\[\]{}«»]*\\[^\s'\x22<>|()\[\]{}«»]*");
re!(DRIVE_FWD_PATH, r"\b[a-z]:/[^\s'\x22<>|()\[\]{}]*");
re!(UNIX_PATH, r"(^|[\s(\[=,:'\x22])(?:~|\.{1,2})?(?:/[\w.@+~\-]+)+/?");
re!(REL_PATH, r"\b[\w.\-]+(?:/[\w.\-]+)+");
re!(
    FILE_NAME,
    r"\b[\w\-][\w.\-]*\.(?:exe|dll|sys|msi|msix|appx|bat|cmd|ps1|psm1|psd1|sh|py|pyc|rs|js|mjs|cjs|ts|tsx|jsx|json|toml|yaml|yml|xml|ini|cfg|conf|config|txt|log|md|csv|html?|css|c|cc|cpp|h|hpp|cs|csproj|sln|java|kt|go|rb|php|lock|zip|7z|rar|tar|gz|tgz|iso|png|jpe?g|gif|bmp|svg|pdf|docx?|xlsx?|pptx?|db|sqlite|dat|tmp|bak|reg|lnk|vbs|jar|class|whl|so|dylib|lib|obj|pdb|wav|mp3|mp4)\b"
);
re!(LOC, r"(<(?:path|file|url|q)>)(?::\d+)+");
re!(IP, r"\b\d{1,3}(?:\.\d{1,3}){3}(?::\d{1,5})?\b");
re!(
    ISO_DATE,
    r"\b\d{4}-\d{2}-\d{2}(?:[t ]\d{2}:\d{2}(?::\d{2})?(?:[.,]\d+)?(?:z|[+\-]\d{2}:?\d{2})?)?"
);
re!(DMY_DATE, r"\b\d{1,2}[./]\d{1,2}[./]\d{2,4}\b");
re!(TIME, r"\b\d{1,2}:\d{2}(?::\d{2})?(?:[.,]\d+)?(?:\s?[ap]m\b)?");
re!(HEX_0X, r"\b0x[0-9a-f]+\b");
re!(HRESULT, r"^0x[8c][0-9a-f]{7}$");
re!(HEX_BARE, r"\b[0-9a-f]{7,}\b");
re!(NUM, r"\b\d+(?:[.,]\d+)*\b");
re!(RUST_FRAME, r"^\d*\s*\|");

/// Строки-«шум», которые меняются от запуска к запуску и не описывают ошибку.
fn is_noise_line(l: &str) -> bool {
    // декоративные строки: ~~~~, ^^^^, -----, ====, + ~~~
    if l.chars().all(|c| "~^-=*_+|. \t".contains(c)) {
        return true;
    }
    // эхо команды PowerShell: «+ gti status» (но не «+ CategoryInfo …»)
    if let Some(rest) = l.strip_prefix("+ ") {
        let r = rest.trim_start();
        return !(r.starts_with("categoryinfo") || r.starts_with("fullyqualifiederrorid"));
    }
    // кадр исходника rustc: «3 |     let x = y;»
    RUST_FRAME.is_match(l)
}

fn normalize_line(line: &str, keep_quotes: bool) -> String {
    let mut s = PS_CMD_PREFIX.replace(line, "<cmd> : ").into_owned();
    s = URL.replace_all(&s, "<url>").into_owned();
    s = EMAIL.replace_all(&s, "<email>").into_owned();
    s = GUID.replace_all(&s, "<guid>").into_owned();
    s = PS_STRING_REF.replace_all(&s, "(<q>)").into_owned();
    if !keep_quotes {
        s = Q_DOUBLE.replace_all(&s, "<q>").into_owned();
        s = Q_BACKTICK.replace_all(&s, "<q>").into_owned();
        s = Q_GUILLEMET.replace_all(&s, "<q>").into_owned();
        s = Q_CURLY.replace_all(&s, "<q>").into_owned();
        s = Q_SINGLE.replace_all(&s, "${1}<q>").into_owned();
    }
    s = BACKSLASH_TOKEN.replace_all(&s, "<path>").into_owned();
    s = DRIVE_FWD_PATH.replace_all(&s, "<path>").into_owned();
    s = UNIX_PATH.replace_all(&s, "${1}<path>").into_owned();
    s = REL_PATH
        .replace_all(&s, |c: &Captures| {
            let m = &c[0];
            if m.contains('.') { "<path>".to_string() } else { m.to_string() }
        })
        .into_owned();
    s = FILE_NAME.replace_all(&s, "<file>").into_owned();
    s = LOC.replace_all(&s, "${1}").into_owned();
    s = IP.replace_all(&s, "<ip>").into_owned();
    s = ISO_DATE.replace_all(&s, "<date>").into_owned();
    s = DMY_DATE.replace_all(&s, "<date>").into_owned();
    s = TIME.replace_all(&s, "<time>").into_owned();
    s = HEX_0X
        .replace_all(&s, |c: &Captures| {
            let m = &c[0];
            if HRESULT.is_match(m) { m.to_string() } else { "<hex>".to_string() }
        })
        .into_owned();
    s = HEX_BARE
        .replace_all(&s, |c: &Captures| {
            let m = &c[0];
            let has_digit = m.chars().any(|ch| ch.is_ascii_digit());
            let has_alpha = m.chars().any(|ch| ch.is_ascii_alphabetic());
            if has_digit && has_alpha { "<hex>".to_string() } else { m.to_string() }
        })
        .into_owned();
    s = NUM.replace_all(&s, "<n>").into_owned();
    s.trim().trim_end_matches(['.', '!', ';', ',', ':']).trim().to_string()
}

fn normalize(text: &str, keep_quotes: bool) -> String {
    let no_ansi = ANSI.replace_all(text, "");
    let folded = fold(&no_ansi);
    let mut lines: Vec<String> = Vec::new();
    for raw in folded.lines() {
        let l = raw.trim();
        if l.is_empty() || is_noise_line(l) {
            continue;
        }
        let n = normalize_line(l, keep_quotes);
        if n.is_empty() || lines.contains(&n) {
            continue;
        }
        lines.push(n);
        if lines.len() >= MAX_LINES {
            break;
        }
    }
    // схлопнуть пробелы и подряд идущие одинаковые заглушки («<path> <path>» от пути с пробелом)
    let mut out: Vec<&str> = Vec::new();
    let joined = lines.join(" ");
    for tok in joined.split_whitespace() {
        let placeholder = tok.starts_with('<') && tok.ends_with('>');
        if placeholder && out.last() == Some(&tok) {
            continue;
        }
        out.push(tok);
    }
    let s = out.join(" ");
    crate::text::truncate_chars(&s, MAX_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS_GTI: &str = "gti : The term 'gti' is not recognized as the name of a cmdlet, function, script file, or operable program. Check the spelling of the name, or if a path was included, verify that the path is correct and try again.\r\nAt line:1 char:1\r\n+ gti status\r\n+ ~~~\r\n    + CategoryInfo          : ObjectNotFound: (gti:String) [], CommandNotFoundException\r\n    + FullyQualifiedErrorId : CommandNotFoundException\r\n";
    const PS_COMIT: &str = "comit : The term 'comit' is not recognized as the name of a cmdlet, function, script file, or operable program. Check the spelling of the name, or if a path was included, verify that the path is correct and try again.\nAt line:3 char:12\n+ comit -m \"fix bug 42\"\n+ ~~~~~\n    + CategoryInfo          : ObjectNotFound: (comit:String) [], CommandNotFoundException\n    + FullyQualifiedErrorId : CommandNotFoundException\n";

    /// Пары «одинаковые по сути» → одна сигнатура.
    const SAME: &[(&str, &str)] = &[
        (PS_GTI, PS_COMIT),
        (
            "git: 'comit' is not a git command. See 'git --help'.",
            "git: 'stauts' is not a git command. See 'git --help'.",
        ),
        (
            "Cannot find path 'C:\\Users\\bob\\file.txt' because it does not exist.",
            "Cannot find path 'D:\\work\\x\\y.log' because it does not exist.",
        ),
        (
            "open C:\\Users\\alice\\Documents\\report.docx: Access is denied.",
            "open D:\\data\\c.txt: Access is denied.",
        ),
        (
            "bash: /usr/local/bin/foo: No such file or directory",
            "bash: /opt/bar/baz: No such file or directory",
        ),
        (
            "Connection refused on port 8080 after 3 retries",
            "Connection refused on port 443 after 10 retries",
        ),
        (
            "Segmentation fault at address 0x00007ff6a1b2c3d4",
            "Segmentation fault at address 0xdeadbeef01",
        ),
        (
            "Failed to load component {6F9619FF-8B86-D011-B42D-00C04FC964FF}",
            "Failed to load component {123e4567-e89b-12d3-a456-426614174000}",
        ),
        (
            "[2024-05-01 12:30:45] ERROR database timeout",
            "[2025-11-22 08:01:02.123] ERROR database timeout",
        ),
        (
            "error[E0425]: cannot find value `x` in this scope\n --> src/main.rs:3:5\n  |\n3 |     x + 1\n  |     ^ not found in this scope",
            "error[E0425]: cannot find value `counter` in this scope\n --> src\\lib.rs:120:17\n    |\n120 |     counter += 1;\n    |     ^^^^^^^ not found in this scope",
        ),
        (
            "ModuleNotFoundError: No module named 'requests'",
            "ModuleNotFoundError: No module named 'numpy'",
        ),
        (
            "Request timed out connecting to 192.168.1.10:8443",
            "Request timed out connecting to 10.0.0.5:80",
        ),
        ("ACCESS   DENIED", "access denied"),
        ("\x1b[31merror\x1b[0m: build failed", "error: build failed."),
        (
            "Failed to fetch https://example.com/a?b=1: HTTP 503",
            "Failed to fetch https://other.org/x: HTTP 502",
        ),
        (
            "npm ERR! code ENOENT\nnpm ERR! path C:\\Users\\bob\\proj\\package.json",
            "npm ERR! code ENOENT\nnpm ERR! path C:\\Program Files\\other app\\package.json",
        ),
        ("Файл не найдён", "файл не найден"),
        (
            "Отказано в доступе по пути «C:\\Windows\\System32\\x.dll».",
            "Отказано в доступе по пути «D:\\a.dll»",
        ),
        ("Operation timed out after 30000 ms", "Operation timed out after 15 ms"),
        (
            "Traceback (most recent call last):\n  File \"C:\\app\\main.py\", line 10, in <module>\nKeyError: 'name'",
            "Traceback (most recent call last):\n  File \"/home/u/x.py\", line 99, in <module>\nKeyError: 'id'",
        ),
        (
            "Remove-Item : Cannot remove item C:\\tmp\\a.txt: The process cannot access the file because it is being used by another process.",
            "Remove-Item : Cannot remove item D:\\b\\c.log: The process cannot access the file because it is being used by another process.",
        ),
        ("error: could not compile `jarvis-core` (lib) due to 3 previous errors", "error: could not compile `foo` (lib) due to 1 previous errors"),
        ("Не удалось подключиться к user@example.com", "Не удалось подключиться к admin@corp.local"),
        ("Commit a1b2c3d4e5 not found", "Commit 0f9e8d7c6b5a not found"),
    ];

    /// Разные ошибки → разные сигнатуры.
    const DIFFERENT: &[(&str, &str)] = &[
        ("Access is denied.", "The system cannot find the file specified."),
        ("error[E0425]: cannot find value `x` in this scope", "error[E0308]: mismatched types"),
        (
            "Windows Update failed with error 0x80070002",
            "Windows Update failed with error 0x800f0922",
        ),
        ("git: 'comit' is not a git command.", "fatal: not a git repository (or any of the parent directories): .git"),
        (PS_GTI, "Remove-Item : Access to the path 'C:\\x' is denied."),
        ("ModuleNotFoundError: No module named 'x'", "ImportError: cannot import name 'x'"),
        ("connect ECONNREFUSED 127.0.0.1:5432", "connect ETIMEDOUT 127.0.0.1:5432"),
        ("There is not enough space on the disk.", "Permission denied"),
        (
            "CS0103: The name 'x' does not exist in the current context",
            "CS0246: The type or namespace name 'x' could not be found",
        ),
        ("Отказано в доступе", "Не удаётся найти указанный файл"),
        ("Exception code 0xC0000005", "Exception code 0xC0000409"),
    ];

    #[test]
    fn same_errors_share_signature() {
        for (i, (a, b)) in SAME.iter().enumerate() {
            assert_eq!(
                error_signature(a),
                error_signature(b),
                "пара #{i}: {:?} vs {:?}",
                normalize_error(a),
                normalize_error(b)
            );
        }
    }

    #[test]
    fn different_errors_have_different_signatures() {
        for (i, (a, b)) in DIFFERENT.iter().enumerate() {
            assert_ne!(error_signature(a), error_signature(b), "пара #{i}: {:?}", normalize_error(a));
        }
    }

    #[test]
    fn enough_table_cases() {
        assert!(SAME.len() + DIFFERENT.len() >= 15);
    }

    #[test]
    fn signature_shape() {
        let s = error_signature("anything");
        assert_eq!(s.len(), 16);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(error_signature(""), error_signature("   \n  "));
    }

    #[test]
    fn exact_signature_keeps_quoted_names() {
        let a = ErrorSig::of(PS_GTI);
        let b = ErrorSig::of(PS_COMIT);
        assert_eq!(a.signature, b.signature);
        assert_ne!(a.exact, b.exact);
        // но пути и числа в точной сигнатуре всё равно нормализуются
        let c = ErrorSig::of("Cannot find path 'C:\\a\\b.txt' (line 3)");
        let d = ErrorSig::of("Cannot find path 'D:\\zz.txt' (line 9)");
        assert_eq!(c.exact, d.exact);
    }

    #[test]
    fn normalized_text_is_readable() {
        let n = normalize_error(PS_GTI);
        assert!(n.starts_with("<cmd> : the term <q> is not recognized"), "{n}");
        assert!(!n.contains("gti"), "{n}");
        assert!(n.contains("commandnotfoundexception"));
        assert!(!n.contains("  "));
        let r = normalize_error("error[E0425]: cannot find value `x`\n --> src/main.rs:3:5");
        assert!(r.contains("e0425"), "{r}");
        assert!(r.contains("<path>") && !r.contains(":3"), "{r}");
    }

    #[test]
    fn long_text_is_bounded() {
        let long = "ошибка ".repeat(500);
        assert!(normalize_error(&long).chars().count() <= MAX_CHARS);
    }
}
