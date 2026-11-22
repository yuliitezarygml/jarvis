//! Парсер сочетаний клавиш: `ctrl+shift+s`, `alt+f4`, `win+r`, `ctrl++`, последовательности `ctrl+k ctrl+c`.
//! Русские буквы трактуются как физические клавиши раскладки ЙЦУКЕН (`ctrl+ы` = `ctrl+s`).

/// Виртуальные коды клавиш (Win32 VK_*).
pub mod vk {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const MENU: u16 = 0x12;
    pub const PAUSE: u16 = 0x13;
    pub const CAPITAL: u16 = 0x14;
    pub const ESCAPE: u16 = 0x1B;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const SNAPSHOT: u16 = 0x2C;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const LWIN: u16 = 0x5B;
    pub const RWIN: u16 = 0x5C;
    pub const APPS: u16 = 0x5D;
    pub const NUMPAD0: u16 = 0x60;
    pub const MULTIPLY: u16 = 0x6A;
    pub const ADD: u16 = 0x6B;
    pub const SUBTRACT: u16 = 0x6D;
    pub const DECIMAL: u16 = 0x6E;
    pub const DIVIDE: u16 = 0x6F;
    pub const F1: u16 = 0x70;
    pub const NUMLOCK: u16 = 0x90;
    pub const SCROLL: u16 = 0x91;
    pub const LSHIFT: u16 = 0xA0;
    pub const RSHIFT: u16 = 0xA1;
    pub const LCONTROL: u16 = 0xA2;
    pub const RCONTROL: u16 = 0xA3;
    pub const LMENU: u16 = 0xA4;
    pub const RMENU: u16 = 0xA5;
    pub const BROWSER_BACK: u16 = 0xA6;
    pub const BROWSER_FORWARD: u16 = 0xA7;
    pub const BROWSER_REFRESH: u16 = 0xA8;
    pub const VOLUME_MUTE: u16 = 0xAD;
    pub const VOLUME_DOWN: u16 = 0xAE;
    pub const VOLUME_UP: u16 = 0xAF;
    pub const MEDIA_NEXT_TRACK: u16 = 0xB0;
    pub const MEDIA_PREV_TRACK: u16 = 0xB1;
    pub const MEDIA_STOP: u16 = 0xB2;
    pub const MEDIA_PLAY_PAUSE: u16 = 0xB3;
    pub const OEM_1: u16 = 0xBA; // ;:
    pub const OEM_PLUS: u16 = 0xBB; // =+
    pub const OEM_COMMA: u16 = 0xBC;
    pub const OEM_MINUS: u16 = 0xBD;
    pub const OEM_PERIOD: u16 = 0xBE;
    pub const OEM_2: u16 = 0xBF; // /?
    pub const OEM_3: u16 = 0xC0; // `~
    pub const OEM_4: u16 = 0xDB; // [{
    pub const OEM_5: u16 = 0xDC; // \|
    pub const OEM_6: u16 = 0xDD; // ]}
    pub const OEM_7: u16 = 0xDE; // '"
}

/// Клавиши, для которых нужен флаг KEYEVENTF_EXTENDEDKEY.
pub fn is_extended(code: u16) -> bool {
    use vk::*;
    matches!(
        code,
        RCONTROL | RMENU | INSERT | DELETE | HOME | END | PRIOR | NEXT | LEFT | UP | RIGHT | DOWN | NUMLOCK
            | SNAPSHOT | DIVIDE | LWIN | RWIN | APPS
    ) || (BROWSER_BACK..=0xB7).contains(&code)
}

pub fn is_modifier(code: u16) -> bool {
    use vk::*;
    matches!(code, SHIFT | CONTROL | MENU | LWIN | RWIN | LSHIFT | RSHIFT | LCONTROL | RCONTROL | LMENU | RMENU)
}

/// Порядок нажатия модификаторов: Ctrl, Alt, Shift, Win.
fn modifier_rank(code: u16) -> u8 {
    use vk::*;
    match code {
        CONTROL | LCONTROL | RCONTROL => 0,
        MENU | LMENU | RMENU => 1,
        SHIFT | LSHIFT | RSHIFT => 2,
        _ => 3,
    }
}

/// ЙЦУКЕН → физическая клавиша US-раскладки.
fn cyrillic_key(c: char) -> Option<u16> {
    use vk::*;
    let k = match c {
        'й' => b'Q' as u16,
        'ц' => b'W' as u16,
        'у' => b'E' as u16,
        'к' => b'R' as u16,
        'е' => b'T' as u16,
        'н' => b'Y' as u16,
        'г' => b'U' as u16,
        'ш' => b'I' as u16,
        'щ' => b'O' as u16,
        'з' => b'P' as u16,
        'х' => OEM_4,
        'ъ' => OEM_6,
        'ф' => b'A' as u16,
        'ы' => b'S' as u16,
        'в' => b'D' as u16,
        'а' => b'F' as u16,
        'п' => b'G' as u16,
        'р' => b'H' as u16,
        'о' => b'J' as u16,
        'л' => b'K' as u16,
        'д' => b'L' as u16,
        'ж' => OEM_1,
        'э' => OEM_7,
        'я' => b'Z' as u16,
        'ч' => b'X' as u16,
        'с' => b'C' as u16,
        'м' => b'V' as u16,
        'и' => b'B' as u16,
        'т' => b'N' as u16,
        'ь' => b'M' as u16,
        'б' => OEM_COMMA,
        'ю' => OEM_PERIOD,
        'ё' => OEM_3,
        _ => return None,
    };
    Some(k)
}

/// Имя клавиши → VK. Регистр не важен.
pub fn key_code(name: &str) -> Option<u16> {
    use vk::*;
    let n = name.trim().to_lowercase();
    let mut chars = n.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u16);
        }
        if let Some(k) = cyrillic_key(c) {
            return Some(k);
        }
        let k = match c {
            '+' | '=' => OEM_PLUS,
            '-' => OEM_MINUS,
            ',' => OEM_COMMA,
            '.' => OEM_PERIOD,
            '/' => OEM_2,
            '\\' => OEM_5,
            ';' => OEM_1,
            '\'' => OEM_7,
            '`' => OEM_3,
            '[' => OEM_4,
            ']' => OEM_6,
            ' ' => SPACE,
            _ => return None,
        };
        return Some(k);
    }
    if let Some(num) = n.strip_prefix('f').and_then(|d| d.parse::<u16>().ok()) {
        return (1..=24).contains(&num).then(|| F1 + num - 1);
    }
    for prefix in ["numpad", "num"] {
        if let Some(d) = n.strip_prefix(prefix).and_then(|d| d.parse::<u16>().ok()) {
            return (d <= 9).then_some(NUMPAD0 + d);
        }
    }
    let k = match n.as_str() {
        "ctrl" | "control" | "ctl" | "ктрл" => CONTROL,
        "lctrl" | "leftctrl" | "lcontrol" => LCONTROL,
        "rctrl" | "rightctrl" | "rcontrol" => RCONTROL,
        "shift" | "шифт" => SHIFT,
        "lshift" | "leftshift" => LSHIFT,
        "rshift" | "rightshift" => RSHIFT,
        "alt" | "menu" | "option" | "альт" => MENU,
        "lalt" | "leftalt" => LMENU,
        "ralt" | "rightalt" | "altgr" => RMENU,
        "win" | "windows" | "super" | "meta" | "cmd" | "lwin" | "вин" => LWIN,
        "rwin" => RWIN,
        "enter" | "return" | "ввод" => RETURN,
        "esc" | "escape" => ESCAPE,
        "tab" | "таб" => TAB,
        "space" | "spacebar" | "пробел" => SPACE,
        "backspace" | "bksp" | "back" | "bs" => BACK,
        "delete" | "del" => DELETE,
        "insert" | "ins" => INSERT,
        "home" => HOME,
        "end" => END,
        "pageup" | "pgup" | "prior" => PRIOR,
        "pagedown" | "pgdn" | "pgdown" | "next" => NEXT,
        "up" | "arrowup" | "uparrow" => UP,
        "down" | "arrowdown" | "downarrow" => DOWN,
        "left" | "arrowleft" | "leftarrow" => LEFT,
        "right" | "arrowright" | "rightarrow" => RIGHT,
        "printscreen" | "prtsc" | "prtscr" | "print" | "snapshot" => SNAPSHOT,
        "capslock" | "caps" => CAPITAL,
        "numlock" => NUMLOCK,
        "scrolllock" => SCROLL,
        "pause" | "break" => PAUSE,
        "apps" | "contextmenu" | "context" => APPS,
        "plus" | "equal" | "equals" => OEM_PLUS,
        "minus" | "dash" | "hyphen" => OEM_MINUS,
        "comma" => OEM_COMMA,
        "period" | "dot" => OEM_PERIOD,
        "slash" => OEM_2,
        "backslash" => OEM_5,
        "semicolon" => OEM_1,
        "quote" | "apostrophe" => OEM_7,
        "backquote" | "backtick" | "grave" | "tilde" => OEM_3,
        "bracketleft" | "lbracket" => OEM_4,
        "bracketright" | "rbracket" => OEM_6,
        "multiply" | "nummul" => MULTIPLY,
        "add" | "numadd" | "numplus" => ADD,
        "subtract" | "numsub" | "numminus" => SUBTRACT,
        "decimal" | "numdecimal" => DECIMAL,
        "divide" | "numdiv" => DIVIDE,
        "volumeup" | "volup" => VOLUME_UP,
        "volumedown" | "voldown" => VOLUME_DOWN,
        "volumemute" | "mute" => VOLUME_MUTE,
        "playpause" | "mediaplaypause" => MEDIA_PLAY_PAUSE,
        "nexttrack" | "medianext" => MEDIA_NEXT_TRACK,
        "prevtrack" | "mediaprev" => MEDIA_PREV_TRACK,
        "mediastop" => MEDIA_STOP,
        "browserback" => BROWSER_BACK,
        "browserforward" => BROWSER_FORWARD,
        "browserrefresh" => BROWSER_REFRESH,
        _ => return None,
    };
    Some(k)
}

/// Одно сочетание: клавиши в порядке нажатия (модификаторы первыми), отпускаются в обратном порядке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub keys: Vec<u16>,
}

impl Chord {
    pub fn modifiers(&self) -> impl Iterator<Item = u16> + '_ {
        self.keys.iter().copied().filter(|k| is_modifier(*k))
    }
}

/// Разобрать строку сочетаний. Сочетания в последовательности разделяются пробелами.
pub fn parse_hotkey(spec: &str) -> Result<Vec<Chord>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("пустое сочетание клавиш".into());
    }
    let mut chords = Vec::new();
    for part in spec.split_whitespace() {
        chords.push(parse_chord(part)?);
    }
    if chords.len() > 8 {
        return Err("слишком длинная последовательность (максимум 8 сочетаний)".into());
    }
    Ok(chords)
}

/// «ctrl++» и «+» — клавиша плюс; в остальных случаях «+» — разделитель.
fn tokenize(part: &str) -> Result<Vec<&str>, String> {
    if part == "+" {
        return Ok(vec!["+"]);
    }
    let (head, plus_key) = match part.strip_suffix("++") {
        Some(h) => (h, true),
        None => (part, false),
    };
    let mut tokens: Vec<&str> = if head.is_empty() { Vec::new() } else { head.split('+').collect() };
    if tokens.iter().any(|t| t.is_empty()) {
        return Err(format!("не понял сочетание «{part}»"));
    }
    if plus_key {
        tokens.push("+");
    }
    Ok(tokens)
}

fn parse_chord(part: &str) -> Result<Chord, String> {
    let tokens = tokenize(part)?;
    let mut mods: Vec<u16> = Vec::new();
    let mut keys: Vec<u16> = Vec::new();
    for t in tokens {
        let code = key_code(t).ok_or_else(|| format!("неизвестная клавиша «{t}»"))?;
        let bucket = if is_modifier(code) { &mut mods } else { &mut keys };
        if !bucket.contains(&code) {
            bucket.push(code);
        }
    }
    let has_ctrl = mods.iter().any(|&m| modifier_rank(m) == 0);
    let has_alt = mods.iter().any(|&m| modifier_rank(m) == 1);
    if has_ctrl && has_alt && keys.contains(&vk::DELETE) {
        return Err("Ctrl+Alt+Del нельзя отправить программно (защищённое сочетание Windows)".into());
    }
    if keys.len() > 3 {
        return Err("слишком много клавиш в одном сочетании".into());
    }
    mods.sort_by_key(|&m| modifier_rank(m));
    mods.extend(keys);
    Ok(Chord { keys: mods })
}

/// Человекочитаемое имя сочетания для подсветки: «Ctrl+Shift+S».
pub fn describe(chords: &[Chord]) -> String {
    chords
        .iter()
        .map(|c| c.keys.iter().map(|&k| key_label(k)).collect::<Vec<_>>().join("+"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn key_label(code: u16) -> String {
    use vk::*;
    match code {
        CONTROL | LCONTROL | RCONTROL => "Ctrl".into(),
        MENU | LMENU | RMENU => "Alt".into(),
        SHIFT | LSHIFT | RSHIFT => "Shift".into(),
        LWIN | RWIN => "Win".into(),
        RETURN => "Enter".into(),
        ESCAPE => "Esc".into(),
        TAB => "Tab".into(),
        SPACE => "Space".into(),
        BACK => "Backspace".into(),
        DELETE => "Del".into(),
        OEM_PLUS => "+".into(),
        OEM_MINUS => "-".into(),
        c if (F1..F1 + 24).contains(&c) => format!("F{}", c - F1 + 1),
        c if c.is_ascii() && (c as u8).is_ascii_alphanumeric() => (c as u8 as char).to_string(),
        c => format!("VK{c:#04X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::vk::*;
    use super::*;

    fn one(s: &str) -> Vec<u16> {
        let c = parse_hotkey(s).unwrap();
        assert_eq!(c.len(), 1, "{s}");
        c[0].keys.clone()
    }

    #[test]
    fn basic_combos() {
        assert_eq!(one("ctrl+shift+s"), vec![CONTROL, SHIFT, b'S' as u16]);
        assert_eq!(one("Shift+Ctrl+S"), vec![CONTROL, SHIFT, b'S' as u16], "модификаторы сортируются");
        assert_eq!(one("alt+f4"), vec![MENU, F1 + 3]);
        assert_eq!(one("win+r"), vec![LWIN, b'R' as u16]);
        assert_eq!(one("enter"), vec![RETURN]);
        assert_eq!(one("F12"), vec![F1 + 11]);
        assert_eq!(one("ctrl+alt+t"), vec![CONTROL, MENU, b'T' as u16]);
        assert_eq!(one("win"), vec![LWIN]);
        assert_eq!(one("ctrl+pagedown"), vec![CONTROL, NEXT]);
        assert_eq!(one("num5"), vec![NUMPAD0 + 5]);
    }

    #[test]
    fn plus_key_and_punctuation() {
        assert_eq!(one("ctrl++"), vec![CONTROL, OEM_PLUS]);
        assert_eq!(one("ctrl+plus"), vec![CONTROL, OEM_PLUS]);
        assert_eq!(one("ctrl+-"), vec![CONTROL, OEM_MINUS]);
        assert_eq!(one("+"), vec![OEM_PLUS]);
        assert_eq!(one("ctrl+/"), vec![CONTROL, OEM_2]);
        assert_eq!(one("ctrl+,"), vec![CONTROL, OEM_COMMA]);
    }

    #[test]
    fn cyrillic_letters_map_to_physical_keys() {
        assert_eq!(one("ctrl+ы"), vec![CONTROL, b'S' as u16]);
        assert_eq!(one("Ctrl+С"), vec![CONTROL, b'C' as u16]);
        assert_eq!(one("ктрл+м"), vec![CONTROL, b'V' as u16]);
    }

    #[test]
    fn sequences() {
        let c = parse_hotkey("ctrl+k ctrl+c").unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].keys, vec![CONTROL, b'C' as u16]);
        assert_eq!(describe(&c), "Ctrl+K Ctrl+C");
    }

    #[test]
    fn errors() {
        assert!(parse_hotkey("").is_err());
        assert!(parse_hotkey("ctrl+foo").unwrap_err().contains("foo"));
        assert!(parse_hotkey("ctrl+alt+del").is_err());
        assert!(parse_hotkey("f25").is_err());
        assert!(parse_hotkey("ctrl+").is_err());
        assert!(parse_hotkey("ctrl++s").is_err());
    }

    #[test]
    fn extended_flags() {
        assert!(is_extended(LEFT) && is_extended(DELETE) && is_extended(LWIN) && is_extended(RMENU));
        assert!(is_extended(VOLUME_UP) && is_extended(MEDIA_PLAY_PAUSE));
        assert!(!is_extended(CONTROL) && !is_extended(b'A' as u16) && !is_extended(RETURN));
    }
}
