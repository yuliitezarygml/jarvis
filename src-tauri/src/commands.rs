//! Tauri-команды (вызываются из фронтенда через `invoke`). Имена и сигнатуры — контракт для интегратора;
//! команды с пометкой «ДЕМО» возвращают мок-данные (`demo: true`) и подлежат замене.

use std::sync::{Arc, Mutex};

use jarvis_core::config::HudMode;
use jarvis_core::event::ActionStatus;
use jarvis_core::{AssistantState, CancellationToken, Config, Event, EventSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::config_store::{self, ConfigStore};
use crate::demo;
use crate::hooks::AppHooks;
use crate::simulator;
use crate::sink::{EMERGENCY_EVENT, TauriSink};
use crate::windows_manager::{self, OverlayGeometry, WindowsState};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Общий синк событий в состоянии Tauri (`Arc<dyn EventSink>` отдаётся бэкенду интегратором).
pub struct SinkState<R: Runtime>(pub TauriSink<R>);

/// Запущенный сценарий симулятора.
#[derive(Default)]
pub struct SimState(Mutex<Option<CancellationToken>>);

// ───────────────────────────── окна ─────────────────────────────

#[tauri::command]
pub fn set_hud_mode<R: Runtime>(app: AppHandle<R>, mode: HudMode) -> CmdResult<()> {
    windows_manager::set_mode(&app, mode).map_err(err)
}

#[tauri::command]
pub fn get_hud_mode(state: State<'_, WindowsState>) -> HudMode {
    state.mode()
}

#[tauri::command]
pub fn toggle_hud_mode<R: Runtime>(app: AppHandle<R>) -> CmdResult<HudMode> {
    windows_manager::toggle_mode(&app).map_err(err)
}

#[tauri::command]
pub fn open_settings<R: Runtime>(app: AppHandle<R>, tab: Option<String>) -> CmdResult<()> {
    windows_manager::open_settings(&app, tab.as_deref()).map_err(err)
}

#[tauri::command]
pub fn overlay_geometry<R: Runtime>(app: AppHandle<R>) -> Option<OverlayGeometry> {
    windows_manager::overlay_geometry(&app)
}

// ───────────────────────────── конфиг ─────────────────────────────

/// Конфиг для UI: ключи замаскированы (`gsk…abcd`).
#[tauri::command]
pub fn config_get(store: State<'_, ConfigStore>) -> Config {
    config_store::masked(&store.get())
}

/// Сохранить конфиг из UI. Маска вместо ключа = «не менять». Возвращает сохранённое (замаскированным).
#[tauri::command]
pub fn config_set(store: State<'_, ConfigStore>, config: Config) -> CmdResult<Config> {
    store.set_from_ui(config).map(|c| config_store::masked(&c))
}

#[tauri::command]
pub fn config_path_get(store: State<'_, ConfigStore>) -> String {
    store.path().display().to_string()
}

// ───────────────────────────── управление ─────────────────────────────

/// Аварийная остановка (кнопка в HUD; хоткей Ctrl+Alt+K вызывает то же самое).
#[tauri::command]
pub fn emergency_stop<R: Runtime>(app: AppHandle<R>) {
    do_emergency_stop(&app, "button");
}

pub fn do_emergency_stop<R: Runtime>(app: &AppHandle<R>, source: &str) {
    app.state::<AppHooks>().emergency_stop(source);
    if let Some(t) = app.state::<SimState>().0.lock().unwrap().take() {
        t.cancel();
    }
    let sink = &app.state::<SinkState<R>>().0;
    let (running, pending) = {
        let cache = sink.cache();
        let c = cache.lock().unwrap();
        (c.running_actions(), c.pending_confirm_id())
    };
    for e in running {
        if let Event::Action { id, tool, summary, .. } = e {
            sink.emit(Event::Action { id, tool, summary, status: ActionStatus::Cancelled });
        }
    }
    if let Some(id) = pending {
        sink.emit(Event::ConfirmResolved { id, approved: false });
    }
    sink.emit(Event::ControlStop);
    sink.emit(Event::State { state: AssistantState::Idle });
    let _ = app.emit(EMERGENCY_EVENT, serde_json::json!({ "source": source }));
}

/// Ответ на `ConfirmRequest` (кнопки «Да/Нет» в HUD или истёкший таймер → `approved: false`).
#[tauri::command]
pub fn confirm_respond<R: Runtime>(app: AppHandle<R>, id: String, approved: bool) {
    app.state::<AppHooks>().confirm(&id, approved);
    app.state::<SinkState<R>>().0.emit(Event::ConfirmResolved { id, approved });
}

/// Снимок текущего состояния интерфейса — новое окно прогоняет его через свой редьюсер.
#[tauri::command]
pub fn events_replay<R: Runtime>(sink: State<'_, SinkState<R>>) -> Vec<Event> {
    sink.0.replay()
}

// ───────────────────────────── симулятор ─────────────────────────────

/// Проиграть демо-сценарий (все состояния, субтитры, волна, свечение с целями, подтверждение, источники,
/// телеметрия, ошибка). Повторный вызов перезапускает сценарий. Возвращает длительность, мс.
#[tauri::command]
pub fn simulate_demo<R: Runtime>(app: AppHandle<R>) -> u64 {
    let token = app.state::<AppHooks>().demo_token().child_token();
    if let Some(prev) = app.state::<SimState>().0.lock().unwrap().replace(token.clone()) {
        prev.cancel();
    }
    let sink: Arc<dyn EventSink> = Arc::new(app.state::<SinkState<R>>().0.clone());
    simulator::spawn_demo(sink, token);
    simulator::demo_duration_ms()
}

#[tauri::command]
pub fn simulate_stop<R: Runtime>(app: AppHandle<R>) {
    if let Some(t) = app.state::<SimState>().0.lock().unwrap().take() {
        t.cancel();
    }
}

/// Отправить одно произвольное событие (dev-панель, отладка интеграции).
#[tauri::command]
pub fn simulate_event<R: Runtime>(sink: State<'_, SinkState<R>>, event: Event) {
    sink.0.emit(event);
}

// ───────────────────────────── ДЕМО: провайдеры, серверы, поиск, аудио ─────────────────────────────

#[tauri::command]
pub fn provider_presets() -> Vec<demo::ProviderPreset> {
    demo::provider_presets()
}

/// ДЕМО. Проверка ключа и список моделей с ранжировкой по ролям. `api_key` может быть маской сохранённого ключа.
#[tauri::command]
pub fn provider_check(store: State<'_, ConfigStore>, provider_id: String, base_url: String, api_key: String) -> demo::ProviderCheck {
    let key = config_store::resolve_key(&store.get(), &provider_id, &api_key);
    let needs_key = demo::provider_presets().iter().find(|p| p.id == provider_id).is_none_or(|p| p.needs_key);
    demo::provider_check(&provider_id, &base_url, &key, needs_key)
}

/// ДЕМО. `kind`: `tts` | `stt`.
#[tauri::command]
pub fn voice_check(kind: String, url: String) -> demo::ServiceCheck {
    demo::voice_check(&kind, &url)
}

#[tauri::command]
pub fn search_presets() -> Vec<demo::SearchPreset> {
    demo::search_presets()
}

/// ДЕМО. Тестовый поисковый запрос.
#[tauri::command]
pub fn search_check(store: State<'_, ConfigStore>, id: String, api_key: String, extra: String) -> demo::ServiceCheck {
    let key = config_store::resolve_search_key(&store.get(), &id, &api_key);
    demo::search_check(&id, &key, &extra)
}

/// ДЕМО. Список микрофонов и устройств вывода.
#[tauri::command]
pub fn audio_devices() -> demo::AudioDevices {
    demo::audio_devices()
}

// ───────────────────────────── ДЕМО: память, KB, история ─────────────────────────────

#[tauri::command]
pub fn memory_list(mem: State<'_, demo::MemoryStore>) -> demo::MemoryData {
    mem.list()
}

/// ДЕМО. `kind`: `facts` | `lessons` | `playbooks`; `item` с пустым `id` — новая запись.
#[tauri::command]
pub fn memory_upsert(mem: State<'_, demo::MemoryStore>, kind: String, item: serde_json::Value) -> CmdResult<demo::MemoryData> {
    mem.upsert(&kind, item)
}

#[tauri::command]
pub fn memory_delete(mem: State<'_, demo::MemoryStore>, kind: String, id: String) -> CmdResult<demo::MemoryData> {
    mem.delete(&kind, &id)
}

/// ДЕМО. Поиск по базе знаний Windows.
#[tauri::command]
pub fn kb_search(query: String) -> demo::KbResult {
    demo::kb_search(&query)
}

/// ДЕМО. История диалогов.
#[tauri::command]
pub fn history_list() -> demo::HistoryResult {
    demo::history()
}

/// Сведения для экрана «Окна»/«Разрешения».
#[derive(Serialize)]
pub struct AppInfo {
    pub version: String,
    pub config_path: String,
    pub debug_build: bool,
    pub elevated: bool,
}

#[tauri::command]
pub fn app_info<R: Runtime>(app: AppHandle<R>, store: State<'_, ConfigStore>) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        config_path: store.path().display().to_string(),
        debug_build: cfg!(debug_assertions),
        elevated: is_elevated(),
    }
}

/// Запущен ли процесс с повышенными правами (TokenElevation).
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: токен текущего процесса, буфер нужного размера на стеке, хэндл закрывается.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut std::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}
