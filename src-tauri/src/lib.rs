//! J.A.R.V.I.S. — оболочка Tauri 2: окна hud/corner/overlay/settings, трей, глобальные хоткеи,
//! мост событий `jarvis_core::Event` → фронтенд, конфиг, симулятор и демо-заглушки для экранов.
//!
//! Подключение бэкенда (интегратор):
//! - события: `app.state::<commands::SinkState<Wry>>().0` — это `jarvis_core::EventSink` (`TauriSink`);
//! - остановка/подтверждения: `app.state::<hooks::AppHooks>().set(Arc::new(MyHooks))`;
//! - демо-команды в `commands.rs` помечены «ДЕМО» — заменить тела на настоящие модули.

pub mod commands;
pub mod config_store;
pub mod demo;
pub mod hooks;
pub mod hotkeys;
pub mod simulator;
pub mod sink;
pub mod tray;
pub mod windows_manager;

use std::sync::Arc;

use jarvis_core::config::HudMode;
use jarvis_core::{Event, EventSink};
use tauri::Manager;

use crate::commands::{SimState, SinkState};
use crate::config_store::ConfigStore;
use crate::hooks::AppHooks;
use crate::sink::TauriSink;
use crate::windows_manager::{WindowEffects, WindowsState};

pub fn run() {
    let (store, config_error) = ConfigStore::open(config_store::config_path());
    let initial = store.get();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // повторный запуск — просто развернуть HUD
            let _ = windows_manager::set_mode(app, HudMode::Fullscreen);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(store)
        .manage(WindowsState::default())
        .manage(AppHooks::default())
        .manage(SimState::default())
        .manage(demo::MemoryStore::default())
        .setup(move |app| {
            let handle = app.handle().clone();
            let sink = TauriSink::new(handle.clone()).with_effects(Arc::new(WindowEffects));
            app.manage(SinkState(sink.clone()));

            windows_manager::create_windows(&handle, &initial)?;
            tray::create(&handle)?;
            hotkeys::register(&handle);

            if let Some(e) = config_error {
                eprintln!("[jarvis] config.json: {e}");
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    sink.emit(Event::Error { message: format!("config.json не прочитан, взяты настройки по умолчанию: {e}") });
                });
            }
            Ok(())
        })
        .on_window_event(windows_manager::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::set_hud_mode,
            commands::get_hud_mode,
            commands::toggle_hud_mode,
            commands::open_settings,
            commands::overlay_geometry,
            commands::config_get,
            commands::config_set,
            commands::config_path_get,
            commands::emergency_stop,
            commands::confirm_respond,
            commands::events_replay,
            commands::simulate_demo,
            commands::simulate_stop,
            commands::simulate_event,
            commands::provider_presets,
            commands::provider_check,
            commands::voice_check,
            commands::search_presets,
            commands::search_check,
            commands::audio_devices,
            commands::memory_list,
            commands::memory_upsert,
            commands::memory_delete,
            commands::kb_search,
            commands::history_list,
            commands::app_info,
        ])
        .build(tauri::generate_context!())
        .expect("не удалось запустить J.A.R.V.I.S.")
        .run(|_app, event| {
            // Окна служебные и не закрываются, но на всякий случай не выходим, пока не нажали «Выход».
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}
