//! J.A.R.V.I.S. — оболочка Tauri 2 и ядро-оркестратор.
//!
//! - окна hud/corner/overlay/settings, трей, глобальные хоткеи, мост событий `jarvis_core::Event` → фронтенд;
//! - [`runtime::Core`] собирает все модули (агент, LLM, голос, память, ПК, система, интернет, база знаний)
//!   и получает события HUD через `TauriSink`, остановку и подтверждения — через `UiHooks`;
//! - симулятор (dev-панель) остался для проверки интерфейса без бэкенда.

pub mod api;
pub mod commands;
pub mod config_store;
pub mod hooks;
pub mod hotkeys;
pub mod runtime;
pub mod simulator;
pub mod sink;
pub mod tray;
pub mod windows_manager;

use std::sync::Arc;

use jarvis_core::config::HudMode;
use jarvis_core::{Event, EventSink};
use tauri::Manager;

use crate::commands::{CoreState, SimState, SinkState};
use crate::config_store::ConfigStore;
use crate::hooks::AppHooks;
use crate::runtime::{Core, CoreHooks, CoreOptions};
use crate::sink::TauriSink;
use crate::windows_manager::{WindowEffects, WindowsState};

pub fn run() {
    let config_path = config_store::config_path();
    let root = config_store::project_root(&config_path);
    let (store, config_error) = ConfigStore::open(config_path);
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
        .setup(move |app| {
            let handle = app.handle().clone();
            let sink = TauriSink::new(handle.clone())
                .with_effects(Arc::new(WindowEffects))
                .with_effects(Arc::new(commands::ConfirmTimeouts));
            app.manage(SinkState(sink.clone()));

            // Ядро: все модули, один реестр инструментов, агент, голос, телеметрия.
            let events: Arc<dyn EventSink> = Arc::new(sink.clone());
            let rt = tauri::async_runtime::handle().inner().clone();
            let core = Core::start(initial.clone(), events, CoreOptions::app(root.clone()), rt);
            app.state::<AppHooks>().set(Arc::new(CoreHooks(Arc::downgrade(&core))));
            let problems = core.startup_problems();
            app.manage(CoreState(core));
            // правки config.json снаружи — в ядро без перезапуска
            config_store::spawn_watcher(handle.clone());

            windows_manager::create_windows(&handle, &initial)?;
            tray::create(&handle)?;
            hotkeys::register(&handle);

            let mut messages: Vec<String> = Vec::new();
            if let Some(e) = config_error {
                eprintln!("[jarvis] config.json: {e}");
                messages.push(format!("config.json не прочитан, взяты настройки по умолчанию: {e}"));
            }
            messages.extend(problems);
            if !messages.is_empty() {
                // окна ещё грузятся — покажем чуть позже
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    for m in messages {
                        eprintln!("[jarvis] {m}");
                        sink.emit(Event::Error { message: m });
                    }
                });
            }
            Ok(())
        })
        .on_window_event(windows_manager::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::set_hud_mode,
            commands::get_hud_mode,
            commands::toggle_hud_mode,
            commands::toggle_hud_fullscreen,
            commands::exit_hud_fullscreen,
            commands::open_settings,
            commands::overlay_geometry,
            commands::config_get,
            commands::config_set,
            commands::config_path_get,
            commands::emergency_stop,
            commands::confirm_respond,
            commands::open_url,
            commands::events_replay,
            commands::simulate_demo,
            commands::simulate_stop,
            commands::simulate_event,
            commands::agent_submit,
            commands::mic_get,
            commands::mic_set,
            commands::assistant_status,
            commands::provider_presets,
            commands::provider_check,
            commands::provider_apply_best,
            commands::voice_check,
            commands::search_presets,
            commands::search_check,
            commands::audio_devices,
            commands::audio_mic_test,
            commands::audio_mic_test_stop,
            commands::audio_sound_test,
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
