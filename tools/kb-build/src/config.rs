//! `kb_sources.json`: какие репозитории клонировать, какие подпути брать и как строить URL.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct SourcesFile {
    #[serde(default)]
    pub version: u32,
    pub sources: Vec<SourceCfg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[default]
    Git,
    /// Папка в проекте (`kb/seed`).
    Local,
}

/// Откуда брать «основную» команду документа (страница самой команды → буст в `kb_lookup_command`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandsFrom {
    #[default]
    None,
    /// Заголовок — командлет `Verb-Noun` или `about_*` (PowerShell).
    CmdletTitle,
    /// Заголовок — имя exe (`robocopy`, `sc.exe config`) — справочник windows-commands.
    ExeTitle,
    /// Имя файла — имя exe (`procmon.md`) — Sysinternals.
    Filename,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UrlRule {
    /// Префикс пути в репозитории (`WindowsServerDocs/`); пустая строка — любой путь.
    pub from: String,
    /// Чем заменить префикс в URL (`windows-server/`).
    pub to: String,
    /// Строка запроса без `?` (`view=powershell-7.6`).
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UrlCfg {
    /// `https://learn.microsoft.com/en-us/`; пусто — URL относительный (шпаргалки).
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub rules: Vec<UrlRule>,
    /// URL в нижнем регистре (канонический вид learn.microsoft.com).
    #[serde(default)]
    pub lowercase: bool,
    /// Не отрезать `.md`.
    #[serde(default)]
    pub keep_ext: bool,
    /// `Module/Module.md` → `module/` (страница модуля PowerShell).
    #[serde(default)]
    pub landing_dir: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceCfg {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: SourceKind,
    #[serde(default)]
    pub repo: String,
    /// Запасные адреса (форки/зеркала), пробуются по порядку, если `repo` недоступен.
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub branch: String,
    /// Каталоги для `git sparse-checkout set` (cone).
    #[serde(default)]
    pub sparse: Vec<String>,
    /// Для `kind: local` — путь от корня проекта.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub url: UrlCfg,
    #[serde(default)]
    pub commands: CommandsFrom,
    /// Префиксы путей, для которых работает `commands` (пусто — все).
    #[serde(default)]
    pub command_scope: Vec<String>,
    /// Дополнительные имена команды для файла: `"…/process-explorer.md": ["procexp"]`.
    #[serde(default)]
    pub command_aliases: BTreeMap<String, Vec<String>>,
    /// Версии справочника по приоритету: файл из второй версии пропускается, если тот же путь есть в первой.
    #[serde(default)]
    pub dedupe_versions: Vec<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl SourceCfg {
    /// `repo`, затем `mirrors`, без пустых и повторов.
    pub fn candidate_urls(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for u in std::iter::once(&self.repo).chain(&self.mirrors) {
            if !u.is_empty() && !v.contains(u) {
                v.push(u.clone());
            }
        }
        v
    }
}

pub fn load(path: &Path) -> Result<SourcesFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let f: SourcesFile = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut ids = std::collections::HashSet::new();
    for s in &f.sources {
        if !ids.insert(s.id.as_str()) {
            return Err(format!("дубликат id источника: {}", s.id));
        }
        if s.kind == SourceKind::Git && (s.repo.is_empty() || s.branch.is_empty() || s.sparse.is_empty()) {
            return Err(format!("{}: для git нужны repo, branch и sparse", s.id));
        }
        if s.kind == SourceKind::Local && s.path.is_empty() {
            return Err(format!("{}: для local нужен path", s.id));
        }
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_sources_file_is_valid() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("kb_sources.json");
        let f = load(&p).unwrap();
        let ids: Vec<_> = f.sources.iter().map(|s| s.id.as_str()).collect();
        for want in ["windows-commands", "powershell", "sysinternals", "win32", "windows-client", "seed"] {
            assert!(ids.contains(&want), "нет источника {want}");
        }
        let wc = f.sources.iter().find(|s| s.id == "windows-client").unwrap();
        assert!(wc.candidate_urls().len() >= 2, "нужны зеркала для закрытого windows-itpro-docs");
        assert!(f.sources.iter().all(|s| s.kind == SourceKind::Local || s.url.base.starts_with("https://learn.microsoft.com/")));
    }

    #[test]
    fn rejects_bad_config() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        std::fs::write(&p, r#"{"sources":[{"id":"a","name":"A","repo":"x","branch":"main","url":{}}]}"#).unwrap();
        assert!(load(&p).unwrap_err().contains("sparse"));
        std::fs::write(&p, r#"{"sources":[{"id":"a","name":"A","kind":"local","path":"p","url":{}},{"id":"a","name":"B","kind":"local","path":"q","url":{}}]}"#).unwrap();
        assert!(load(&p).unwrap_err().contains("дубликат"));
    }
}
