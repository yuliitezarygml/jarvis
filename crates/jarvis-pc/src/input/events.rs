//! Промежуточное представление ввода и его перевод в структуры Win32 `INPUT` (без отправки — тестируется).

use jarvis_core::Rect;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MapVirtualKeyW,
    VIRTUAL_KEY,
};

use super::hotkey::{is_extended, vk};

/// Метка в `dwExtraInfo` всех наших событий («JARV») — по ней хуки отличают ввод Джарвиса от пользователя.
pub const INJECTED_TAG: usize = 0x4A41_5256;
/// Один «щелчок» колеса.
pub const WHEEL_DELTA: i32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "" | "left" | "l" | "левая" | "лкм" => Some(Self::Left),
            "right" | "r" | "правая" | "пкм" => Some(Self::Right),
            "middle" | "m" | "wheel" | "средняя" => Some(Self::Middle),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key { vk: u16, up: bool },
    /// UTF-16 единица через KEYEVENTF_UNICODE (кириллица и любые символы без учёта раскладки).
    Unicode { unit: u16, up: bool },
    /// Абсолютная позиция в пикселях виртуального рабочего стола.
    MoveAbs { x: i32, y: i32 },
    Button { button: MouseButton, up: bool },
    /// Положительное — вверх/вправо.
    Wheel { delta: i32, horizontal: bool },
}

/// Пиксель виртуального стола → нормализованная координата 0..=65535 (MOUSEEVENTF_VIRTUALDESK).
/// Windows переводит обратно как `n * size / 65536` (с отбрасыванием), поэтому берём потолок.
pub fn normalize_axis(offset: i32, size: i32) -> i32 {
    if size <= 1 {
        return 0;
    }
    let off = offset.clamp(0, size - 1) as i64;
    let n = (off * 65536 + size as i64 - 1) / size as i64;
    n.min(65535) as i32
}

pub fn normalize(x: i32, y: i32, vs: Rect) -> (i32, i32) {
    (normalize_axis(x - vs.x, vs.w), normalize_axis(y - vs.y, vs.h))
}

/// Обратное преобразование (так считает Windows) — для проверки.
pub fn denormalize_axis(n: i32, size: i32) -> i32 {
    ((n as i64 * size as i64) >> 16) as i32
}

fn keyboard(ki: KEYBDINPUT) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
}

fn mouse(mi: MOUSEINPUT) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi } }
}

fn scan_code(code: u16) -> u16 {
    // SAFETY: чистое преобразование таблицы раскладки, без побочных эффектов.
    (unsafe { MapVirtualKeyW(code as u32, MAPVK_VK_TO_VSC) } & 0xFF) as u16
}

/// Событие → `INPUT`. `vs` — виртуальный рабочий стол (для абсолютных координат).
pub fn to_input(ev: &InputEvent, vs: Rect) -> INPUT {
    match *ev {
        InputEvent::Key { vk: code, up } => {
            let mut flags = KEYBD_EVENT_FLAGS(0);
            if is_extended(code) {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if up {
                flags |= KEYEVENTF_KEYUP;
            }
            keyboard(KEYBDINPUT {
                wVk: VIRTUAL_KEY(code),
                wScan: scan_code(code),
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_TAG,
            })
        }
        InputEvent::Unicode { unit, up } => {
            let mut flags = KEYEVENTF_UNICODE;
            if up {
                flags |= KEYEVENTF_KEYUP;
            }
            keyboard(KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: INJECTED_TAG })
        }
        InputEvent::MoveAbs { x, y } => {
            let (dx, dy) = normalize(x, y, vs);
            mouse(MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: INJECTED_TAG,
            })
        }
        InputEvent::Button { button, up } => {
            let flags: MOUSE_EVENT_FLAGS = match (button, up) {
                (MouseButton::Left, false) => MOUSEEVENTF_LEFTDOWN,
                (MouseButton::Left, true) => MOUSEEVENTF_LEFTUP,
                (MouseButton::Right, false) => MOUSEEVENTF_RIGHTDOWN,
                (MouseButton::Right, true) => MOUSEEVENTF_RIGHTUP,
                (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEDOWN,
                (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEUP,
            };
            mouse(MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: INJECTED_TAG })
        }
        InputEvent::Wheel { delta, horizontal } => mouse(MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: delta as u32,
            dwFlags: if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL },
            time: 0,
            dwExtraInfo: INJECTED_TAG,
        }),
    }
}

pub fn key_tap(code: u16) -> [InputEvent; 2] {
    [InputEvent::Key { vk: code, up: false }, InputEvent::Key { vk: code, up: true }]
}

/// Текст → события, сгруппированные по символам (каждая группа — нажатие+отпускание, отправляется атомарно).
/// `\n` и `\r\n` → Enter, `\t` → Tab, остальное — KEYEVENTF_UNICODE (суррогатные пары — двумя единицами).
pub fn text_events(text: &str) -> Vec<Vec<InputEvent>> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(key_tap(vk::RETURN).to_vec());
            }
            '\n' => out.push(key_tap(vk::RETURN).to_vec()),
            '\t' => out.push(key_tap(vk::TAB).to_vec()),
            c => {
                let mut buf = [0u16; 2];
                let mut group = Vec::with_capacity(4);
                for &unit in c.encode_utf16(&mut buf).iter() {
                    group.push(InputEvent::Unicode { unit, up: false });
                    group.push(InputEvent::Unicode { unit, up: true });
                }
                out.push(group);
            }
        }
    }
    out
}

/// Нажатия сочетания: (модификаторы вниз) (клавиши вниз+вверх) (модификаторы вверх в обратном порядке).
pub fn chord_events(keys: &[u16]) -> [Vec<InputEvent>; 3] {
    use super::hotkey::is_modifier;
    let mods: Vec<u16> = keys.iter().copied().filter(|k| is_modifier(*k)).collect();
    let mut main: Vec<u16> = keys.iter().copied().filter(|k| !is_modifier(*k)).collect();
    // Сочетание только из модификаторов (например, «win») — последний модификатор и есть «клавиша».
    let mut mods = mods;
    if main.is_empty() {
        if let Some(last) = mods.pop() {
            main.push(last);
        }
    }
    let down: Vec<InputEvent> = mods.iter().map(|&k| InputEvent::Key { vk: k, up: false }).collect();
    let mut tap: Vec<InputEvent> = main.iter().map(|&k| InputEvent::Key { vk: k, up: false }).collect();
    tap.extend(main.iter().rev().map(|&k| InputEvent::Key { vk: k, up: true }));
    let up: Vec<InputEvent> = mods.iter().rev().map(|&k| InputEvent::Key { vk: k, up: true }).collect();
    [down, tap, up]
}

/// Что сейчас зажато нашими событиями — чтобы при отмене всё отпустить.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HeldTracker {
    keys: Vec<u16>,
    buttons: Vec<MouseButton>,
}

impl HeldTracker {
    pub const fn new() -> Self {
        Self { keys: Vec::new(), buttons: Vec::new() }
    }

    pub fn apply(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Key { vk: code, up: false } if !self.keys.contains(&code) => self.keys.push(code),
            InputEvent::Key { vk: code, up: true } => self.keys.retain(|&k| k != code),
            InputEvent::Button { button, up: false } if !self.buttons.contains(&button) => self.buttons.push(button),
            InputEvent::Button { button, up: true } => self.buttons.retain(|&b| b != button),
            _ => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons.is_empty()
    }

    /// События отпускания: сначала кнопки мыши, потом клавиши в обратном порядке нажатия.
    pub fn release_events(&self) -> Vec<InputEvent> {
        let mut out: Vec<InputEvent> = self.buttons.iter().map(|&b| InputEvent::Button { button: b, up: true }).collect();
        out.extend(self.keys.iter().rev().map(|&k| InputEvent::Key { vk: k, up: true }));
        out
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.buttons.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;

    const VS: Rect = Rect { x: -1920, y: 0, w: 3840, h: 1080 };

    #[test]
    fn normalization_roundtrip() {
        for size in [1080, 1440, 1920, 2560, 3840, 5760] {
            for off in [0, 1, 2, size / 3, size / 2, size - 2, size - 1] {
                let n = normalize_axis(off, size);
                assert!((0..=65535).contains(&n));
                assert_eq!(denormalize_axis(n, size), off, "size={size} off={off} n={n}");
            }
        }
        // выход за край — прижимаем к краю
        assert_eq!(normalize_axis(-50, 1920), 0);
        assert_eq!(normalize_axis(5000, 1920), normalize_axis(1919, 1920));
        // отрицательные координаты левого монитора
        let (nx, ny) = normalize(-1920, 0, VS);
        assert_eq!((nx, ny), (0, 0));
        let (nx, _) = normalize(0, 0, VS);
        assert_eq!(denormalize_axis(nx, VS.w) + VS.x, 0);
    }

    #[test]
    fn key_inputs_built_correctly() {
        let i = to_input(&InputEvent::Key { vk: vk::LEFT, up: true }, VS);
        assert_eq!(i.r#type, INPUT_KEYBOARD);
        // SAFETY: тип INPUT_KEYBOARD — активно поле ki.
        let ki = unsafe { i.Anonymous.ki };
        assert_eq!(ki.wVk, VIRTUAL_KEY(vk::LEFT));
        assert!(ki.dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
        assert!(ki.dwFlags.contains(KEYEVENTF_KEYUP));
        assert_eq!(ki.dwExtraInfo, INJECTED_TAG);
        assert_ne!(ki.wScan, 0, "скан-код стрелки");

        let i = to_input(&InputEvent::Unicode { unit: 'Ж' as u16, up: false }, VS);
        // SAFETY: см. выше.
        let ki = unsafe { i.Anonymous.ki };
        assert_eq!(ki.wVk, VIRTUAL_KEY(0));
        assert_eq!(ki.wScan, 'Ж' as u16);
        assert_eq!(ki.dwFlags, KEYEVENTF_UNICODE);
    }

    #[test]
    fn mouse_inputs_built_correctly() {
        let i = to_input(&InputEvent::MoveAbs { x: 0, y: 540 }, VS);
        assert_eq!(i.r#type, INPUT_MOUSE);
        // SAFETY: тип INPUT_MOUSE — активно поле mi.
        let mi = unsafe { i.Anonymous.mi };
        assert!(mi.dwFlags.contains(MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE));
        assert_eq!(denormalize_axis(mi.dx, VS.w) + VS.x, 0);
        assert_eq!(denormalize_axis(mi.dy, VS.h) + VS.y, 540);

        let i = to_input(&InputEvent::Button { button: MouseButton::Right, up: false }, VS);
        // SAFETY: см. выше.
        assert_eq!(unsafe { i.Anonymous.mi }.dwFlags, MOUSEEVENTF_RIGHTDOWN);

        let i = to_input(&InputEvent::Wheel { delta: -3 * WHEEL_DELTA, horizontal: false }, VS);
        // SAFETY: см. выше.
        let mi = unsafe { i.Anonymous.mi };
        assert_eq!(mi.dwFlags, MOUSEEVENTF_WHEEL);
        assert_eq!(mi.mouseData as i32, -360);
    }

    #[test]
    fn text_to_events_cyrillic_newlines_surrogates() {
        // П р ⏎ и ⇥ в е т 😀 ⏎
        let g = text_events("Пр\r\nи\tвет😀\n");
        assert_eq!(g.len(), 10);
        assert_eq!(g[0], vec![InputEvent::Unicode { unit: 'П' as u16, up: false }, InputEvent::Unicode { unit: 'П' as u16, up: true }]);
        assert_eq!(g[2], key_tap(vk::RETURN).to_vec(), "\\r\\n — один Enter");
        assert_eq!(g[4], key_tap(vk::TAB).to_vec());
        assert_eq!(g[8].len(), 4, "эмодзи — суррогатная пара");
        assert!(matches!(g[8][0], InputEvent::Unicode { unit, up: false } if (0xD800..0xDC00).contains(&unit)));
        assert_eq!(g[9], key_tap(vk::RETURN).to_vec());
        assert!(text_events("").is_empty());
    }

    #[test]
    fn chord_order() {
        let [down, tap, up] = chord_events(&[vk::CONTROL, vk::SHIFT, b'S' as u16]);
        assert_eq!(down, vec![InputEvent::Key { vk: vk::CONTROL, up: false }, InputEvent::Key { vk: vk::SHIFT, up: false }]);
        assert_eq!(tap, vec![InputEvent::Key { vk: b'S' as u16, up: false }, InputEvent::Key { vk: b'S' as u16, up: true }]);
        assert_eq!(up, vec![InputEvent::Key { vk: vk::SHIFT, up: true }, InputEvent::Key { vk: vk::CONTROL, up: true }]);
        let [down, tap, up] = chord_events(&[vk::LWIN]);
        assert!(down.is_empty() && up.is_empty());
        assert_eq!(tap, key_tap(vk::LWIN).to_vec());
    }

    #[test]
    fn held_tracker_releases_everything() {
        let mut t = HeldTracker::default();
        let [down, _, _] = chord_events(&[vk::CONTROL, vk::MENU, b'T' as u16]);
        for e in &down {
            t.apply(e);
        }
        t.apply(&InputEvent::Button { button: MouseButton::Left, up: false });
        t.apply(&InputEvent::Unicode { unit: 1, up: false });
        let rel = t.release_events();
        assert_eq!(
            rel,
            vec![
                InputEvent::Button { button: MouseButton::Left, up: true },
                InputEvent::Key { vk: vk::MENU, up: true },
                InputEvent::Key { vk: vk::CONTROL, up: true },
            ]
        );
        for e in &rel {
            t.apply(e);
        }
        assert!(t.is_empty());
    }

    #[test]
    fn button_names() {
        assert_eq!(MouseButton::parse("Right"), Some(MouseButton::Right));
        assert_eq!(MouseButton::parse(""), Some(MouseButton::Left));
        assert_eq!(MouseButton::parse("пкм"), Some(MouseButton::Right));
        assert_eq!(MouseButton::parse("x"), None);
    }
}
