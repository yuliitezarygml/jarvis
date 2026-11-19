mod audio;
mod capture;
mod commands;
mod core;
mod desktop;
mod events;
#[cfg(windows)]
mod kb;
mod platform;
mod provider;
mod speech_normalizer;
mod storage;
mod tools;
mod tts;
use std::sync::Arc;
use tauri::{Emitter, Manager};
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_handle = app.handle().clone();
            let dir = app.path().app_data_dir()?;
            let core = core::Core::open(
                &dir,
                Arc::new(move |event| {
                    let _ = app_handle.emit("jarvis-event", event);
                }),
            )?;
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::get_app_info,
            desktop::bootstrap,
            desktop::messages,
            desktop::send_text,
            desktop::interrupt,
            desktop::microphone,
            desktop::preferences,
            desktop::save_settings,
            desktop::session_action,
            desktop::telemetry,
            desktop::connections,
            desktop::run_tool
        ])
        .run(tauri::generate_context!())
        .expect("error while running Jarvis");
}
