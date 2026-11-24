//! Значок в трее: меню «Во весь экран / В угол / Характер ▸ / Настройки / Выход», левый клик — переключить режим.

use jarvis_core::config::{HudMode, PersonaMode};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::config_store::ConfigStore;
use crate::windows_manager;

pub const TRAY_ID: &str = "jarvis-tray";

/// id пунктов меню → действие (вынесено для тестов).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TrayAction {
    Fullscreen,
    Corner,
    Wallpaper,
    Settings,
    /// Подменю «Характер»: выбрать режим.
    Persona(PersonaMode),
    Quit,
}

/// id пункта подменю «Характер».
pub fn persona_item_id(mode: PersonaMode) -> String {
    format!("persona:{}", mode.as_str())
}

/// Пункты подменю «Характер» — чтобы обновлять галочки, когда режим сменили не из трея.
struct PersonaMenu<R: Runtime>(Vec<(PersonaMode, CheckMenuItem<R>)>);

/// Отметить текущий режим характера в подменю трея (вызывается после каждого сохранения конфига).
pub fn sync_persona<R: Runtime>(app: &AppHandle<R>, mode: PersonaMode) {
    if let Some(menu) = app.try_state::<PersonaMenu<R>>() {
        for (m, item) in &menu.0 {
            let _ = item.set_checked(*m == mode);
        }
    }
}

pub fn action_for(id: &str) -> Option<TrayAction> {
    Some(match id {
        "fullscreen" => TrayAction::Fullscreen,
        "corner" => TrayAction::Corner,
        "wallpaper" => TrayAction::Wallpaper,
        "settings" => TrayAction::Settings,
        "quit" => TrayAction::Quit,
        other => {
            let mode = other.strip_prefix("persona:")?;
            TrayAction::Persona(PersonaMode::ALL.into_iter().find(|m| m.as_str() == mode)?)
        }
    })
}

fn run_action<R: Runtime>(app: &AppHandle<R>, action: TrayAction) {
    let res = match action {
        TrayAction::Fullscreen => windows_manager::set_mode(app, HudMode::Fullscreen),
        TrayAction::Corner => windows_manager::set_mode(app, HudMode::Corner),
        TrayAction::Wallpaper => {
            let app = app.clone();
            std::thread::spawn(move || {
                let on = !windows_manager::wallpaper_on();
                if let Err(e) = windows_manager::set_wallpaper(&app, on, true) {
                    eprintln!("[jarvis] трей: {e}");
                }
            });
            Ok(())
        }
        TrayAction::Settings => {
            // создание окна — не из обработчика цикла событий, а из отдельного потока
            let app = app.clone();
            std::thread::spawn(move || {
                if let Err(e) = windows_manager::open_settings(&app, None) {
                    eprintln!("[jarvis] трей: {e}");
                }
            });
            Ok(())
        }
        TrayAction::Persona(mode) => {
            // запись конфига и применение к ядру — не из обработчика цикла событий
            let app = app.clone();
            std::thread::spawn(move || set_persona(&app, mode));
            Ok(())
        }
        TrayAction::Quit => {
            app.exit(0);
            Ok(())
        }
    };
    if let Err(e) = res {
        eprintln!("[jarvis] трей: {e}");
    }
}

/// Сохранить режим характера (`ConfigStore::update`) и сразу применить (`after_save` → `Core::apply_config`,
/// окна получают `CONFIG_CHANGED_EVENT`, галочки трея обновляются там же).
pub fn set_persona<R: Runtime>(app: &AppHandle<R>, mode: PersonaMode) {
    let store = app.state::<ConfigStore>();
    if store.get().persona.mode == mode {
        // Windows сам снимает галочку при повторном клике — вернуть её
        sync_persona(app, mode);
        return;
    }
    match store.update(|c| c.persona.mode = mode) {
        Ok(saved) => {
            let core = app.state::<crate::commands::CoreState>();
            crate::commands::after_save(app, &core.0, &saved);
        }
        Err(e) => {
            eprintln!("[jarvis] трей: не удалось сохранить характер: {e}");
            sync_persona(app, store.get().persona.mode);
        }
    }
}

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let full = MenuItem::with_id(app, "fullscreen", "Во весь экран", true, Some("Ctrl+Alt+J"))?;
    let corner = MenuItem::with_id(app, "corner", "В угол", true, None::<&str>)?;
    let wallpaper = MenuItem::with_id(app, "wallpaper", "Живые обои: вкл/выкл", true, None::<&str>)?;
    let current = app.try_state::<ConfigStore>().map(|s| s.get().persona.mode).unwrap_or_default();
    let persona_items = PersonaMode::ALL
        .into_iter()
        .map(|m| Ok((m, CheckMenuItem::with_id(app, persona_item_id(m), m.title(), true, m == current, None::<&str>)?)))
        .collect::<tauri::Result<Vec<_>>>()?;
    let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> =
        persona_items.iter().map(|(_, i)| i as &dyn tauri::menu::IsMenuItem<R>).collect();
    let persona = Submenu::with_items(app, "Характер", true, &refs)?;
    let settings = MenuItem::with_id(app, "settings", "Настройки", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&full, &corner, &wallpaper, &persona, &settings, &sep, &quit])?;
    app.manage(PersonaMenu(persona_items));

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
        assert_eq!(action_for("wallpaper"), Some(TrayAction::Wallpaper));
        assert_eq!(action_for("settings"), Some(TrayAction::Settings));
        assert_eq!(action_for("quit"), Some(TrayAction::Quit));
        assert_eq!(action_for("other"), None);
        for m in PersonaMode::ALL {
            assert_eq!(action_for(&persona_item_id(m)), Some(TrayAction::Persona(m)));
        }
        assert_eq!(action_for("persona:rude"), None);
    }
}
