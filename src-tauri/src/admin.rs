use anyhow::{bail, ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions},
    sync::Mutex,
    time::timeout,
};
use uuid::Uuid;
use windows_sys::Win32::{
    Security::SECURITY_ATTRIBUTES,
    System::Threading::{GetProcessId, WaitForSingleObject},
};

unsafe extern "C" {
    fn jv_elevated() -> i32;
    fn jv_safe_install() -> i32;
    fn jv_runas(args: *const u16) -> *mut c_void;
    fn jv_verify_peer(pipe: *mut c_void, pid: u32, client: i32) -> i32;
    fn jv_pipe_descriptor() -> *mut c_void;
    fn jv_free_descriptor(sd: *mut c_void);
    fn jv_security_log_count(count: *mut u32) -> i32;
}
const IO_TIMEOUT: Duration = Duration::from_secs(10);
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    SecurityLogCount,
}
impl Operation {
    fn risk(&self) -> u8 {
        match self {
            Self::SecurityLogCount => 0,
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    sequence: u64,
    operation: Operation,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    sequence: u64,
    records: Option<u32>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    token: String,
}
async fn read<T: DeserializeOwned>(io: &mut (impl AsyncRead + Unpin)) -> Result<T> {
    let n = io.read_u32().await? as usize;
    ensure!((1..=2048).contains(&n), "Некорректный размер сообщения");
    let mut data = vec![0; n];
    io.read_exact(&mut data).await?;
    Ok(serde_json::from_slice(&data)?)
}
async fn send<T: Serialize>(io: &mut (impl AsyncWrite + Unpin), value: &T) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    ensure!(data.len() <= 2048, "Слишком длинное сообщение");
    io.write_u32(data.len() as u32).await?;
    io.write_all(&data).await?;
    io.flush().await?;
    Ok(())
}
fn address(id: &Uuid) -> String {
    format!(r"\\.\pipe\jarvis-admin-{id}")
}
fn server(id: &Uuid) -> Result<NamedPipeServer> {
    unsafe {
        let sd = jv_pipe_descriptor();
        ensure!(!sd.is_null(), "Не удалось ограничить доступ к каналу");
        let mut attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let result = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .max_instances(1)
            .create_with_security_attributes_raw(
                address(id),
                (&mut attrs as *mut SECURITY_ATTRIBUTES).cast(),
            );
        jv_free_descriptor(sd);
        Ok(result?)
    }
}
struct Session {
    pipe: NamedPipeServer,
    process: OwnedHandle,
    token: String,
    sequence: u64,
}
impl Session {
    async fn start() -> Result<Self> {
        ensure!(
            unsafe { jv_elevated() } == 0,
            "Основное приложение должно работать без повышения"
        );
        let id = Uuid::new_v4();
        let mut pipe = server(&id)?;
        let minutes = std::env::var("JARVIS_ADMIN_IDLE_MINUTES")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(15)
            .clamp(1, 60);
        let args = format!("--jarvis-elevated {id} {} {minutes}", std::process::id());
        let process = tokio::task::spawn_blocking(move || -> Result<OwnedHandle> {
            ensure!(
                unsafe { jv_safe_install() } == 1,
                "Для UAC требуется доверенная подпись Authenticode и установка в Program Files"
            );
            let wide: Vec<_> = args.encode_utf16().chain(Some(0)).collect();
            let raw = unsafe { jv_runas(wide.as_ptr()) };
            ensure!(
                !raw.is_null(),
                "Windows не запустила брокер: {}",
                std::io::Error::last_os_error()
            );
            Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
        })
        .await??;
        timeout(IO_TIMEOUT, pipe.connect()).await??;
        let pid = unsafe { GetProcessId(process.as_raw_handle()) };
        ensure!(
            pid != 0 && unsafe { jv_verify_peer(pipe.as_raw_handle(), pid, 1) } == 1,
            "Подлинность процесса брокера не подтверждена"
        );
        let token = Uuid::new_v4().to_string();
        timeout(
            IO_TIMEOUT,
            send(
                &mut pipe,
                &Hello {
                    token: token.clone(),
                },
            ),
        )
        .await??;
        let ready: bool = timeout(IO_TIMEOUT, read(&mut pipe)).await??;
        ensure!(ready, "Брокер отказал в соединении");
        Ok(Self {
            pipe,
            process,
            token,
            sequence: 0,
        })
    }
    async fn query(&mut self) -> Result<Value> {
        self.sequence += 1;
        send(
            &mut self.pipe,
            &Request {
                token: self.token.clone(),
                sequence: self.sequence,
                operation: Operation::SecurityLogCount,
            },
        )
        .await?;
        let reply: Reply = read(&mut self.pipe).await?;
        ensure!(reply.sequence == self.sequence, "Нарушен порядок сообщений");
        Ok(
            json!({"log":"Security","records":reply.records.context("Журнал безопасности недоступен")?}),
        )
    }
}
#[derive(Default)]
pub struct Admin {
    session: Mutex<Option<Session>>,
    generation: AtomicU64,
    starting: AtomicBool,
}
struct Starting<'a>(&'a AtomicBool);
impl Drop for Starting<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Admin {
    pub async fn active(&self) -> bool {
        let mut g = self.session.lock().await;
        if g.as_ref()
            .is_some_and(|s| unsafe { WaitForSingleObject(s.process.as_raw_handle(), 0) } != 258)
        {
            g.take();
        }
        g.is_some()
    }
    pub async fn set(&self, on: bool) -> Result<bool> {
        if !on {
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.session.lock().await.take();
            return Ok(false);
        }
        let epoch = self.generation.load(Ordering::SeqCst);
        if self.active().await {
            return Ok(true);
        }
        ensure!(
            !self.starting.swap(true, Ordering::AcqRel),
            "Запрос UAC уже открыт"
        );
        let _starting = Starting(&self.starting);
        let session = Session::start().await?;
        let mut g = self.session.lock().await;
        ensure!(
            self.generation.load(Ordering::SeqCst) == epoch,
            "Режим отменён"
        );
        *g = Some(session);
        Ok(true)
    }
    pub async fn log_status(&self) -> Result<Value> {
        // Take ownership so cancellation closes any partially consumed frame.
        let mut g = self.session.lock().await;
        let mut s = g.take().context("Режим администратора выключен")?;
        let v = timeout(IO_TIMEOUT, s.query()).await??;
        *g = Some(s);
        Ok(v)
    }
}
async fn broker(id: Uuid, parent: u32, idle: u32) -> Result<()> {
    ensure!(
        unsafe { jv_elevated() } == 1 && unsafe { jv_safe_install() } == 1,
        "Недоверенный запуск брокера"
    );
    // Identification prevents the unprivileged pipe server from impersonating us.
    let mut pipe = ClientOptions::new()
        .security_qos_flags(0x0010_0000 | 0x0001_0000)
        .open(address(&id))?;
    ensure!(
        unsafe { jv_verify_peer(pipe.as_raw_handle(), parent, 0) } == 1,
        "Недоверенный клиент"
    );
    let hello: Hello = timeout(IO_TIMEOUT, read(&mut pipe)).await??;
    Uuid::parse_str(&hello.token)?;
    send(&mut pipe, &true).await?;
    for sequence in 1u64.. {
        let req: Request =
            timeout(Duration::from_secs(u64::from(idle) * 60), read(&mut pipe)).await??;
        ensure!(
            req.token == hello.token && req.sequence == sequence && req.operation.risk() == 0,
            "Запрос отклонён"
        );
        let mut count = 0;
        let records = match req.operation {
            Operation::SecurityLogCount => {
                (unsafe { jv_security_log_count(&mut count) } != 0).then_some(count)
            }
        };
        timeout(IO_TIMEOUT, send(&mut pipe, &Reply { sequence, records })).await??;
    }
    Ok(())
}
pub fn entry() -> Result<bool> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_none_or(|s| s != "--jarvis-elevated") {
        ensure!(
            unsafe { jv_elevated() } == 0,
            "Запустите основной Jarvis без прав администратора"
        );
        return Ok(false);
    }
    if args.len() != 4 {
        bail!("Некорректные параметры брокера");
    }
    let id = Uuid::parse_str(&args[1].to_string_lossy())?;
    let pid = args[2].to_string_lossy().parse()?;
    let minutes = args[3].to_string_lossy().parse()?;
    ensure!((1..=60).contains(&minutes), "Некорректный тайм-аут");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(broker(id, pid, minutes))?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_shell_and_unknown_fields() {
        for v in [
            json!({"token":"x","sequence":1,"operation":"powershell_run"}),
            json!({"token":"x","sequence":1,"operation":"security_log_count","command":"x"}),
            json!({"operation":"security_log_count"}),
        ] {
            assert!(serde_json::from_value::<Request>(v).is_err());
        }
    }
}
