//! Службы: `service_list`/`service_status` (T0, Service Control Manager), `service_start/stop/set_startup` (T3, sc.exe).

use std::ptr::null;
use std::time::Duration;

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde::Serialize;
use serde_json::{Value, json};
use windows_sys::Win32::System::Services::*;

use crate::exec::{Args, RunOptions, run_program};
use crate::util::{opt_str, req_str, spec};
use crate::win::wide;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub state: &'static str,
    pub pid: u32,
    pub start_type: Option<&'static str>,
    pub binary: Option<String>,
    pub account: Option<String>,
}

struct Sc(SC_HANDLE);
impl Drop for Sc {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

fn state_name(s: u32) -> &'static str {
    match s {
        SERVICE_STOPPED => "stopped",
        SERVICE_START_PENDING => "start_pending",
        SERVICE_STOP_PENDING => "stop_pending",
        SERVICE_RUNNING => "running",
        SERVICE_CONTINUE_PENDING => "continue_pending",
        SERVICE_PAUSE_PENDING => "pause_pending",
        SERVICE_PAUSED => "paused",
        _ => "unknown",
    }
}

fn start_name(s: u32) -> &'static str {
    match s {
        SERVICE_AUTO_START => "auto",
        SERVICE_DEMAND_START => "manual",
        SERVICE_DISABLED => "disabled",
        SERVICE_BOOT_START => "boot",
        SERVICE_SYSTEM_START => "system",
        _ => "unknown",
    }
}

unsafe fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

fn manager() -> Result<Sc, ToolError> {
    let h = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT) };
    if h.is_null() {
        return Err(ToolError::Failed(format!("нет доступа к диспетчеру служб: {}", std::io::Error::last_os_error())));
    }
    Ok(Sc(h))
}

pub fn list_services() -> Result<Vec<ServiceInfo>, ToolError> {
    let m = manager()?;
    let mut needed = 0u32;
    let mut count = 0u32;
    let mut resume = 0u32;
    let mut buf: Vec<u8> = vec![0; 256 * 1024];
    let mut out = Vec::new();
    loop {
        let ok = unsafe {
            EnumServicesStatusExW(m.0, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL, buf.as_mut_ptr(), buf.len() as u32, &mut needed, &mut count, &mut resume, null())
        };
        let more = ok == 0 && std::io::Error::last_os_error().raw_os_error() == Some(234); // ERROR_MORE_DATA
        if ok == 0 && !more {
            return Err(ToolError::Failed(format!("EnumServicesStatusEx: {}", std::io::Error::last_os_error())));
        }
        let items = buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW;
        for i in 0..count as usize {
            let it = unsafe { &*items.add(i) };
            out.push(ServiceInfo {
                name: unsafe { pwstr(it.lpServiceName) },
                display_name: unsafe { pwstr(it.lpDisplayName) },
                state: state_name(it.ServiceStatusProcess.dwCurrentState),
                pid: it.ServiceStatusProcess.dwProcessId,
                start_type: None,
                binary: None,
                account: None,
            });
        }
        if !more {
            break;
        }
        if needed as usize > buf.len() {
            buf.resize(needed as usize + 4096, 0);
        }
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    Ok(out)
}

pub fn service_status(name: &str) -> Result<ServiceInfo, ToolError> {
    let m = manager()?;
    let w = wide(name);
    let h = Sc(unsafe { OpenServiceW(m.0, w.as_ptr(), SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG) });
    if h.0.is_null() {
        // может быть передано отображаемое имя
        if let Some(s) = list_services()?.into_iter().find(|s| s.display_name.eq_ignore_ascii_case(name)) {
            return service_status(&s.name);
        }
        return Err(ToolError::Failed(format!("служба «{name}» не найдена")));
    }
    let mut ssp: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
    let mut needed = 0u32;
    let ok = unsafe {
        QueryServiceStatusEx(h.0, SC_STATUS_PROCESS_INFO, &mut ssp as *mut _ as *mut u8, std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32, &mut needed)
    };
    if ok == 0 {
        return Err(ToolError::Failed(format!("QueryServiceStatusEx: {}", std::io::Error::last_os_error())));
    }
    let mut cfg_buf = vec![0u8; 8192];
    let (mut start_type, mut binary, mut account, mut display) = (None, None, None, name.to_string());
    if unsafe { QueryServiceConfigW(h.0, cfg_buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW, cfg_buf.len() as u32, &mut needed) } != 0 {
        let c = unsafe { &*(cfg_buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
        start_type = Some(start_name(c.dwStartType));
        binary = Some(unsafe { pwstr(c.lpBinaryPathName) });
        account = Some(unsafe { pwstr(c.lpServiceStartName) });
        display = unsafe { pwstr(c.lpDisplayName) };
    }
    Ok(ServiceInfo { name: name.to_string(), display_name: display, state: state_name(ssp.dwCurrentState), pid: ssp.dwProcessId, start_type, binary, account })
}

fn valid_name(n: &str) -> Result<(), ToolError> {
    if n.is_empty() || n.len() > 256 || !n.chars().all(|c| c.is_alphanumeric() || "_-. $".contains(c)) {
        return Err(ToolError::InvalidArgs(format!("недопустимое имя службы «{n}»")));
    }
    Ok(())
}

pub struct ServiceList;

#[async_trait]
impl Tool for ServiceList {
    fn spec(&self) -> ToolSpec {
        spec(
            "service_list",
            "Список служб Windows с состоянием.",
            json!({"type":"object","properties":{"filter":{"type":"string"},"state":{"type":"string","enum":["running","stopped","all"],"default":"all"}}}),
            RiskTier::T0,
        )
    }
    async fn call(&self, args: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let mut v = tokio::task::spawn_blocking(list_services).await.map_err(|e| ToolError::Failed(e.to_string()))??;
        if let Some(f) = opt_str(&args, "filter").map(|f| f.to_lowercase()) {
            v.retain(|s| s.name.to_lowercase().contains(&f) || s.display_name.to_lowercase().contains(&f));
        }
        match opt_str(&args, "state").as_deref() {
            Some("running") => v.retain(|s| s.state == "running"),
            Some("stopped") => v.retain(|s| s.state == "stopped"),
            _ => {}
        }
        let lines: Vec<String> = v.iter().take(200).map(|s| format!("{} — {} [{}]", s.name, s.display_name, s.state)).collect();
        Ok(ToolOutput { content: format!("Служб: {}\n{}", v.len(), lines.join("\n")), data: serde_json::to_value(&v).ok(), ..Default::default() })
    }
}

pub struct ServiceStatus;

#[async_trait]
impl Tool for ServiceStatus {
    fn spec(&self) -> ToolSpec {
        spec("service_status", "Состояние и тип запуска службы.", json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}), RiskTier::T0)
    }
    async fn call(&self, args: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let name = req_str(&args, "name")?;
        let s = tokio::task::spawn_blocking(move || service_status(&name)).await.map_err(|e| ToolError::Failed(e.to_string()))??;
        Ok(ToolOutput {
            content: format!("{} ({}): {}, запуск: {}, PID {}", s.display_name, s.name, s.state, s.start_type.unwrap_or("?"), s.pid),
            data: serde_json::to_value(&s).ok(),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SvcAction {
    Start,
    Stop,
    SetStartup,
}

pub struct ServiceControl(pub SvcAction);

#[async_trait]
impl Tool for ServiceControl {
    fn spec(&self) -> ToolSpec {
        let (name, desc, props) = match self.0 {
            SvcAction::Start => ("service_start", "Запустить службу (опасно — с подтверждением).", json!({"name":{"type":"string"}})),
            SvcAction::Stop => ("service_stop", "Остановить службу (опасно — с подтверждением).", json!({"name":{"type":"string"}})),
            SvcAction::SetStartup => (
                "service_set_startup",
                "Изменить тип запуска службы (опасно — с подтверждением).",
                json!({"name":{"type":"string"},"startup":{"type":"string","enum":["auto","delayed_auto","manual","disabled"]}}),
            ),
        };
        spec(name, desc, json!({"type":"object","properties":props,"required":["name"]}), RiskTier::T3)
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let name = req_str(&args, "name")?;
        valid_name(&name)?;
        let a: Vec<String> = match self.0 {
            SvcAction::Start => vec!["start".into(), name.clone()],
            SvcAction::Stop => vec!["stop".into(), name.clone()],
            SvcAction::SetStartup => {
                let st = match opt_str(&args, "startup").as_deref() {
                    Some("auto") => "auto",
                    Some("delayed_auto") => "delayed-auto",
                    Some("manual") => "demand",
                    Some("disabled") => "disabled",
                    _ => return Err(ToolError::InvalidArgs("startup: auto|delayed_auto|manual|disabled".into())),
                };
                vec!["config".into(), name.clone(), "start=".into(), st.into()]
            }
        };
        let r = run_program("sc.exe", Args::List(&a), &RunOptions { timeout: Duration::from_secs(30), ..Default::default() }, &ctx.cancel)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        if r.exit_code != Some(0) {
            return Err(ToolError::Failed(format!("sc.exe {}: {}{}", a[0], r.stdout.trim(), r.stderr.trim())));
        }
        // дождаться конечного состояния
        let want = match self.0 {
            SvcAction::Start => Some("running"),
            SvcAction::Stop => Some("stopped"),
            SvcAction::SetStartup => None,
        };
        let mut st = String::new();
        for _ in 0..40 {
            let n = name.clone();
            let s = tokio::task::spawn_blocking(move || service_status(&n)).await.map_err(|e| ToolError::Failed(e.to_string()))??;
            st = format!("{}, запуск: {}", s.state, s.start_type.unwrap_or("?"));
            if want.is_none_or(|w| s.state == w) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(ToolOutput::text(format!("Служба {name}: {st}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_status_read_only() {
        let v = list_services().unwrap();
        assert!(v.len() > 20);
        let s = service_status("EventLog").unwrap();
        assert_eq!(s.state, "running");
        assert!(s.start_type.is_some());
        assert!(service_status("JarvisNoSuchService123").is_err());
        assert!(valid_name("wuauserv").is_ok() && valid_name("a&b").is_err());
    }

    #[test]
    fn control_is_t3() {
        for a in [SvcAction::Start, SvcAction::Stop, SvcAction::SetStartup] {
            assert_eq!(ServiceControl(a).tier_for(&json!({"name":"Spooler"})), RiskTier::T3);
        }
    }
}
