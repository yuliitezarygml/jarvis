//! Путь файла в репозитории → URL на learn.microsoft.com по правилам из `kb_sources.json`.
//!
//! `WindowsServerDocs/administration/windows-commands/robocopy.md`
//!   → `https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/robocopy`
//! `reference/7.6/Microsoft.PowerShell.Management/Get-Service.md`
//!   → `…/powershell/module/microsoft.powershell.management/get-service?view=powershell-7.6`
//! `…/index.md` → URL каталога со слэшем; с `landing_dir` `Defender/Defender.md` → `…/defender/`.

use crate::config::UrlCfg;

pub fn build_url(cfg: &UrlCfg, rel_path: &str) -> Option<String> {
    let rel = rel_path.replace('\\', "/");
    let (mapped, query) = if cfg.rules.is_empty() {
        (rel.clone(), String::new())
    } else {
        let rule = cfg
            .rules
            .iter()
            .find(|r| rel.len() >= r.from.len() && rel[..r.from.len()].eq_ignore_ascii_case(&r.from))?;
        (format!("{}{}", rule.to, &rel[rule.from.len()..]), rule.query.clone())
    };

    let mut path = mapped;
    if !cfg.keep_ext {
        if let Some(p) = path.strip_suffix(".md").or_else(|| path.strip_suffix(".yml")) {
            path = p.to_string();
        }
        let (dir, stem) = match path.rfind('/') {
            Some(i) => (path[..=i].to_string(), path[i + 1..].to_string()),
            None => (String::new(), path.clone()),
        };
        let parent_name = dir.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        if stem.eq_ignore_ascii_case("index") || (cfg.landing_dir && !parent_name.is_empty() && stem.eq_ignore_ascii_case(parent_name)) {
            path = dir;
        }
    }
    if cfg.lowercase {
        path = path.to_lowercase();
    }
    let mut url = format!("{}{}", cfg.base, path);
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query);
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UrlRule;

    fn cfg(rules: &[(&str, &str, &str)], landing: bool) -> UrlCfg {
        UrlCfg {
            base: "https://learn.microsoft.com/en-us/".into(),
            rules: rules.iter().map(|(f, t, q)| UrlRule { from: f.to_string(), to: t.to_string(), query: q.to_string() }).collect(),
            lowercase: true,
            keep_ext: false,
            landing_dir: landing,
        }
    }

    #[test]
    fn windows_commands() {
        let c = cfg(&[("WindowsServerDocs/", "windows-server/", "")], false);
        assert_eq!(
            build_url(&c, "WindowsServerDocs/administration/windows-commands/robocopy.md").unwrap(),
            "https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/robocopy"
        );
        assert_eq!(
            build_url(&c, "WindowsServerDocs\\administration\\windows-commands\\Netsh-Winsock.md").unwrap(),
            "https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/netsh-winsock"
        );
        assert!(build_url(&c, "other/x.md").is_none());
    }

    #[test]
    fn powershell_versions_and_landing() {
        let c = cfg(
            &[
                ("reference/docs-conceptual/", "powershell/scripting/", ""),
                ("reference/7.6/", "powershell/module/", "view=powershell-7.6"),
                ("reference/5.1/", "powershell/module/", "view=powershell-5.1"),
            ],
            true,
        );
        assert_eq!(
            build_url(&c, "reference/7.6/Microsoft.PowerShell.Management/Get-Service.md").unwrap(),
            "https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/get-service?view=powershell-7.6"
        );
        assert_eq!(
            build_url(&c, "reference/5.1/Microsoft.PowerShell.Core/About/about_Execution_Policies.md").unwrap(),
            "https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies?view=powershell-5.1"
        );
        assert_eq!(
            build_url(&c, "reference/7.6/Microsoft.PowerShell.Management/Microsoft.PowerShell.Management.md").unwrap(),
            "https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/?view=powershell-7.6"
        );
        assert_eq!(
            build_url(&c, "reference/docs-conceptual/learn/ps101/01-getting-started.md").unwrap(),
            "https://learn.microsoft.com/en-us/powershell/scripting/learn/ps101/01-getting-started"
        );
    }

    #[test]
    fn index_pages_become_directories() {
        let c = cfg(&[("sysinternals/", "sysinternals/", "")], false);
        assert_eq!(build_url(&c, "sysinternals/index.md").unwrap(), "https://learn.microsoft.com/en-us/sysinternals/");
        let c = cfg(&[("windows/", "windows/", "")], false);
        assert_eq!(
            build_url(&c, "windows/security/operating-system-security/network-security/windows-firewall/index.md").unwrap(),
            "https://learn.microsoft.com/en-us/windows/security/operating-system-security/network-security/windows-firewall/"
        );
    }

    #[test]
    fn local_seed_keeps_extension() {
        let c = UrlCfg { base: String::new(), rules: vec![UrlRule { from: String::new(), to: "kb/seed/".into(), query: String::new() }], keep_ext: true, ..Default::default() };
        assert_eq!(build_url(&c, "services.md").unwrap(), "kb/seed/services.md");
    }

    #[test]
    fn win32_lowercases_folders() {
        let c = cfg(&[("desktop-src/", "windows/win32/", "")], false);
        assert_eq!(
            build_url(&c, "desktop-src/WinAuto/entry-uiauto-win32.md").unwrap(),
            "https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32"
        );
    }
}
