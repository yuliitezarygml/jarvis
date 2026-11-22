//! Исполнитель процессов: Job Object (таймаут/отмена убивают всё дерево), UTF-8 вывод, лимиты.
//! Инструменты `powershell_run` и `cmd_run`.

use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jarvis_core::{CancellationToken, RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::context::Sys;
use crate::policy::{Shell, classify_command_in};
use crate::util::{opt_str, opt_u64, req_str, spec, truncate_middle};
use crate::win::{self, JobObject};

/// Сколько байт каждого потока держим в памяти.
const MAX_CAPTURE: usize = 2 * 1024 * 1024;
/// Сколько символов каждого потока отдаём модели.
const MAX_MODEL_CHARS: usize = 12_000;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub timeout: Duration,
    pub cwd: Option<PathBuf>,
    pub max_capture: usize,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(60), cwd: None, max_capture: MAX_CAPTURE }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct RunResult {
    pub stdout: String,
    pub stderr: String,
    /// `None` — процесс убит (таймаут/отмена).
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub truncated: bool,
}

/// Аргументы: обычные (Rust экранирует сам) или «сырая» командная строка (для cmd.exe /S /C).
pub enum Args<'a> {
    List(&'a [String]),
    Raw(&'a str),
}

#[derive(Default)]
struct Buf {
    data: Vec<u8>,
    truncated: bool,
}

async fn pump(mut r: impl tokio::io::AsyncRead + Unpin, buf: Arc<Mutex<Buf>>, cap: usize) {
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut b = buf.lock().unwrap();
                let room = cap.saturating_sub(b.data.len());
                b.data.extend_from_slice(&chunk[..n.min(room)]);
                if n > room {
                    b.truncated = true;
                }
            }
        }
    }
}

/// Запустить программу в собственном объекте задания.
pub async fn run_program(program: &str, args: Args<'_>, opts: &RunOptions, cancel: &CancellationToken) -> io::Result<RunResult> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

    let mut cmd = std::process::Command::new(program);
    match args {
        Args::List(a) => {
            cmd.args(a);
        }
        Args::Raw(r) => {
            cmd.raw_arg(r);
        }
    }
    if let Some(cwd) = &opts.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // CREATE_SUSPENDED: успеваем поместить процесс в задание до того, как он запустит потомков
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);

    let start = Instant::now();
    let job = JobObject::new()?;
    let mut child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);
    if let Some(h) = child.raw_handle() {
        if let Err(e) = job.assign(h as _) {
            let _ = child.start_kill();
            return Err(e);
        }
    }
    if let Err(e) = win::resume_process(pid) {
        job.terminate();
        return Err(e);
    }

    let out = Arc::new(Mutex::new(Buf::default()));
    let err = Arc::new(Mutex::new(Buf::default()));
    let t_out = tokio::spawn(pump(child.stdout.take().expect("stdout"), out.clone(), opts.max_capture));
    let t_err = tokio::spawn(pump(child.stderr.take().expect("stderr"), err.clone(), opts.max_capture));

    let mut res = RunResult::default();
    tokio::select! {
        st = child.wait() => {
            res.exit_code = st.ok().and_then(|s| s.code());
        }
        _ = tokio::time::sleep(opts.timeout) => {
            res.timed_out = true;
        }
        _ = cancel.cancelled() => {
            res.cancelled = true;
        }
    }
    if res.timed_out || res.cancelled {
        job.terminate();
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    }
    // Фоновые потомки (например, запущенное приложение) могут держать канал открытым — не ждём их долго
    let grace = Duration::from_millis(if res.timed_out || res.cancelled { 1000 } else { 500 });
    let _ = tokio::time::timeout(grace, async {
        let _ = (&mut { t_out }).await;
    })
    .await;
    let _ = tokio::time::timeout(Duration::from_millis(200), t_err).await;

    let o = out.lock().unwrap();
    let e = err.lock().unwrap();
    res.stdout = win::decode_console(&o.data);
    res.stderr = win::decode_console(&e.data);
    res.truncated = o.truncated || e.truncated;
    res.duration_ms = start.elapsed().as_millis() as u64;
    Ok(res)
}

/// Пролог PowerShell: UTF-8 вывод, без прогресс-баров.
const PS_PRELUDE: &str = "$ProgressPreference='SilentlyContinue';try{[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)}catch{};$OutputEncoding=[Text.UTF8Encoding]::new($false);";

pub async fn run_powershell(script: &str, opts: &RunOptions, cancel: &CancellationToken) -> io::Result<RunResult> {
    let full = format!("{PS_PRELUDE}\n{script}");
    let args = vec![
        "-NoLogo".to_string(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-Command".into(),
        full,
    ];
    run_program("powershell.exe", Args::List(&args), opts, cancel).await
}

pub async fn run_cmd(command: &str, opts: &RunOptions, cancel: &CancellationToken) -> io::Result<RunResult> {
    let raw = format!("/D /S /C \"chcp 65001>nul & {command}\"");
    run_program("cmd.exe", Args::Raw(&raw), opts, cancel).await
}

/// Ответ инструмента: код возврата, stdout, stderr (обрезанные и без секретов).
pub(crate) fn format_result(r: &RunResult, redact: bool) -> (String, usize) {
    let mut redacted = 0;
    let mut part = |s: &str| {
        let s = s.trim_end();
        let (s, n) = if redact { crate::redact::redact_secrets(s) } else { (s.to_string(), 0) };
        redacted += n;
        truncate_middle(&s, MAX_MODEL_CHARS).0
    };
    let out = part(&r.stdout);
    let err = part(&r.stderr);
    let mut s = match r.exit_code {
        Some(c) => format!("код возврата: {c}\n"),
        None => "процесс завершён принудительно\n".to_string(),
    };
    if !out.is_empty() {
        s.push_str("--- stdout ---\n");
        s.push_str(&out);
        s.push('\n');
    }
    if !err.is_empty() {
        s.push_str("--- stderr ---\n");
        s.push_str(&err);
        s.push('\n');
    }
    if out.is_empty() && err.is_empty() {
        s.push_str("(вывода нет)\n");
    }
    if r.truncated {
        s.push_str("(вывод обрезан)\n");
    }
    if redacted > 0 {
        s.push_str(&format!("(скрыто секретов: {redacted})\n"));
    }
    (s, redacted)
}

pub struct ShellRun {
    sys: Arc<Sys>,
    shell: Shell,
}

impl ShellRun {
    pub fn new(sys: Arc<Sys>, shell: Shell) -> Self {
        Self { sys, shell }
    }

    fn cwd(&self, args: &Value) -> Option<PathBuf> {
        opt_str(args, "cwd").map(PathBuf::from).or_else(|| self.sys.cfg.default_cwd.clone())
    }
}

#[async_trait]
impl Tool for ShellRun {
    fn spec(&self) -> ToolSpec {
        let (name, desc) = match self.shell {
            Shell::PowerShell => (
                "powershell_run",
                "Выполнить команду PowerShell 5.1 (powershell.exe -NoProfile -NonInteractive). Возвращает код возврата, stdout и stderr. Уровень риска определяется по команде; опасное требует подтверждения.",
            ),
            Shell::Cmd => ("cmd_run", "Выполнить команду cmd.exe (/C). Возвращает код возврата, stdout и stderr."),
        };
        spec(
            name,
            desc,
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Команда целиком"},
                    "cwd": {"type": "string", "description": "Рабочая папка (по умолчанию — профиль пользователя)"},
                    "timeout_sec": {"type": "integer", "minimum": 1, "maximum": 600, "default": 60}
                },
                "required": ["command"]
            }),
            RiskTier::T3,
        )
    }

    fn tier_for(&self, args: &Value) -> RiskTier {
        match opt_str(args, "command") {
            Some(c) => {
                let ctx = self.sys.policy_ctx(self.cwd(args));
                classify_command_in(self.shell, &c, &ctx).tier
            }
            None => RiskTier::T3,
        }
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let command = req_str(&args, "command")?;
        let timeout = Duration::from_secs(opt_u64(&args, "timeout_sec").unwrap_or(60).clamp(1, 600));
        let cwd = self.cwd(&args);
        if let Some(c) = &cwd
            && !c.is_dir()
        {
            return Err(ToolError::InvalidArgs(format!("папка не найдена: {}", c.display())));
        }
        let opts = RunOptions { timeout, cwd, ..Default::default() };
        let r = match self.shell {
            Shell::PowerShell => run_powershell(&command, &opts, &ctx.cancel).await,
            Shell::Cmd => run_cmd(&command, &opts, &ctx.cancel).await,
        }
        .map_err(|e| ToolError::Failed(format!("не удалось запустить: {e}")))?;
        if r.cancelled {
            return Err(ToolError::Cancelled);
        }
        if r.timed_out {
            return Err(ToolError::Timeout);
        }
        let (content, redacted) = format_result(&r, self.sys.cfg.redact_output);
        Ok(ToolOutput {
            content,
            data: Some(json!({"exit_code": r.exit_code, "duration_ms": r.duration_ms, "truncated": r.truncated, "redacted": redacted})),
            untrusted: true,
            attachments: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_date_and_utf8() {
        let r = run_powershell("Get-Date -Format 'yyyy'; Write-Output 'привет, сэр'", &RunOptions::default(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.exit_code, Some(0), "{r:?}");
        assert!(r.stdout.contains("20"), "{r:?}");
        assert!(r.stdout.contains("привет, сэр"), "{r:?}");
    }

    #[tokio::test]
    async fn nonzero_exit_and_stderr() {
        let r = run_powershell("Write-Error 'ошибка'; exit 3", &RunOptions::default(), &CancellationToken::new()).await.unwrap();
        assert_eq!(r.exit_code, Some(3));
        assert!(r.stderr.contains("ошибка"), "{r:?}");
        let r = run_powershell("Get-Item C:\\definitely\\missing\\path_xyz", &RunOptions::default(), &CancellationToken::new()).await.unwrap();
        assert_eq!(r.exit_code, Some(1));
    }

    #[tokio::test]
    async fn cmd_runs() {
        let r = run_cmd("echo привет & exit /b 2", &RunOptions::default(), &CancellationToken::new()).await.unwrap();
        assert_eq!(r.exit_code, Some(2));
        assert!(r.stdout.contains("привет"), "{r:?}");
    }

    fn alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{STILL_ACTIVE, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::*;
        let h = win::Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) });
        if !h.valid() {
            return false;
        }
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(h.0, &mut code) };
        code == STILL_ACTIVE as u32 && unsafe { WaitForSingleObject(h.0, 0) } == WAIT_TIMEOUT
    }

    #[tokio::test]
    async fn timeout_kills_tree() {
        // дочерний ping живёт 30 с; печатаем его PID и засыпаем
        let script = "$p = Start-Process ping -ArgumentList '-n','30','127.0.0.1' -NoNewWindow -PassThru; [Console]::Out.WriteLine($p.Id); [Console]::Out.Flush(); Start-Sleep 5";
        let opts = RunOptions { timeout: Duration::from_secs(3), ..Default::default() };
        let t = Instant::now();
        let r = run_powershell(script, &opts, &CancellationToken::new()).await.unwrap();
        assert!(r.timed_out, "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(5), "не уложились: {:?}", t.elapsed());
        let pid: u32 = r.stdout.lines().find_map(|l| l.trim().parse().ok()).expect("pid ребёнка");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!alive(pid), "дочерний процесс {pid} пережил таймаут");
    }

    #[tokio::test]
    async fn sleep_with_one_second_limit() {
        let opts = RunOptions { timeout: Duration::from_secs(1), ..Default::default() };
        let t = Instant::now();
        let r = run_powershell("Start-Sleep 5", &opts, &CancellationToken::new()).await.unwrap();
        assert!(r.timed_out && r.exit_code.is_none());
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    #[tokio::test]
    async fn cancellation() {
        let c = CancellationToken::new();
        let c2 = c.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            c2.cancel();
        });
        let r = run_powershell("Start-Sleep 10", &RunOptions::default(), &c).await.unwrap();
        assert!(r.cancelled);
    }

    #[test]
    fn formatting_redacts() {
        let r = RunResult { stdout: "password=hunter2\n".into(), exit_code: Some(0), ..Default::default() };
        let (s, n) = format_result(&r, true);
        assert_eq!(n, 1);
        assert!(!s.contains("hunter2") && s.contains("код возврата: 0"));
    }
}
