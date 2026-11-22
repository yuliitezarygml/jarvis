//! Зоны путей для политики: профиль и рабочие папки (T2), вне их (T3), защищённые (T3 + повторное подтверждение).

use std::path::{Path, PathBuf};

use super::PolicyCtx;

/// Куда указывает путь.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathZone {
    /// Внутри профиля пользователя, рабочих папок, `data/` или TEMP.
    User,
    /// Ровно корень профиля/рабочей папки (удалять его — опасно).
    UserRoot { profile: bool },
    /// Вне профиля и рабочих папок (другой диск, другой пользователь, сетевой путь).
    Outside,
    /// Системный путь: `C:\Windows`, `Program Files`, `ProgramData`, `System32`, корень диска…
    Protected(String),
    /// Путь вычисляется во время выполнения (`$path`, `$_`, `(…)`) или относительный без известного cwd.
    Unknown,
}

/// Что делаем с путём.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOp {
    Read,
    Write,
    Delete { recursive: bool },
}

/// Уровень риска операции над файловым путём: (tier, protected_path, причина).
pub fn path_tier(zone: &PathZone, op: PathOp) -> (jarvis_core::RiskTier, bool, String) {
    use jarvis_core::RiskTier::*;
    match (zone, op) {
        (_, PathOp::Read) => (T0, false, "чтение".into()),
        (PathZone::Protected(why), _) => (T3, true, format!("защищённый путь ({why})")),
        (PathZone::Unknown, _) => (T3, false, "путь вычисляется во время выполнения — нельзя проверить".into()),
        (PathZone::Outside, PathOp::Write) => (T3, false, "запись вне профиля и рабочих папок".into()),
        (PathZone::Outside, PathOp::Delete { .. }) => (T3, false, "удаление вне профиля и рабочих папок".into()),
        (PathZone::UserRoot { .. }, PathOp::Write) => (T2, false, "изменение в профиле/рабочей папке".into()),
        (PathZone::UserRoot { profile }, PathOp::Delete { .. }) => {
            (T3, *profile, "удаление корневой папки профиля/рабочей папки".into())
        }
        (PathZone::User, PathOp::Delete { recursive: true }) => (T3, false, "рекурсивное удаление".into()),
        (PathZone::User, PathOp::Delete { recursive: false }) => (T2, false, "удаление в профиле/рабочей папке".into()),
        (PathZone::User, PathOp::Write) => (T2, false, "изменение в профиле/рабочей папке".into()),
    }
}

/// Зона литерального пути (без подстановки переменных оболочки), например для файловых инструментов.
pub fn path_zone(path: &str, ctx: &PolicyCtx) -> PathZone {
    zone_of(path, false, ctx).0
}

/// Зона аргумента команды. `expandable` — возможна подстановка `$x`/`%X%`.
/// Второй элемент — нормализованный путь (нижний регистр), если удалось.
pub(crate) fn zone_of(raw: &str, expandable: bool, ctx: &PolicyCtx) -> (PathZone, Option<String>) {
    let Some(expanded) = expand_vars(raw, expandable, ctx) else {
        return (PathZone::Unknown, None);
    };
    let Some(norm) = normalize(&expanded, ctx) else {
        return (PathZone::Unknown, None);
    };
    match norm {
        Norm::Device => (PathZone::Protected("устройство".into()), None),
        Norm::Unc(p) => (PathZone::Outside, Some(p)),
        Norm::Local(comps) => {
            let joined = join(&comps);
            (zone_of_components(&comps, ctx), Some(joined))
        }
    }
}

enum Norm {
    Device,
    Unc(String),
    /// Компоненты в нижнем регистре, первый — `c:`.
    Local(Vec<String>),
}

fn join(comps: &[String]) -> String {
    if comps.len() == 1 {
        format!("{}\\", comps[0])
    } else {
        comps.join("\\")
    }
}

fn lower_path(p: &Path) -> Vec<String> {
    let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
    let s = s.strip_prefix("\\\\?\\").unwrap_or(&s).to_string();
    s.split('\\').filter(|c| !c.is_empty() && *c != ".").map(String::from).collect()
}

fn starts_with(comps: &[String], root: &[String]) -> bool {
    !root.is_empty() && comps.len() >= root.len() && comps[..root.len()] == root[..]
}

const PROTECTED_TOP: &[(&str, &str)] = &[
    ("windows", "C:\\Windows"),
    ("program files", "Program Files"),
    ("program files (x86)", "Program Files (x86)"),
    ("programdata", "ProgramData"),
    ("$recycle.bin", "Корзина"),
    ("system volume information", "System Volume Information"),
    ("recovery", "Recovery"),
    ("boot", "Boot"),
    ("efi", "EFI"),
    ("config.msi", "Config.Msi"),
    ("windows.old", "Windows.old"),
    ("$windows.~bt", "$WINDOWS.~BT"),
    ("$windows.~ws", "$WINDOWS.~WS"),
    ("bootmgr", "bootmgr"),
    ("pagefile.sys", "pagefile.sys"),
    ("hiberfil.sys", "hiberfil.sys"),
    ("swapfile.sys", "swapfile.sys"),
    ("documents and settings", "Documents and Settings"),
];

fn zone_of_components(comps: &[String], ctx: &PolicyCtx) -> PathZone {
    // подстановочные знаки: оцениваем родителя первого компонента с `*`/`?`
    let wc = comps.iter().position(|c| c.contains('*') || c.contains('?'));
    let base: &[String] = match wc {
        Some(i) => &comps[..i],
        None => comps,
    };
    if base.len() <= 1 {
        return PathZone::Protected("корень диска".into());
    }
    if let Some((_, name)) = PROTECTED_TOP.iter().find(|(n, _)| *n == base[1]) {
        return PathZone::Protected((*name).into());
    }
    if base.iter().any(|c| matches!(c.as_str(), "system32" | "syswow64" | "winsxs" | "sysnative")) {
        return PathZone::Protected("System32".into());
    }
    if base[1] == "users" && base.len() == 2 {
        return PathZone::Protected("папка всех профилей C:\\Users".into());
    }
    if let Some(last) = base.last()
        && (last.starts_with("ntuser.dat") || last.starts_with("usrclass.dat"))
    {
        return PathZone::Protected("файл реестра профиля".into());
    }
    let mut roots: Vec<(Vec<String>, bool)> = Vec::new();
    if let Some(p) = &ctx.profile {
        roots.push((lower_path(p), true));
    }
    for w in &ctx.work_dirs {
        roots.push((lower_path(w), false));
    }
    if let Some(t) = &ctx.temp {
        roots.push((lower_path(t), false));
    }
    for (root, is_profile) in &roots {
        if root.len() >= 2 && starts_with(base, root) {
            if base.len() == root.len() && wc.is_none() {
                return PathZone::UserRoot { profile: *is_profile };
            }
            // `C:\Users\me\*` — подстановка прямо в корне профиля: оцениваем как корень
            if base.len() == root.len() {
                return PathZone::UserRoot { profile: *is_profile };
            }
            return PathZone::User;
        }
    }
    PathZone::Outside
}

fn sys_drive(ctx: &PolicyCtx) -> String {
    format!("{}:", ctx.system_drive.to_ascii_lowercase())
}

/// Подставить известные переменные окружения. `None` — осталась неизвестная переменная.
fn expand_vars(raw: &str, expandable: bool, ctx: &PolicyCtx) -> Option<String> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return None;
    }
    let profile = ctx.profile.as_ref().map(|p| p.to_string_lossy().to_string());
    if (s == "~" || s.starts_with("~\\") || s.starts_with("~/")) && profile.is_some() {
        s = format!("{}{}", profile.clone().unwrap(), &s[1..]);
    }
    if !expandable {
        return Some(s);
    }
    let sd = sys_drive(ctx);
    let lookup = |name: &str| -> Option<String> {
        let prof = profile.clone();
        Some(match name {
            "userprofile" | "home" => prof?,
            "temp" | "tmp" => ctx.temp.as_ref().map(|t| t.to_string_lossy().to_string()).or(prof.map(|p| p + "\\AppData\\Local\\Temp"))?,
            "appdata" => prof? + "\\AppData\\Roaming",
            "localappdata" => prof? + "\\AppData\\Local",
            "onedrive" => prof? + "\\OneDrive",
            "systemroot" | "windir" => format!("{sd}\\Windows"),
            "programfiles" | "programw6432" => format!("{sd}\\Program Files"),
            "programfiles(x86)" => format!("{sd}\\Program Files (x86)"),
            "commonprogramfiles" => format!("{sd}\\Program Files\\Common Files"),
            "programdata" | "allusersprofile" => format!("{sd}\\ProgramData"),
            "systemdrive" | "homedrive" => sd.clone(),
            "homepath" => {
                let p = prof?;
                p.get(2..).map(String::from)?
            }
            "public" => format!("{sd}\\Users\\Public"),
            "cd" | "pwd" => ctx.cwd.as_ref()?.to_string_lossy().to_string(),
            _ => return None,
        })
    };
    // cmd: %NAME%
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '%' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '%') {
                let name: String = chars[i + 1..i + 1 + end].iter().collect::<String>().to_lowercase();
                out.push_str(&lookup(&name)?);
                i += end + 2;
                continue;
            }
            // одиночный `%i` — переменная цикла for
            return None;
        } else if c == '!' && chars[i + 1..].contains(&'!') {
            return None; // отложенное раскрытие cmd
        } else if c == '$' {
            // PowerShell: $env:NAME, ${env:NAME}, $HOME, $PWD
            let (name, len) = if chars.get(i + 1) == Some(&'{') {
                let end = chars[i + 2..].iter().position(|&c| c == '}')?;
                (chars[i + 2..i + 2 + end].iter().collect::<String>(), end + 3)
            } else {
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == ':') {
                    j += 1;
                }
                (chars[i + 1..j].iter().collect::<String>(), j - i)
            };
            let low = name.to_lowercase();
            let key = low.strip_prefix("env:").unwrap_or(&low);
            if !low.starts_with("env:") && !matches!(key, "home" | "pwd") {
                return None;
            }
            out.push_str(&lookup(key)?);
            i += len;
        } else {
            out.push(c);
            i += 1;
        }
    }
    Some(out)
}

fn normalize(s: &str, ctx: &PolicyCtx) -> Option<Norm> {
    let mut s = s.replace('/', "\\");
    let low = s.to_lowercase();
    for pre in ["microsoft.powershell.core\\filesystem::", "filesystem::"] {
        if low.starts_with(pre) {
            s = s[pre.len()..].to_string();
            break;
        }
    }
    let low = s.to_lowercase();
    if low.starts_with("\\\\.\\") || low.starts_with("\\\\?\\globalroot") {
        return Some(Norm::Device);
    }
    let s = if let Some(rest) = low.strip_prefix("\\\\?\\unc\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = low.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        low
    };
    if s.starts_with("\\\\") {
        return Some(Norm::Unc(s));
    }
    let bytes: Vec<char> = s.chars().collect();
    let (drive, rest): (String, String) = if bytes.len() >= 2 && bytes[1] == ':' && bytes[0].is_ascii_alphabetic() {
        (format!("{}:", bytes[0]), bytes[2..].iter().collect())
    } else if s.starts_with('\\') {
        let d = ctx
            .cwd
            .as_ref()
            .and_then(|c| lower_path(c).into_iter().next())
            .unwrap_or_else(|| sys_drive(ctx));
        (d, s.clone())
    } else {
        let cwd = ctx.cwd.as_ref()?;
        let mut base = lower_path(cwd);
        if base.is_empty() || !base[0].ends_with(':') {
            return None;
        }
        let first = base.remove(0);
        (first, format!("\\{}\\{}", base.join("\\"), s))
    };
    let mut comps = vec![drive];
    for c in rest.split('\\') {
        match c {
            "" | "." => {}
            ".." => {
                if comps.len() > 1 {
                    comps.pop();
                }
            }
            c => comps.push(c.trim_end_matches([' ', '.']).to_string()),
        }
    }
    // пустые после обрезки точек/пробелов («C:\Windows.» == «C:\Windows»)
    comps.retain(|c| !c.is_empty());
    Some(Norm::Local(comps))
}

/// Профиль, TEMP и т.п. из окружения процесса.
pub(crate) fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

// ───────────────────────────── Реестр ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hive {
    CurrentUser,
    /// HKLM, HKCR (сливается с HKLM\Software\Classes), HKCC.
    Machine,
    /// HKU — профили всех пользователей.
    Users,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegPath {
    pub hive: Hive,
    /// Путь внутри куста, нижний регистр, без ведущего `\`.
    pub key: String,
}

impl RegPath {
    /// Политики и ассоциации — чувствительно даже в HKCU.
    pub fn sensitive(&self) -> bool {
        self.key.contains("\\policies") || self.key.starts_with("software\\policies") || self.key.starts_with("policies")
    }
}

/// `HKLM:\…`, `HKCU\…`, `HKEY_LOCAL_MACHINE\…`, `Registry::HKEY_…`.
pub(crate) fn parse_reg_path(raw: &str) -> Option<RegPath> {
    let low = raw.trim().trim_matches('"').replace('/', "\\").to_lowercase();
    let mut s = low.as_str();
    for pre in ["microsoft.powershell.core\\registry::", "registry::"] {
        if let Some(r) = s.strip_prefix(pre) {
            s = r;
        }
    }
    let s = s.trim_start_matches('\\');
    let (head, rest) = match s.find(['\\', ':']) {
        Some(i) => (&s[..i], s[i..].trim_start_matches(':').trim_start_matches('\\')),
        None => (s, ""),
    };
    let hive = match head {
        "hkcu" | "hkey_current_user" => Hive::CurrentUser,
        "hklm" | "hkey_local_machine" | "hkcr" | "hkey_classes_root" | "hkcc" | "hkey_current_config" => Hive::Machine,
        "hku" | "hkey_users" => Hive::Users,
        _ => return None,
    };
    // «hkcu» без двоеточия/слэша — это просто слово, а не путь реестра (кроме полного имени куста)
    if s.len() == head.len() && !head.starts_with("hkey_") && !raw.contains(':') {
        return None;
    }
    let mut key = rest.trim_end_matches('\\').to_string();
    if matches!(head, "hkcr" | "hkey_classes_root") {
        key = format!("software\\classes\\{key}");
    }
    Some(RegPath { hive, key })
}

/// Путь другого провайдера PowerShell (`Env:`, `Variable:`, `Function:`, `Alias:`, `Cert:`, `WSMan:`).
pub(crate) fn ps_provider(raw: &str) -> Option<&'static str> {
    let low = raw.trim().to_lowercase();
    let head = low.split(':').next()?;
    if !low.contains(':') || head.len() < 2 {
        return None;
    }
    Some(match head {
        "env" => "env",
        "variable" => "variable",
        "function" => "function",
        "alias" => "alias",
        "cert" => "cert",
        "wsman" => "wsman",
        "hklm" | "hkcu" | "registry" => return None,
        h if h.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !low.contains("::") => "psdrive",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> PolicyCtx {
        PolicyCtx {
            cwd: Some("C:\\Users\\test\\proj".into()),
            profile: Some("C:\\Users\\test".into()),
            work_dirs: vec!["D:\\jarvis".into()],
            temp: Some("C:\\Users\\test\\AppData\\Local\\Temp".into()),
            system_drive: 'C',
        }
    }

    #[test]
    fn zones() {
        let c = ctx();
        assert_eq!(path_zone("C:\\Users\\test\\Documents\\a.txt", &c), PathZone::User);
        assert_eq!(path_zone("d:\\JARVIS\\src\\x.rs", &c), PathZone::User);
        assert_eq!(path_zone("notes.txt", &c), PathZone::User);
        assert_eq!(path_zone("..\\..\\other\\x", &c), PathZone::Outside);
        assert_eq!(path_zone("C:\\Users\\test", &c), PathZone::UserRoot { profile: true });
        assert_eq!(path_zone("C:\\Users\\testing\\x", &c), PathZone::Outside);
        assert!(matches!(path_zone("C:\\Windows\\System32\\drivers\\etc\\hosts", &c), PathZone::Protected(_)));
        assert!(matches!(path_zone("c:/program files/app", &c), PathZone::Protected(_)));
        assert!(matches!(path_zone("C:\\", &c), PathZone::Protected(_)));
        assert!(matches!(path_zone("E:\\*", &c), PathZone::Protected(_)));
        assert!(matches!(path_zone("\\Windows\\x", &c), PathZone::Protected(_)));
        assert!(matches!(path_zone("C:\\Windows.\\x", &c), PathZone::Protected(_)));
        assert!(matches!(path_zone("\\\\.\\PhysicalDrive0", &c), PathZone::Protected(_)));
        assert_eq!(path_zone("\\\\server\\share\\x", &c), PathZone::Outside);
        assert_eq!(path_zone("D:\\Games\\x", &c), PathZone::Outside);
        assert!(matches!(path_zone("C:\\Users\\test\\NTUSER.DAT", &c), PathZone::Protected(_)));
    }

    #[test]
    fn vars() {
        let c = ctx();
        assert_eq!(zone_of("$env:USERPROFILE\\x", true, &c).0, PathZone::User);
        assert_eq!(zone_of("%TEMP%\\a", true, &c).0, PathZone::User);
        assert_eq!(zone_of("~\\Desktop\\a", false, &c).0, PathZone::User);
        assert!(matches!(zone_of("$env:windir\\x", true, &c).0, PathZone::Protected(_)));
        assert!(matches!(zone_of("%ProgramFiles(x86)%\\x", true, &c).0, PathZone::Protected(_)));
        assert_eq!(zone_of("$path", true, &c).0, PathZone::Unknown);
        assert_eq!(zone_of("$env:FOO\\x", true, &c).0, PathZone::Unknown);
        // в одинарных кавычках `$` — литерал
        assert_eq!(zone_of("C:\\Users\\test\\$x", false, &c).0, PathZone::User);
    }

    #[test]
    fn registry() {
        assert_eq!(parse_reg_path("HKCU:\\Software\\X").unwrap().hive, Hive::CurrentUser);
        assert_eq!(parse_reg_path("HKLM\\SYSTEM\\x").unwrap().hive, Hive::Machine);
        assert_eq!(parse_reg_path("Registry::HKEY_LOCAL_MACHINE\\SOFTWARE").unwrap().hive, Hive::Machine);
        assert_eq!(parse_reg_path("HKCR\\.txt").unwrap().key, "software\\classes\\.txt");
        assert!(parse_reg_path("hkcu").is_none());
        assert!(parse_reg_path("C:\\x").is_none());
        assert!(parse_reg_path("HKCU:\\Software\\Policies\\X").unwrap().sensitive());
        assert_eq!(ps_provider("Env:\\PATH"), Some("env"));
        assert_eq!(ps_provider("C:\\x"), None);
    }
}
