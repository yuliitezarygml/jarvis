use crate::core::Core;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    process::{Child, Command},
    sync::Mutex,
    time::Duration,
};

unsafe extern "C" {
    fn jv_endpoint_volume(request: i32) -> i32;
    fn jv_close_app(pid: u32);
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Volume {
    percent: u8,
}
pub fn volume(args: Option<&Value>) -> Result<Value> {
    let requested = match args {
        Some(v) => {
            let a: Volume = serde_json::from_value(v.clone())?;
            ensure!(a.percent <= 100, "Громкость от 0 до 100");
            i32::from(a.percent)
        }
        None => -1,
    };
    let readback = unsafe { jv_endpoint_volume(requested) };
    ensure!(readback >= 0, "Аудиоустройство недоступно");
    Ok(json!({"percent":readback,"verified":requested<0 || readback==requested}))
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Hash)]
#[serde(rename_all = "snake_case")]
enum App {
    CharacterMap,
    WindowsVersion,
    Notepad,
}
impl App {
    fn executable(self) -> &'static str {
        match self {
            Self::CharacterMap => "charmap.exe",
            Self::WindowsVersion => "winver.exe",
            Self::Notepad => "notepad.exe",
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppRequest {
    app: App,
}
#[derive(Default)]
pub struct Apps(Mutex<HashMap<App, Child>>);
impl Apps {
    fn launch(&self, app: App) -> Result<Value> {
        let mut owned = self.0.lock().unwrap();
        if let Some(child) = owned.get_mut(&app) {
            if child.try_wait()?.is_none() {
                return Ok(json!({"app":app.executable(),"verified":true,"already_running":true}));
            }
        }
        let mut child = Command::new(super::system_dir()?.join(app.executable())).spawn()?;
        let alive = child.try_wait()?.is_none();
        let pid = child.id();
        if alive {
            owned.insert(app, child);
        }
        Ok(json!({"app":app.executable(),"pid":pid,"verified":alive}))
    }
    fn close(&self, app: App) -> Result<u32> {
        let mut owned = self.0.lock().unwrap();
        let child = owned
            .get_mut(&app)
            .context("У Jarvis нет запущенного экземпляра этого приложения")?;
        ensure!(child.try_wait()?.is_none(), "Приложение уже завершилось");
        let pid = child.id();
        // Keep the process handle alive. An unrelated process cannot reuse this PID.
        unsafe { jv_close_app(pid) };
        Ok(pid)
    }
    fn stopped(&self, app: App, pid: u32) -> Result<bool> {
        let mut owned = self.0.lock().unwrap();
        if let Some(child) = owned.get_mut(&app) {
            if child.id() != pid {
                return Ok(true);
            }
            if child.try_wait()?.is_none() {
                return Ok(false);
            }
            owned.remove(&app);
        }
        Ok(true)
    }
}
pub async fn app_action(core: &Core, turn: &str, tool: &str, args: &Value) -> Result<Value> {
    let a: AppRequest = serde_json::from_value(args.clone())?;
    let apps = &core.actions.apps;
    if tool == "app_launch" {
        return core
            .for_turn(turn, || apps.launch(a.app))
            .context("Отменено")?;
    }
    let pid = core
        .for_turn(turn, || apps.close(a.app))
        .context("Отменено")??;
    for _ in 0..20 {
        if apps.stopped(a.app, pid)? {
            return Ok(json!({"app":a.app.executable(),"verified":true}));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(
        json!({"verified":false,"message":"Запрошено закрытие. Возможно, приложение ожидает сохранения документа."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_accepts_only_known_apps_without_arguments() {
        assert!(serde_json::from_value::<AppRequest>(json!({"app":"notepad"})).is_ok());
        for v in [
            json!({"app":"cmd.exe"}),
            json!({"app":"notepad","args":["file"]}),
            json!({"app":"C:\\x.exe"}),
        ] {
            assert!(serde_json::from_value::<AppRequest>(v).is_err());
        }
        assert!(volume(Some(&json!({"percent":101}))).is_err());
    }
}
