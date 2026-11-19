use super::{PlatformBackend, Telemetry};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
    process::Stdio,
    ptr::null,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};
use windows_sys::Win32::System::{
    JobObjects::*, SystemInformation::GetSystemDirectoryW, Threading::CREATE_NO_WINDOW,
};

pub struct WindowsBackend;
pub fn windows_family(build: u32, product: u32) -> &'static str {
    match (product, build) {
        (2 | 3, _) => "Windows Server",
        (1, 22000..) => "Windows 11",
        (1, 10240..) => "Windows 10",
        _ => "unknown",
    }
}
#[async_trait::async_trait]
impl PlatformBackend for WindowsBackend {
    async fn system_info(&self, telemetry: &Telemetry) -> Result<Value> {
        let mut info=run_template(
            "$os=Get-CimInstance -ClassName Win32_OperatingSystem; $p=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent()); [pscustomobject]@{build=[uint32]$os.BuildNumber;product_type=[uint32]$os.ProductType;edition=$os.Caption;admin=$p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator);language=[cultureinfo]::CurrentUICulture.Name;powershell=$PSVersionTable.PSVersion.ToString();language_mode=[string]$ExecutionContext.SessionState.LanguageMode;process_64=[Environment]::Is64BitProcess}",
            None
        ).await?;
        let build = info["build"].as_u64().context("Номер сборки отсутствует")? as u32;
        let product = info["product_type"]
            .as_u64()
            .context("Тип ОС отсутствует")? as u32;
        info["family"] = json!(windows_family(build, product));
        let mut result = telemetry.sample();
        result["windows"] = info;
        Ok(result)
    }
}
pub fn system_dir() -> Result<PathBuf> {
    let mut path = [0u16; 260];
    let len = unsafe { GetSystemDirectoryW(path.as_mut_ptr(), path.len() as u32) } as usize;
    ensure!(
        len != 0 && len < path.len(),
        "Системный каталог Windows недоступен"
    );
    Ok(PathBuf::from(OsString::from_wide(&path[..len])))
}
fn job_for(child: &Child) -> Result<OwnedHandle> {
    // The static script waits for stdin until the job has assumed ownership.
    unsafe {
        let raw = CreateJobObjectW(null(), null());
        ensure!(!raw.is_null(), "Не удалось создать задание Windows");
        let job = OwnedHandle::from_raw_handle(raw);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
        );
        ensure!(
            configured != 0
                && AssignProcessToJobObject(
                    raw,
                    child
                        .raw_handle()
                        .context("Дескриптор процесса отсутствует")?
                ) != 0,
            "Не удалось привязать процесс к заданию Windows"
        );
        Ok(job)
    }
}
pub(super) async fn run_template(template: &'static str, name: Option<&str>) -> Result<Value> {
    let executable = system_dir()?.join("WindowsPowerShell/v1.0/powershell.exe");
    let script=format!("[void][Console]::ReadLine(); $ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $env:PSModulePath=Join-Path $PSHOME Modules; try {{ $result=& {{ {template} }}; ConvertTo-Json -InputObject $result -Compress -Depth 4 }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}");
    let mut cmd = Command::new(executable);
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &script,
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .creation_flags(CREATE_NO_WINDOW)
    .kill_on_drop(true)
    .env("JARVIS_TOOL_NAME", name.unwrap_or(""));
    let mut child = cmd.spawn()?;
    let _job = job_for(&child)?;
    child
        .stdin
        .take()
        .context("stdin")?
        .write_all(b"\r\n")
        .await?;
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .context("Инструмент превысил 15 секунд")??;
    ensure!(
        output.status.success() && output.stderr.is_empty(),
        "PowerShell завершился с кодом {:?}. {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(400)
            .collect::<String>()
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
pub struct Diagnostic {
    pub name: &'static str,
    pub description: &'static str,
    template: &'static str,
}
macro_rules! diagnostics {
    ($(($name:literal,$desc:literal,$code:literal)),+ $(,)?)=>{
        pub const DIAGNOSTICS:&[Diagnostic]=&[$(Diagnostic{name:$name,description:$desc,template:$code}),+];
    }
}
diagnostics!(
    ("process_list","Процессы этого компьютера","@(Get-Process | Select-Object -First 80 Id,ProcessName,WorkingSet64)"),
    ("service_status","Состояние локальных служб","@(Get-Service | Select-Object -First 80 Name,DisplayName,@{n='Status';e={$_.Status.ToString()}})"),
    ("task_list","Зарегистрированные задачи Windows","@(Get-ScheduledTask | Select-Object -First 80 TaskName,State)"),
    ("startup_list","Записи автозагрузки Windows","@(Get-CimInstance Win32_StartupCommand | Select-Object -First 60 Name,Location)"),
    ("eventlog_query","Сводка последних системных ошибок без содержимого событий","@(Get-WinEvent -FilterHashtable @{LogName='System';Level=2} -MaxEvents 12 | Select-Object Id,ProviderName,@{n='Time';e={$_.TimeCreated.ToString('o')}})"),
    ("net_info","Состояние локальных сетевых адаптеров","@(Get-NetAdapter | Select-Object -First 32 Name,Status,LinkSpeed)"),
    ("disk_info","Физические диски и их состояние","@(Get-Disk | Select-Object -First 32 Number,FriendlyName,Size,HealthStatus)"),
    ("volume_info","Тома и свободное место","@(Get-Volume | Select-Object -First 32 DriveLetter,FileSystem,Size,SizeRemaining)"),
    ("driver_list","Установленные драйверы устройств","@(Get-CimInstance Win32_PnPSignedDriver | Select-Object -First 80 DeviceName,DriverVersion,IsSigned)"),
    ("user_list","Локальные пользователи","@(Get-LocalUser | Select-Object -First 80 Name,Enabled)"),
    ("group_list","Локальные группы","@(Get-LocalGroup | Select-Object -First 80 Name)"),
    ("windows_feature_list","Состояние компонентов Windows","@(Get-WindowsOptionalFeature -Online | Select-Object -First 80 FeatureName,State)"),
);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandName {
    name: String,
}
pub async fn diagnostic(name: &str, args: &Value) -> Result<Value> {
    if name == "command_help" {
        let a: CommandName = serde_json::from_value(args.clone())?;
        ensure!(
            a.name.len() <= 80
                && a.name
                    .split_once('-')
                    .is_some_and(|(verb, noun)| !verb.is_empty()
                        && !noun.is_empty()
                        && verb
                            .bytes()
                            .chain(noun.bytes())
                            .all(|c| c.is_ascii_alphanumeric())),
            "Требуется имя командлета Verb-Noun"
        );
        return run_template("$c=Get-Command -Name $env:JARVIS_TOOL_NAME -CommandType Cmdlet,Function; $h=Get-Help -Name $c.Name; [pscustomobject]@{name=$c.Name;module=$c.ModuleName;synopsis=[string]$h.Synopsis;supports_whatif=$c.Parameters.ContainsKey('WhatIf')}",Some(&a.name)).await;
    }
    ensure!(args == &json!({}), "Инструмент принимает пустой объект");
    let d = DIAGNOSTICS
        .iter()
        .find(|d| d.name == name)
        .context("Неизвестный инструмент Windows")?;
    run_template(d.template, None).await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn build_and_product_type() {
        assert_eq!(windows_family(19045, 1), "Windows 10");
        assert_eq!(windows_family(22000, 1), "Windows 11");
        assert_eq!(windows_family(26100, 3), "Windows Server");
    }
    #[tokio::test]
    async fn help_rejects_shell_syntax_without_launching() {
        for name in [
            "Get-Item;calc",
            "Get-`Item",
            "Get-Item|x",
            "Get-$env:Path",
            "Get-*",
            "Get-Item & x",
            "Get-%a%^",
            "Get-\"Item",
        ] {
            assert!(diagnostic("command_help", &json!({"name":name}))
                .await
                .is_err());
        }
    }
}
