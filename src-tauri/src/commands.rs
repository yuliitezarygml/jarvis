//! Tauri-команды (вызываются из фронтенда через `invoke`). Все экраны обслуживают настоящие модули через
//! ядро `runtime::Core`; демо осталось только в симуляторе (dev-панель: `simulate_*`).

use std::sync::{Arc, Mutex};

use jarvis_core::config::HudMode;
use jarvis_core::event::ActionStatus;
use jarvis_core::{AssistantState, CancellationToken, Config, Event, EventSink};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::api;
use crate::config_store::{self, ConfigStore};
use crate::hooks::AppHooks;
use crate::runtime::{Core, MicStatus, services};
use crate::simulator;
use crate::sink::{EMERGENCY_EVENT, TauriSink};
use crate::windows_manager::{self, OverlayGeometry, WindowsState};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Общий синк событий в состоянии Tauri (`Arc<dyn EventSink>` отдаётся бэкенду интегратором).
pub struct SinkState(pub TauriSink<tauri::Wry>);

/// Запущенный сценарий симулятора.
#[derive(Default)]
pub struct SimState(Mutex<Option<CancellationToken>>);

// ───────────────────────────── окна ─────────────────────────────
// Команды, которые трогают окна, — async: синхронная команда выполняется в потоке WebView2,
// и создание/показ окна из неё на Windows может зависнуть (предупреждение в документации Tauri).

#[tauri::command]
pub async fn set_hud_mode(app: AppHandle, mode: HudMode) -> CmdResult<()> {
    windows_manager::set_mode(&app, mode).map_err(err)
}

#[tauri::command]
pub fn get_hud_mode(state: State<'_, WindowsState>) -> HudMode {
    state.mode()
}

#[tauri::command]
pub async fn toggle_hud_mode(app: AppHandle) -> CmdResult<HudMode> {
    windows_manager::toggle_mode(&app).map_err(err)
}

/// Живые обои: HUD на рабочем столе под иконками. Возвращает новое состояние.
#[tauri::command]
pub async fn set_wallpaper_mode(app: AppHandle, on: bool) -> CmdResult<bool> {
    let app2 = app.clone();
    // Окно создаётся вне цикла событий.
    tauri::async_runtime::spawn_blocking(move || windows_manager::set_wallpaper(&app2, on, true))
        .await
        .map_err(|e| e.to_string())?
}

/// Клик по полю ввода на обоях: клавиатуру обоям не передать — открыть окно HUD и поставить курсор в поле.
#[tauri::command]
pub async fn focus_command(app: AppHandle) -> CmdResult<()> {
    tracing::info!("живые обои: клик по полю ввода — открываю окно HUD");
    windows_manager::set_mode(&app, HudMode::Fullscreen).map_err(err)?;
    app.emit_to(windows_manager::HUD, FOCUS_COMMAND_EVENT, ()).map_err(err)
}

/// Диагностика живых обоев: страница сообщает, что увидела клик (доходит ли ввод до WebView2).
#[tauri::command]
pub fn wallpaper_debug(what: String) {
    tracing::info!(%what, "живые обои: страница получила событие");
}

/// Событие окну HUD: поставить курсор в «Сказать Джарвису…».
pub const FOCUS_COMMAND_EVENT: &str = "jarvis://focus_command";

#[tauri::command]
pub fn get_wallpaper_mode() -> bool {
    windows_manager::wallpaper_on()
}

/// Виджет в углу: открыть/закрыть панель подписей (что сказали, что отвечает, что делает).
/// Возвращает сторону панели: `left` / `right` / `none`.
#[tauri::command]
pub async fn corner_caption(app: AppHandle, open: bool) -> CmdResult<String> {
    windows_manager::set_corner_caption(&app, open).map(str::to_string).map_err(err)
}

/// HUD: полный экран ↔ обычное окно. `true` = теперь полный экран.
#[tauri::command]
pub async fn toggle_hud_fullscreen(app: AppHandle) -> CmdResult<bool> {
    windows_manager::toggle_hud_fullscreen(&app).map_err(err)
}

/// Выйти из полного экрана HUD (Esc). `true`, если HUD был на весь экран.
#[tauri::command]
pub async fn exit_hud_fullscreen(app: AppHandle) -> CmdResult<bool> {
    windows_manager::exit_hud_fullscreen(&app).map_err(err)
}

#[tauri::command]
pub async fn open_settings(app: AppHandle, tab: Option<String>) -> CmdResult<()> {
    windows_manager::open_settings(&app, tab.as_deref()).map_err(err)
}

#[tauri::command]
pub async fn overlay_geometry(app: AppHandle) -> CmdResult<Option<OverlayGeometry>> {
    Ok(windows_manager::overlay_geometry(&app))
}

// ───────────────────────────── конфиг ─────────────────────────────

/// Конфиг для UI: ключи замаскированы (`gsk…abcd`).
#[tauri::command]
pub fn config_get(store: State<'_, ConfigStore>) -> Config {
    config_store::masked(&store.get())
}

/// Сохранить конфиг из UI. Маска вместо ключа = «не менять». Возвращает сохранённое (замаскированным).
/// Модели и провайдеры, автономия, зрение, поиск, серверы голоса и resource guard применяются сразу
/// (`Core::apply_config`); папка данных и папки поиска — после перезапуска (об этом сообщение в HUD).
/// Внешний вид применяется во всех окнах сразу (событие `CONFIG_CHANGED_EVENT`, размер виджета).
/// `base` — конфиг, который экран получил при открытии: поля, которые пользователь не менял, берутся из
/// файла (правки config.json снаружи и режим/позиция HUD не затираются старой копией).
#[tauri::command]
pub async fn config_set(
    app: AppHandle,
    store: State<'_, ConfigStore>,
    core: State<'_, CoreState>,
    config: Config,
    base: Option<Config>,
) -> CmdResult<Config> {
    let saved = store.set_from_ui(config, base)?;
    Ok(after_save(&app, &core.0, &saved))
}

/// Сохранённый конфиг → ядро, размер виджета, все окна (тема, панели HUD). Возвращает замаскированный.
pub fn after_save<R: tauri::Runtime>(app: &AppHandle<R>, core: &Core, saved: &Config) -> Config {
    core.apply_config(saved);
    apply_corner_size(app, saved.appearance.corner_size);
    if saved.hud.wallpaper != windows_manager::wallpaper_on() {
        let (app, on) = (app.clone(), saved.hud.wallpaper);
        std::thread::spawn(move || {
            if let Err(e) = windows_manager::set_wallpaper(&app, on, false) {
                tracing::warn!(error = %e, "живые обои");
            }
        });
    }
    let masked = config_store::masked(saved);
    let _ = app.emit(config_store::CONFIG_CHANGED_EVENT, &masked);
    masked
}

/// Размер виджета в углу (`appearance.corner_size`). Правый нижний угол остаётся на месте,
/// поэтому виджет у края экрана не уезжает за край.
pub fn apply_corner_size<R: tauri::Runtime>(app: &AppHandle<R>, size: jarvis_core::config::CornerSize) {
    // Размер считается по квадрату реактора — сначала закрыть панель подписей (страница откроет снова).
    let _ = windows_manager::set_corner_caption(app, false);
    let Some(w) = app.get_webview_window(windows_manager::CORNER) else { return };
    let scale = w.scale_factor().unwrap_or(1.0);
    let side = (size.px() * scale).round() as i32;
    let (Ok(cur), Ok(pos)) = (w.outer_size(), w.outer_position()) else { return };
    let (cw, ch) = (cur.width as i32, cur.height as i32);
    if cw == side && ch == side {
        return;
    }
    let _ = w.set_size(tauri::PhysicalSize::new(side as u32, side as u32));
    let _ = w.set_position(tauri::PhysicalPosition::new(pos.x + cw - side, pos.y + ch - side));
}

#[tauri::command]
pub fn config_path_get(store: State<'_, ConfigStore>) -> String {
    store.path().display().to_string()
}

// ───────────────────────────── управление ─────────────────────────────

/// Аварийная остановка (кнопка в HUD; хоткей Ctrl+Alt+K вызывает то же самое).
#[tauri::command]
pub fn emergency_stop(app: AppHandle) {
    do_emergency_stop(&app, "button");
}

pub fn do_emergency_stop(app: &AppHandle, source: &str) {
    app.state::<AppHooks>().emergency_stop(source);
    if let Some(t) = app.state::<SimState>().0.lock().unwrap().take() {
        t.cancel();
    }
    let sink = &app.state::<SinkState>().0;
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
        app.state::<AppHooks>().confirm(&id, false);
        sink.emit(Event::ConfirmResolved { id, approved: false });
    }
    sink.emit(Event::ControlStop);
    sink.emit(Event::State { state: AssistantState::Idle });
    let _ = app.emit(EMERGENCY_EVENT, serde_json::json!({ "source": source }));
}

/// Ответ на `ConfirmRequest` (кнопки «Да/Нет» в HUD). Ответ на уже закрытый запрос — ошибка.
#[tauri::command]
pub fn confirm_respond(app: AppHandle, id: String, approved: bool) -> CmdResult<()> {
    resolve_confirm(&app, &id, approved).then_some(()).ok_or_else(|| "Этот запрос уже закрыт".to_string())
}

/// Закрыть открытый запрос подтверждения: хук бэкенда + событие `ConfirmResolved`. false — запрос уже не открыт.
pub fn resolve_confirm(app: &AppHandle, id: &str, approved: bool) -> bool {
    let sink = &app.state::<SinkState>().0;
    if sink.cache().lock().unwrap().pending_confirm_id().as_deref() != Some(id) {
        return false;
    }
    app.state::<AppHooks>().confirm(id, approved);
    // Настоящий бэкенд (ConfirmGate) сам шлёт ConfirmResolved; заглушка — нет, тогда закрываем здесь.
    if sink.cache().lock().unwrap().pending_confirm_id().as_deref() == Some(id) {
        sink.emit(Event::ConfirmResolved { id: id.to_string(), approved });
    }
    true
}

/// Таймаут подтверждения на стороне оболочки: если за `timeout_ms` (+1,5 с запаса) никто не ответил —
/// «нет» (fail-closed), чтобы запрос не висел, даже если бэкенд не следит за временем.
pub struct ConfirmTimeouts;

impl crate::sink::SinkEffects<tauri::Wry> for ConfirmTimeouts {
    fn on_event(&self, app: &AppHandle, event: &Event) {
        if let Event::ConfirmRequest { id, timeout_ms, .. } = event {
            let (app, id, wait) = (app.clone(), id.clone(), *timeout_ms + 1_500);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(wait));
                resolve_confirm(&app, &id, false);
            });
        }
    }
}

/// Открыть ссылку (источник, статья KB) в браузере по умолчанию. Только http(s).
#[tauri::command]
pub fn open_url(url: String) -> CmdResult<()> {
    let ok = url.starts_with("https://") || url.starts_with("http://");
    if !ok || url.chars().any(|c| c.is_control() || c == '"') {
        return Err("Открываются только ссылки http:// и https://".into());
    }
    #[cfg(windows)]
    let cmd = std::process::Command::new("explorer").arg(&url).spawn();
    #[cfg(not(windows))]
    let cmd = std::process::Command::new("xdg-open").arg(&url).spawn();
    cmd.map(|_| ()).map_err(err)
}

/// Снимок текущего состояния интерфейса — новое окно прогоняет его через свой редьюсер.
#[tauri::command]
pub fn events_replay(sink: State<'_, SinkState>) -> Vec<Event> {
    sink.0.replay()
}

// ───────────────────────────── симулятор ─────────────────────────────

/// Проиграть демо-сценарий (все состояния, субтитры, волна, свечение с целями, подтверждение, источники,
/// телеметрия, ошибка). Повторный вызов перезапускает сценарий. Возвращает длительность, мс.
#[tauri::command]
pub fn simulate_demo(app: AppHandle) -> u64 {
    let token = app.state::<AppHooks>().demo_token().child_token();
    if let Some(prev) = app.state::<SimState>().0.lock().unwrap().replace(token.clone()) {
        prev.cancel();
    }
    let sink: Arc<dyn EventSink> = Arc::new(app.state::<SinkState>().0.clone());
    simulator::spawn_demo(sink, token);
    simulator::demo_duration_ms()
}

#[tauri::command]
pub fn simulate_stop(app: AppHandle) {
    if let Some(t) = app.state::<SimState>().0.lock().unwrap().take() {
        t.cancel();
    }
}

/// Отправить одно произвольное событие (dev-панель, отладка интеграции).
#[tauri::command]
pub fn simulate_event(sink: State<'_, SinkState>, event: Event) {
    sink.0.emit(event);
}

// ───────────────────────────── ядро: команды, микрофон, статус ─────────────────────────────

/// Ядро-оркестратор в состоянии Tauri.
pub struct CoreState(pub Arc<Core>);

/// Текстовая команда из HUD («Сказать Джарвису…») — то же, что голосом после «Джарвис, …».
#[tauri::command]
pub fn agent_submit(core: State<'_, CoreState>, text: String) -> CmdResult<()> {
    core.0.submit(&text, false)
}

#[tauri::command]
pub fn mic_get(core: State<'_, CoreState>) -> MicStatus {
    core.0.mic_status()
}

/// Включить/выключить прослушивание микрофона (кнопка в HUD). Открытие устройства может занять секунды.
#[tauri::command]
pub async fn mic_set(core: State<'_, CoreState>, enabled: bool) -> CmdResult<MicStatus> {
    let core = core.0.clone();
    tauri::async_runtime::spawn_blocking(move || core.set_mic(enabled)).await.map_err(err)
}

/// Сводка для HUD: готова ли модель, микрофон, что не открылось при старте.
#[derive(Serialize)]
pub struct AssistantStatus {
    pub llm_ready: bool,
    /// «Нет ключа LLM — добавьте его в Настройках…», если модель «мозг» недоступна.
    pub llm_problem: Option<String>,
    pub brain_model: Option<String>,
    pub mic: MicStatus,
    pub issues: Vec<String>,
    pub tools: usize,
    pub busy: bool,
}

#[tauri::command]
pub fn assistant_status(core: State<'_, CoreState>) -> AssistantStatus {
    let c = &core.0;
    let problem = c.models.brain.problem();
    AssistantStatus {
        llm_ready: problem.is_none(),
        brain_model: c.models.brain.current().map(|m| m.id()),
        llm_problem: problem,
        mic: c.mic_status(),
        issues: c.issues.iter().map(|i| i.message.clone()).collect(),
        tools: c.agent.tools().len(),
        busy: c.is_busy(),
    }
}

// ───────────────────────────── провайдеры, серверы, поиск, аудио ─────────────────────────────

#[tauri::command]
pub fn provider_presets(core: State<'_, CoreState>) -> Vec<api::ProviderPreset> {
    services::provider_presets(&core.0)
}

/// Проверка ключа, список моделей, живая проверка и рейтинг по ролям (jarvis-llm `check_provider`).
/// `api_key` может быть маской сохранённого ключа.
#[tauri::command]
pub async fn provider_check(
    store: State<'_, ConfigStore>,
    core: State<'_, CoreState>,
    provider_id: String,
    base_url: String,
    api_key: String,
) -> CmdResult<api::ProviderCheck> {
    let key = config_store::resolve_key(&store.get(), &provider_id, &api_key);
    // Будущее проверки из jarvis-llm не доказывается как `Send` (ограничение rustc на замыканиях с dyn-трейтами),
    // поэтому оно выполняется через `block_on` в блокирующем потоке рантайма.
    let core = core.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(services::provider_check(&core, &provider_id, &base_url, key))
    })
    .await
    .map_err(err)
}

/// «Выбрать лучшее автоматически» (jarvis-llm `apply_best`) по последним проверкам: цепочки ролей
/// записываются в черновик из UI, он сохраняется в config.json и применяется без перезапуска.
#[tauri::command]
pub async fn provider_apply_best(
    app: AppHandle,
    store: State<'_, ConfigStore>,
    core: State<'_, CoreState>,
    config: Config,
    base: Option<Config>,
    provider_ids: Option<Vec<String>>,
) -> CmdResult<Config> {
    let saved = store.set_from_ui_with(config, base, |merged| services::apply_best(&core.0, merged, provider_ids.as_deref()))?;
    Ok(after_save(&app, &core.0, &saved))
}

/// `kind`: `tts` (`GET /ready`) | `stt` (`GET /health`).
#[tauri::command]
pub async fn voice_check(core: State<'_, CoreState>, kind: String, url: String) -> CmdResult<api::ServiceCheck> {
    let c = &core.0;
    Ok(services::voice_check(&c.config(), &c.root, &kind, &url).await)
}

#[tauri::command]
pub fn search_presets() -> Vec<api::SearchPreset> {
    services::search_presets()
}

/// Тестовый поисковый запрос (jarvis-web `validate_config`).
#[tauri::command]
pub async fn search_check(store: State<'_, ConfigStore>, id: String, api_key: String, extra: String) -> CmdResult<api::ServiceCheck> {
    let key = config_store::resolve_search_key(&store.get(), &id, &api_key);
    Ok(services::search_check(&id, key, &extra).await)
}

/// Микрофоны и устройства вывода (только имена; устройства не открываются).
#[tauri::command]
pub async fn audio_devices() -> CmdResult<api::AudioDevices> {
    tauri::async_runtime::spawn_blocking(services::audio_devices).await.map_err(err)?
}

/// Уровень микрофона во время «Проверить микрофон» (f32 RMS 0…1, ≈30 Гц).
pub const MIC_TEST_EVENT: &str = "jarvis://mic_test_level";

/// Флаг остановки текущей проверки микрофона.
static MIC_TEST: Mutex<Option<Arc<std::sync::atomic::AtomicBool>>> = Mutex::new(None);

/// «Проверить микрофон» в настройках: открыть выбранный (ещё не сохранённый) микрофон на `seconds` с,
/// уровень — событиями `MIC_TEST_EVENT`. Возвращает имя открытого устройства. Микрофон без кнопки не открывается.
#[tauri::command]
pub async fn audio_mic_test(app: AppHandle, device: Option<String>, seconds: Option<u64>) -> CmdResult<String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(prev) = MIC_TEST.lock().unwrap().replace(stop.clone()) {
        prev.store(true, Ordering::Release);
    }
    tauri::async_runtime::spawn_blocking(move || {
        services::mic_test(device, seconds.unwrap_or(10), &stop, move |rms| {
            let _ = app.emit(MIC_TEST_EVENT, rms);
        })
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn audio_mic_test_stop() {
    if let Some(stop) = MIC_TEST.lock().unwrap().take() {
        stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// «Проверить звук»: тестовая фраза голосом Джарвиса через выбранное устройство и громкость (без сохранения).
#[tauri::command]
pub async fn audio_sound_test(core: State<'_, CoreState>, device: Option<String>, volume: Option<f32>) -> CmdResult<api::ServiceCheck> {
    let c = core.0.clone();
    let cfg = c.config();
    Ok(services::sound_test(&cfg, &c.root, device, volume.unwrap_or_else(|| cfg.audio.gain())).await)
}

// ───────────────────────────── память, KB, история ─────────────────────────────

fn memory_of(core: &Core) -> CmdResult<jarvis_memory::Memory> {
    core.memory.clone().ok_or_else(|| {
        core.issues
            .iter()
            .find(|i| i.module == "memory")
            .map(|i| i.message.clone())
            .unwrap_or_else(|| "Память не открыта".into())
    })
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

#[tauri::command]
pub async fn memory_list(core: State<'_, CoreState>) -> CmdResult<api::MemoryData> {
    let mem = memory_of(&core.0)?;
    blocking(move || services::memory_list(&mem)).await
}

/// `kind`: `facts` | `lessons` | `playbooks`; `item` с пустым `id` — новая запись.
#[tauri::command]
pub async fn memory_upsert(core: State<'_, CoreState>, kind: String, item: serde_json::Value) -> CmdResult<api::MemoryData> {
    let mem = memory_of(&core.0)?;
    blocking(move || services::memory_upsert(&mem, &kind, item)).await
}

#[tauri::command]
pub async fn memory_delete(core: State<'_, CoreState>, kind: String, id: String) -> CmdResult<api::MemoryData> {
    let mem = memory_of(&core.0)?;
    blocking(move || services::memory_delete(&mem, &kind, &id)).await
}

/// Поиск по базе знаний Windows (`data/kb.sqlite`).
#[tauri::command]
pub async fn kb_search(core: State<'_, CoreState>, query: String) -> CmdResult<api::KbResult> {
    let kb = core.0.kb.clone().ok_or_else(|| {
        core.0
            .issues
            .iter()
            .find(|i| i.module == "kb")
            .map(|i| i.message.clone())
            .unwrap_or_else(|| "База знаний не открыта".into())
    })?;
    blocking(move || services::kb_search(&kb, &query)).await
}

/// История диалогов (сеансы и реплики памяти).
#[tauri::command]
pub async fn history_list(core: State<'_, CoreState>) -> CmdResult<api::HistoryResult> {
    let mem = memory_of(&core.0)?;
    blocking(move || services::history_list(&mem)).await
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
pub fn app_info(app: AppHandle, store: State<'_, ConfigStore>) -> AppInfo {
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
