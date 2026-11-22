//! Окна верхнего уровня, мониторы, фокус, исключение окон из захвата экрана.

use jarvis_core::{Rect, ToolError};
use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GA_ROOT, GWL_EXSTYLE, GetAncestor, GetClassNameW, GetForegroundWindow,
    GetSystemMetrics, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SW_RESTORE, SetForegroundWindow, SetWindowDisplayAffinity, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

use crate::args::fold;
use crate::dpi::DpiScope;

/// Окно верхнего уровня. `hwnd` хранится числом: HWND не `Send`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub class: String,
    pub pid: u32,
    /// Имя exe: `notepad.exe`.
    pub process: String,
    pub rect: Rect,
    pub minimized: bool,
    pub foreground: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// 0 — основной, дальше слева направо.
    pub index: usize,
    pub name: String,
    pub rect: Rect,
    pub work: Rect,
    pub primary: bool,
}

pub(crate) fn hwnd(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

pub(crate) fn rect_from(r: RECT) -> Rect {
    Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

/// Пересечение прямоугольников (может быть пустым: w/h = 0).
pub(crate) fn intersect(a: Rect, b: Rect) -> Rect {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.w).min(b.x + b.w);
    let y2 = (a.y + a.h).min(b.y + b.h);
    Rect { x: x1, y: y1, w: (x2 - x1).max(0), h: (y2 - y1).max(0) }
}

pub(crate) fn contains(r: Rect, x: i32, y: i32) -> bool {
    x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h
}

pub(crate) fn center(r: Rect) -> (i32, i32) {
    (r.x + r.w / 2, r.y + r.h / 2)
}

/// Виртуальный рабочий стол (все мониторы) в физических пикселях.
pub fn virtual_screen() -> Rect {
    let _dpi = DpiScope::per_monitor();
    // SAFETY: чтение системных метрик.
    unsafe {
        Rect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

unsafe extern "system" fn enum_windows_cb(h: HWND, lp: LPARAM) -> BOOL {
    // SAFETY: lp — указатель на Vec<isize>, живущий на время EnumWindows.
    let v = unsafe { &mut *(lp.0 as *mut Vec<isize>) };
    v.push(h.0 as isize);
    BOOL(1)
}

/// Все окна верхнего уровня в Z-порядке (сверху вниз).
pub(crate) fn top_level_hwnds() -> Vec<isize> {
    let mut v: Vec<isize> = Vec::new();
    // SAFETY: колбэк пишет только в v.
    unsafe {
        let _ = EnumWindows(Some(enum_windows_cb), LPARAM(&mut v as *mut Vec<isize> as isize));
    }
    v
}

pub(crate) fn window_title(h: isize) -> String {
    // SAFETY: буфер нужного размера.
    unsafe {
        let len = GetWindowTextLengthW(hwnd(h));
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd(h), &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..n.min(buf.len())])
    }
}

pub(crate) fn window_class(h: isize) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: буфер фиксированного размера.
    let n = unsafe { GetClassNameW(hwnd(h), &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

fn is_cloaked(h: isize) -> bool {
    let mut cloaked: u32 = 0;
    // SAFETY: DWMWA_CLOAKED пишет u32.
    unsafe {
        DwmGetWindowAttribute(hwnd(h), DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4).is_ok() && cloaked != 0
    }
}

/// Видимые границы окна (без невидимой рамки DWM).
pub fn window_rect(h: isize) -> Option<Rect> {
    let _dpi = DpiScope::per_monitor();
    let mut r = RECT::default();
    // SAFETY: RECT нужного размера.
    unsafe {
        if DwmGetWindowAttribute(
            hwnd(h),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
        {
            return Some(rect_from(r));
        }
        GetWindowRect(hwnd(h), &mut r).ok().map(|_| rect_from(r))
    }
}

fn window_pid(h: isize) -> u32 {
    let mut pid = 0u32;
    // SAFETY: pid — выходной параметр.
    unsafe { GetWindowThreadProcessId(hwnd(h), Some(&mut pid)) };
    pid
}

pub(crate) fn process_name(pid: u32) -> String {
    // SAFETY: дескриптор закрываем сами.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok();
        let _ = CloseHandle(handle);
        if !ok {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        path.rsplit(['\\', '/']).next().unwrap_or_default().to_string()
    }
}

/// Окно «приложения»: видимое, не скрытое DWM, с заголовком, не tool-window.
fn is_app_window(h: isize) -> bool {
    // SAFETY: чтение свойств окна.
    unsafe {
        if !IsWindowVisible(hwnd(h)).as_bool() || GetAncestor(hwnd(h), GA_ROOT).0 as isize != h {
            return false;
        }
        let ex = GetWindowLongW(hwnd(h), GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
            return false;
        }
    }
    !is_cloaked(h) && !window_title(h).trim().is_empty()
}

fn own_pid() -> u32 {
    // SAFETY: без аргументов.
    unsafe { GetCurrentProcessId() }
}

pub(crate) fn foreground_hwnd() -> isize {
    // SAFETY: без аргументов.
    unsafe { GetForegroundWindow().0 as isize }
}

/// Информация об окне (без фильтров «приложения»).
pub fn window_info(h: isize) -> Option<WindowInfo> {
    // SAFETY: проверка дескриптора.
    if h == 0 || !unsafe { IsWindow(Some(hwnd(h))) }.as_bool() {
        return None;
    }
    let pid = window_pid(h);
    Some(WindowInfo {
        hwnd: h,
        title: window_title(h),
        class: window_class(h),
        pid,
        process: process_name(pid),
        rect: window_rect(h).unwrap_or_default(),
        // SAFETY: чтение состояния окна.
        minimized: unsafe { IsIconic(hwnd(h)) }.as_bool(),
        foreground: foreground_hwnd() == h,
    })
}

/// Окна приложений в Z-порядке, без окон самого Джарвиса (HUD/overlay).
pub fn list_windows() -> Vec<WindowInfo> {
    let me = own_pid();
    top_level_hwnds()
        .into_iter()
        .filter(|&h| is_app_window(h) && window_pid(h) != me)
        .filter_map(window_info)
        .collect()
}

/// Активное окно пользователя. Если в фокусе окно Джарвиса — верхнее окно приложения под ним.
pub fn foreground_app_window() -> Option<WindowInfo> {
    let fg = foreground_hwnd();
    if fg != 0 && window_pid(fg) != own_pid() {
        if let Some(w) = window_info(fg) {
            return Some(w);
        }
    }
    list_windows().into_iter().find(|w| !w.minimized)
}

/// Поиск окна по подстроке заголовка или имени процесса (без учёта регистра).
/// Порядок: активное окно → точное совпадение заголовка → по Z-порядку.
pub fn find_window(query: &str) -> Option<WindowInfo> {
    let q = fold(query.trim());
    if q.is_empty() {
        return None;
    }
    let windows = list_windows();
    let matches = |w: &WindowInfo| {
        fold(&w.title).contains(&q) || fold(&w.process).trim_end_matches(".exe") == q.trim_end_matches(".exe")
    };
    windows
        .iter()
        .find(|w| w.foreground && matches(w))
        .or_else(|| windows.iter().find(|w| fold(&w.title) == q))
        .or_else(|| windows.iter().find(|w| matches(w)))
        .cloned()
}

fn find_by_class(class: &str) -> Option<WindowInfo> {
    let hwnds = top_level_hwnds();
    let matching: Vec<isize> = hwnds.into_iter().filter(|&h| window_class(h).eq_ignore_ascii_case(class)).collect();
    // SAFETY: чтение видимости окна.
    let visible = matching.iter().copied().find(|&h| unsafe { IsWindowVisible(hwnd(h)) }.as_bool());
    visible.or(matching.first().copied()).and_then(window_info)
}

/// Окно по запросу агента: `None`/пусто — активное окно пользователя; `class:Имя`; `hwnd:123`;
/// `taskbar`/`панель задач`; иначе — подстрока заголовка или имя процесса.
pub fn resolve_window(query: Option<&str>) -> Result<WindowInfo, ToolError> {
    let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) else {
        return foreground_app_window().ok_or_else(|| ToolError::Failed("нет активного окна".into()));
    };
    let lower = q.to_lowercase();
    if matches!(lower.as_str(), "taskbar" | "панель задач" | "панель_задач") {
        return find_by_class("Shell_TrayWnd").ok_or_else(|| ToolError::Failed("панель задач не найдена".into()));
    }
    if let Some(c) = q.strip_prefix("class:") {
        return find_by_class(c.trim()).ok_or_else(|| ToolError::Failed(format!("нет окна класса «{}»", c.trim())));
    }
    if let Some(h) = q.strip_prefix("hwnd:") {
        let h = h.trim();
        let parsed = match h.strip_prefix("0x") {
            Some(hex) => isize::from_str_radix(hex, 16).ok(),
            None => h.parse::<isize>().ok(),
        };
        return parsed.and_then(window_info).ok_or_else(|| ToolError::Failed(format!("нет окна {q}")));
    }
    find_window(q).ok_or_else(|| {
        let titles: Vec<String> = list_windows().into_iter().take(10).map(|w| format!("«{}»", w.title)).collect();
        ToolError::Failed(format!("окно «{q}» не найдено. Открыты: {}", titles.join(", ")))
    })
}

/// Вывести окно на передний план. Обходит блокировку смены фокуса (последний ввод — от нас).
pub fn focus_window(h: isize) -> Result<(), ToolError> {
    let _dpi = DpiScope::per_monitor();
    // SAFETY: стандартная последовательность активации окна.
    unsafe {
        if !IsWindow(Some(hwnd(h))).as_bool() {
            return Err(ToolError::Failed("окно уже закрыто".into()));
        }
        if IsIconic(hwnd(h)).as_bool() {
            let _ = ShowWindow(hwnd(h), SW_RESTORE);
        }
        if foreground_hwnd() == h {
            return Ok(());
        }
        // Нулевое движение мыши: наш процесс становится источником последнего ввода → SetForegroundWindow разрешён.
        let nudge = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } },
        };
        SendInput(&[nudge], std::mem::size_of::<INPUT>() as i32);
        if SetForegroundWindow(hwnd(h)).as_bool() && wait_foreground(h) {
            return Ok(());
        }
        // Запасной путь: присоединиться к очереди ввода текущего активного окна.
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
        let _ = BringWindowToTop(hwnd(h));
        let _ = SetForegroundWindow(hwnd(h));
        if attached {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
    if wait_foreground(h) { Ok(()) } else { Err(ToolError::Failed("Windows не дал переключить фокус на окно".into())) }
}

fn wait_foreground(h: isize) -> bool {
    for _ in 0..20 {
        if foreground_hwnd() == h {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    false
}

unsafe extern "system" fn enum_monitors_cb(m: HMONITOR, _dc: HDC, _r: *mut RECT, lp: LPARAM) -> BOOL {
    // SAFETY: lp — указатель на Vec<HMONITOR-как-isize>.
    let v = unsafe { &mut *(lp.0 as *mut Vec<isize>) };
    v.push(m.0 as isize);
    BOOL(1)
}

/// Мониторы: [0] — основной, остальные слева направо.
pub fn monitors() -> Vec<MonitorInfo> {
    let _dpi = DpiScope::per_monitor();
    let mut handles: Vec<isize> = Vec::new();
    // SAFETY: колбэк пишет только в handles.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(enum_monitors_cb), LPARAM(&mut handles as *mut Vec<isize> as isize));
    }
    let mut out: Vec<MonitorInfo> = handles
        .into_iter()
        .filter_map(|m| {
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            // SAFETY: MONITORINFOEXW начинается с MONITORINFO, cbSize выставлен.
            let ok = unsafe { GetMonitorInfoW(HMONITOR(m as *mut c_void), &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
            ok.as_bool().then(|| {
                let n = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
                MonitorInfo {
                    index: 0,
                    name: String::from_utf16_lossy(&info.szDevice[..n]),
                    rect: rect_from(info.monitorInfo.rcMonitor),
                    work: rect_from(info.monitorInfo.rcWork),
                    primary: info.monitorInfo.dwFlags & 1 != 0,
                }
            })
        })
        .collect();
    out.sort_by_key(|m| (!m.primary, m.rect.x, m.rect.y));
    for (i, m) in out.iter_mut().enumerate() {
        m.index = i;
    }
    out
}

/// Монитор, на котором больше всего площади прямоугольника (по умолчанию — основной).
pub(crate) fn monitor_for(r: Rect) -> Option<MonitorInfo> {
    let mons = monitors();
    mons.iter()
        .max_by_key(|m| {
            let i = intersect(m.rect, r);
            i.w as i64 * i.h as i64
        })
        .filter(|m| {
            let i = intersect(m.rect, r);
            i.w > 0 && i.h > 0
        })
        .or_else(|| mons.first())
        .cloned()
}

/// Скрыть окно из любых снимков экрана (включая наши): вызывать для окон HUD и оверлея.
/// `hwnd` — сырой дескриптор (`tauri::WebviewWindow::hwnd()?.0 as isize`). Окно должно принадлежать этому процессу.
pub fn exclude_from_capture(hwnd_raw: isize) -> Result<(), ToolError> {
    // SAFETY: системный вызов на дескрипторе окна.
    unsafe { SetWindowDisplayAffinity(hwnd(hwnd_raw), WDA_EXCLUDEFROMCAPTURE) }
        .map_err(|e| ToolError::Failed(format!("SetWindowDisplayAffinity: {e}")))
}

/// Вернуть окно в снимки экрана.
pub fn include_in_capture(hwnd_raw: isize) -> Result<(), ToolError> {
    // SAFETY: системный вызов на дескрипторе окна.
    unsafe { SetWindowDisplayAffinity(hwnd(hwnd_raw), WDA_NONE) }
        .map_err(|e| ToolError::Failed(format!("SetWindowDisplayAffinity: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_math() {
        let a = Rect { x: 0, y: 0, w: 100, h: 50 };
        let b = Rect { x: 80, y: 40, w: 100, h: 100 };
        assert_eq!(intersect(a, b), Rect { x: 80, y: 40, w: 20, h: 10 });
        assert_eq!(intersect(a, Rect { x: 200, y: 0, w: 5, h: 5 }).w, 0);
        assert!(contains(a, 0, 0) && !contains(a, 100, 10));
        assert_eq!(center(a), (50, 25));
    }

    #[test]
    fn virtual_screen_and_monitors_are_sane() {
        let vs = virtual_screen();
        assert!(vs.w > 0 && vs.h > 0, "{vs:?}");
        let mons = monitors();
        assert!(!mons.is_empty());
        assert!(mons[0].primary);
        for m in &mons {
            let i = intersect(vs, m.rect);
            assert_eq!(i, m.rect, "монитор вне виртуального стола");
        }
    }

    #[test]
    fn window_list_reads() {
        // Только чтение: список окон и активное окно.
        let list = list_windows();
        for w in &list {
            assert!(!w.title.is_empty());
            assert_ne!(w.pid, own_pid());
        }
        let _ = foreground_app_window();
    }
}
