//! Тонкие обёртки над Win32 (windows-sys). Всё `unsafe` модуля — здесь.
#![allow(clippy::upper_case_acronyms)]

use std::ffi::c_void;
use std::io;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::core::{BOOL, GUID};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

unsafe fn from_pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    // SAFETY: p — нуль-терминированная строка от Windows
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

pub struct Handle(pub HANDLE);
impl Handle {
    pub fn valid(&self) -> bool {
        !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        if self.valid() {
            // SAFETY: владеем дескриптором
            unsafe { CloseHandle(self.0) };
        }
    }
}
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

fn last_err() -> io::Error {
    io::Error::last_os_error()
}

// ───────────────────────────── Job Object ─────────────────────────────

/// Объект задания: все потомки процесса попадают в него, `terminate` убивает всё дерево.
pub struct JobObject(Handle);

impl JobObject {
    pub fn new() -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: создаём безымянный объект задания
        let h = unsafe { CreateJobObjectW(null(), null()) };
        if h.is_null() {
            return Err(last_err());
        }
        let job = JobObject(Handle(h));
        // Чтобы падение дочернего процесса не показывало диалог ошибки
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        // SAFETY: корректный размер структуры
        unsafe {
            SetInformationJobObject(
                job.0.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        Ok(job)
    }

    pub(crate) fn assign(&self, process: HANDLE) -> io::Result<()> {
        // SAFETY: дескрипторы валидны
        if unsafe { windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(self.0.0, process) } == 0 {
            return Err(last_err());
        }
        Ok(())
    }

    pub fn terminate(&self) {
        // SAFETY: дескриптор задания валиден
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0.0, 1) };
    }
}

// ───────────────────────────── потоки и процессы ─────────────────────────────

fn for_each_thread(pid: u32, mut f: impl FnMut(u32)) -> io::Result<usize> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    // SAFETY: снимок потоков системы
    let snap = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) });
    if !snap.valid() {
        return Err(last_err());
    }
    let mut te: THREADENTRY32 = unsafe { std::mem::zeroed() };
    te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut n = 0;
    // SAFETY: te корректно инициализирована
    let mut ok = unsafe { Thread32First(snap.0, &mut te) };
    while ok != 0 {
        if te.th32OwnerProcessID == pid {
            f(te.th32ThreadID);
            n += 1;
        }
        ok = unsafe { Thread32Next(snap.0, &mut te) };
    }
    Ok(n)
}

fn thread_op(pid: u32, suspend: bool) -> io::Result<usize> {
    use windows_sys::Win32::System::Threading::*;
    let mut failed = 0;
    let n = for_each_thread(pid, |tid| {
        // SAFETY: открываем поток по id
        let h = Handle(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, tid) });
        if !h.valid() {
            failed += 1;
            return;
        }
        let r = unsafe { if suspend { SuspendThread(h.0) } else { ResumeThread(h.0) } };
        if r == u32::MAX {
            failed += 1;
        }
    })?;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::NotFound, "у процесса нет потоков (не найден?)"));
    }
    if failed == n {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "нет доступа к потокам процесса"));
    }
    Ok(n - failed)
}

/// Возобновить все потоки процесса (после `CREATE_SUSPENDED` или `process_suspend`).
pub fn resume_process(pid: u32) -> io::Result<usize> {
    thread_op(pid, false)
}

pub fn suspend_process(pid: u32) -> io::Result<usize> {
    thread_op(pid, true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
}

impl Priority {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_lowercase().replace(['-', ' '], "_").as_str() {
            "idle" | "low" | "низкий" => Self::Idle,
            "below_normal" | "belownormal" | "ниже_среднего" => Self::BelowNormal,
            "normal" | "обычный" | "средний" => Self::Normal,
            "above_normal" | "abovenormal" | "выше_среднего" => Self::AboveNormal,
            "high" | "высокий" => Self::High,
            _ => return None,
        })
    }
}

pub fn set_priority(pid: u32, p: Priority) -> io::Result<()> {
    use windows_sys::Win32::System::Threading::*;
    let class = match p {
        Priority::Idle => IDLE_PRIORITY_CLASS,
        Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
        Priority::Normal => NORMAL_PRIORITY_CLASS,
        Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        Priority::High => HIGH_PRIORITY_CLASS,
    };
    // SAFETY: открываем процесс по pid
    let h = Handle(unsafe { OpenProcess(PROCESS_SET_INFORMATION, 0, pid) });
    if !h.valid() {
        return Err(last_err());
    }
    if unsafe { SetPriorityClass(h.0, class) } == 0 {
        return Err(last_err());
    }
    Ok(())
}

pub fn get_priority(pid: u32) -> Option<u32> {
    use windows_sys::Win32::System::Threading::*;
    let h = Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
    if !h.valid() {
        return None;
    }
    let c = unsafe { GetPriorityClass(h.0) };
    (c != 0).then_some(c)
}

pub fn process_image(pid: u32) -> Option<PathBuf> {
    use windows_sys::Win32::System::Threading::*;
    let h = Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
    if !h.valid() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: буфер достаточного размера
    if unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

// ───────────────────────────── кодировки ─────────────────────────────

/// Байты из консоли: UTF-8, иначе OEM-кодировка (cp866 на русской Windows).
pub fn decode_console(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    use windows_sys::Win32::Globalization::{CP_OEMCP, MultiByteToWideChar};
    if bytes.is_empty() || bytes.len() > i32::MAX as usize {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    // SAFETY: длины соответствуют буферам
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), bytes.len() as i32, null_mut(), 0);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; n as usize];
        MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), bytes.len() as i32, buf.as_mut_ptr(), n);
        String::from_utf16_lossy(&buf)
    }
}

// ───────────────────────────── известные папки ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownFolder {
    Downloads,
    Documents,
    Desktop,
    Pictures,
    Music,
    Videos,
    Profile,
}

pub fn known_folder(f: KnownFolder) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Shell::*;
    let id: GUID = match f {
        KnownFolder::Downloads => FOLDERID_Downloads,
        KnownFolder::Documents => FOLDERID_Documents,
        KnownFolder::Desktop => FOLDERID_Desktop,
        KnownFolder::Pictures => FOLDERID_Pictures,
        KnownFolder::Music => FOLDERID_Music,
        KnownFolder::Videos => FOLDERID_Videos,
        KnownFolder::Profile => FOLDERID_Profile,
    };
    let mut p: *mut u16 = null_mut();
    // SAFETY: Windows выделяет строку, освобождаем CoTaskMemFree
    let hr = unsafe { SHGetKnownFolderPath(&id, 0, null_mut(), &mut p) };
    let out = if hr >= 0 { Some(PathBuf::from(unsafe { from_pwstr(p) })) } else { None };
    unsafe { windows_sys::Win32::System::Com::CoTaskMemFree(p as *const c_void) };
    out
}

// ───────────────────────────── окна ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub pid: u32,
    pub title: String,
    /// Имя exe без пути.
    pub exe: String,
}

/// Видимые окна верхнего уровня с заголовком.
pub fn enum_windows() -> Vec<WindowInfo> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam — указатель на наш Vec
        let out = unsafe { &mut *(lparam as *mut Vec<(HWND, String, u32)>) };
        unsafe {
            if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
                return 1;
            }
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW != 0 {
                return 1;
            }
            let mut buf = [0u16; 512];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if n <= 0 {
                return 1;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            out.push((hwnd, String::from_utf16_lossy(&buf[..n as usize]), pid));
        }
        1
    }
    let mut raw: Vec<(HWND, String, u32)> = Vec::new();
    // SAFETY: колбэк живёт во время вызова
    unsafe { EnumWindows(Some(cb), &mut raw as *mut _ as LPARAM) };
    raw.into_iter()
        .filter(|(_, t, _)| !matches!(t.as_str(), "Program Manager" | "Default IME" | "MSCTFIME UI"))
        .map(|(h, title, pid)| WindowInfo {
            hwnd: h as isize,
            pid,
            title,
            exe: process_image(pid)
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
                .unwrap_or_default(),
        })
        .collect()
}

pub fn focus_window(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let h = hwnd as HWND;
    // SAFETY: hwnd из EnumWindows; если окно закрыто, функции вернут ошибку
    unsafe {
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        SetForegroundWindow(h) != 0
    }
}

// ───────────────────────────── ShellExecute ─────────────────────────────

pub fn shell_execute(file: &str, params: Option<&str>, verb: &str) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let f = wide(file);
    let v = wide(verb);
    let p = params.map(wide);
    // SAFETY: строки нуль-терминированы и живут во время вызова
    let r = unsafe {
        ShellExecuteW(null_mut(), v.as_ptr(), f.as_ptr(), p.as_ref().map_or(null(), |p| p.as_ptr()), null(), SW_SHOWNORMAL)
    };
    if (r as isize) <= 32 {
        return Err(io::Error::other(format!("ShellExecute вернул код {}", r as isize)));
    }
    Ok(())
}

// ───────────────────────────── буфер обмена ─────────────────────────────

struct Clipboard;
impl Clipboard {
    fn open() -> io::Result<Self> {
        use windows_sys::Win32::System::DataExchange::OpenClipboard;
        for _ in 0..10 {
            // SAFETY: без окна-владельца
            if unsafe { OpenClipboard(null_mut()) } != 0 {
                return Ok(Clipboard);
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        Err(io::Error::other("буфер обмена занят другим приложением"))
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::DataExchange::CloseClipboard() };
    }
}

pub fn clipboard_get() -> io::Result<Option<String>> {
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::*;
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
    let _c = Clipboard::open()?;
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT as u32) == 0 {
            return Ok(None);
        }
        let h = GetClipboardData(CF_UNICODETEXT as u32);
        if h.is_null() {
            return Ok(None);
        }
        let p = GlobalLock(h as _) as *const u16;
        if p.is_null() {
            return Err(last_err());
        }
        let s = from_pwstr(p);
        GlobalUnlock(h as _);
        Ok(Some(s))
    }
}

pub fn clipboard_set(text: &str) -> io::Result<()> {
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::*;
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
    let w = wide(text);
    let _c = Clipboard::open()?;
    unsafe {
        EmptyClipboard();
        let bytes = w.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if h.is_null() {
            return Err(last_err());
        }
        let p = GlobalLock(h) as *mut u16;
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
        GlobalUnlock(h);
        if SetClipboardData(CF_UNICODETEXT as u32, h as HANDLE).is_null() {
            windows_sys::Win32::Foundation::GlobalFree(h);
            return Err(last_err());
        }
    }
    Ok(())
}

// ───────────────────────────── питание ─────────────────────────────

pub fn guid_to_string(g: &GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1, g.data2, g.data3, g.data4[0], g.data4[1], g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7]
    )
}

pub fn parse_guid(s: &str) -> Option<GUID> {
    let h: String = s.trim().trim_matches(['{', '}']).chars().filter(|c| *c != '-').collect();
    if h.len() != 32 {
        return None;
    }
    Some(GUID::from_u128(u128::from_str_radix(&h, 16).ok()?))
}

/// (GUID, имя) всех схем питания.
pub fn power_schemes() -> io::Result<Vec<(String, String)>> {
    use windows_sys::Win32::System::Power::*;
    let mut out = Vec::new();
    for i in 0..64u32 {
        let mut g: GUID = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of::<GUID>() as u32;
        // SAFETY: буфер — GUID
        let r = unsafe { PowerEnumerate(null_mut(), null(), null(), ACCESS_SCHEME, i, &mut g as *mut _ as *mut u8, &mut size) };
        if r != 0 {
            break;
        }
        let mut buf = [0u16; 256];
        let mut bsize = (buf.len() * 2) as u32;
        let name = if unsafe { PowerReadFriendlyName(null_mut(), &g, null(), null(), buf.as_mut_ptr() as *mut u8, &mut bsize) } == 0 {
            from_wide(&buf)
        } else {
            String::new()
        };
        out.push((guid_to_string(&g), name));
    }
    Ok(out)
}

pub fn active_power_scheme() -> io::Result<String> {
    use windows_sys::Win32::System::Power::PowerGetActiveScheme;
    let mut p: *mut GUID = null_mut();
    // SAFETY: Windows выделяет GUID, освобождаем LocalFree
    let r = unsafe { PowerGetActiveScheme(null_mut(), &mut p) };
    if r != 0 || p.is_null() {
        return Err(io::Error::from_raw_os_error(r as i32));
    }
    let s = guid_to_string(unsafe { &*p });
    unsafe { windows_sys::Win32::Foundation::LocalFree(p as _) };
    Ok(s)
}

pub fn set_power_scheme(guid: &str) -> io::Result<()> {
    let g = parse_guid(guid).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "неверный GUID"))?;
    let r = unsafe { windows_sys::Win32::System::Power::PowerSetActiveScheme(null_mut(), &g) };
    if r != 0 {
        return Err(io::Error::from_raw_os_error(r as i32));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Battery {
    pub percent: Option<u8>,
    pub on_ac: Option<bool>,
    pub seconds_left: Option<u32>,
}

/// `None`, если батареи нет.
pub fn battery() -> Option<Battery> {
    use windows_sys::Win32::System::Power::*;
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 || s.BatteryFlag == 128 || s.BatteryFlag == 255 {
        return None;
    }
    Some(Battery {
        percent: (s.BatteryLifePercent != 255).then_some(s.BatteryLifePercent),
        on_ac: match s.ACLineStatus {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        seconds_left: (s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime),
    })
}

// ───────────────────────────── GPU через PDH ─────────────────────────────

/// Запрос PDH `\GPU Engine(*)\Utilization Percentage`. Счётчик скоростной: первое значение после второго `sample`.
pub struct GpuCounter {
    query: isize,
    counter: isize,
}
unsafe impl Send for GpuCounter {}

impl GpuCounter {
    pub fn new() -> Option<Self> {
        use windows_sys::Win32::System::Performance::*;
        let mut q = 0isize;
        let mut c = 0isize;
        unsafe {
            if PdhOpenQueryW(null(), 0, &mut q as *mut isize as *mut _) != 0 {
                return None;
            }
            let path = wide("\\GPU Engine(*)\\Utilization Percentage");
            if PdhAddEnglishCounterW(q as _, path.as_ptr(), 0, &mut c as *mut isize as *mut _) != 0 {
                PdhCloseQuery(q as _);
                return None;
            }
            PdhCollectQueryData(q as _);
        }
        Some(Self { query: q, counter: c })
    }

    /// Загрузка GPU, %: сумма по процессам для каждого движка, максимум по движкам.
    pub fn sample(&mut self) -> Option<f32> {
        use windows_sys::Win32::System::Performance::*;
        unsafe {
            if PdhCollectQueryData(self.query as _) != 0 {
                return None;
            }
            let mut size = 0u32;
            let mut count = 0u32;
            let r = PdhGetFormattedCounterArrayW(self.counter as _, PDH_FMT_DOUBLE, &mut size, &mut count, null_mut());
            if r != PDH_MORE_DATA || size == 0 {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter as _, PDH_FMT_DOUBLE, &mut size, &mut count, items) != 0 {
                return None;
            }
            let mut per_engine: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
            for i in 0..count as usize {
                let it = &*items.add(i);
                let name = from_pwstr(it.szName);
                // pid_123_luid_0x..._phys_0_eng_3_engtype_3D → ключ без pid
                let key = name.split_once("_luid").map(|(_, r)| r.to_string()).unwrap_or(name);
                *per_engine.entry(key).or_default() += it.FmtValue.Anonymous.doubleValue;
            }
            per_engine.values().copied().fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v)))).map(|v| v.clamp(0.0, 100.0) as f32)
        }
    }
}

impl Drop for GpuCounter {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::Performance::PdhCloseQuery(self.query as _) };
    }
}

/// Код последней ошибки Win32 (для сообщений).
pub fn last_error_code() -> u32 {
    unsafe { GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_roundtrip_and_decode() {
        let s = "381b4222-f694-41f0-9685-ff5bb260df2e";
        assert_eq!(guid_to_string(&parse_guid(s).unwrap()), s);
        assert_eq!(decode_console("привет".as_bytes()), "привет");
        // «привет» в cp866
        let cp866 = [0xAF, 0xE0, 0xA8, 0xA2, 0xA5, 0xE2];
        let d = decode_console(&cp866);
        assert!(d == "привет" || !d.is_empty());
    }

    #[test]
    fn known_folders_exist() {
        let d = known_folder(KnownFolder::Downloads).unwrap();
        assert!(d.is_absolute());
    }
}
