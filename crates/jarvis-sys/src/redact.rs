//! Вырезание секретов перед отправкой текста (вывод терминала, файлы, буфер обмена) в облачную модель
//! (план, раздел 3г). Заменяет секрет на `[REDACTED:вид]`, оставляя имя ключа (`password=[REDACTED:password]`).

use regex::{Captures, Regex};
use std::sync::LazyLock;

/// Маркер замены. Файловые инструменты отказываются записывать текст с этим маркером,
/// чтобы модель случайно не затёрла настоящий секрет.
pub const MARKER: &str = "[REDACTED:";

struct Rule {
    kind: &'static str,
    re: Regex,
    /// Номер группы с самим секретом (0 — всё совпадение).
    group: usize,
}

fn rule(kind: &'static str, re: &str, group: usize) -> Rule {
    Rule { kind, re: Regex::new(re).expect("regex"), group }
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        // блоки приватных ключей (PEM/OpenSSH/PGP)
        rule(
            "private_key",
            r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----[\s\S]*?-----END (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----",
            0,
        ),
        // JWT: header.payload.signature, header и payload — base64url JSON (начинаются с eyJ)
        rule("jwt", r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}", 0),
        rule("groq", r"\bgsk_[A-Za-z0-9]{20,}", 0),
        rule("github", r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}", 0),
        rule("github", r"\bgithub_pat_[A-Za-z0-9_]{40,}", 0),
        rule("anthropic", r"\bsk-ant-[A-Za-z0-9_-]{20,}", 0),
        rule("openai", r"\bsk-(?:proj|svcacct|admin)-[A-Za-z0-9_-]{20,}", 0),
        rule("openai", r"\bsk-[A-Za-z0-9]{32,}", 0),
        rule("openrouter", r"\bsk-or-v1-[A-Za-z0-9]{32,}", 0),
        rule("slack", r"\bxox[abposr]-[A-Za-z0-9-]{10,}", 0),
        rule("aws_key", r"\b(?:AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}\b", 0),
        rule(
            "aws_secret",
            r#"(?i)\b(?:aws_?secret_?(?:access_?)?key|secret_?access_?key)\b["']?\s*[:=]\s*["']?([A-Za-z0-9/+=]{40})"#,
            1,
        ),
        rule("google_api_key", r"\bAIza[0-9A-Za-z_-]{35}", 0),
        rule("stripe", r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{20,}", 0),
        rule("hf", r"\bhf_[A-Za-z0-9]{30,}", 0),
        // Authorization: Bearer …/Basic …
        rule("bearer", r"(?i)\bbearer\s+([A-Za-z0-9._~+/=-]{12,})", 1),
        rule("basic_auth", r"(?i)\bauthorization\s*:\s*basic\s+([A-Za-z0-9+/=]{8,})", 1),
        // пароль в URL: scheme://user:pass@host
        rule("url_password", r"(?i)\b[a-z][a-z0-9+.-]*://[^\s:/@]+:([^\s@/]{1,})@", 1),
    ]
});

/// `password=…`, `pwd=…`, `token: "…"`, `$env:OPENAI_API_KEY = "…"`, `--password x`.
static KV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        (?P<key>
            (?:\$env:|%)?
            [A-Za-z0-9_.-]*?
            (?:password|passwd|pwd|pass|passphrase|secret|token|api[_-]?key|apikey|access[_-]?key|private[_-]?key|client[_-]?secret|auth[_-]?token|пароль|токен)
            [A-Za-z0-9_-]*
            %?
        )
        ["']?
        (?P<sep>\s*[=:]\s*|\s+)
        (?P<val>"[^"\r\n]*"|'[^'\r\n]*'|[^\s&;,'"<>)\]}]+)
        "#,
    )
    .expect("regex")
});

/// Слова-значения, которые не являются секретами.
fn is_placeholder(v: &str) -> bool {
    let t = v.trim_matches(['"', '\'']);
    let l = t.to_lowercase();
    t.is_empty()
        || t.starts_with(MARKER)
        || t.starts_with('$')
        || t.starts_with("%")
        || t.starts_with("${")
        || t.starts_with("{{")
        || t.starts_with('<')
        || t.chars().all(|c| c == '*' || c == '•' || c == 'x' || c == 'X')
        || matches!(
            l.as_str(),
            "true" | "false" | "null" | "none" | "nil" | "yes" | "no" | "on" | "off" | "required" | "optional"
                | "string" | "str" | "undefined" | "redacted" | "hidden" | "env" | "secret" | "password" | "token"
                | "(convertto-securestring" | "read-host"
        )
}

/// Похоже ли значение на секрет (для разделителя «пробел» и «:» требуем больше признаков).
fn looks_secret(v: &str, sep: &str) -> bool {
    let t = v.trim_matches(['"', '\'']);
    if is_placeholder(t) {
        return false;
    }
    let quoted = v.starts_with(['"', '\'']);
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let has_sym = t.chars().any(|c| !c.is_alphanumeric());
    let mixed = t.chars().any(|c| c.is_uppercase()) && t.chars().any(|c| c.is_lowercase());
    if sep.contains('=') {
        return true;
    }
    if sep.contains(':') {
        return quoted || (t.chars().count() >= 6 && (has_digit || has_sym || mixed));
    }
    // разделитель-пробел: `--password hunter2`, `-Token abc123`
    t.chars().count() >= 6 && (has_digit || has_sym || mixed)
}

/// Вырезать секреты. Возвращает новый текст и число замен.
pub fn redact_secrets(text: &str) -> (String, usize) {
    let mut count = 0usize;
    let mut out = text.to_string();
    for r in RULES.iter() {
        if !r.re.is_match(&out) {
            continue;
        }
        let replaced = r
            .re
            .replace_all(&out, |c: &Captures| {
                let whole = c.get(0).unwrap();
                let Some(m) = c.get(r.group) else { return whole.as_str().to_string() };
                if m.as_str().starts_with(MARKER) {
                    return whole.as_str().to_string();
                }
                count += 1;
                let s = whole.as_str();
                let (a, b) = (m.start() - whole.start(), m.end() - whole.start());
                format!("{}{MARKER}{}]{}", &s[..a], r.kind, &s[b..])
            })
            .into_owned();
        out = replaced;
    }
    let replaced = KV
        .replace_all(&out, |c: &Captures| {
            let whole = c.get(0).unwrap().as_str();
            let key = &c["key"];
            let sep = &c["sep"];
            let val = &c["val"];
            // пробел как разделитель — только для флагов командной строки (`--password x`, `-Token x`)
            if !sep.contains(['=', ':']) {
                let start = c.get(0).unwrap().start();
                let flag = key.starts_with('-') || out[..start].ends_with('-');
                if !flag {
                    return whole.to_string();
                }
            }
            if !looks_secret(val, sep) {
                return whole.to_string();
            }
            count += 1;
            let kind = key.trim_matches(['%', '$']).trim_start_matches("env:").to_lowercase();
            let q = if val.starts_with('"') {
                "\""
            } else if val.starts_with('\'') {
                "'"
            } else {
                ""
            };
            let prefix = &whole[..whole.len() - val.len()];
            format!("{prefix}{q}{MARKER}{}]{q}", short_kind(&kind))
        })
        .into_owned();
    out = replaced;
    (out, count)
}

fn short_kind(k: &str) -> &'static str {
    if k.contains("pass") || k.contains("pwd") || k.contains("пароль") {
        "password"
    } else if k.contains("token") || k.contains("токен") {
        "token"
    } else if k.contains("key") {
        "key"
    } else {
        "secret"
    }
}

/// Есть ли в тексте маркер вырезанного секрета.
pub fn contains_redaction(text: &str) -> bool {
    text.contains(MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (текст, должно ли что-то быть вырезано, подстрока, которой не должно остаться)
    const POSITIVE: &[(&str, &str)] = &[
        (concat!("GROQ_API_KEY=gs", "k_abcdefghijklmnopqrstuvwxyz0123456789ABCD"), concat!("gs", "k_abcdefghij")),
        (concat!("token gh", "p_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab в выводе"), concat!("gh", "p_ABCDEF")),
        (concat!("gh", "o_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab"), concat!("gh", "o_ABCDEF")),
        (concat!("github", "_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOP"), concat!("github", "_pat_11ABC")),
        (concat!("OPENAI_API_KEY: s", "k-proj-AbCdEfGhIjKlMnOpQrStUvWxYz_0123456789-abc"), concat!("s", "k-proj-AbCd")),
        (concat!("key = s", "k-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKL"), concat!("s", "k-abcdefghijklmnop")),
        (concat!("x-api-key: s", "k-ant-api03-AbCdEfGhIjKlMnOpQrStUvWx-yz0123456789"), concat!("s", "k-ant-api03")),
        // Фейковые токены собраны через concat!, чтобы сканер секретов GitHub не принимал их за настоящие.
        (concat!("SLACK=xo", "xb-123456789012-1234567890123-AbCdEfGhIjKlMnOp"), concat!("xo", "xb-1234")),
        (concat!("xo", "xp-123456789012-123456789012-abcdef"), concat!("xo", "xp-1234")),
        (concat!("aws_access_key_id = AK", "IAIOSFODNN7EXAMPLE"), concat!("AK", "IAIOSFODNN7EXAMPLE")),
        (concat!("aws_secret_access_key = wJalrXUtnFEMI", "/K7MDENG/bPxRfiCYEXAMPLEKEY"), "wJalrXUtnFEMI"),
        (concat!("https://maps.googleapis.com/x?key=AI", "zaSyA1234567890abcdefghijklmnopqrstuv"), concat!("AI", "zaSyA12345")),
        ("Authorization: Bearer abc.def-ghi_jkl012345", "abc.def-ghi"),
        ("curl -H \"Authorization: Basic dXNlcjpwYXNzd29yZA==\" x", "dXNlcjpw"),
        (concat!("jwt eyJhbGciOiJIUzI1NiJ9.", "eyJzdWIiOiIxMjM0NTY3ODkwIn0.", "dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"), "eyJzdWIi"),
        ("password=hunter2", "hunter2"),
        ("pwd=Qwerty123;", "Qwerty123"),
        ("https://site.ru/login?user=bob&token=a1b2c3d4e5&x=1", "a1b2c3d4e5"),
        ("export SECRET=s3cr3t-value", "s3cr3t-value"),
        ("$env:OPENAI_API_KEY = \"my-very-secret-key-123\"", "my-very-secret-key-123"),
        ("set DB_PASSWORD=P@ssw0rd!", "P@ssw0rd!"),
        ("Server=db;Database=app;User Id=sa;Password=Sup3rS3cret;", "Sup3rS3cret"),
        ("postgres://admin:p4ssw0rd@db.local:5432/app", "p4ssw0rd"),
        ("mongodb+srv://user:hunter22@cluster0.example.net/db", "hunter22"),
        ("{\"password\": \"correct horse battery\"}", "correct horse"),
        ("client_secret: 'abcDEF123456'", "abcDEF123456"),
        ("mysql -u root --password=Root2024 db", "Root2024"),
        ("Connect-X -Password Xyz12345", "Xyz12345"),
        ("пароль: Qwerty2024!", "Qwerty2024"),
        (
            concat!("-----BEGIN OPENSSH PRIV", "ATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nAAAAB3NzaC1yc2E=\n-----END OPENSSH PRIV", "ATE KEY-----"),
            "b3BlbnNzaC1rZXkt",
        ),
        (concat!("-----BEGIN RSA PRIV", "ATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIV", "ATE KEY-----"), "MIIEpAIB"),
        (concat!("HF_TOKEN=h", "f_AbCdEfGhIjKlMnOpQrStUvWxYz01234567"), concat!("h", "f_AbCdEf")),
    ];

    const NEGATIVE: &[&str] = &[
        "Привет, сэр! Сегодня 22 ноября, всё работает.",
        "commit a94a8fe5ccb19ba61c4c0873d391e987982fbbd3 Author: Bob",
        "git log: 5949efc feat: implement scheduler for TTS jobs",
        "uuid 123e4567-e89b-12d3-a456-426614174000",
        "sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "task-manager-is-a-tool and risk-assessment works",
        "Скопируйте ключ из настроек провайдера и вставьте его в поле.",
        "Enter your password below to continue",
        "The password field is required",
        "password: required",
        "token = $env:GITHUB_TOKEN",
        "Get-Credential -Message 'password'",
        "if ($token) { Write-Host 'ok' }",
        "C:\\Users\\test\\Documents\\passwords.txt не найден",
        "skeleton sk-learn style naming",
        "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq\n-----END PUBLIC KEY-----",
        "error[E0382]: borrow of moved value: `text`",
        "Bearer token is missing",
    ];

    #[test]
    fn positives() {
        for (text, secret) in POSITIVE {
            let (out, n) = redact_secrets(text);
            assert!(n >= 1, "ничего не вырезано: {text}");
            assert!(!out.contains(secret), "секрет остался: {text} → {out}");
            assert!(out.contains(MARKER), "{out}");
        }
    }

    #[test]
    fn negatives() {
        for text in NEGATIVE {
            let (out, n) = redact_secrets(text);
            assert_eq!((out.as_str(), n), (*text, 0), "ложное срабатывание");
        }
    }

    #[test]
    fn keeps_key_names_and_counts() {
        let (out, n) = redact_secrets("a password=x1 b token=y2y2 c");
        assert_eq!(n, 2);
        assert_eq!(out, "a password=[REDACTED:password] b token=[REDACTED:token] c");
        let (out, n) = redact_secrets(concat!("GROQ_API_KEY=gs", "k_abcdefghijklmnopqrstuvwxyz0123"));
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "GROQ_API_KEY=[REDACTED:groq]");
        // повторный проход не меняет текст
        assert_eq!(redact_secrets(&out), (out.clone(), 0));
        assert!(contains_redaction(&out));
    }
}
