//! Глобальные хоткеи: Ctrl+Alt+J — переключить HUD (весь экран ↔ угол), Ctrl+Alt+K — аварийная остановка.
//! Хоткеи регистрирует только этот слой; бэкенд получает остановку через `UiHooks::emergency_stop`.

use jarvis_core::{Event, EventSink};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::commands::{SinkState, do_emergency_stop};
use crate::windows_manager;

pub fn toggle_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyJ)
}

pub fn stop_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyK)
}

/// Зарегистрировать хоткеи. Если сочетание занято другой программой — сообщение в HUD, без падения.
pub fn register(app: &AppHandle) {
    let gs = app.global_shortcut();
    let toggle = gs.on_shortcut(toggle_shortcut(), |app, _s, ev| {
        if ev.state == ShortcutState::Pressed
            && let Err(e) = windows_manager::toggle_mode(app)
        {
            eprintln!("[jarvis] Ctrl+Alt+J: {e}");
        }
    });
    let stop = gs.on_shortcut(stop_shortcut(), |app, _s, ev| {
        if ev.state == ShortcutState::Pressed {
            do_emergency_stop(app, "hotkey");
        }
    });
    for (name, res) in [("Ctrl+Alt+J", toggle), ("Ctrl+Alt+K", stop)] {
        if let Err(e) = res {
            let msg = format!("Не удалось занять сочетание {name}: {e}. Возможно, его использует другая программа.");
            eprintln!("[jarvis] {msg}");
            let app = app.clone();
            // окна ещё грузятся — покажем ошибку чуть позже
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                if let Some(s) = app.try_state::<SinkState>() {
                    s.0.emit(Event::Error { message: msg });
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_are_ctrl_alt_j_and_k() {
        assert_eq!(toggle_shortcut(), "ctrl+alt+KeyJ".parse::<Shortcut>().unwrap());
        assert_eq!(stop_shortcut(), "Control+Alt+K".parse::<Shortcut>().unwrap());
        assert_ne!(toggle_shortcut().id(), stop_shortcut().id());
    }
}
