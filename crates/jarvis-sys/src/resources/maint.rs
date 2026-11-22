//! Обслуживание: `temp_cleanup` (T2), `network_info`/`ping` (T0), `dns_flush` (T2), `winget_search` (T0),
//! `winget_install/upgrade/uninstall` (T3), `scheduled_tasks_list` (T0).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use jarvis_core::{CancellationToken, RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde::Serialize;
use serde_json::{Value, json};

use crate::exec::{Args, RunOptions, format_result, run_powershell, run_program};
use crate::undo::{UndoAction, UndoJournal};
use crate::util::{fmt_bytes, opt_bool, opt_str, opt_u64, req_str, spec, truncate_middle};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CleanupStats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    /// Занятые/без доступа — пропущены.
    pub skipped: u64,
}

/// Удалить файлы старше `older_than` в `root` (рекурсивно), затем пустые папки. `dry_run` — только посчитать.
pub fn cleanup_dir(root: &Path, older_than: Duration, dry_run: bool, keep: &[PathBuf]) -> CleanupStats {
    let mut st = CleanupStats::default();
    let cutoff = SystemTime::now() - older_than;
    fn walk(dir: &Path, cutoff: SystemTime, dry: bool, keep: &[PathBuf], st: &mut CleanupStats, depth: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            st.skipped += 1;
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if keep.iter().any(|k| p.starts_with(k)) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if depth < 32 {
                    walk(&p, cutoff, dry, keep, st, depth + 1);
                }
                let empty = std::fs::read_dir(&p).map(|mut r| r.next().is_none()).unwrap_or(false);
                let old = e.metadata().and_then(|m| m.modified()).map(|m| m < cutoff).unwrap_or(false);
                if empty && old && (dry || std::fs::remove_dir(&p).is_ok()) {
                    st.dirs += 1;
                }
                continue;
            }
            let Ok(m) = e.metadata() else {
                st.skipped += 1;
                continue;
            };
            if m.modified().map(|t| t >= cutoff).unwrap_or(true) {
                continue;
            }
            if dry || std::fs::remove_file(&p).is_ok() {
                st.files += 1;
                st.bytes += m.len();
            } else {
                st.skipped += 1;
            }
        }
    }
    walk(root, cutoff, dry_run, keep, &mut st, 0);
    st
}

pub struct TempCleanup(pub Arc<UndoJournal>);

#[async_trait]
impl Tool for TempCleanup {
    fn spec(&self) -> ToolSpec {
        spec(
            "temp_cleanup",
            "Очистить временные файлы пользователя (%TEMP%) старше N часов. include_system=true — ещё C:\\Windows\\Temp (опасно). dry_run=true — только посчитать. Удалённое не возвращается.",
            json!({"type":"object","properties":{"older_than_hours":{"type":"integer","default":24},"dry_run":{"type":"boolean","default":false},"include_system":{"type":"boolean","default":false}}}),
            RiskTier::T2,
        )
    }
    fn tier_for(&self, args: &Value) -> RiskTier {
        if opt_bool(args, "include_system") == Some(true) {
            RiskTier::T3
        } else if opt_bool(args, "dry_run") == Some(true) {
            RiskTier::T0
        } else {
            RiskTier::T2
        }
    }
    async fn call(&self, args: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let hours = opt_u64(&args, "older_than_hours").unwrap_or(24).max(1);
        let dry = opt_bool(&args, "dry_run").unwrap_or(false);
        let mut roots: Vec<PathBuf> = std::env::var_os("TEMP").map(PathBuf::from).into_iter().collect();
        if opt_bool(&args, "include_system") == Some(true) {
            roots.push(PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("Temp"));
        }
        let keep = vec![self.0.dir().to_path_buf()];
        let undo = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut total = CleanupStats::default();
            for r in &roots {
                let s = cleanup_dir(r, Duration::from_secs(hours * 3600), dry, &keep);
                total.files += s.files;
                total.dirs += s.dirs;
                total.bytes += s.bytes;
                total.skipped += s.skipped;
            }
            if !dry && total.files > 0 {
                let _ = undo.record("temp_cleanup", "очистка временных файлов", vec![UndoAction::Note { text: format!("удалено {} файлов", total.files) }]);
            }
            Ok(ToolOutput {
                content: format!(
                    "{} {} файлов, {} папок, {} (пропущено занятых: {})",
                    if dry { "Можно удалить" } else { "Удалено" },
                    total.files,
                    total.dirs,
                    fmt_bytes(total.bytes),
                    total.skipped
                ),
                data: serde_json::to_value(&total).ok(),
                ..Default::default()
            })
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
    }
}

pub struct NetworkInfo;

#[async_trait]
impl Tool for NetworkInfo {
    fn spec(&self) -> ToolSpec {
        spec("network_info", "Сетевые адаптеры: адреса, MAC, трафик.", json!({"type":"object","properties":{}}), RiskTier::T0)
    }
    async fn call(&self, _a: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let v = tokio::task::spawn_blocking(super::networks).await.map_err(|e| ToolError::Failed(e.to_string()))?;
        let lines: Vec<String> = v
            .iter()
            .map(|n| format!("{} ({}): {} — принято {}, отправлено {}", n.name, n.mac, if n.ips.is_empty() { "нет адреса".into() } else { n.ips.join(", ") }, fmt_bytes(n.rx_total), fmt_bytes(n.tx_total)))
            .collect();
        Ok(ToolOutput { content: lines.join("\n"), data: serde_json::to_value(&v).ok(), ..Default::default() })
    }
}

/// Имя хоста/адрес без опасных символов.
pub fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with('-') && h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:_".contains(c))
}

pub async fn ping(host: &str, count: u64, cancel: &CancellationToken) -> Result<String, ToolError> {
    if !valid_host(host) {
        return Err(ToolError::InvalidArgs(format!("недопустимый адрес «{host}»")));
    }
    let args = vec!["-n".to_string(), count.clamp(1, 10).to_string(), host.to_string()];
    let r = run_program("ping.exe", Args::List(&args), &RunOptions { timeout: Duration::from_secs(30), ..Default::default() }, cancel)
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?;
    Ok(format_result(&r, false).0)
}

pub struct Ping;

#[async_trait]
impl Tool for Ping {
    fn spec(&self) -> ToolSpec {
        spec("ping", "Проверить доступность узла (ping).", json!({"type":"object","properties":{"host":{"type":"string"},"count":{"type":"integer","default":4}},"required":["host"]}), RiskTier::T0)
    }
    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let host = req_str(&args, "host")?;
        Ok(ToolOutput::text(ping(&host, opt_u64(&args, "count").unwrap_or(4), &ctx.cancel).await?))
    }
}

pub struct DnsFlush;

#[async_trait]
impl Tool for DnsFlush {
    fn spec(&self) -> ToolSpec {
        spec("dns_flush", "Сбросить кэш DNS (ipconfig /flushdns).", json!({"type":"object","properties":{}}), RiskTier::T2)
    }
    async fn call(&self, _a: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let args = vec!["/flushdns".to_string()];
        let r = run_program("ipconfig.exe", Args::List(&args), &RunOptions::default(), &ctx.cancel).await.map_err(|e| ToolError::Failed(e.to_string()))?;
        if r.exit_code != Some(0) {
            return Err(ToolError::Failed(format_result(&r, false).0));
        }
        Ok(ToolOutput::text("Кэш DNS сброшен."))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WingetAction {
    Search,
    Install,
    Upgrade,
    Uninstall,
}

pub struct Winget(pub WingetAction);

fn valid_pkg(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && !s.starts_with('-') && !s.contains(['"', '&', '|', '<', '>', '^', '\n'])
}

#[async_trait]
impl Tool for Winget {
    fn spec(&self) -> ToolSpec {
        let (name, desc, props, tier) = match self.0 {
            WingetAction::Search => ("winget_search", "Найти программу в winget.", json!({"query":{"type":"string"}}), RiskTier::T0),
            WingetAction::Install => ("winget_install", "Установить программу через winget (опасно — с подтверждением).", json!({"id":{"type":"string","description":"Точный Id пакета из winget_search"}}), RiskTier::T3),
            WingetAction::Upgrade => ("winget_upgrade", "Обновить программу (id) или все (all=true) через winget.", json!({"id":{"type":"string"},"all":{"type":"boolean"}}), RiskTier::T3),
            WingetAction::Uninstall => ("winget_uninstall", "Удалить программу через winget (опасно — с подтверждением).", json!({"id":{"type":"string"}}), RiskTier::T3),
        };
        spec(name, desc, json!({"type":"object","properties":props}), tier)
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let common = ["--accept-source-agreements", "--disable-interactivity"];
        let mut a: Vec<String> = Vec::new();
        let timeout = match self.0 {
            WingetAction::Search => {
                let q = req_str(&args, "query")?;
                if !valid_pkg(&q) {
                    return Err(ToolError::InvalidArgs("недопустимый запрос".into()));
                }
                a.extend(["search".into(), q]);
                60
            }
            act => {
                let all = act == WingetAction::Upgrade && opt_bool(&args, "all") == Some(true);
                a.push(match act {
                    WingetAction::Install => "install",
                    WingetAction::Upgrade => "upgrade",
                    _ => "uninstall",
                }
                .into());
                if all {
                    a.push("--all".into());
                } else {
                    let id = req_str(&args, "id")?;
                    if !valid_pkg(&id) {
                        return Err(ToolError::InvalidArgs("недопустимый id".into()));
                    }
                    a.extend(["--id".into(), id, "--exact".into()]);
                }
                a.push("--silent".into());
                if act != WingetAction::Uninstall {
                    a.push("--accept-package-agreements".into());
                }
                1800
            }
        };
        a.extend(common.iter().map(|s| s.to_string()));
        let r = run_program("winget.exe", Args::List(&a), &RunOptions { timeout: Duration::from_secs(timeout), ..Default::default() }, &ctx.cancel)
            .await
            .map_err(|e| ToolError::Failed(format!("winget: {e}")))?;
        if r.timed_out {
            return Err(ToolError::Timeout);
        }
        // winget рисует прогресс-бары символами \r и псевдографикой — оставляем последние строки
        let mut out = format_result(&r, false).0;
        out = out.lines().filter(|l| !l.trim().chars().all(|c| "-\\|/█▒ ".contains(c))).collect::<Vec<_>>().join("\n");
        Ok(ToolOutput { content: truncate_middle(&out, 8000).0, untrusted: true, data: Some(json!({"exit_code": r.exit_code})), ..Default::default() })
    }
}

pub struct ScheduledTasksList;

#[async_trait]
impl Tool for ScheduledTasksList {
    fn spec(&self) -> ToolSpec {
        spec(
            "scheduled_tasks_list",
            "Задачи планировщика Windows. По умолчанию без системных (\\Microsoft\\).",
            json!({"type":"object","properties":{"include_microsoft":{"type":"boolean","default":false},"filter":{"type":"string"}}}),
            RiskTier::T0,
        )
    }
    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let ms = opt_bool(&args, "include_microsoft").unwrap_or(false);
        let filter = opt_str(&args, "filter").map(|f| f.to_lowercase());
        let script = "Get-ScheduledTask | Select-Object TaskName,TaskPath,@{n='State';e={[string]$_.State}} | ConvertTo-Json -Compress";
        let r = run_powershell(script, &RunOptions { timeout: Duration::from_secs(60), ..Default::default() }, &ctx.cancel)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        #[derive(serde::Deserialize, Serialize)]
        struct T {
            #[serde(rename = "TaskName")]
            name: String,
            #[serde(rename = "TaskPath")]
            path: String,
            #[serde(rename = "State")]
            state: String,
        }
        let t = r.stdout.trim();
        let mut v: Vec<T> = if t.starts_with('[') { serde_json::from_str(t).unwrap_or_default() } else { serde_json::from_str::<T>(t).map(|x| vec![x]).unwrap_or_default() };
        if v.is_empty() && r.exit_code != Some(0) {
            return Err(ToolError::Failed(format_result(&r, false).0));
        }
        if !ms {
            v.retain(|x| !x.path.starts_with("\\Microsoft\\"));
        }
        if let Some(f) = &filter {
            v.retain(|x| x.name.to_lowercase().contains(f) || x.path.to_lowercase().contains(f));
        }
        let lines: Vec<String> = v.iter().take(200).map(|x| format!("{}{} [{}]", x.path, x.name, x.state)).collect();
        Ok(ToolOutput { content: format!("Задач: {}\n{}", v.len(), lines.join("\n")), data: serde_json::to_value(&v).ok(), ..Default::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_only_old_files_in_fixture() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("old.tmp");
        let sub = t.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(&old, vec![0u8; 1000]).unwrap();
        std::fs::write(sub.join("new.tmp"), "x").unwrap();
        let past = SystemTime::now() - Duration::from_secs(3 * 86400);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(past).unwrap();
        let keep = t.path().join("keep");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("k.tmp"), "k").unwrap();
        std::fs::File::options().write(true).open(keep.join("k.tmp")).unwrap().set_modified(past).unwrap();

        let dry = cleanup_dir(t.path(), Duration::from_secs(86400), true, &[keep.clone()]);
        assert_eq!((dry.files, dry.bytes), (1, 1000));
        assert!(old.exists());
        let real = cleanup_dir(t.path(), Duration::from_secs(86400), false, &[keep.clone()]);
        assert_eq!(real.files, 1);
        assert!(!old.exists() && sub.join("new.tmp").exists() && keep.join("k.tmp").exists());
    }

    #[test]
    fn host_and_pkg_validation() {
        assert!(valid_host("8.8.8.8") && valid_host("ya.ru") && valid_host("::1"));
        assert!(!valid_host("a & calc") && !valid_host("-t"));
        assert!(valid_pkg("VideoLAN.VLC") && !valid_pkg("x\" & y"));
        assert_eq!(TempCleanup(Arc::new(UndoJournal::new(std::env::temp_dir(), Arc::new(crate::undo::SystemTrash)))).tier_for(&json!({"include_system": true})), RiskTier::T3);
    }

    #[tokio::test]
    async fn ping_localhost() {
        let out = ping("127.0.0.1", 1, &CancellationToken::new()).await.unwrap();
        assert!(out.contains("код возврата: 0"), "{out}");
        assert!(ping("a;b", 1, &CancellationToken::new()).await.is_err());
    }

    #[tokio::test]
    async fn scheduled_tasks_read_only() {
        let out = ScheduledTasksList.call(json!({"include_microsoft": true}), &ToolCtx::for_tests()).await.unwrap();
        assert!(out.content.starts_with("Задач: "), "{}", out.content);
    }
}
