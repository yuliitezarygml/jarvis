//! Значок в трее: меню «Во весь экран / В угол / Настройки / Выход», левый клик — переключить режим.

use jarvis_core::config::HudMode;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Runtime};

use crate::windows_manager;

pub const TRAY_ID: &str = "jarvis-tray";

/// id пунктов меню → действие (вынесено для тестов).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TrayAction {
    Fullscreen,
    Corner,
    Settings,
    Quit,
}

pub fn action_for(id: &str) -> Option<TrayAction> {
    Some(match id {
        "fullscreen" => TrayAction::Fullscreen,
        "corner" => TrayAction::Corner,
        "settings" => TrayAction::Settings,
        "quit" => TrayAction::Quit,
        _ => return None,
    })
}

fn run_action<R: Runtime>(app: &AppHandle<R>, action: TrayAction) {
    let res = match action {
        TrayAction::Fullscreen => windows_manager::set_mode(app, HudMode::Fullscreen),
        TrayAction::Corner => windows_manager::set_mode(app, HudMode::Corner),
        TrayAction::Settings => windows_manager::open_settings(app, None),
        TrayAction::Quit => {
            app.exit(0);
            Ok(())
        }
    };
    if let Err(e) = res {
        eprintln!("[jarvis] трей: {e}");
    }
}

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let full = MenuItem::with_id(app, "fullscreen", "Во весь экран", true, Some("Ctrl+Alt+J"))?;
    let corner = MenuItem::with_id(app, "corner", "В угол", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Настройки", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&full, &corner, &settings, &sep, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("J.A.R.V.I.S.")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if let Some(a) = action_for(event.id().as_ref()) {
                run_action(app, a);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                let _ = windows_manager::toggle_mode(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids() {
        assert_eq!(action_for("fullscreen"), Some(TrayAction::Fullscreen));
        assert_eq!(action_for("corner"), Some(TrayAction::Corner));
        assert_eq!(action_for("settings"), Some(TrayAction::Settings));
        assert_eq!(action_for("quit"), Some(TrayAction::Quit));
        assert_eq!(action_for("other"), None);
    }
}
