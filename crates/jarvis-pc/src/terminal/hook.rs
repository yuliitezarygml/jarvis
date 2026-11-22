//! Хук оболочки: после каждой команды PowerShell/bash пишет `{data_dir}/terminal/last.json` (только метаданные и ошибка).
//!
//! Формат `last.json` (UTF-8, один объект; запись атомарная — через временный файл и переименование):
//! ```json
//! {"version":1,"shell":"powershell","shell_version":"5.1.26100","command":"git comit -m x","exit_code":1,
//!  "success":false,"error_text":null,"cwd":"C:\\proj","time":"2026-11-22T16:40:00.1234567+03:00",
//!  "time_unix_ms":1795354800123,"pid":1234,"history_id":42}
//! ```
//! `error_text` — `$Error[0]` для ошибок PowerShell (stderr внешних программ хук не видит — его добирает UIA/OCR).

use jarvis_core::ToolError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BEGIN_MARKER: &str = "# >>> JARVIS terminal hook >>>";
pub const END_MARKER: &str = "# <<< JARVIS terminal hook <<<";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookShell {
    /// Windows PowerShell 5.1 (`powershell.exe`).
    Powershell,
    /// PowerShell 7+ (`pwsh.exe`).
    Pwsh,
    /// Git Bash / WSL (`~/.bashrc`, PROMPT_COMMAND).
    Bash,
}

impl HookShell {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "powershell" | "ps" | "windows_powershell" | "powershell5" => Some(Self::Powershell),
            "pwsh" | "powershell7" | "ps7" => Some(Self::Pwsh),
            "bash" | "git_bash" | "gitbash" | "wsl" => Some(Self::Bash),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Powershell => "powershell",
            Self::Pwsh => "pwsh",
            Self::Bash => "bash",
        }
    }
}

/// Запись хука.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HookRecord {
    pub version: u32,
    pub shell: String,
    pub shell_version: Option<String>,
    pub command: String,
    pub exit_code: Option<i32>,
    pub success: Option<bool>,
    pub error_text: Option<String>,
    pub cwd: Option<String>,
    pub time: Option<String>,
    pub time_unix_ms: Option<u64>,
    pub pid: Option<u32>,
    pub history_id: Option<u64>,
}

pub fn terminal_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("terminal")
}

pub fn hook_file(data_dir: &Path) -> PathBuf {
    terminal_dir(data_dir).join("last.json")
}

/// Прочитать `last.json` (BOM допускается). Нет файла — `None`.
pub fn read_last(data_dir: &Path) -> Result<Option<HookRecord>, ToolError> {
    let path = hook_file(data_dir);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ToolError::Failed(format!("чтение {}: {e}", path.display()))),
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(text).map(Some).map_err(|e| ToolError::Failed(format!("повреждён {}: {e}", path.display())))
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `D:\jarvis\data` → (`/d/jarvis/data`, `/mnt/d/jarvis/data`) для Git Bash и WSL.
pub fn unix_paths(p: &Path) -> (String, String) {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(d), Some(':')) if d.is_ascii_alphabetic() => {
            let rest: String = chars.collect();
            let d = d.to_ascii_lowercase();
            (format!("/{d}{rest}"), format!("/mnt/{d}{rest}"))
        }
        _ => (s.clone(), s),
    }
}

const PS_TEMPLATE: &str = r#"# Managed by J.A.R.V.I.S.: after each command writes metadata to last.json. Remove: shell_hook_uninstall.
if (-not (Get-Variable -Name __JarvisHook -Scope Global -ErrorAction SilentlyContinue)) {
    $global:__JarvisHook = @{ Dir = __DIR__; Shell = __SHELL__; LastId = -1; LastErr = $null; Prompt = $function:prompt }
    function global:prompt {
        $jOk = $?
        $jCode = $global:LASTEXITCODE
        try {
            $jH = Get-History -Count 1
            if ($jH -and $jH.Id -ne $global:__JarvisHook.LastId) {
                $global:__JarvisHook.LastId = $jH.Id
                $jErr = $null
                if (-not $jOk -and $global:Error.Count -gt 0 -and -not [object]::ReferenceEquals($global:Error[0], $global:__JarvisHook.LastErr)) {
                    $jErr = ($global:Error[0] | Out-String).Trim()
                }
                if ($global:Error.Count -gt 0) { $global:__JarvisHook.LastErr = $global:Error[0] }
                if ($jOk) { $jExit = 0 } elseif ($jCode) { $jExit = [int]$jCode } else { $jExit = 1 }
                $jRec = [ordered]@{
                    version = 1; shell = $global:__JarvisHook.Shell; shell_version = $PSVersionTable.PSVersion.ToString()
                    command = $jH.CommandLine; exit_code = $jExit; success = [bool]$jOk; error_text = $jErr
                    cwd = (Get-Location).ProviderPath; time = (Get-Date).ToString('o')
                    time_unix_ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(); pid = $PID; history_id = $jH.Id
                }
                $jDir = $global:__JarvisHook.Dir
                if (-not (Test-Path -LiteralPath $jDir)) { $null = New-Item -ItemType Directory -Path $jDir -Force }
                $jTmp = Join-Path $jDir ('last.json.' + $PID + '.tmp')
                [IO.File]::WriteAllText($jTmp, ($jRec | ConvertTo-Json -Compress -Depth 3), (New-Object Text.UTF8Encoding $false))
                Move-Item -LiteralPath $jTmp -Destination (Join-Path $jDir 'last.json') -Force
            }
        } catch { }
        $global:LASTEXITCODE = $jCode
        if ($global:__JarvisHook.Prompt) { & $global:__JarvisHook.Prompt } else { "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) " }
    }
}"#;

const BASH_TEMPLATE: &str = r#"# Managed by J.A.R.V.I.S.: after each command writes metadata to last.json. Remove: shell_hook_uninstall.
__jarvis_json() { local s=$1; s=${s//\\/\\\\}; s=${s//\"/\\\"}; s=${s//$'\t'/\\t}; s=${s//$'\r'/\\r}; s=${s//$'\n'/\\n}; printf '%s' "$s"; }
__jarvis_hook() {
  local code=$? line hid cmd dir cwd ok
  line=$(HISTTIMEFORMAT= builtin history 1)
  hid=$(printf '%s' "$line" | sed -E 's/^ *([0-9]+).*/\1/')
  cmd=$(printf '%s' "$line" | sed -E 's/^ *[0-9]+\*? *//')
  if [ -n "$hid" ] && [ "$hid" != "${__JARVIS_LAST_HID:-}" ]; then
    __JARVIS_LAST_HID=$hid
    if [ -d /mnt/c ] && [ ! -d /c ]; then dir=__WSL_DIR__; else dir=__MSYS_DIR__; fi
    cwd=$(pwd -W 2>/dev/null || pwd)
    if [ "$code" -eq 0 ]; then ok=true; else ok=false; fi
    mkdir -p "$dir" 2>/dev/null
    printf '{"version":1,"shell":"bash","command":"%s","exit_code":%d,"success":%s,"cwd":"%s","time_unix_ms":%s,"pid":%d}' \
      "$(__jarvis_json "$cmd")" "$code" "$ok" "$(__jarvis_json "$cwd")" "$(date +%s%3N)" "$$" > "$dir/last.json.$$" \
      && mv -f "$dir/last.json.$$" "$dir/last.json"
  fi
  return $code
}
case ";${PROMPT_COMMAND:-};" in *";__jarvis_hook;"*) ;; *) PROMPT_COMMAND="__jarvis_hook${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;; esac"#;

/// Текст хука для профиля оболочки (между маркерами). Ничего не устанавливает.
pub fn shell_hook_script(shell: HookShell, data_dir: &Path) -> String {
    let dir = terminal_dir(data_dir);
    let body = match shell {
        HookShell::Powershell | HookShell::Pwsh => PS_TEMPLATE
            .replace("__DIR__", &ps_quote(&dir.to_string_lossy()))
            .replace("__SHELL__", &ps_quote(shell.name())),
        HookShell::Bash => {
            let (msys, wsl) = unix_paths(&dir);
            BASH_TEMPLATE.replace("__MSYS_DIR__", &sh_quote(&msys)).replace("__WSL_DIR__", &sh_quote(&wsl))
        }
    };
    format!("{BEGIN_MARKER}\n{body}\n{END_MARKER}\n")
}

/// Убрать блок хука из текста профиля. Возвращает (текст, был ли блок).
pub fn remove_block(profile: &str) -> (String, bool) {
    let Some(start) = profile.find(BEGIN_MARKER) else { return (profile.to_string(), false) };
    let Some(end_rel) = profile[start..].find(END_MARKER) else { return (profile.to_string(), false) };
    let mut end = start + end_rel + END_MARKER.len();
    // съедаем перевод строки после маркера
    if profile[end..].starts_with("\r\n") {
        end += 2;
    } else if profile[end..].starts_with('\n') {
        end += 1;
    }
    let mut head = profile[..start].to_string();
    // и пустую строку-разделитель, которую добавили при установке
    if head.ends_with("\r\n\r\n") {
        head.truncate(head.len() - 2);
    } else if head.ends_with("\n\n") {
        head.truncate(head.len() - 1);
    }
    (format!("{head}{}", &profile[end..]), true)
}

/// Вставить (или заменить) блок хука в конец профиля. `crlf` — переводы строк профиля.
pub fn install_block(profile: &str, block: &str, crlf: bool) -> String {
    let (mut text, _) = remove_block(profile);
    let nl = if crlf { "\r\n" } else { "\n" };
    let block = if crlf { block.replace("\r\n", "\n").replace('\n', "\r\n") } else { block.replace("\r\n", "\n") };
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push_str(nl);
        }
        text.push_str(nl);
    }
    text.push_str(&block);
    text
}

/// Путь профиля: `$PROFILE` (CurrentUserCurrentHost) для PowerShell, `~/.bashrc` для bash.
pub fn profile_path(shell: HookShell) -> Result<PathBuf, ToolError> {
    match shell {
        HookShell::Powershell => Ok(documents_dir()?.join("WindowsPowerShell").join("Microsoft.PowerShell_profile.ps1")),
        HookShell::Pwsh => Ok(documents_dir()?.join("PowerShell").join("Microsoft.PowerShell_profile.ps1")),
        HookShell::Bash => std::env::var_os("USERPROFILE")
            .map(|h| PathBuf::from(h).join(".bashrc"))
            .ok_or_else(|| ToolError::Failed("не задан USERPROFILE".into())),
    }
}

/// «Документы» с учётом перенаправления (OneDrive).
fn documents_dir() -> Result<PathBuf, ToolError> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Documents, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
    // SAFETY: строку освобождаем CoTaskMemFree.
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Documents, KNOWN_FOLDER_FLAG(0), None)
            .map_err(|e| ToolError::Failed(format!("папка «Документы»: {e}")))?;
        let s = p.to_string().map_err(|e| ToolError::Failed(e.to_string()));
        CoTaskMemFree(Some(p.0 as *const _));
        Ok(PathBuf::from(s?))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookReport {
    pub shell: HookShell,
    pub profile: PathBuf,
    pub backup: Option<PathBuf>,
    pub changed: bool,
    pub note: String,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Прочитать профиль: (текст, был BOM, CRLF). Нет файла — пустой.
fn read_profile(path: &Path) -> Result<Option<(String, bool, bool)>, ToolError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ToolError::Failed(format!("чтение {}: {e}", path.display()))),
    };
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, &bytes[..]),
    };
    if body.starts_with(&[0xFF, 0xFE]) || body.starts_with(&[0xFE, 0xFF]) {
        return Err(ToolError::Unsupported(format!("{} в UTF-16 — добавьте хук вручную (shell_hook_script)", path.display())));
    }
    let text = String::from_utf8(body.to_vec())
        .map_err(|_| ToolError::Unsupported(format!("{} не в UTF-8 — добавьте хук вручную (shell_hook_script)", path.display())))?;
    let crlf = text.contains("\r\n");
    Ok(Some((text, bom, crlf)))
}

fn backup(path: &Path, data_dir: &Path) -> Result<Option<PathBuf>, ToolError> {
    if !path.exists() {
        return Ok(None);
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "profile".into());
    let dir = terminal_dir(data_dir).join("backup");
    std::fs::create_dir_all(&dir).map_err(|e| ToolError::Failed(format!("{}: {e}", dir.display())))?;
    let dst = dir.join(format!("{name}.{}.bak", now_unix()));
    std::fs::copy(path, &dst).map_err(|e| ToolError::Failed(format!("бэкап {}: {e}", path.display())))?;
    Ok(Some(dst))
}

fn write_profile(path: &Path, text: &str, bom: bool) -> Result<(), ToolError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ToolError::Failed(format!("{}: {e}", dir.display())))?;
    }
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if bom {
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    bytes.extend_from_slice(text.as_bytes());
    let tmp = path.with_extension("jarvis.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| ToolError::Failed(format!("запись {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| ToolError::Failed(format!("запись {}: {e}", path.display())))
}

/// Установить хук в профиль (T2): бэкап в `{data_dir}/terminal/backup/`, старый блок заменяется.
pub fn shell_hook_install(shell: HookShell, data_dir: &Path) -> Result<HookReport, ToolError> {
    let profile = profile_path(shell)?;
    let existing = read_profile(&profile)?;
    let backup = backup(&profile, data_dir)?;
    let block = shell_hook_script(shell, data_dir);
    let is_ps = shell != HookShell::Bash;
    let (old, bom, crlf) = existing.unwrap_or((String::new(), is_ps, is_ps));
    let new = install_block(&old, &block, crlf && is_ps);
    // PowerShell 5.1 читает .ps1 без BOM как ANSI: при не-ASCII добавляем BOM.
    let bom = is_ps && (bom || !new.is_ascii());
    let changed = new != old;
    if changed {
        write_profile(&profile, &new, bom)?;
    }
    std::fs::create_dir_all(terminal_dir(data_dir)).ok();
    let note = if is_ps {
        "Хук сработает в новых окнах PowerShell. Если политика выполнения Restricted, профиль не загрузится: \
         Set-ExecutionPolicy -Scope CurrentUser RemoteSigned."
    } else {
        "Хук сработает в новых окнах bash (Git Bash). Для WSL добавьте shell_hook_script в ~/.bashrc внутри WSL."
    };
    Ok(HookReport { shell, profile, backup, changed, note: note.into() })
}

/// Удалить хук из профиля (T2), с бэкапом.
pub fn shell_hook_uninstall(shell: HookShell, data_dir: &Path) -> Result<HookReport, ToolError> {
    let profile = profile_path(shell)?;
    let Some((old, bom, _)) = read_profile(&profile)? else {
        return Ok(HookReport { shell, profile, backup: None, changed: false, note: "профиля нет — нечего удалять".into() });
    };
    let (new, found) = remove_block(&old);
    if !found {
        return Ok(HookReport { shell, profile, backup: None, changed: false, note: "хук не установлен".into() });
    }
    let backup = backup(&profile, data_dir)?;
    write_profile(&profile, &new, bom)?;
    Ok(HookReport { shell, profile, backup, changed: true, note: "хук удалён; действует в новых окнах".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jarvis-pc-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn powershell_script_text() {
        let s = shell_hook_script(HookShell::Powershell, Path::new("D:\\jarvis\\data"));
        assert!(s.starts_with(BEGIN_MARKER));
        assert!(s.trim_end().ends_with(END_MARKER));
        assert!(s.contains("Dir = 'D:\\jarvis\\data\\terminal'"));
        assert!(s.contains("Shell = 'powershell'"));
        assert!(s.contains("function global:prompt"));
        assert!(s.contains("$global:LASTEXITCODE = $jCode"), "код возврата пользователя не портится");
        assert!(s.contains("Move-Item"), "атомарная запись");
        assert!(!s.contains("__DIR__") && !s.contains("__SHELL__"));
        let pwsh = shell_hook_script(HookShell::Pwsh, Path::new("C:\\it's"));
        assert!(pwsh.contains("Dir = 'C:\\it''s\\terminal'"), "экранирование кавычки");
        assert!(pwsh.contains("Shell = 'pwsh'"));
    }

    #[test]
    fn bash_script_text() {
        let s = shell_hook_script(HookShell::Bash, Path::new("D:\\jarvis\\data"));
        assert!(s.contains("dir='/d/jarvis/data/terminal'"));
        assert!(s.contains("dir='/mnt/d/jarvis/data/terminal'"));
        assert!(s.contains("PROMPT_COMMAND=\"__jarvis_hook"));
        assert!(!s.contains('\r'), "bash не терпит CR");
        assert_eq!(unix_paths(Path::new("C:\\Users\\x")), ("/c/Users/x".into(), "/mnt/c/Users/x".into()));
    }

    #[test]
    fn install_and_remove_block_roundtrip() {
        let block = shell_hook_script(HookShell::Powershell, Path::new("D:\\data"));
        let original = "Import-Module posh-git\r\nSet-Alias g git\r\n";
        let installed = install_block(original, &block, true);
        assert!(installed.starts_with(original));
        assert!(installed.contains(BEGIN_MARKER));
        assert!(!installed.replace("\r\n", "").contains('\n'), "все переводы строк CRLF");
        // повторная установка не дублирует
        let twice = install_block(&installed, &block, true);
        assert_eq!(twice.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(twice, installed);
        let (removed, found) = remove_block(&installed);
        assert!(found);
        assert_eq!(removed, original);
        assert_eq!(remove_block(original), (original.to_string(), false));
        // пустой профиль
        let fresh = install_block("", &block, false);
        assert!(fresh.starts_with(BEGIN_MARKER));
        assert_eq!(remove_block(&fresh).0, "");
    }

    #[test]
    fn reads_last_json() {
        let dir = tmp_dir("hook");
        assert_eq!(read_last(&dir).unwrap(), None);
        std::fs::create_dir_all(terminal_dir(&dir)).unwrap();
        // как пишет PowerShell 5.1 ConvertTo-Json (с BOM от другого писателя — тоже ок)
        let json = "\u{feff}{\"version\":1,\"shell\":\"powershell\",\"shell_version\":\"5.1.26100.1\",\"command\":\"git comit -m x\",\
                    \"exit_code\":1,\"success\":false,\"error_text\":null,\"cwd\":\"C:\\\\Users\\\\yulii\\\\proj\",\
                    \"time\":\"2026-11-22T16:40:00.1234567+03:00\",\"time_unix_ms\":1795354800123,\"pid\":4242,\"history_id\":7,\"extra\":1}";
        std::fs::write(hook_file(&dir), json).unwrap();
        let r = read_last(&dir).unwrap().unwrap();
        assert_eq!(r.command, "git comit -m x");
        assert_eq!(r.exit_code, Some(1));
        assert_eq!(r.success, Some(false));
        assert_eq!(r.cwd.as_deref(), Some("C:\\Users\\yulii\\proj"));
        assert_eq!(r.time_unix_ms, Some(1_795_354_800_123));
        // формат bash-хука
        std::fs::write(
            hook_file(&dir),
            r#"{"version":1,"shell":"bash","command":"npm rn build","exit_code":1,"success":false,"cwd":"D:/proj","time_unix_ms":1795354800999,"pid":77}"#,
        )
        .unwrap();
        let r = read_last(&dir).unwrap().unwrap();
        assert_eq!((r.shell.as_str(), r.exit_code, r.error_text), ("bash", Some(1), None));
        std::fs::write(hook_file(&dir), "{broken").unwrap();
        assert!(read_last(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn profile_paths_resolve_without_touching_files() {
        let p = profile_path(HookShell::Powershell).unwrap();
        assert!(p.ends_with("WindowsPowerShell\\Microsoft.PowerShell_profile.ps1"), "{}", p.display());
        let b = profile_path(HookShell::Bash).unwrap();
        assert!(b.ends_with(".bashrc"));
        assert_eq!(HookShell::parse("PowerShell"), Some(HookShell::Powershell));
        assert_eq!(HookShell::parse("zsh"), None);
    }
}
