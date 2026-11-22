//! Ввод (T1): мышь абсолютными координатами виртуального рабочего стола с плавной траекторией,
//! текст через KEYEVENTF_UNICODE, сочетания клавиш по VK, медиаклавиши.

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

/// Напечатать текст. Длинный текст отправляется пачками, чтобы уложиться в таймаут инструмента.
pub(crate) fn type_text(r: &mut InputRunner, text: &str, delay_ms: u64) -> Result<usize, ToolError> {
    let groups = text_events(text);
    let chunk = if groups.len() > 400 { 8 } else { 1 };
    for g in groups.chunks(chunk) {
        let flat: Vec<InputEvent> = g.concat();
        r.send(&flat)?;
        if delay_ms > 0 {
            r.pause(delay_ms)?;
        }
    }
    Ok(groups.len())
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
