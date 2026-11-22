//! Computer control through Windows UI Automation (the accessibility tree), with SendInput as a fallback.
//!
//! The model never sees raw pixels here. It lists windows, reads a numbered tree of the controls in one
//! window, then acts on a control by its number. Every action returns the refreshed tree of the window.
//! All COM objects live on one dedicated thread; callers talk to it through a channel.
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex, OnceLock,
    },
    time::Duration,
};
use windows::{
    core::{BOOL, BSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, RECT},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{Accessibility::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

/// Set by the global kill switch (Ctrl+Alt+K); cleared when a new turn starts.
static STOP: AtomicBool = AtomicBool::new(false);
pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}
pub fn reset_stop() {
    STOP.store(false, Ordering::SeqCst);
}
/// Registers Ctrl+Alt+K system-wide. `on_stop` runs on the hotkey thread.
pub fn spawn_kill_switch(on_stop: impl Fn() + Send + 'static) {
    std::thread::spawn(move || unsafe {
        if RegisterHotKey(None, 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x4B).is_err() {
            eprintln!("Ctrl+Alt+K занято другой программой: аварийная остановка недоступна");
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOTKEY {
                stop();
                on_stop();
            }
        }
    });
}

pub const TOOLS: [&str; 7] = [
    "ui_windows",
    "ui_state",
    "ui_click",
    "ui_set_value",
    "ui_type",
    "ui_press_key",
    "ui_scroll",
];
pub fn is_ui_tool(name: &str) -> bool {
    TOOLS.contains(&name)
}
pub fn schemas() -> Vec<Value> {
    fn s(name: &str, desc: &str, props: Value, req: &[&str]) -> Value {
        json!({"type":"function","function":{"name":name,"description":desc,"parameters":{"type":"object","properties":props,"required":req,"additionalProperties":false}}})
    }
    let w = json!({"type":"integer","description":"id окна из ui_windows"});
    let e =
        json!({"type":"integer","description":"номер элемента из последнего ui_state этого окна"});
    vec![
        s("ui_windows","Управление компьютером: список открытых окон (id, заголовок, программа). Начинай с него.",json!({}),&[]),
        s("ui_state","Управление компьютером: пронумерованный список элементов окна (кнопки, поля, текст). Номера нужны для остальных ui_* инструментов.",json!({"window":w}),&["window"]),
        s("ui_click","Нажать элемент по номеру (через UI Automation, без мыши, если возможно). Либо клик мышью по координатам экрана x,y. Любое действие возвращает обновлённый список элементов.",json!({"window":w,"element":e,"x":{"type":"integer"},"y":{"type":"integer"},"button":{"type":"string","enum":["left","right"]},"double":{"type":"boolean"},"mouse":{"type":"boolean","description":"true: принудительно кликнуть мышью по центру элемента"}}),&["window"]),
        s("ui_set_value","Записать текст в поле ввода по номеру элемента (заменяет содержимое).",json!({"window":w,"element":e,"text":{"type":"string","maxLength":4000}}),&["window","element","text"]),
        s("ui_type","Напечатать текст с клавиатуры в окно. Если указан element, сначала фокус на нём. \\n — Enter.",json!({"window":w,"element":e,"text":{"type":"string","maxLength":2000}}),&["window","text"]),
        s("ui_press_key","Нажать сочетание клавиш в окне, например ctrl+s, enter, esc, f5, ctrl+shift+t.",json!({"window":w,"keys":{"type":"string","maxLength":40}}),&["window","keys"]),
        s("ui_scroll","Прокрутить колесом мыши над элементом или центром окна. Положительное dy вниз, отрицательное вверх (в строках).",json!({"window":w,"element":e,"dy":{"type":"integer","minimum":-50,"maximum":50}}),&["window","dy"]),
    ]
}

type Job = Box<dyn FnOnce(&mut State) + Send>;
struct State {
    auto: IUIAutomation,
    elements: Vec<IUIAutomationElement>,
    window: isize,
}
static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
fn worker() -> &'static mpsc::Sender<Job> {
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            let auto = unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            };
            let Ok(auto) = auto else { return };
            let mut state = State {
                auto,
                elements: vec![],
                window: 0,
            };
            while let Ok(job) = rx.recv() {
                job(&mut state);
            }
        });
        tx
    })
}
async fn on_worker<T: Send + 'static>(
    f: impl FnOnce(&mut State) -> Result<T> + Send + 'static,
) -> Result<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    worker()
        .send(Box::new(move |s| {
            let _ = tx.send(f(s));
        }))
        .map_err(|_| anyhow::anyhow!("Поток UI Automation недоступен"))?;
    rx.await.context("Поток UI Automation остановился")?
}

fn hwnd(id: isize) -> HWND {
    HWND(id as *mut c_void)
}
fn window_pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}
fn process_name(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        if ok.is_err() {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().unwrap_or("").to_owned()
    }
}
fn title(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}
unsafe extern "system" fn collect(h: HWND, l: LPARAM) -> BOOL {
    let out = &mut *(l.0 as *mut Vec<isize>);
    if IsWindowVisible(h).as_bool()
        && GetWindow(h, GW_OWNER).map_or(true, |o| o.0.is_null())
        && GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0
    {
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
        if cloaked == 0 && GetWindowTextLengthW(h) > 0 {
            out.push(h.0 as isize);
        }
    }
    BOOL(1)
}
fn list_windows() -> Value {
    let mut ids: Vec<isize> = vec![];
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut ids as *mut Vec<isize> as isize));
    }
    let own = std::process::id();
    let fg = unsafe { GetForegroundWindow() }.0 as isize;
    json!(ids
        .into_iter()
        .filter(|&id| window_pid(hwnd(id)) != own)
        .map(|id| {
            let h = hwnd(id);
            json!({"window":id,"title":title(h),"app":process_name(window_pid(h)),"active":id==fg,"minimized":unsafe{IsIconic(h)}.as_bool()})
        })
        .collect::<Vec<_>>())
}

fn control_name(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50023 => "Tree",
        50024 => "TreeItem",
        50025 => "Custom",
        50026 => "Group",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50032 => "Window",
        50033 => "Pane",
        50036 => "Table",
        _ => "Other",
    }
}
fn clip(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}
fn text_of(e: &IUIAutomationElement) -> Option<String> {
    unsafe {
        if let Ok(p) = e.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
            if let Ok(v) = p.CurrentValue() {
                return Some(v.to_string());
            }
        }
        let p = e
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .ok()?;
        Some(p.DocumentRange().ok()?.GetText(1500).ok()?.to_string())
    }
}
impl State {
    fn check_window(&self, id: isize) -> Result<HWND> {
        let h = hwnd(id);
        ensure!(
            unsafe { IsWindow(Some(h)) }.as_bool(),
            "Окна с таким id нет. Вызови ui_windows"
        );
        ensure!(
            window_pid(h) != std::process::id(),
            "Окно Jarvis трогать нельзя"
        );
        Ok(h)
    }
    fn snapshot(&mut self, id: isize, limit: usize) -> Result<Value> {
        let h = self.check_window(id)?;
        unsafe {
            let root = self.auto.ElementFromHandle(h)?;
            let cache = self.auto.CreateCacheRequest()?;
            for p in [
                UIA_NamePropertyId,
                UIA_ControlTypePropertyId,
                UIA_BoundingRectanglePropertyId,
                UIA_IsEnabledPropertyId,
                UIA_IsOffscreenPropertyId,
                UIA_IsPasswordPropertyId,
                UIA_AutomationIdPropertyId,
            ] {
                cache.AddProperty(p)?;
            }
            let found = root.FindAllBuildCache(
                TreeScope_Descendants,
                &self.auto.ControlViewCondition()?,
                &cache,
            )?;
            self.elements.clear();
            self.window = id;
            let mut items = vec![];
            let mut seen = std::collections::HashSet::new();
            let mut values = 0;
            for i in 0..found.Length()? {
                if items.len() >= limit {
                    break;
                }
                let Ok(e) = found.GetElement(i) else { continue };
                if e.CachedIsOffscreen().is_ok_and(|b| b.as_bool()) {
                    continue;
                }
                let r: RECT = e.CachedBoundingRectangle().unwrap_or_default();
                if r.right - r.left <= 0 || r.bottom - r.top <= 0 {
                    continue;
                }
                let kind = e.CachedControlType().map_or(0, |c| c.0);
                let name = e.CachedName().map(|n| n.to_string()).unwrap_or_default();
                let auto_id = e
                    .CachedAutomationId()
                    .map(|n| n.to_string())
                    .unwrap_or_default();
                let password = e.CachedIsPassword().is_ok_and(|b| b.as_bool());
                let kind_name = control_name(kind);
                if name.is_empty() && auto_id.is_empty() && !matches!(kind, 50004 | 50030 | 50003) {
                    continue;
                }
                if !seen.insert((kind, name.clone(), r.left, r.top)) {
                    continue;
                }
                let mut item = json!({"i":self.elements.len(),"type":kind_name,"rect":[r.left,r.top,r.right-r.left,r.bottom-r.top]});
                if !name.is_empty() {
                    item["name"] = json!(clip(&name, 80));
                }
                if !auto_id.is_empty() {
                    item["id"] = json!(clip(&auto_id, 40));
                }
                if !e.CachedIsEnabled().map_or(true, |b| b.as_bool()) {
                    item["enabled"] = json!(false);
                }
                if password {
                    item["password"] = json!(true);
                } else if matches!(kind, 50004 | 50030 | 50003) && values < 12 {
                    values += 1;
                    if let Some(v) = text_of(&e) {
                        item["value"] = json!(clip(&v, 300));
                    }
                }
                self.elements.push(e);
                items.push(item);
            }
            Ok(
                json!({"window":id,"title":title(h),"app":process_name(window_pid(h)),"elements":items}),
            )
        }
    }
    fn element(&self, args: &Value) -> Result<(IUIAutomationElement, usize)> {
        let w = args["window"].as_i64().context("Нужен window")? as isize;
        ensure!(
            w == self.window,
            "Сначала вызови ui_state для этого окна: номера элементов устарели"
        );
        let i = args["element"].as_u64().context("Нужен element")? as usize;
        let e = self
            .elements
            .get(i)
            .context("Нет элемента с таким номером")?;
        unsafe {
            e.CurrentBoundingRectangle()
                .context("Элемент исчез. Вызови ui_state заново")?;
            ensure!(
                e.CurrentProcessId()? as u32 != std::process::id(),
                "Окно Jarvis трогать нельзя"
            );
        }
        Ok((e.clone(), i))
    }
}

fn focus(h: HWND) -> Result<()> {
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        for _ in 0..3 {
            let _ = SetForegroundWindow(h);
            std::thread::sleep(Duration::from_millis(80));
            if GetForegroundWindow() == h {
                return Ok(());
            }
            // Windows refuses focus steals unless an input event came first; a bare Alt tap lifts that.
            send(&[key_input(VK_MENU, false), key_input(VK_MENU, true)]);
        }
        bail!("Не удалось передать фокус окну. Возможно, оно запущено от администратора")
    }
}
fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
fn unicode_input(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE
                    | if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
fn mouse_input(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
fn send(inputs: &[INPUT]) {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
}
/// Refuses to send input while Jarvis itself is the foreground window (it would click its own prompts).
fn ensure_not_own_foreground() -> Result<()> {
    ensure!(
        window_pid(unsafe { GetForegroundWindow() }) != std::process::id(),
        "Фокус у окна Jarvis. Ввод заблокирован"
    );
    Ok(())
}
fn parse_key(name: &str) -> Result<VIRTUAL_KEY> {
    Ok(match name {
        "ctrl" | "control" => VK_CONTROL,
        "alt" => VK_MENU,
        "shift" => VK_SHIFT,
        "win" | "meta" | "super" => VK_LWIN,
        "enter" | "return" => VK_RETURN,
        "esc" | "escape" => VK_ESCAPE,
        "tab" => VK_TAB,
        "space" => VK_SPACE,
        "backspace" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" | "page_up" => VK_PRIOR,
        "pagedown" | "page_down" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        f if f.len() >= 2
            && f.starts_with('f')
            && f[1..].parse::<u16>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            VIRTUAL_KEY(VK_F1.0 + f[1..].parse::<u16>().unwrap() - 1)
        }
        c if c.len() == 1 && c.as_bytes()[0].is_ascii_alphanumeric() => {
            VIRTUAL_KEY(c.as_bytes()[0].to_ascii_uppercase() as u16)
        }
        other => bail!("Неизвестная клавиша: {other}"),
    })
}
fn press(keys: &str) -> Result<()> {
    let parts: Vec<String> = keys
        .to_lowercase()
        .split('+')
        .map(|p| p.trim().to_owned())
        .collect();
    ensure!(
        !parts.is_empty() && parts.iter().all(|p| !p.is_empty()),
        "Пустое сочетание клавиш"
    );
    let codes = parts
        .iter()
        .map(|p| parse_key(p))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        !(parts.iter().any(|p| p == "win") && parts.iter().any(|p| p == "l")),
        "Блокировку компьютера через ввод клавиш выполнять нельзя"
    );
    let mut inputs: Vec<INPUT> = codes.iter().map(|&c| key_input(c, false)).collect();
    inputs.extend(codes.iter().rev().map(|&c| key_input(c, true)));
    ensure_not_own_foreground()?;
    send(&inputs);
    Ok(())
}
fn type_text(text: &str) -> Result<()> {
    ensure_not_own_foreground()?;
    for ch in text.chars() {
        if STOP.load(Ordering::SeqCst) {
            bail!("Остановлено пользователем (Ctrl+Alt+K)");
        }
        if ch == '\n' {
            send(&[key_input(VK_RETURN, false), key_input(VK_RETURN, true)]);
        } else {
            let mut units = [0u16; 2];
            for &u in ch.encode_utf16(&mut units).iter() {
                send(&[unicode_input(u, false), unicode_input(u, true)]);
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    Ok(())
}
fn center(e: &IUIAutomationElement) -> Result<(i32, i32)> {
    let r = unsafe { e.CurrentBoundingRectangle()? };
    Ok(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
}
fn mouse_click(x: i32, y: i32, right: bool, double: bool) -> Result<()> {
    ensure_not_own_foreground()?;
    let (down, up) = if right {
        (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
    } else {
        (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
    };
    unsafe { SetCursorPos(x, y)? };
    for _ in 0..if double { 2 } else { 1 } {
        send(&[mouse_input(down, 0), mouse_input(up, 0)]);
        std::thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}

fn act(s: &mut State, tool: &str, args: &Value) -> Result<Value> {
    let w = args["window"].as_i64().context("Нужен window")? as isize;
    let h = s.check_window(w)?;
    match tool {
        "ui_click" => {
            let right = args["button"] == "right";
            let double = args["double"] == true;
            if let (Some(x), Some(y)) = (args["x"].as_i64(), args["y"].as_i64()) {
                focus(h)?;
                mouse_click(x as i32, y as i32, right, double)?;
                return Ok(json!({"did":"mouse_click","x":x,"y":y}));
            }
            let (e, _) = s.element(args)?;
            unsafe {
                if !right && !double && args["mouse"] != true {
                    if let Ok(p) =
                        e.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    {
                        p.Invoke()?;
                        return Ok(json!({"did":"invoke"}));
                    }
                    if let Ok(p) =
                        e.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                    {
                        p.Toggle()?;
                        return Ok(json!({"did":"toggle"}));
                    }
                    if let Ok(p) = e.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    ) {
                        p.Select()?;
                        return Ok(json!({"did":"select"}));
                    }
                    if let Ok(p) = e.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    ) {
                        if p.CurrentExpandCollapseState()? == ExpandCollapseState_Collapsed {
                            p.Expand()?;
                        } else {
                            p.Collapse()?;
                        }
                        return Ok(json!({"did":"expand_collapse"}));
                    }
                }
            }
            let (x, y) = center(&e)?;
            focus(h)?;
            mouse_click(x, y, right, double)?;
            Ok(json!({"did":"mouse_click","x":x,"y":y}))
        }
        "ui_set_value" => {
            let (e, _) = s.element(args)?;
            unsafe {
                ensure!(
                    !e.CurrentIsPassword()?.as_bool(),
                    "В поле пароля писать нельзя"
                );
                let text = args["text"].as_str().context("Нужен text")?;
                let p = e
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .context("Поле не поддерживает запись значения. Используй ui_type")?;
                p.SetValue(&BSTR::from(text))?;
                Ok(json!({"did":"set_value","chars":text.chars().count()}))
            }
        }
        "ui_type" => {
            let text = args["text"].as_str().context("Нужен text")?;
            ensure!(text.chars().count() <= 2000, "Текст слишком длинный");
            if args.get("element").is_some() {
                let (e, _) = s.element(args)?;
                unsafe {
                    ensure!(
                        !e.CurrentIsPassword()?.as_bool(),
                        "В поле пароля писать нельзя"
                    );
                    focus(h)?;
                    e.SetFocus()?;
                }
            } else {
                focus(h)?;
            }
            type_text(text)?;
            Ok(json!({"did":"type","chars":text.chars().count()}))
        }
        "ui_press_key" => {
            focus(h)?;
            press(args["keys"].as_str().context("Нужен keys")?)?;
            Ok(json!({"did":"press_key"}))
        }
        "ui_scroll" => {
            let dy = args["dy"].as_i64().context("Нужен dy")?.clamp(-50, 50) as i32;
            focus(h)?;
            let (x, y) = if args.get("element").is_some() {
                center(&s.element(args)?.0)?
            } else {
                let mut r = RECT::default();
                unsafe { GetWindowRect(h, &mut r)? };
                ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
            };
            ensure_not_own_foreground()?;
            unsafe { SetCursorPos(x, y)? };
            send(&[mouse_input(MOUSEEVENTF_WHEEL, -dy * 120)]);
            Ok(json!({"did":"scroll","dy":dy}))
        }
        _ => bail!("Неизвестный инструмент"),
    }
}

static LAST: Mutex<(String, u32)> = Mutex::new((String::new(), 0));
fn loop_guard(tool: &str, args: &Value) -> Result<()> {
    let sig = format!("{tool}:{args}");
    let mut last = LAST.lock().unwrap();
    if last.0 == sig {
        last.1 += 1;
    } else {
        *last = (sig, 1);
    }
    ensure!(
        last.1 <= 3,
        "Одинаковый вызов повторён несколько раз и не помог. Смени подход: другой элемент, клавиши, или спроси пользователя"
    );
    Ok(())
}

/// Runs one UI tool. Actions return the refreshed tree of the window after the interface settles.
pub async fn exec(tool: &str, args: &Value) -> Result<Value> {
    ensure!(
        !STOP.load(Ordering::SeqCst),
        "Остановлено пользователем (Ctrl+Alt+K)"
    );
    ensure!(is_ui_tool(tool), "Неизвестный инструмент");
    loop_guard(tool, args)?;
    let tool_name = tool.to_owned();
    let args = args.clone();
    match tool {
        "ui_windows" => Ok(json!({"windows":list_windows()})),
        "ui_state" => {
            let w = args["window"].as_i64().context("Нужен window")? as isize;
            on_worker(move |s| s.snapshot(w, 220)).await
        }
        _ => {
            let w = args["window"].as_i64().context("Нужен window")? as isize;
            let result = {
                let (t, a) = (tool_name, args.clone());
                on_worker(move |s| act(s, &t, &a)).await?
            };
            // Wait until the window stops changing: apps process injected input asynchronously.
            let mut state: Option<Value> = None;
            for _ in 0..12 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let next = on_worker(move |s| s.snapshot(w, 120)).await.ok();
                let settled = next.is_some() && next == state;
                state = next;
                if settled {
                    break;
                }
            }
            Ok(json!({"result":result,"state":state}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_parse_and_dangerous_combos_are_refused() {
        assert_eq!(parse_key("enter").unwrap(), VK_RETURN);
        assert_eq!(parse_key("f5").unwrap(), VK_F5);
        assert_eq!(parse_key("s").unwrap(), VIRTUAL_KEY(b'S' as u16));
        assert!(parse_key("f13").is_err());
        assert!(parse_key("blah").is_err());
        assert!(press("win+l").is_err());
        assert!(press("ctrl++s").is_err());
    }
    #[test]
    fn schemas_match_the_tool_list() {
        let names: Vec<String> = schemas()
            .iter()
            .map(|s| s["function"]["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, TOOLS);
    }
    #[test]
    fn loop_guard_stops_identical_repeats() {
        let a = json!({"window":1,"element":2,"loop-test":true});
        for _ in 0..3 {
            loop_guard("ui_click", &a).unwrap();
        }
        assert!(loop_guard("ui_click", &a).is_err());
        loop_guard("ui_click", &json!({"window":9})).unwrap();
    }
    /// Drives a real Notepad window: needs an interactive desktop, so it only runs on request.
    #[tokio::test]
    #[ignore]
    async fn drives_notepad_through_the_accessibility_tree() {
        reset_stop();
        let child = std::process::Command::new("notepad.exe").spawn().unwrap();
        let mut window = None;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let list = exec("ui_windows", &json!({})).await.unwrap();
            window = list["windows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| {
                    w["app"]
                        .as_str()
                        .unwrap_or("")
                        .eq_ignore_ascii_case("notepad.exe")
                })
                .and_then(|w| w["window"].as_i64());
            if window.is_some() {
                break;
            }
        }
        let window = window.expect("Блокнот не появился");
        let pid = window_pid(hwnd(window as isize));
        let outcome = async {
            let mut state = Value::Null;
            for _ in 0..20 {
                state = exec("ui_state", &json!({"window":window})).await?;
                let ready = state["elements"].as_array().is_some_and(|e| {
                    e.iter().any(|x| matches!(x["type"].as_str(), Some("Document" | "Edit")))
                });
                if ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            let els = state["elements"].as_array().context("нет elements")?;
            println!("elements: {}", els.len());
            for e in els.iter().take(15) {
                println!("  {e}");
            }
            let edit = els
                .iter()
                .find(|e| matches!(e["type"].as_str(), Some("Document" | "Edit")))
                .context("нет поля ввода")?["i"]
                .as_u64()
                .unwrap();
            let after = exec(
                "ui_type",
                &json!({"window":window,"element":edit,"text":"abc xyz hello привет 123"}),
            )
            .await?;
            let value = after["state"]["elements"]
                .as_array()
                .context("нет обновлённого состояния")?
                .iter()
                .filter_map(|e| e["value"].as_str())
                .collect::<Vec<_>>()
                .join("|");
            println!("values after typing: {value}");
            tokio::time::sleep(Duration::from_secs(3)).await;
            let late = exec("ui_state", &json!({"window":window})).await?;
            for e in late["elements"].as_array().unwrap().iter().filter(|e| e.get("value").is_some()) {
                println!("LATE value of {}: {:?}", e["type"], e["value"]);
            }
            ensure!(
                value.contains("abc xyz hello привет 123"),
                "текст не появился: {value}"
            );
            // The password-safety and own-window rules hold on a real handle too.
            let own = exec("ui_state", &json!({"window":0})).await;
            ensure!(own.is_err(), "окно 0 не должно приниматься");
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
        let _ = child;
        outcome.unwrap();
    }
}
