//! J.A.R.V.I.S. — системный модуль: политика рисков, PowerShell/cmd, файлы с откатом, ресурсы, поиск и открытие.
//!
//! Быстрый старт:
//! ```no_run
//! # use std::sync::Arc;
//! let cfg = jarvis_sys::SysConfig::from_core(&jarvis_core::Config::default(), std::path::Path::new("d:\\jarvis"));
//! let sys = jarvis_sys::Sys::new(cfg);
//! let mut reg = jarvis_core::ToolRegistry::new();
//! reg.extend(jarvis_sys::tools(sys.clone()));
//! ```

pub mod confirm;
pub mod guard;
pub mod policy;
pub mod redact;
pub mod util;

#[cfg(windows)]
pub mod context;
#[cfg(windows)]
pub mod exec;
#[cfg(windows)]
pub mod files;
#[cfg(windows)]
pub mod open;
#[cfg(windows)]
pub mod resources;
#[cfg(windows)]
pub mod search;
#[cfg(windows)]
pub mod undo;
#[cfg(windows)]
pub mod win;

pub use confirm::{ConfirmDecision, ConfirmGate, ConfirmTicket, ConfirmVia, IgnoreReason, TranscriptVerdict};
pub use guard::{GuardAlert, GuardConfig, ResourceGuard};
pub use policy::{Classification, PathOp, PathZone, PolicyCtx, Shell, classify_command, classify_command_in, classify_path_op};
pub use redact::redact_secrets;

#[cfg(windows)]
pub use context::{Sys, SysConfig};
#[cfg(windows)]
pub use resources::telemetry::spawn_telemetry;

#[cfg(windows)]
use std::sync::Arc;

/// Все инструменты модуля для реестра агента.
#[cfg(windows)]
pub fn tools(sys: Arc<Sys>) -> Vec<Arc<dyn jarvis_core::Tool>> {
    use files::{FileOp, FileTool};
    use open::{ClipboardGet, ClipboardSet, OpenTarget, RevealInExplorer, UrlOpen};
    use resources::maint::{DnsFlush, NetworkInfo, Ping, ScheduledTasksList, TempCleanup, Winget, WingetAction};
    use resources::power::{PowerPlanGet, PowerPlanSet};
    use resources::services::{ServiceControl, ServiceList, ServiceStatus, SvcAction};
    use resources::startup::{StartupDisable, StartupList};
    use resources::{ProcAction, ProcessControl, ProcessList, SysSnapshot};
    let undo = sys.undo.clone();
    vec![
        Arc::new(exec::ShellRun::new(sys.clone(), Shell::PowerShell)),
        Arc::new(exec::ShellRun::new(sys.clone(), Shell::Cmd)),
        Arc::new(undo::UndoLast(undo.clone())),
        Arc::new(FileTool::new(sys.clone(), FileOp::Read)),
        Arc::new(FileTool::new(sys.clone(), FileOp::Write)),
        Arc::new(FileTool::new(sys.clone(), FileOp::Move)),
        Arc::new(FileTool::new(sys.clone(), FileOp::Copy)),
        Arc::new(FileTool::new(sys.clone(), FileOp::Delete)),
        Arc::new(SysSnapshot),
        Arc::new(ProcessList),
        Arc::new(ProcessControl(ProcAction::Kill)),
        Arc::new(ProcessControl(ProcAction::Priority)),
        Arc::new(ProcessControl(ProcAction::Suspend)),
        Arc::new(ProcessControl(ProcAction::Resume)),
        Arc::new(ServiceList),
        Arc::new(ServiceStatus),
        Arc::new(ServiceControl(SvcAction::Start)),
        Arc::new(ServiceControl(SvcAction::Stop)),
        Arc::new(ServiceControl(SvcAction::SetStartup)),
        Arc::new(PowerPlanGet),
        Arc::new(PowerPlanSet(undo.clone())),
        Arc::new(StartupList),
        Arc::new(StartupDisable(undo.clone())),
        Arc::new(TempCleanup(undo)),
        Arc::new(NetworkInfo),
        Arc::new(Ping),
        Arc::new(DnsFlush),
        Arc::new(Winget(WingetAction::Search)),
        Arc::new(Winget(WingetAction::Install)),
        Arc::new(Winget(WingetAction::Upgrade)),
        Arc::new(Winget(WingetAction::Uninstall)),
        Arc::new(ScheduledTasksList),
        Arc::new(OpenTarget(sys.clone())),
        Arc::new(search::FileSearch(sys.clone())),
        Arc::new(RevealInExplorer(sys.clone())),
        Arc::new(UrlOpen(sys.clone())),
        Arc::new(ClipboardGet(sys)),
        Arc::new(ClipboardSet),
    ]
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn registry_has_all_tools_with_unique_names() {
        let t = tempfile::tempdir().unwrap();
        let sys = Sys::new(SysConfig::for_tests(t.path()));
        let mut reg = jarvis_core::ToolRegistry::new();
        reg.extend(tools(sys));
        assert_eq!(reg.len(), 38);
        for s in reg.specs() {
            assert_eq!(s.parameters["type"], "object", "{}", s.name);
            assert!(!s.description.is_empty());
        }
    }

    #[tokio::test]
    async fn powershell_run_tool_end_to_end() {
        use jarvis_core::{RiskTier, ToolCtx, ToolError};
        use serde_json::json;
        let t = tempfile::tempdir().unwrap();
        let sys = Sys::new(SysConfig::for_tests(t.path()));
        let ps = exec::ShellRun::new(sys.clone(), Shell::PowerShell);
        use jarvis_core::Tool;
        assert_eq!(ps.tier_for(&json!({"command": "Get-Date"})), RiskTier::T0);
        assert_eq!(ps.tier_for(&json!({"command": "Remove-Item C:\\Windows\\x -Recurse"})), RiskTier::T3);
        assert!(sys.classify_call("powershell_run", &json!({"command": "Remove-Item C:\\Windows\\x"})).unwrap().protected_path);
        let out = ps.call(json!({"command": "Get-Date"}), &ToolCtx::for_tests()).await.unwrap();
        assert!(out.untrusted && out.content.starts_with("код возврата: 0"), "{}", out.content);
        let out = ps.call(json!({"command": "exit 5"}), &ToolCtx::for_tests()).await.unwrap();
        assert!(out.content.starts_with("код возврата: 5"));
        let e = ps.call(json!({"command": "Start-Sleep 5", "timeout_sec": 1}), &ToolCtx::for_tests()).await.unwrap_err();
        assert_eq!(e, ToolError::Timeout);
    }
}
