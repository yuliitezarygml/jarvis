//! Клоны источников: `git clone --depth 1 --filter=blob:none --sparse` + `git sparse-checkout set --cone …`.
//! Git вызывается без запроса учётных данных (несуществующий репозиторий GitHub иначе спрашивает логин),
//! адреса пробуются по порядку: `repo`, затем `mirrors`.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneInfo {
    pub url: String,
    pub commit: String,
    pub commit_date: String,
    /// Что происходило: «клонирован», «обновлён», «оставлен как есть», неудачные адреса.
    pub notes: Vec<String>,
}

fn git_cmd(cwd: Option<&Path>) -> Command {
    let mut c = Command::new("git");
    if let Some(d) = cwd {
        c.arg("-C").arg(d);
    }
    c.args(["-c", "credential.helper=", "-c", "core.askPass=", "-c", "advice.detachedHead=false"]);
    c.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        // оборвать зависшую передачу (меньше 1 КБ/с в течение минуты)
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
        .env("GIT_HTTP_LOW_SPEED_TIME", "60");
    c
}

/// Выполнить git; Ok(stdout) или Err(stderr/сообщение).
pub fn git(cwd: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let out = git_cmd(cwd).args(args).output().map_err(|e| format!("не удалось запустить git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("ошибка git").trim().to_string();
        Err(format!("git {}: {msg}", args.first().copied().unwrap_or("")))
    }
}

/// Существует ли ветка по адресу (быстрая проверка без клонирования).
pub fn remote_has_branch(url: &str, branch: &str) -> Result<(), String> {
    let r = git(None, &["ls-remote", "--exit-code", "--heads", url, branch])?;
    if r.is_empty() { Err(format!("нет ветки {branch}")) } else { Ok(()) }
}

fn head_info(dir: &Path) -> (String, String) {
    let s = git(Some(dir), &["log", "-1", "--format=%H%n%cI"]).unwrap_or_default();
    let mut it = s.lines();
    (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
}

fn set_sparse(dir: &Path, sparse: &[String]) -> Result<(), String> {
    let mut args = vec!["sparse-checkout", "set", "--cone"];
    args.extend(sparse.iter().map(String::as_str));
    git(Some(dir), &args).map(|_| ())
}

/// Гарантировать клон источника в `dir`.
/// * нет клона → перебрать адреса, склонировать первый доступный;
/// * клон есть, `update` → fetch последнего коммита ветки и `reset --hard` (при недоступном origin — зеркала);
/// * клон есть, без `update` → только применить список sparse (новые подпути докачаются).
/// `offline` — сеть не трогаем вообще.
pub fn ensure_clone(dir: &Path, urls: &[String], branch: &str, sparse: &[String], update: bool, offline: bool) -> Result<CloneInfo, String> {
    let mut notes = Vec::new();
    if dir.join(".git").exists() {
        let origin = git(Some(dir), &["remote", "get-url", "origin"]).unwrap_or_default();
        if offline {
            let (commit, date) = head_info(dir);
            notes.push("офлайн: клон оставлен как есть".into());
            return Ok(CloneInfo { url: origin, commit, commit_date: date, notes });
        }
        if let Err(e) = set_sparse(dir, sparse) {
            notes.push(format!("sparse-checkout: {e}"));
        }
        if !update {
            let (commit, date) = head_info(dir);
            notes.push("клон уже есть (обновление: --update)".into());
            return Ok(CloneInfo { url: origin, commit, commit_date: date, notes });
        }
        let mut order: Vec<&String> = urls.iter().filter(|u| **u == origin).collect();
        order.extend(urls.iter().filter(|u| **u != origin));
        for u in order {
            if let Err(e) = remote_has_branch(u, branch) {
                notes.push(format!("{u}: недоступен ({e})"));
                continue;
            }
            if *u != origin {
                git(Some(dir), &["remote", "set-url", "origin", u])?;
                notes.push(format!("origin переключён на {u}"));
            }
            git(Some(dir), &["fetch", "--depth", "1", "--filter=blob:none", "origin", branch])?;
            git(Some(dir), &["reset", "--hard", "FETCH_HEAD"])?;
            let (commit, date) = head_info(dir);
            notes.push("обновлён".into());
            return Ok(CloneInfo { url: u.clone(), commit, commit_date: date, notes });
        }
        // все адреса недоступны — остаёмся на имеющемся клоне
        let (commit, date) = head_info(dir);
        notes.push("обновить не удалось, используется имеющийся клон".into());
        return Ok(CloneInfo { url: origin, commit, commit_date: date, notes });
    }

    if offline {
        return Err("клона нет, а сеть отключена (--offline)".into());
    }
    for u in urls {
        if let Err(e) = remote_has_branch(u, branch) {
            notes.push(format!("{u}: недоступен ({e})"));
            continue;
        }
        if dir.exists() {
            let _ = std::fs::remove_dir_all(dir);
        }
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let d = dir.to_string_lossy().to_string();
        let res = git(None, &["clone", "--depth", "1", "--filter=blob:none", "--sparse", "--branch", branch, u, &d])
            .and_then(|_| set_sparse(dir, sparse));
        match res {
            Ok(()) => {
                let (commit, date) = head_info(dir);
                notes.push(format!("склонирован {u}"));
                return Ok(CloneInfo { url: u.clone(), commit, commit_date: date, notes });
            }
            Err(e) => {
                notes.push(format!("{u}: {e}"));
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
    Err(format!("ни один адрес не доступен: {}", notes.join("; ")))
}

/// Размер каталога (включая .git), байт.
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => total += e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Локальный «удалённый» репозиторий: clone/sparse/update без сети.
    #[test]
    fn sparse_clone_and_update_from_local_repo() {
        if git(None, &["--version"]).is_err() {
            return; // git не установлен — пропускаем
        }
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote");
        std::fs::create_dir_all(remote.join("docs/a")).unwrap();
        std::fs::create_dir_all(remote.join("docs/b")).unwrap();
        std::fs::write(remote.join("docs/a/one.md"), "# One").unwrap();
        std::fs::write(remote.join("docs/b/two.md"), "# Two").unwrap();
        let r = Some(remote.as_path());
        git(r, &["init", "-q", "-b", "main"]).unwrap();
        git(r, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]).unwrap();
        git(r, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "init"]).unwrap();
        git(r, &["config", "uploadpack.allowFilter", "true"]).unwrap();
        // file:// — чтобы работали --depth и --filter
        let url = format!("file:///{}", remote.to_string_lossy().replace('\\', "/").trim_start_matches('/'));
        let bad = format!("{url}-missing");
        let dir = tmp.path().join("clone");

        let info = ensure_clone(&dir, &[bad.clone(), url.clone()], "main", &["docs/a".into()], false, false).unwrap();
        assert_eq!(info.url, url);
        assert_eq!(info.commit.len(), 40);
        assert!(info.notes.iter().any(|n| n.contains("недоступен")));
        assert!(dir.join("docs/a/one.md").exists());
        assert!(!dir.join("docs/b/two.md").exists(), "sparse: docs/b не должен попасть");

        std::fs::write(remote.join("docs/a/one.md"), "# One v2").unwrap();
        git(r, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-am", "v2"]).unwrap();
        let info2 = ensure_clone(&dir, &[url.clone()], "main", &["docs/a".into(), "docs/b".into()], true, false).unwrap();
        assert_ne!(info2.commit, info.commit);
        assert_eq!(std::fs::read_to_string(dir.join("docs/a/one.md")).unwrap(), "# One v2");
        assert!(dir.join("docs/b/two.md").exists());

        let off = ensure_clone(&dir, &[], "main", &[], true, true).unwrap();
        assert_eq!(off.commit, info2.commit);
        assert!(ensure_clone(&tmp.path().join("none"), &[url], "main", &[], false, true).is_err());
        assert!(dir_size(&dir) > 0);
    }
}
