//! Автозагрузка: `startup_list` (T0) и `startup_disable` (HKCU/папка пользователя — T2, HKLM/общая папка — T3).
//! Отключение — как в Диспетчере задач: значение в `Explorer\StartupApproved\…` (запись Run не удаляется),
//! перед изменением — `reg export` и прежнее значение в журнал отката.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde::Serialize;
use serde_json::{Value, json};
use winreg::enums::*;
use winreg::{RegKey, RegValue};
use windows_sys::Win32::System::Registry::HKEY;

use crate::undo::{UndoAction, UndoJournal};
use crate::util::{opt_str, req_str, spec};

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN32: &str = "Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    HkcuRun,
    HklmRun,
    HklmRun32,
    UserFolder,
    CommonFolder,
}

impl Location {
    pub fn machine(self) -> bool {
        matches!(self, Self::HklmRun | Self::HklmRun32 | Self::CommonFolder)
    }
    pub fn tier(self) -> RiskTier {
        if self.machine() { RiskTier::T3 } else { RiskTier::T2 }
    }
    /// (куст, путь, текстовый ключ для reg.exe) ключа StartupApproved.
    fn approved(self) -> (HKEY, String, String) {
        let (hive, hs, sub) = match self {
            Self::HkcuRun => (HKEY_CURRENT_USER, "HKCU", "Run"),
            Self::HklmRun => (HKEY_LOCAL_MACHINE, "HKLM", "Run"),
            Self::HklmRun32 => (HKEY_LOCAL_MACHINE, "HKLM", "Run32"),
            Self::UserFolder => (HKEY_CURRENT_USER, "HKCU", "StartupFolder"),
            Self::CommonFolder => (HKEY_LOCAL_MACHINE, "HKLM", "StartupFolder"),
        };
        let path = format!("{APPROVED}\\{sub}");
        (hive, path.clone(), format!("{hs}\\{path}"))
    }
    fn label(self) -> &'static str {
        match self {
            Self::HkcuRun => "реестр пользователя (HKCU\\Run)",
            Self::HklmRun => "реестр системы (HKLM\\Run)",
            Self::HklmRun32 => "реестр системы, 32 бит (HKLM\\WOW6432Node\\Run)",
            Self::UserFolder => "папка автозагрузки пользователя",
            Self::CommonFolder => "общая папка автозагрузки",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StartupEntry {
    pub name: String,
    pub command: String,
    pub location: Location,
    pub enabled: bool,
}

/// Первый байт StartupApproved: чётный (02/06) — включено, нечётный (03/07) — отключено.
pub fn approved_enabled(bytes: Option<&[u8]>) -> bool {
    bytes.and_then(|b| b.first()).is_none_or(|b| b % 2 == 0)
}

/// Значение «отключено» с меткой времени (FILETIME).
pub fn disabled_value(now_unix: i64) -> Vec<u8> {
    let ft = ((now_unix.max(0) as u64) + 11_644_473_600) * 10_000_000;
    let mut v = vec![3u8, 0, 0, 0];
    v.extend_from_slice(&ft.to_le_bytes());
    v
}

fn approved_bytes(loc: Location, name: &str) -> Option<Vec<u8>> {
    let (hive, path, _) = loc.approved();
    let k = RegKey::predef(hive).open_subkey(path).ok()?;
    k.get_raw_value(name).ok().map(|v| v.bytes.into_owned())
}

fn folder_entries(dir: Option<PathBuf>, loc: Location, out: &mut Vec<StartupEntry>) {
    let Some(dir) = dir else { return };
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        out.push(StartupEntry {
            enabled: approved_enabled(approved_bytes(loc, &name).as_deref()),
            command: e.path().to_string_lossy().to_string(),
            name,
            location: loc,
        });
    }
}

pub fn list() -> Vec<StartupEntry> {
    let mut out = Vec::new();
    for (hive, path, loc) in [(HKEY_CURRENT_USER, RUN, Location::HkcuRun), (HKEY_LOCAL_MACHINE, RUN, Location::HklmRun), (HKEY_LOCAL_MACHINE, RUN32, Location::HklmRun32)] {
        let Ok(k) = RegKey::predef(hive).open_subkey(path) else { continue };
        for (name, v) in k.enum_values().flatten() {
            out.push(StartupEntry {
                enabled: approved_enabled(approved_bytes(loc, &name).as_deref()),
                command: v.to_string(),
                name,
                location: loc,
            });
        }
    }
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let programdata = std::env::var_os("ProgramData").map(PathBuf::from);
    let startup = "Microsoft\\Windows\\Start Menu\\Programs\\Startup";
    folder_entries(appdata.map(|a| a.join(startup)), Location::UserFolder, &mut out);
    folder_entries(programdata.map(|a| a.join(startup)), Location::CommonFolder, &mut out);
    out
}

fn find<'a>(entries: &'a [StartupEntry], name: &str, location: Option<&str>) -> Option<&'a StartupEntry> {
    let n = name.to_lowercase();
    let loc_ok = |e: &StartupEntry| match location {
        Some("user" | "hkcu") => !e.location.machine(),
        Some("machine" | "hklm") => e.location.machine(),
        _ => true,
    };
    entries
        .iter()
        .filter(|e| loc_ok(e))
        .find(|e| e.name.to_lowercase() == n)
        .or_else(|| entries.iter().filter(|e| loc_ok(e)).find(|e| e.name.to_lowercase().contains(&n)))
}

fn hive_of(key: &str) -> Option<(HKEY, &str)> {
    let (h, rest) = key.split_once('\\')?;
    let hive = match h.to_uppercase().as_str() {
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        _ => return None,
    };
    Some((hive, rest))
}

fn reg_type(t: u32) -> RegType {
    match t {
        1 => REG_SZ,
        2 => REG_EXPAND_SZ,
        4 => REG_DWORD,
        7 => REG_MULTI_SZ,
        11 => REG_QWORD,
        _ => REG_BINARY,
    }
}

/// Вернуть прежнее значение параметра (или удалить, если его не было). Используется `undo_last`.
pub fn restore_reg_value(key: &str, name: &str, previous: Option<&[u8]>, t: u32) -> io::Result<()> {
    let (hive, path) = hive_of(key).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("неизвестный куст: {key}")))?;
    let (k, _) = RegKey::predef(hive).create_subkey(path)?;
    match previous {
        Some(b) => k.set_raw_value(name, &RegValue { bytes: std::borrow::Cow::Borrowed(b), vtype: reg_type(t) }),
        None => match k.delete_value(name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        },
    }
}

pub struct StartupList;

#[async_trait]
impl Tool for StartupList {
    fn spec(&self) -> ToolSpec {
        spec("startup_list", "Программы в автозагрузке (реестр Run и папки автозагрузки) с состоянием вкл/выкл.", json!({"type":"object","properties":{}}), RiskTier::T0)
    }
    async fn call(&self, _a: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let v = tokio::task::spawn_blocking(list).await.map_err(|e| ToolError::Failed(e.to_string()))?;
        let lines: Vec<String> = v
            .iter()
            .map(|e| format!("{} [{}] — {} ({})", e.name, if e.enabled { "вкл" } else { "выкл" }, e.location.label(), e.command))
            .collect();
        Ok(ToolOutput { content: if lines.is_empty() { "Автозагрузка пуста.".into() } else { lines.join("\n") }, data: serde_json::to_value(&v).ok(), ..Default::default() })
    }
}

pub struct StartupDisable(pub Arc<UndoJournal>);

#[async_trait]
impl Tool for StartupDisable {
    fn spec(&self) -> ToolSpec {
        spec(
            "startup_disable",
            "Отключить программу в автозагрузке (как Диспетчер задач; запись не удаляется, откат — undo_last).",
            json!({"type":"object","properties":{"name":{"type":"string"},"scope":{"type":"string","enum":["user","machine"]}},"required":["name"]}),
            RiskTier::T2,
        )
    }

    fn tier_for(&self, args: &Value) -> RiskTier {
        let Some(name) = opt_str(args, "name") else { return RiskTier::T3 };
        let entries = list();
        find(&entries, &name, opt_str(args, "scope").as_deref()).map_or(RiskTier::T3, |e| e.location.tier())
    }

    async fn call(&self, args: Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let name = req_str(&args, "name")?;
        let scope = opt_str(&args, "scope");
        let undo = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let entries = list();
            let e = find(&entries, &name, scope.as_deref()).ok_or_else(|| ToolError::Failed(format!("«{name}» нет в автозагрузке")))?.clone();
            if !e.enabled {
                return Ok(ToolOutput::text(format!("«{}» уже отключена", e.name)));
            }
            let (hive, path, keytext) = e.location.approved();
            let mut actions = vec![undo.export_registry(&keytext).map_err(|x| ToolError::Failed(x.to_string()))?];
            let previous = approved_bytes(e.location, &e.name);
            let (k, _) = RegKey::predef(hive).create_subkey(&path).map_err(|x| ToolError::Failed(format!("реестр: {x}")))?;
            k.set_raw_value(&e.name, &RegValue { bytes: disabled_value(crate::undo::now_unix()).into(), vtype: REG_BINARY })
                .map_err(|x| ToolError::Failed(format!("реестр: {x}")))?;
            actions.push(UndoAction::RegValue { key: keytext, name: e.name.clone(), previous, reg_type: 3 });
            undo.record("startup_disable", &format!("автозагрузка отключена: {}", e.name), actions).map_err(|x| ToolError::Failed(x.to_string()))?;
            Ok(ToolOutput::text(format!("Автозагрузка «{}» отключена ({})", e.name, e.location.label())))
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_bytes_logic() {
        assert!(approved_enabled(None));
        assert!(approved_enabled(Some(&[2, 0, 0, 0])));
        assert!(approved_enabled(Some(&[6, 0])));
        assert!(!approved_enabled(Some(&[3, 0, 0, 0])));
        let v = disabled_value(1_700_000_000);
        assert_eq!(v.len(), 12);
        assert!(!approved_enabled(Some(&v)));
    }

    #[test]
    fn list_and_tiers_read_only() {
        let v = list();
        let entries = vec![
            StartupEntry { name: "OneDrive".into(), command: "x".into(), location: Location::HkcuRun, enabled: true },
            StartupEntry { name: "SecurityHealth".into(), command: "y".into(), location: Location::HklmRun, enabled: true },
        ];
        assert_eq!(find(&entries, "onedrive", None).unwrap().location.tier(), RiskTier::T2);
        assert_eq!(find(&entries, "security", None).unwrap().location.tier(), RiskTier::T3);
        assert!(find(&entries, "security", Some("user")).is_none());
        let _ = v; // реальный список читается без ошибок
        assert_eq!(StartupDisable(Arc::new(UndoJournal::new(std::env::temp_dir(), Arc::new(crate::undo::SystemTrash)))).tier_for(&json!({"name": "JarvisNoSuchEntry"})), RiskTier::T3);
    }
}
