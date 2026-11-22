//! Минимальные glob-шаблоны для include/exclude: `**` — любые каталоги, `*` — внутри сегмента, `?` — один символ.
//! Сравнение без учёта регистра (Windows).

pub fn matches(pattern: &str, path: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().replace('\\', "/").chars().collect();
    let s: Vec<char> = path.to_lowercase().replace('\\', "/").chars().collect();
    m(&p, &s)
}

fn m(p: &[char], s: &[char]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some('*') if p.get(1) == Some(&'*') => {
            // `**/` может съесть ноль каталогов
            let rest = if p.get(2) == Some(&'/') { &p[3..] } else { &p[2..] };
            (0..=s.len()).any(|i| (i == 0 || s[i - 1] == '/' || p.get(2) != Some(&'/')) && m(rest, &s[i..]))
        }
        Some('*') => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != '/').any(|i| m(&p[1..], &s[i..])),
        Some('?') => !s.is_empty() && s[0] != '/' && m(&p[1..], &s[1..]),
        Some(c) => !s.is_empty() && s[0] == *c && m(&p[1..], &s[1..]),
    }
}

/// Файл проходит, если совпал хотя бы с одним include (пустой список — все) и ни с одним exclude.
pub fn allowed(path: &str, include: &[String], exclude: &[String]) -> bool {
    (include.is_empty() || include.iter().any(|g| matches(g, path))) && !exclude.iter().any(|g| matches(g, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(matches("**/*.md", "a/b/c.md"));
        assert!(matches("**/*.md", "c.md"));
        assert!(matches("windows/**/*.md", "windows/security/x.md"));
        assert!(matches("windows/**/*.md", "windows/x.md"));
        assert!(!matches("windows/*.md", "windows/security/x.md"));
        assert!(matches("**/includes/**", "windows/security/includes/a.md"));
        assert!(!matches("**/includes/**", "windows/security/include.md"));
        assert!(matches("windows/client-management/mdm/**", "windows/client-management/mdm/policy-csp.md"));
        assert!(matches("WindowsServerDocs/**/*.MD", "windowsserverdocs/administration/x.md"));
        assert!(matches("a?c.md", "abc.md"));
        assert!(!matches("a?c.md", "a/c.md"));
    }

    #[test]
    fn include_exclude() {
        let inc = vec!["**/*.md".to_string()];
        let exc = vec!["**/media/**".to_string()];
        assert!(allowed("a/b.md", &inc, &exc));
        assert!(!allowed("a/media/b.md", &inc, &exc));
        assert!(!allowed("a/b.png", &inc, &exc));
        assert!(allowed("anything", &[], &[]));
    }
}
