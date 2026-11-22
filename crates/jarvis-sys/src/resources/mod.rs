//! Управление ресурсами (план, раздел 3): снимок системы, процессы, службы, питание, автозагрузка,
//! обслуживание, сеть, winget, планировщик, телеметрия.

pub mod maint;
pub mod power;
pub mod services;
pub mod startup;
pub mod telemetry;

use std::time::Duration;

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde::Serialize;
use serde_json::{Value, json};
use sysinfo::{Disks, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::policy::CRITICAL_PROCESSES;
use crate::util::{fmt_bytes, opt_bool, opt_str, opt_u64, spec};
use crate::win;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiskInfo {
    pub mount: String,
    pub name: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NetInfo {
    pub name: String,
    pub mac: String,
    pub ips: Vec<String>,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub cpu_total: f32,
    pub cpu_per_core: Vec<f32>,
    pub cpu_brand: String,
    pub ram_used: u64,
    pub ram_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disks: Vec<DiskInfo>,
    pub networks: Vec<NetInfo>,
    pub battery: Option<win::Battery>,
    pub gpu: Option<f32>,
    pub uptime_secs: u64,
}

pub fn disks() -> Vec<DiskInfo> {
    Disks::new_with_refreshed_list()
        .list()
        .iter()
        .map(|d| DiskInfo {
            mount: d.mount_point().to_string_lossy().to_string(),
            name: d.name().to_string_lossy().to_string(),
            fs: d.file_system().to_string_lossy().to_string(),
            total: d.total_space(),
            free: d.available_space(),
            removable: d.is_removable(),
        })
        .collect()
}

pub fn networks() -> Vec<NetInfo> {
    let n = Networks::new_with_refreshed_list();
    let mut v: Vec<NetInfo> = n
        .list()
        .iter()
        .map(|(name, d)| NetInfo {
            name: name.clone(),
            mac: d.mac_address().to_string(),
            ips: d.ip_networks().iter().map(|i| i.to_string()).collect(),
            rx_total: d.total_received(),
            tx_total: d.total_transmitted(),
        })
        .collect();
    v.sort_by(|a, b| (b.rx_total + b.tx_total).cmp(&(a.rx_total + a.tx_total)));
    v
}

/// Загрузка GPU, %: NVML (фича `nvml`), иначе PDH. Блокирует на ~`wait`.
pub fn gpu_usage(wait: Duration) -> Option<f32> {
    #[cfg(feature = "nvml")]
    if let Ok(n) = nvml_wrapper::Nvml::init()
        && let Ok(d) = n.device_by_index(0)
        && let Ok(u) = d.utilization_rates()
    {
        return Some(u.gpu as f32);
    }
    let mut c = win::GpuCounter::new()?;
    std::thread::sleep(wait);
    c.sample()
}

/// Полный снимок (блокирует ~0,3 с ради замера CPU/GPU).
pub fn snapshot() -> Snapshot {
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    let gpu_thread = std::thread::spawn(|| gpu_usage(Duration::from_millis(300)));
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(300)));
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    Snapshot {
        cpu_total: sys.global_cpu_usage(),
        cpu_per_core: sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
        cpu_brand: sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
        ram_used: sys.used_memory(),
        ram_total: sys.total_memory(),
        swap_used: sys.used_swap(),
        swap_total: sys.total_swap(),
        disks: disks(),
        networks: networks(),
        battery: win::battery(),
        gpu: gpu_thread.join().ok().flatten(),
        uptime_secs: System::uptime(),
    }
}

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 { 0.0 } else { a as f64 * 100.0 / b as f64 }
}

fn format_snapshot(s: &Snapshot) -> String {
    let mut o = format!(
        "ЦП: {:.0} % ({}; ядер {}: {})\nОЗУ: {} из {} ({:.0} %)\nФайл подкачки: {} из {}\n",
        s.cpu_total,
        s.cpu_brand,
        s.cpu_per_core.len(),
        s.cpu_per_core.iter().map(|c| format!("{c:.0}")).collect::<Vec<_>>().join(" "),
        fmt_bytes(s.ram_used),
        fmt_bytes(s.ram_total),
        pct(s.ram_used, s.ram_total),
        fmt_bytes(s.swap_used),
        fmt_bytes(s.swap_total),
    );
    if let Some(g) = s.gpu {
        o.push_str(&format!("GPU: {g:.0} %\n"));
    }
    for d in &s.disks {
        o.push_str(&format!("Диск {} ({}): свободно {} из {} ({:.0} %)\n", d.mount, d.fs, fmt_bytes(d.free), fmt_bytes(d.total), pct(d.free, d.total)));
    }
    for n in s.networks.iter().filter(|n| !n.ips.is_empty()).take(4) {
        o.push_str(&format!("Сеть {}: {} (принято {}, отправлено {})\n", n.name, n.ips.join(", "), fmt_bytes(n.rx_total), fmt_bytes(n.tx_total)));
    }
    if let Some(b) = &s.battery {
        o.push_str(&format!(
            "Батарея: {}{}\n",
            b.percent.map(|p| format!("{p} %")).unwrap_or_else(|| "?".into()),
            match b.on_ac {
                Some(true) => ", от сети",
                Some(false) => ", от батареи",
                None => "",
            }
        ));
    }
    o.push_str(&format!("Время работы: {} ч {} мин", s.uptime_secs / 3600, s.uptime_secs / 60 % 60));
    o
}

// ───────────────────────────── процессы ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,
    pub memory: u64,
    /// Процесс текущего пользователя (`None` — нет доступа к токену: системный/чужой).
    pub mine: Option<bool>,
    pub parent: Option<u32>,
    /// Сколько процессов объединено (при группировке по имени).
    pub count: usize,
}

fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing().with_cpu().with_memory().with_user(UpdateKind::OnlyIfNotSet)
}

/// Все процессы с загрузкой ЦП (замер `sample`).
pub fn processes(sample: Duration) -> Vec<ProcInfo> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
    std::thread::sleep(sample);
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
    let cores = sys.cpus().len().max(1) as f32;
    let cores = if cores <= 1.0 { std::thread::available_parallelism().map(|n| n.get() as f32).unwrap_or(1.0) } else { cores };
    let me = sysinfo::get_current_pid().ok().and_then(|p| sys.process(p)).and_then(|p| p.user_id().cloned());
    sys.processes()
        .values()
        .map(|p| ProcInfo {
            pid: p.pid().as_u32(),
            name: p.name().to_string_lossy().to_string(),
            cpu: p.cpu_usage() / cores,
            memory: p.memory(),
            mine: match (p.user_id(), &me) {
                (Some(u), Some(m)) => Some(u == m),
                _ => None,
            },
            parent: p.parent().map(|x| x.as_u32()),
            count: 1,
        })
        .collect()
}

/// Объединить процессы с одинаковым именем (chrome.exe ×30 → один ряд).
pub fn group_by_name(v: Vec<ProcInfo>) -> Vec<ProcInfo> {
    let mut m: std::collections::HashMap<String, ProcInfo> = std::collections::HashMap::new();
    for p in v {
        let k = p.name.to_lowercase();
        m.entry(k)
            .and_modify(|g| {
                g.cpu += p.cpu;
                g.memory += p.memory;
                g.count += 1;
            })
            .or_insert(p);
    }
    m.into_values().collect()
}

pub fn sort_procs(v: &mut [ProcInfo], by: &str) {
    match by {
        "cpu" => v.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal)),
        "name" => v.sort_by_key(|a| a.name.to_lowercase()),
        "pid" => v.sort_by_key(|a| a.pid),
        _ => v.sort_by(|a, b| b.memory.cmp(&a.memory)),
    }
}

/// Имя без `.exe` для речи: «chrome».
pub fn display_name(n: &str) -> String {
    let l = n.to_string();
    l.strip_suffix(".exe").or_else(|| l.strip_suffix(".EXE")).unwrap_or(&l).to_string()
}

/// Уровень вмешательства в процесс: свой несистемный — T2, системный/критичный/чужой — T3.
pub fn process_tier(pid: u32) -> (RiskTier, String) {
    let own = std::process::id();
    if pid == own {
        return (RiskTier::T3, "это процесс самого Джарвиса".into());
    }
    if pid <= 4 {
        return (RiskTier::T3, "системный процесс".into());
    }
    let mut sys = System::new();
    let p = Pid::from_u32(pid);
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p, Pid::from_u32(own)]), true, ProcessRefreshKind::nothing().with_user(UpdateKind::Always));
    let Some(proc_) = sys.process(p) else { return (RiskTier::T3, format!("процесс {pid} не найден")) };
    let name = display_name(&proc_.name().to_string_lossy()).to_lowercase();
    if CRITICAL_PROCESSES.contains(&name.as_str()) {
        return (RiskTier::T3, format!("системный процесс {name}"));
    }
    let me = sys.process(Pid::from_u32(own)).and_then(|x| x.user_id().cloned());
    match (proc_.user_id(), me) {
        (Some(u), Some(m)) if *u == m => (RiskTier::T2, format!("процесс пользователя {name}")),
        (Some(_), Some(_)) => (RiskTier::T3, format!("процесс другого пользователя ({name})")),
        _ => (RiskTier::T3, format!("нет доступа к владельцу процесса {name} — вероятно, системный")),
    }
}

/// PID по аргументам `pid` или `name` (все процессы с таким именем).
fn target_pids(args: &Value) -> Result<Vec<u32>, ToolError> {
    if let Some(pid) = opt_u64(args, "pid") {
        return Ok(vec![pid as u32]);
    }
    let name = opt_str(args, "name").ok_or_else(|| ToolError::InvalidArgs("нужен pid или name".into()))?.to_lowercase();
    let name = name.trim_end_matches(".exe");
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let pids: Vec<u32> = sys
        .processes()
        .values()
        .filter(|p| display_name(&p.name().to_string_lossy()).to_lowercase() == name)
        .map(|p| p.pid().as_u32())
        .collect();
    if pids.is_empty() {
        return Err(ToolError::Failed(format!("процесс «{name}» не найден")));
    }
    Ok(pids)
}

fn tier_of_targets(args: &Value) -> RiskTier {
    match target_pids(args) {
        Ok(p) => p.iter().map(|&p| process_tier(p).0).max().unwrap_or(RiskTier::T3),
        Err(_) => RiskTier::T3,
    }
}

pub struct SysSnapshot;

#[async_trait]
impl Tool for SysSnapshot {
    fn spec(&self) -> ToolSpec {
        spec("sys_snapshot", "Снимок ресурсов: ЦП по ядрам, ОЗУ/подкачка, GPU, диски, сеть, батарея.", json!({"type":"object","properties":{}}), RiskTier::T0)
    }
    async fn call(&self, _a: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let s = tokio::task::spawn_blocking(snapshot).await.map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok(ToolOutput { content: format_snapshot(&s), data: serde_json::to_value(&s).ok(), ..Default::default() })
    }
}

pub struct ProcessList;

#[async_trait]
impl Tool for ProcessList {
    fn spec(&self) -> ToolSpec {
        spec(
            "process_list",
            "Список процессов, отсортированный по памяти/ЦП. group=true объединяет одноимённые (chrome ×30).",
            json!({"type":"object","properties":{
                "sort_by":{"type":"string","enum":["memory","cpu","name","pid"],"default":"memory"},
                "top":{"type":"integer","default":15},
                "group":{"type":"boolean","default":true},
                "filter":{"type":"string","description":"Подстрока имени"}
            }}),
            RiskTier::T0,
        )
    }
    async fn call(&self, args: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let by = opt_str(&args, "sort_by").unwrap_or_else(|| "memory".into());
        let top = opt_u64(&args, "top").unwrap_or(15).clamp(1, 500) as usize;
        let group = opt_bool(&args, "group").unwrap_or(true);
        let filter = opt_str(&args, "filter").map(|f| f.to_lowercase());
        let mut v = tokio::task::spawn_blocking(|| processes(Duration::from_millis(400))).await.map_err(|e| ToolError::Failed(e.to_string()))?;
        if let Some(f) = &filter {
            v.retain(|p| p.name.to_lowercase().contains(f));
        }
        if group {
            v = group_by_name(v);
        }
        sort_procs(&mut v, &by);
        v.truncate(top);
        let lines: Vec<String> = v
            .iter()
            .map(|p| {
                let cnt = if p.count > 1 { format!(" ×{}", p.count) } else { String::new() };
                format!("{} (PID {}{cnt}): ЦП {:.1} %, память {}", display_name(&p.name), p.pid, p.cpu, fmt_bytes(p.memory))
            })
            .collect();
        Ok(ToolOutput { content: lines.join("\n"), data: serde_json::to_value(&v).ok(), ..Default::default() })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProcAction {
    Kill,
    Priority,
    Suspend,
    Resume,
}

pub struct ProcessControl(pub ProcAction);

fn descendants(sys: &System, root: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        for c in sys.processes().values().filter(|x| x.parent().map(|q| q.as_u32()) == Some(p)) {
            let id = c.pid().as_u32();
            if !out.contains(&id) && id != root {
                out.push(id);
                stack.push(id);
            }
        }
    }
    out
}

#[async_trait]
impl Tool for ProcessControl {
    fn spec(&self) -> ToolSpec {
        let target = json!({"pid":{"type":"integer"},"name":{"type":"string","description":"Имя процесса (все с таким именем)"}});
        let mut props = target.as_object().unwrap().clone();
        let (name, desc) = match self.0 {
            ProcAction::Kill => {
                props.insert("tree".into(), json!({"type":"boolean","default":false,"description":"Вместе с дочерними"}));
                ("process_kill", "Завершить процесс. Свой несистемный — T2, системный/чужой — T3.")
            }
            ProcAction::Priority => {
                props.insert("priority".into(), json!({"type":"string","enum":["idle","below_normal","normal","above_normal","high"]}));
                ("process_priority", "Изменить приоритет процесса.")
            }
            ProcAction::Suspend => ("process_suspend", "Приостановить процесс (все потоки)."),
            ProcAction::Resume => ("process_resume", "Возобновить приостановленный процесс."),
        };
        spec(name, desc, json!({"type":"object","properties":props}), RiskTier::T2)
    }

    fn tier_for(&self, args: &Value) -> RiskTier {
        tier_of_targets(args)
    }

    async fn call(&self, args: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let pids = target_pids(&args)?;
        let own = std::process::id();
        if pids.contains(&own) {
            return Err(ToolError::Denied("нельзя применить к процессу самого Джарвиса".into()));
        }
        let action = self.0;
        let tree = opt_bool(&args, "tree").unwrap_or(false);
        let prio = opt_str(&args, "priority");
        tokio::task::spawn_blocking(move || {
            let mut done = Vec::new();
            match action {
                ProcAction::Kill => {
                    let mut sys = System::new();
                    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
                    let mut all = pids.clone();
                    if tree {
                        for p in &pids {
                            all.extend(descendants(&sys, *p));
                        }
                    }
                    for p in all {
                        if p == own {
                            continue;
                        }
                        if let Some(pr) = sys.process(Pid::from_u32(p)) {
                            if pr.kill() {
                                done.push(format!("завершён {} (PID {p})", display_name(&pr.name().to_string_lossy())));
                            } else {
                                done.push(format!("не удалось завершить PID {p}"));
                            }
                        }
                    }
                }
                ProcAction::Priority => {
                    let pr = win::Priority::parse(&prio.unwrap_or_default()).ok_or_else(|| ToolError::InvalidArgs("priority: idle|below_normal|normal|above_normal|high".into()))?;
                    for p in pids {
                        win::set_priority(p, pr).map_err(|e| ToolError::Failed(format!("PID {p}: {e}")))?;
                        done.push(format!("PID {p}: приоритет {pr:?}"));
                    }
                }
                ProcAction::Suspend | ProcAction::Resume => {
                    for p in pids {
                        let r = if action == ProcAction::Suspend { win::suspend_process(p) } else { win::resume_process(p) };
                        let n = r.map_err(|e| ToolError::Failed(format!("PID {p}: {e}")))?;
                        done.push(format!("PID {p}: {} потоков {}", n, if action == ProcAction::Suspend { "приостановлено" } else { "возобновлено" }));
                    }
                }
            }
            Ok(ToolOutput::text(done.join("\n")))
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_has_data() {
        let s = snapshot();
        assert!(s.ram_total > 0 && s.ram_used > 0);
        assert!(!s.cpu_per_core.is_empty());
        assert!(!s.disks.is_empty());
        assert!(format_snapshot(&s).contains("ОЗУ"));
    }

    #[test]
    fn process_list_contains_self() {
        let me = std::process::id();
        let v = processes(Duration::from_millis(200));
        let p = v.iter().find(|p| p.pid == me).expect("текущий процесс в списке");
        assert!(p.memory > 0);
        assert_eq!(p.mine, Some(true));
        let g = group_by_name(v.clone());
        assert!(g.len() <= v.len());
    }

    #[test]
    fn tiers() {
        assert_eq!(process_tier(std::process::id()).0, RiskTier::T3);
        assert_eq!(process_tier(4).0, RiskTier::T3);
        assert_eq!(tier_of_targets(&json!({"name": "csrss"})), RiskTier::T3);
    }

    /// Только собственный дочерний процесс: тир T2, приостановка/возобновление/приоритет/завершение.
    #[tokio::test]
    async fn control_own_child() {
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let pid = child.id();
        std::thread::sleep(Duration::from_millis(200));
        let args = json!({"pid": pid});
        assert_eq!(ProcessControl(ProcAction::Kill).tier_for(&args), RiskTier::T2, "{}", process_tier(pid).1);
        let ctx = ToolCtx::for_tests();
        ProcessControl(ProcAction::Suspend).call(args.clone(), &ctx).await.unwrap();
        ProcessControl(ProcAction::Resume).call(args.clone(), &ctx).await.unwrap();
        ProcessControl(ProcAction::Priority).call(json!({"pid": pid, "priority": "below_normal"}), &ctx).await.unwrap();
        ProcessControl(ProcAction::Kill).call(args.clone(), &ctx).await.unwrap();
        let st = child.wait().unwrap();
        assert!(!st.success());
        let own = ProcessControl(ProcAction::Kill).call(json!({"pid": std::process::id()}), &ctx).await;
        assert!(matches!(own, Err(ToolError::Denied(_))));
    }
}
