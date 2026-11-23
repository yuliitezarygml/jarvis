//! Окно как живые обои: дочернее окно слоя обоев рабочего стола (под иконками), как у Wallpaper Engine.
//!
//! Рабочий стол Windows: `Progman` → `SHELLDLL_DefView` (иконки) и `WorkerW` (картинка обоев).
//! Сообщение 0x052C просит Progman создать слой `WorkerW`. Дальше два варианта раскладки:
//! - Windows 11 24H2+: `WorkerW` — дочернее окно `Progman`. Наше окно делаем дочерним `Progman`
//!   и ставим по Z сразу под `SHELLDLL_DefView`: над картинкой обоев, под иконками.
//! - Раньше: `WorkerW` — отдельное окно верхнего уровня за окном с `SHELLDLL_DefView`;
//!   наше окно становится дочерним этого `WorkerW`.
//! Клики идут иконкам/рабочему столу (наше окно под ними). Если Explorer перезапустится, слой пропадёт
//! вместе с нашим окном — `windows_manager` создаёт его заново.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, GA_PARENT, GetAncestor, GetSystemMetrics, IsWindow, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN, SMTO_NORMAL, SPI_GETDESKWALLPAPER, SPI_SETDESKWALLPAPER, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageTimeoutW, SetParent, SetWindowPos, SystemParametersInfoW,
    };
    use windows::core::{BOOL, w};

    /// Куда прикреплено окно (для проверки, жив ли слой).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Attached {
        pub parent: isize,
    }

    fn h(raw: isize) -> HWND {
        HWND(raw as *mut c_void)
    }

    /// Найти окно верхнего уровня с `SHELLDLL_DefView` и вернуть `WorkerW`, идущий за ним (классическая раскладка).
    unsafe extern "system" fn find_worker(top: HWND, out: LPARAM) -> BOOL {
        // SAFETY: системные вызовы на дескрипторах, которые даёт EnumWindows; out — *mut isize от вызывающего.
        unsafe {
            if FindWindowExW(Some(top), None, w!("SHELLDLL_DefView"), None).is_ok()
                && let Ok(worker) = FindWindowExW(None, Some(top), w!("WorkerW"), None)
            {
                *(out.0 as *mut isize) = worker.0 as isize;
                return BOOL(0);
            }
        }
        BOOL(1)
    }

    /// Прикрепить окно `hwnd` к слою обоев и растянуть на прямоугольник экрана (физические пиксели).
    pub fn attach(hwnd: isize, x: i32, y: i32, w_: i32, h_: i32) -> Result<Attached, String> {
        // SAFETY: только системные вызовы Win32 на окнах рабочего стола и своём окне.
        unsafe {
            let progman = FindWindowW(w!("Progman"), None).map_err(|e| format!("нет окна Progman: {e}"))?;
            // Попросить Progman создать слой WorkerW (без ответа — не страшно, раскладка могла уже быть).
            let mut res = 0usize;
            let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0xD), LPARAM(0x1), SMTO_NORMAL, 1000, Some(&mut res));
            let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
            let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;

            // 24H2+: WorkerW внутри Progman.
            if FindWindowExW(Some(progman), None, w!("WorkerW"), None).is_ok() {
                SetParent(h(hwnd), Some(progman)).map_err(|e| format!("SetParent(Progman): {e}"))?;
                let icons = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None).ok();
                SetWindowPos(h(hwnd), icons, x - vx, y - vy, w_, h_, flags).map_err(|e| format!("SetWindowPos: {e}"))?;
                return Ok(Attached { parent: progman.0 as isize });
            }

            // Классика: WorkerW верхнего уровня за окном с иконками.
            let mut worker: isize = 0;
            let _ = EnumWindows(Some(find_worker), LPARAM(&mut worker as *mut isize as isize));
            if worker == 0 {
                return Err("не найден слой обоев WorkerW".into());
            }
            SetParent(h(hwnd), Some(h(worker))).map_err(|e| format!("SetParent(WorkerW): {e}"))?;
            SetWindowPos(h(hwnd), None, x - vx, y - vy, w_, h_, flags).map_err(|e| format!("SetWindowPos: {e}"))?;
            Ok(Attached { parent: worker })
        }
    }

    /// Окно всё ещё на слое обоев (Explorer не перезапускался).
    pub fn still_attached(hwnd: isize, a: Attached) -> bool {
        // GetParent у окна без WS_CHILD возвращает владельца, а не родителя — поэтому GetAncestor(GA_PARENT).
        // SAFETY: проверки существования окон.
        unsafe {
            IsWindow(Some(h(hwnd))).as_bool()
                && IsWindow(Some(h(a.parent))).as_bool()
                && GetAncestor(h(hwnd), GA_PARENT).0 as isize == a.parent
        }
    }

    /// Открепить окно (перед закрытием) и попросить Windows перерисовать обычные обои:
    /// иначе на рабочем столе остаётся последний кадр.
    pub fn detach(hwnd: isize) {
        // SAFETY: системные вызовы; буфер пути живёт до конца функции.
        unsafe {
            let _ = SetParent(h(hwnd), None);
            let mut buf = [0u16; 520];
            if SystemParametersInfoW(
                SPI_GETDESKWALLPAPER,
                buf.len() as u32,
                Some(buf.as_mut_ptr() as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .is_ok()
            {
                let _ = SystemParametersInfoW(
                    SPI_SETDESKWALLPAPER,
                    0,
                    Some(buf.as_mut_ptr() as *mut c_void),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                );
            }
        }
    }
}

/// Пересылка мыши в обои, как у Wallpaper Engine: обои лежат под иконками, поэтому клики получает
/// рабочий стол. Глобальный перехватчик мыши (WH_MOUSE_LL) смотрит, что под курсором; если это пустой
/// рабочий стол (иконки, слой обоев) — копия движения/клика/прокрутки уходит в окно ввода WebView2 обоев.
/// Клавиатуру так не передать: поле ввода на обоях открывает обычное окно HUD.
#[cfg(windows)]
mod input {
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, EnumChildWindows, GA_ROOT, GetAncestor, GetClassNameW, GetMessageW, IsWindow, MSG,
        MSLLHOOKSTRUCT, PostMessageW, SetWindowsHookExW, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WindowFromPoint,
    };
    use windows::core::BOOL;

    /// Окно обоев (корень) и найденное в нём окно ввода Chromium.
    static ROOT: AtomicIsize = AtomicIsize::new(0);
    static TARGET: AtomicIsize = AtomicIsize::new(0);
    static STARTED: AtomicBool = AtomicBool::new(false);

    fn h(raw: isize) -> HWND {
        HWND(raw as *mut std::ffi::c_void)
    }

    fn class_of(w: HWND) -> String {
        let mut buf = [0u16; 64];
        // SAFETY: буфер на стеке, длина передаётся.
        let n = unsafe { GetClassNameW(w, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    unsafe extern "system" fn find_render(child: HWND, out: LPARAM) -> BOOL {
        if class_of(child) == "Chrome_RenderWidgetHostHWND" {
            // SAFETY: out — *mut isize от вызывающего.
            unsafe { *(out.0 as *mut isize) = child.0 as isize };
            return BOOL(0);
        }
        BOOL(1)
    }

    /// Окно ввода WebView2 внутри обоев (ищется заново, если пропало).
    fn target() -> Option<HWND> {
        let root = ROOT.load(Ordering::Relaxed);
        if root == 0 {
            return None;
        }
        let t = TARGET.load(Ordering::Relaxed);
        // SAFETY: проверка существования окна.
        if t != 0 && unsafe { IsWindow(Some(h(t))).as_bool() } {
            return Some(h(t));
        }
        let mut found: isize = 0;
        // SAFETY: обход дочерних окон своего окна.
        unsafe {
            let _ = EnumChildWindows(Some(h(root)), Some(find_render), LPARAM(&mut found as *mut isize as isize));
        }
        TARGET.store(found, Ordering::Relaxed);
        (found != 0).then(|| h(found))
    }

    /// Под курсором пустой рабочий стол (иконки или слой обоев), а не окно программы.
    fn is_desktop(w: HWND) -> bool {
        if w.0.is_null() {
            return false;
        }
        // SAFETY: чтение свойств окна.
        let root = unsafe { GetAncestor(w, GA_ROOT) };
        matches!(class_of(root).as_str(), "Progman" | "WorkerW")
            && matches!(class_of(w).as_str(), "SysListView32" | "SHELLDLL_DefView" | "WorkerW" | "Progman")
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && ROOT.load(Ordering::Relaxed) != 0 {
            // SAFETY: для WH_MOUSE_LL lparam указывает на MSLLHOOKSTRUCT.
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let msg = wparam.0 as u32;
            let interesting = matches!(
                msg,
                WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MOUSEWHEEL
            );
            // SAFETY: системные вызовы; PostMessage не блокирует (перехватчик должен отвечать быстро).
            unsafe {
                if msg == WM_LBUTTONDOWN {
                    let under = WindowFromPoint(info.pt);
                    tracing::info!(
                        x = info.pt.x,
                        y = info.pt.y,
                        under = %class_of(under),
                        root = %class_of(GetAncestor(under, GA_ROOT)),
                        desktop = is_desktop(under),
                        target = target().is_some(),
                        "живые обои: клик"
                    );
                }
                if interesting
                    && is_desktop(WindowFromPoint(info.pt))
                    && let Some(t) = target()
                {
                    let mut p = POINT { x: info.pt.x, y: info.pt.y };
                    let _ = ScreenToClient(t, &mut p);
                    let client = LPARAM((((p.y as u16) as isize) << 16) | ((p.x as u16) as isize));
                    let (wp, lp) = match msg {
                        WM_LBUTTONDOWN => (WPARAM(0x0001), client),
                        WM_RBUTTONDOWN => (WPARAM(0x0002), client),
                        // У колеса координаты экранные, в старшем слове wParam — шаг прокрутки.
                        WM_MOUSEWHEEL => (
                            WPARAM((info.mouseData & 0xFFFF_0000) as usize),
                            LPARAM((((info.pt.y as u16) as isize) << 16) | ((info.pt.x as u16) as isize)),
                        ),
                        _ => (WPARAM(0), client),
                    };
                    let _ = PostMessageW(Some(t), msg, wp, lp);
                }
            }
        }
        // SAFETY: передать событие дальше по цепочке — рабочий стол получает его как обычно.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// Начать пересылать мышь в окно обоев `root`.
    pub fn forward_to(root: isize) {
        ROOT.store(root, Ordering::Relaxed);
        TARGET.store(0, Ordering::Relaxed);
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = std::thread::Builder::new().name("wallpaper-mouse".into()).spawn(|| {
            // SAFETY: перехватчик ставится и обслуживается в этом потоке (нужен цикл сообщений).
            unsafe {
                let module = GetModuleHandleW(None).ok();
                let Ok(_hook) = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), module.map(Into::into), 0) else {
                    tracing::warn!("живые обои: перехватчик мыши не установлен — клики по обоям не работают");
                    STARTED.store(false, Ordering::SeqCst);
                    return;
                };
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            }
        });
    }

    /// Перестать пересылать (обои выключены). Перехватчик остаётся, но ничего не делает.
    pub fn stop() {
        ROOT.store(0, Ordering::Relaxed);
        TARGET.store(0, Ordering::Relaxed);
    }
}

#[cfg(windows)]
pub use imp::{Attached, attach, detach, still_attached};
#[cfg(windows)]
pub use input::{forward_to as forward_mouse_to, stop as stop_mouse_forwarding};

#[cfg(not(windows))]
pub fn forward_mouse_to(_: isize) {}
#[cfg(not(windows))]
pub fn stop_mouse_forwarding() {}

#[cfg(not(windows))]
mod imp_stub {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Attached {
        pub parent: isize,
    }
    pub fn attach(_: isize, _: i32, _: i32, _: i32, _: i32) -> Result<Attached, String> {
        Err("живые обои есть только в Windows".into())
    }
    pub fn still_attached(_: isize, _: Attached) -> bool {
        false
    }
    pub fn detach(_: isize) {}
}

#[cfg(not(windows))]
pub use imp_stub::{Attached, attach, detach, still_attached};
