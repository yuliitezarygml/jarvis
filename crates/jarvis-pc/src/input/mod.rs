//! Ввод (T1): мышь абсолютными координатами виртуального рабочего стола с плавной траекторией,
//! текст через KEYEVENTF_UNICODE, сочетания клавиш по VK, медиаклавиши.

pub mod clipboard;
pub mod events;
pub mod hotkey;
pub(crate) mod send;
pub(crate) mod tools;
pub mod trajectory;

use jarvis_core::ToolError;

pub use events::{INJECTED_TAG, InputEvent, MouseButton};
pub use hotkey::{Chord, parse_hotkey};
pub use send::release_all_held;

use events::{chord_events, text_events};
use send::InputRunner;

/// Максимальная длина текста для `key_type`/`terminal_type`.
pub const MAX_TYPE_CHARS: usize = 5000;

/// Текст длиннее — вставляется через буфер обмена (Ctrl+V), короче — печатается по символам.
pub const PASTE_THRESHOLD_CHARS: usize = 30;
/// Минимальная пауза между символами: быстрее некоторые программы (новый Блокнот) теряют и повторяют символы.
const MIN_CHAR_DELAY_MS: u64 = 8;

/// Как был введён текст.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedVia {
    Keys,
    Paste,
}

/// Напечатать текст: короткий — по символам с паузой, длинный — вставкой из буфера обмена
/// (прежнее текстовое содержимое буфера возвращается).
pub(crate) fn type_text(r: &mut InputRunner, text: &str, delay_ms: u64) -> Result<(usize, TypedVia), ToolError> {
    let chars = text.chars().count();
    if chars > PASTE_THRESHOLD_CHARS {
        paste_text(r, text)?;
        return Ok((chars, TypedVia::Paste));
    }
    let delay = delay_ms.max(MIN_CHAR_DELAY_MS);
    let groups = text_events(text);
    for g in &groups {
        r.send(g)?;
        r.pause(delay)?;
    }
    Ok((groups.len(), TypedVia::Keys))
}

/// Вставить текст через буфер обмена: сохранить старый текст → положить новый → Ctrl+V → вернуть старый.
fn paste_text(r: &mut InputRunner, text: &str) -> Result<(), ToolError> {
    // Переводы строк — как в Windows, иначе часть программ склеит строки.
    let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    let previous = clipboard::get_text().ok().flatten();
    clipboard::set_text(&text)?;
    let res = (|| {
        r.pause(40)?;
        let chord = parse_hotkey("ctrl+v").map_err(ToolError::InvalidArgs)?;
        press_chords(r, &chord)?;
        // Программа читает буфер асинхронно — не подменять его сразу.
        r.pause(250)
    })();
    match previous {
        Some(p) => {
            let _ = clipboard::set_text(&p);
        }
        None => clipboard::clear(),
    }
    res
}

/// Нажать последовательность сочетаний: модификаторы → клавиша → отпускание модификаторов.
pub(crate) fn press_chords(r: &mut InputRunner, chords: &[Chord]) -> Result<(), ToolError> {
    for (i, c) in chords.iter().enumerate() {
        if i > 0 {
            r.pause(60)?;
        }
        let [down, tap, up] = chord_events(&c.keys);
        if !down.is_empty() {
            r.send(&down)?;
            r.pause(15)?;
        }
        r.send(&tap)?;
        if !up.is_empty() {
            r.pause(15)?;
            r.send(&up)?;
        }
    }
    Ok(())
}
