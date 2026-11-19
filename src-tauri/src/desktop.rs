use crate::{audio, capture, core::Core, provider, storage::Settings, tools::ToolRegistry, tts};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::State;
type Shared<'a> = State<'a, Arc<Core>>;
fn error(e: anyhow::Error) -> String {
    format!("{e:#}")
}
#[tauri::command]
pub fn bootstrap(core: Shared) -> Result<Value, String> {
    let db = core.db.lock().unwrap();
    Ok(
        json!({"windows":cfg!(windows),"sessions":db.sessions().map_err(error)?,"settings":db.settings().map_err(error)?,"preferences":db.prefs().map_err(error)?,"stt":core.stt.available(),"streamingStt":core.stt.streaming_available(),"microphone":audio::mic_status()}),
    )
}
#[tauri::command]
pub fn messages(core: Shared, session: String) -> Result<Vec<Value>, String> {
    core.db
        .lock()
        .unwrap()
        .history(&session, 300)
        .map_err(error)
}
#[tauri::command]
pub async fn send_text(core: Shared<'_>, session: String, text: String) -> Result<String, String> {
    capture::end(&core, false).await;
    core.start(session, text).map_err(error)
}
#[tauri::command]
pub async fn interrupt(core: Shared<'_>) -> Result<(), String> {
    core.cancel();
    capture::end(&core, false).await;
    core.cancel();
    Ok(())
}
#[tauri::command]
pub async fn microphone(core: Shared<'_>, session: String, mode: String) -> Result<(), String> {
    match mode.as_str() {
        "ptt" | "auto" => capture::start(&core, session, mode == "auto")
            .await
            .map_err(error),
        "submit" | "off" => {
            capture::end(&core, mode == "submit").await;
            Ok(())
        }
        _ => Err("Некорректный режим микрофона".into()),
    }
}
#[tauri::command]
pub fn preferences(
    core: Shared,
    text: Option<String>,
    forget: Option<i64>,
) -> Result<Vec<Value>, String> {
    let db = core.db.lock().unwrap();
    if let Some(t) = text {
        db.preference(&t).map_err(error)?;
    }
    if let Some(id) = forget {
        db.forget(id).map_err(error)?;
    }
    db.prefs().map_err(error)
}
#[tauri::command]
pub fn save_settings(core: Shared, settings: Settings) -> Result<(), String> {
    provider::base(&settings.url).map_err(error)?;
    core.db.lock().unwrap().save(&settings).map_err(error)
}
#[tauri::command]
pub async fn session_action(core: Shared<'_>, remove: Option<String>) -> Result<Value, String> {
    capture::end(&core, false).await;
    core.cancel();
    let mut db = core.db.lock().unwrap();
    let id = if let Some(id) = remove {
        db.delete(&id).map_err(error)?;
        None
    } else {
        Some(db.new_session().map_err(error)?)
    };
    Ok(json!({"sessions":db.sessions().map_err(error)?,"id":id}))
}
#[tauri::command]
pub fn telemetry(core: Shared) -> Value {
    core.telemetry.sample()
}
#[tauri::command]
pub async fn connections(core: Shared<'_>) -> Result<Value, String> {
    let s = core.db.lock().unwrap().settings().map_err(error)?;
    let (models, tts) = tokio::join!(core.llm.models(&s), tts::ready(&core.http));
    let models_error = models.as_ref().err().map(|e| format!("{e:#}"));
    let tts_error = tts.as_ref().err().map(|e| format!("{e:#}"));
    Ok(
        json!({"models":models.unwrap_or_default(),"llmError":models_error,"tts":tts.ok(),"ttsError":tts_error}),
    )
}
#[tauri::command]
pub async fn run_tool(core: Shared<'_>, name: String) -> Result<Value, String> {
    ToolRegistry::run(&core, &name, &json!({}))
        .await
        .map_err(error)
}
#[tauri::command]
pub fn confirm_action(
    core: Shared,
    turn: String,
    digest: String,
    allow: bool,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        core.actions.decide(&turn, &digest, allow).map_err(error)
    }
    #[cfg(not(windows))]
    {
        let _ = (core, turn, digest, allow);
        Err("Недоступно на этой платформе".into())
    }
}
