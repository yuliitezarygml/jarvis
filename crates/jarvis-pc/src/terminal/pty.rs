//! Свой терминал Джарвиса: команда в ConPTY (`portable-pty`), вывод + код возврата, таймаут и отмена.

use jarvis_core::{CancellationToken, ToolError};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject};
use windows::core::PCWSTR;

use super::vt::strip_vt;

/// Сколько последних запусков помнить.
const KEEP_RUNS: usize = 20;
/// Предел хранимого вывода одного запуска.
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunShell {
    /// Windows PowerShell 5.1 (по умолчанию).
    Powershell,
    /// PowerShell 7+.
    Pwsh,
    Cmd,
}

impl RunShell {
    pub fn parse(s: Option<&str>) -> Result<Self, ToolError> {
        match s.map(|s| s.trim().to_lowercase()).as_deref() {
            None | Some("") | Some("powershell") | Some("ps") => Ok(Self::Powershell),
            Some("pwsh") | Some("powershell7") => Ok(Self::Pwsh),
            Some("cmd") | Some("cmd.exe") => Ok(Self::Cmd),
            Some(other) => Err(ToolError::InvalidArgs(format!("shell «{other}»: powershell/pwsh/cmd"))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Powershell => "powershell",
            Self::Pwsh => "pwsh",
            Self::Cmd => "cmd",
        }
    }
}

/// Результат одного запуска.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PtyRun {
    pub id: u64,
    pub command: String,
    pub shell: RunShell,
    pub cwd: String,
    /// `None` — процесс остановлен (таймаут/отмена).
    pub exit_code: Option<i32>,
    /// Вывод без VT-последовательностей.
    pub output: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub started_unix_ms: u64,
    pub duration_ms: u64,
}

impl PtyRun {
    pub fn finished_unix_ms(&self) -> u64 {
        self.started_unix_ms + self.duration_ms
    }
}

/// Последние запуски `terminal_run` — источник №1 для `terminal_context`.
#[derive(Default)]
pub struct PtySessions {
    runs: Mutex<VecDeque<PtyRun>>,
    next_id: AtomicU64,
}

impl PtySessions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn push(&self, run: PtyRun) {
        let mut g = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        g.push_back(run);
        while g.len() > KEEP_RUNS {
            g.pop_front();
        }
    }

    pub fn latest(&self) -> Option<PtyRun> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).back().cloned()
    }

    pub fn list(&self) -> Vec<PtyRun> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
    }
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Обёртка PowerShell: UTF-8 вывод, без прогресс-баров, код возврата = $LASTEXITCODE или 1 при ошибке.
pub fn wrap_powershell(cmd: &str) -> String {
    format!(
        "$ProgressPreference = 'SilentlyContinue'\n\
         try {{ [Console]::OutputEncoding = [Text.Encoding]::UTF8 }} catch {{ }}\n\
         $OutputEncoding = [Text.Encoding]::UTF8\n\
         $global:LASTEXITCODE = 0\n\
         {cmd}\n\
         $jarvisOk = $?\n\
         if ($global:LASTEXITCODE -ne 0) {{ exit $global:LASTEXITCODE }}\n\
         if (-not $jarvisOk) {{ exit 1 }}\n\
         exit 0\n"
    )
}

/// `-EncodedCommand`: base64 от UTF-16LE — без проблем с кавычками командной строки.
pub fn encode_powershell(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Bat-файл для cmd: UTF-8 (chcp 65001), без эха, код возврата последней команды.
pub fn batch_script(cmd: &str) -> String {
    format!("@echo off\r\nchcp 65001 >nul\r\n{}\r\nexit /b %errorlevel%\r\n", cmd.replace("\r\n", "\n").replace('\n', "\r\n"))
}

/// Job Object: на таймауте/отмене останавливаем всё дерево процессов команды.
struct Job(HANDLE);

// SAFETY: дескриптор объекта ядра можно использовать из любого потока.
unsafe impl Send for Job {}

impl Job {
    fn new() -> Option<Self> {
        // SAFETY: создаём безымянный job без атрибутов.
        unsafe { CreateJobObjectW(None, PCWSTR::null()).ok().map(Job) }
    }

    fn assign(&self, process: *mut c_void) -> bool {
        // SAFETY: дескриптор процесса дочернего процесса жив, пока жив child.
        unsafe { AssignProcessToJobObject(self.0, HANDLE(process)).is_ok() }
    }

    fn terminate(&self) {
        // SAFETY: завершение процессов job.
        unsafe {
            let _ = TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: закрываем свой дескриптор (без KILL_ON_JOB_CLOSE: запущенные GUI-программы живут дальше).
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Ошибки portable-pty (`anyhow::Error`) → ToolError.
fn conv<T, E: std::fmt::Display>(r: Result<T, E>, ctx: &str) -> Result<T, ToolError> {
    r.map_err(|e| ToolError::Failed(format!("ConPTY ({ctx}): {e}")))
}

/// Выполнить команду в ConPTY (блокирующе). `tmp_dir` — куда класть bat-файл для cmd.
pub fn run_command(
    id: u64,
    cmd: &str,
    shell: RunShell,
    cwd: &Path,
    timeout: Duration,
    cancel: &CancellationToken,
    tmp_dir: &Path,
) -> Result<PtyRun, ToolError> {
    let started_unix_ms = now_unix_ms();
    let started = Instant::now();
    if !cwd.is_dir() {
        return Err(ToolError::InvalidArgs(format!("нет каталога {}", cwd.display())));
    }

    let mut batch: Option<PathBuf> = None;
    let mut builder = match shell {
        RunShell::Powershell | RunShell::Pwsh => {
            let exe = if shell == RunShell::Pwsh { "pwsh.exe" } else { "powershell.exe" };
            let mut b = CommandBuilder::new(exe);
            b.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"]);
            b.arg(encode_powershell(&wrap_powershell(cmd)));
            b
        }
        RunShell::Cmd => {
            std::fs::create_dir_all(tmp_dir).map_err(|e| ToolError::Failed(format!("{}: {e}", tmp_dir.display())))?;
            let path = tmp_dir.join(format!("run-{}-{id}.cmd", std::process::id()));
            std::fs::write(&path, batch_script(cmd)).map_err(|e| ToolError::Failed(format!("{}: {e}", path.display())))?;
            let mut b = CommandBuilder::new("cmd.exe");
            b.args(["/d", "/c"]);
            b.arg(&path);
            batch = Some(path);
            b
        }
    };
    builder.cwd(cwd);

    let pair = conv(native_pty_system().openpty(PtySize { rows: 50, cols: 200, pixel_width: 0, pixel_height: 0 }), "openpty")?;
    let mut child = conv(pair.slave.spawn_command(builder), "запуск")?;
    drop(pair.slave);
    let job = Job::new();
    if let (Some(job), Some(h)) = (&job, child.as_raw_handle()) {
        job.assign(h as *mut c_void);
    }
    let mut reader = conv(pair.master.try_clone_reader(), "reader")?;
    let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(conv(pair.master.take_writer(), "writer")?));

    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let last_data = Arc::new(Mutex::new(Instant::now()));
    let reader_thread = {
        let buf = buf.clone();
        let last_data = last_data.clone();
        let writer = writer.clone();
        std::thread::Builder::new()
            .name("jarvis-pty-read".into())
            .spawn(move || {
                let mut chunk = [0u8; 8192];
                let mut tail: Vec<u8> = Vec::new();
                loop {
                    match reader.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            // ConPTY (INHERIT_CURSOR) спрашивает позицию курсора — отвечаем, иначе вывод может встать.
                            tail.extend_from_slice(&chunk[..n]);
                            if tail.windows(4).any(|w| w == b"\x1b[6n") {
                                if let Ok(mut w) = writer.lock() {
                                    let _ = w.write_all(b"\x1b[1;1R");
                                    let _ = w.flush();
                                }
                                tail.clear();
                            } else if tail.len() > 8 {
                                tail.drain(..tail.len() - 4);
                            }
                            let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
                            b.extend_from_slice(&chunk[..n]);
                            if b.len() > MAX_OUTPUT_BYTES * 2 {
                                let cut = b.len() - MAX_OUTPUT_BYTES;
                                b.drain(..cut);
                            }
                            *last_data.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                        }
                    }
                }
            })
            .map_err(|e| ToolError::Failed(format!("поток чтения: {e}")))?
    };

    let (mut timed_out, mut cancelled) = (false, false);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {}
            Err(e) => return Err(ToolError::Failed(format!("ожидание процесса: {e}"))),
        }
        if cancel.is_cancelled() {
            cancelled = true;
        } else if started.elapsed() >= timeout {
            timed_out = true;
        }
        if cancelled || timed_out {
            if let Some(j) = &job {
                j.terminate();
            }
            let _ = child.kill();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    // Дождаться, пока ConPTY дорисует хвост вывода (тишина 150 мс, максимум 1,5 с).
    let drain_until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < drain_until {
        let quiet = last_data.lock().unwrap_or_else(|e| e.into_inner()).elapsed();
        if quiet >= Duration::from_millis(150) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // ClosePseudoConsole может ждать завершения conhost — закрываем в отдельном потоке.
    let master = pair.master;
    let closer = std::thread::spawn(move || {
        drop(writer);
        drop(master);
    });
    let join_until = Instant::now() + Duration::from_secs(3);
    while !(reader_thread.is_finished() && closer.is_finished()) && Instant::now() < join_until {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(job);
    if let Some(p) = batch {
        let _ = std::fs::remove_file(p);
    }

    let raw = buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut output = strip_vt(&String::from_utf8_lossy(&raw));
    if output.len() > MAX_OUTPUT_BYTES {
        let mut cut = output.len() - MAX_OUTPUT_BYTES;
        while !output.is_char_boundary(cut) {
            cut += 1;
        }
        output = format!("…\n{}", &output[cut..]);
    }
    Ok(PtyRun {
        id,
        command: cmd.to_string(),
        shell,
        cwd: cwd.display().to_string(),
        exit_code: status.map(|s| s.exit_code() as i32),
        output,
        timed_out,
        cancelled,
        started_unix_ms,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_encoding_roundtrip() {
        use base64::Engine;
        let enc = encode_powershell("echo 'привет'");
        let bytes = base64::engine::general_purpose::STANDARD.decode(enc).unwrap();
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "echo 'привет'");
        let w = wrap_powershell("git status");
        assert!(w.contains("\ngit status\n"));
        assert!(w.contains("exit $global:LASTEXITCODE"));
    }

    #[test]
    fn batch_has_utf8_and_exit_code() {
        let b = batch_script("echo a\necho b");
        assert!(b.starts_with("@echo off\r\nchcp 65001 >nul\r\n"));
        assert!(b.contains("echo a\r\necho b\r\n"));
        assert!(b.ends_with("exit /b %errorlevel%\r\n"));
    }

    #[test]
    fn sessions_keep_last_runs() {
        let s = PtySessions::new();
        for i in 0..25 {
            s.push(PtyRun {
                id: s.next_id(),
                command: format!("c{i}"),
                shell: RunShell::Cmd,
                cwd: ".".into(),
                exit_code: Some(0),
                output: String::new(),
                timed_out: false,
                cancelled: false,
                started_unix_ms: i,
                duration_ms: 1,
            });
        }
        assert_eq!(s.list().len(), KEEP_RUNS);
        assert_eq!(s.latest().unwrap().command, "c24");
        assert_eq!(RunShell::parse(Some("CMD")).unwrap(), RunShell::Cmd);
        assert!(RunShell::parse(Some("fish")).is_err());
    }
}
