//! Окна J.A.R.V.I.S.:
//! - `hud` — большое окно HUD (режим `fullscreen`): обычная рамка Windows, можно свернуть, уменьшить,
//!   развернуть; на весь экран без рамки — кнопкой в HUD или F11 (`toggle_hud_fullscreen`);
//! - `corner` — виджет ~220×220 в углу: прозрачный, всегда сверху, без панели задач, перетаскивается,
//!   позиция сохраняется в `config.json` → `hud.corner_x/corner_y` (физические пиксели);
//! - `overlay` — прозрачное свечение на весь основной монитор: всегда сверху, без фокуса, без панели задач,
//!   клики насквозь (`set_ignore_cursor_events(true)`);
//! - `settings` — обычное окно (создаётся по требованию).
//!
//! На `hud`, `corner` и `overlay` ставится `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`, чтобы
//! интерфейс не попадал в скриншоты, которые агент отправляет vision-модели.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use jarvis_core::config::HudMode;
use jarvis_core::{Config, Event};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, Window, WindowEvent,
};

use crate::config_store::ConfigStore;
use crate::sink::{CURSOR_EVENT, HUD_MODE_EVENT, SETTINGS_TAB_EVENT, SinkEffects};

pub const HUD: &str = "hud";
pub const CORNER: &str = "corner";
pub const OVERLAY: &str = "overlay";
pub const SETTINGS: &str = "settings";

/// Логический размер виджета в углу.
pub const CORNER_SIZE: f64 = 220.0;
/// Отступ виджета от края рабочей области (логические пиксели).
pub const CORNER_MARGIN: f64 = 24.0;

/// Фон HUD до загрузки страницы (без белой вспышки): #02080D.
const HUD_BG: tauri::window::Color = tauri::window::Color(2, 8, 13, 255);

/// Прямоугольник монитора/рабочей области в физических пикселях.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Area {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w as i32 && y < self.y + self.h as i32
    }
}

/// Позиция виджета по умолчанию: правый нижний угол рабочей области (над панелью задач).
pub fn default_corner_position(work: Area, scale: f64) -> (i32, i32) {
    let size = (CORNER_SIZE * scale).round() as i32;
    let margin = (CORNER_MARGIN * scale).round() as i32;
    (work.x + work.w as i32 - size - margin, work.y + work.h as i32 - size - margin)
}

/// Сохранённая позиция, если виджет на ней виден (центр попадает в какой-то монитор), иначе — по умолчанию.
pub fn corner_position(saved: (Option<i32>, Option<i32>), monitors: &[Area], work: Area, scale: f64) -> (i32, i32) {
    if let (Some(x), Some(y)) = saved {
        let half = (CORNER_SIZE * scale / 2.0) as i32;
        if monitors.iter().any(|m| m.contains(x + half, y + half)) {
            return (x, y);
        }
    }
    default_corner_position(work, scale)
}

/// Геометрия оверлея для фронтенда: перевод координат `ControlTarget` (виртуальный рабочий стол) в CSS-пиксели.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct OverlayGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct CursorPos {
    x: i32,
    y: i32,
}

/// Состояние оконного слоя.
#[derive(Default)]
pub struct WindowsState {
    mode: Mutex<HudMode>,
    corner_move_gen: AtomicU64,
    cursor_running: Arc<AtomicBool>,
}

impl WindowsState {
    pub fn mode(&self) -> HudMode {
        *self.mode.lock().unwrap()
    }
}

// ───────────────────────────── защита от захвата экрана ─────────────────────────────

/// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` — окно не попадает в скриншоты/запись экрана.
#[cfg(windows)]
pub fn exclude_from_capture<R: Runtime>(w: &WebviewWindow<R>) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};
    let hwnd = w.hwnd().map_err(|e| e.to_string())?;
    // SAFETY: HWND принадлежит нашему процессу и живёт, пока живо окно Tauri.
    unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn exclude_from_capture<R: Runtime>(_w: &WebviewWindow<R>) -> Result<(), String> {
    Ok(())
}

fn protect<R: Runtime>(w: &WebviewWindow<R>) {
    if let Err(e) = exclude_from_capture(w) {
        eprintln!("[jarvis] WDA_EXCLUDEFROMCAPTURE для «{}» не установлен: {e}", w.label());
    }
}

// ───────────────────────────── мониторы ─────────────────────────────

fn primary_monitor<R: Runtime>(app: &AppHandle<R>) -> Option<tauri::Monitor> {
    app.primary_monitor().ok().flatten().or_else(|| app.available_monitors().ok()?.into_iter().next())
}

fn area_of(m: &tauri::Monitor) -> Area {
    Area { x: m.position().x, y: m.position().y, w: m.size().width, h: m.size().height }
}

fn work_area_of(m: &tauri::Monitor) -> Area {
    let wa = m.work_area();
    Area { x: wa.position.x, y: wa.position.y, w: wa.size.width, h: wa.size.height }
}

// ───────────────────────────── создание окон ─────────────────────────────

/// Создать hud, corner и overlay при старте. Видим только окно текущего режима (+ overlay).
pub fn create_windows<R: Runtime>(app: &AppHandle<R>, cfg: &Config) -> tauri::Result<()> {
    let mode = cfg.hud.mode;
    *app.state::<WindowsState>().mode.lock().unwrap() = mode;

    let hud = WebviewWindowBuilder::new(app, HUD, WebviewUrl::App("hud/".into()))
        .title("J.A.R.V.I.S.")
        .decorations(true)
        .resizable(true)
        .maximizable(true)
        .minimizable(true)
        .inner_size(1440.0, 900.0)
        .min_inner_size(960.0, 620.0)
        .center()
        .maximized(true)
        .theme(Some(tauri::Theme::Dark))
        .skip_taskbar(false)
        .background_color(HUD_BG)
        .visible(mode == HudMode::Fullscreen)
        .build()?;
    protect(&hud);

    let corner = WebviewWindowBuilder::new(app, CORNER, WebviewUrl::App("corner/".into()))
        .title("J.A.R.V.I.S.")
        .inner_size(CORNER_SIZE, CORNER_SIZE)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;
    protect(&corner);
    if let Some(m) = primary_monitor(app) {
        let monitors: Vec<Area> = app.available_monitors().unwrap_or_default().iter().map(area_of).collect();
        let (x, y) = corner_position((cfg.hud.corner_x, cfg.hud.corner_y), &monitors, work_area_of(&m), m.scale_factor());
        corner.set_position(PhysicalPosition::new(x, y))?;
    }
    if mode == HudMode::Corner {
        corner.show()?;
    }

    create_overlay(app)?;
    Ok(())
}

/// Оверлей создаётся сразу видимым и без фокуса: так Windows показывает его через SW_SHOWNOACTIVATE
/// и он никогда не забирает фокус. Пока Джарвис не управляет ПК, страница пустая и не рисует кадры.
fn create_overlay<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let (pos, size, scale) = match primary_monitor(app) {
        Some(m) => (*m.position(), *m.size(), m.scale_factor()),
        None => (PhysicalPosition::new(0, 0), PhysicalSize::new(1920, 1080), 1.0),
    };
    let overlay = WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("overlay/".into()))
        .title("J.A.R.V.I.S. overlay")
        .position(pos.x as f64 / scale, pos.y as f64 / scale)
        .inner_size(size.width as f64 / scale, size.height as f64 / scale)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(true)
        .build()?;
    overlay.set_position(pos)?;
    overlay.set_size(size)?;
    overlay.set_ignore_cursor_events(true)?;
    protect(&overlay);
    Ok(overlay)
}

/// Открыть (или показать) окно настроек, опционально на вкладке `tab`.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>, tab: Option<&str>) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(SETTINGS) {
        w.show()?;
        w.unminimize()?;
        w.set_focus()?;
        if let Some(tab) = tab {
            w.emit_to(SETTINGS, SETTINGS_TAB_EVENT, tab)?;
        }
        return Ok(());
    }
    let url = match tab {
        Some(t) if t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => format!("settings/#{t}"),
        _ => "settings/".to_string(),
    };
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App(url.into()))
        .title("J.A.R.V.I.S. — Настройки")
        .inner_size(1180.0, 800.0)
        .min_inner_size(900.0, 600.0)
        .background_color(HUD_BG)
        .center()
        .build()?;
    Ok(())
}

// ───────────────────────────── режимы ─────────────────────────────

/// Переключить HUD: `fullscreen` ↔ `corner`. Режим сохраняется в `config.json`.
pub fn set_mode<R: Runtime>(app: &AppHandle<R>, mode: HudMode) -> tauri::Result<()> {
    *app.state::<WindowsState>().mode.lock().unwrap() = mode;
    let hud = app.get_webview_window(HUD);
    let corner = app.get_webview_window(CORNER);
    match mode {
        HudMode::Fullscreen => {
            if let Some(h) = &hud {
                // Окно сохраняет размер, который выбрал пользователь (обычное, развёрнутое или полный экран).
                h.show()?;
                h.unminimize()?;
                h.set_focus()?;
            }
            if let Some(c) = &corner {
                c.hide()?;
            }
        }
        HudMode::Corner => {
            if let Some(c) = &corner {
                c.show()?;
            }
            if let Some(h) = &hud {
                h.hide()?;
            }
        }
    }
    let store = app.state::<ConfigStore>();
    if store.get().hud.mode != mode
        && let Err(e) = store.update(|c| c.hud.mode = mode)
    {
        eprintln!("[jarvis] не удалось сохранить режим HUD: {e}");
    }
    app.emit(HUD_MODE_EVENT, mode)?;
    Ok(())
}

/// Окно HUD: полный экран без рамки ↔ обычное окно. Возвращает новое состояние (`true` = полный экран).
pub fn toggle_hud_fullscreen<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<bool> {
    let Some(h) = app.get_webview_window(HUD) else { return Ok(false) };
    let next = !h.is_fullscreen()?;
    h.set_fullscreen(next)?;
    h.set_focus()?;
    Ok(next)
}

/// Выйти из полного экрана HUD. `true`, если HUD был на весь экран.
pub fn exit_hud_fullscreen<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<bool> {
    let Some(h) = app.get_webview_window(HUD) else { return Ok(false) };
    if h.is_fullscreen()? {
        h.set_fullscreen(false)?;
        return Ok(true);
    }
    Ok(false)
}

pub fn toggle_mode<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<HudMode> {
    let next = match app.state::<WindowsState>().mode() {
        HudMode::Fullscreen => HudMode::Corner,
        HudMode::Corner => HudMode::Fullscreen,
    };
    set_mode(app, next)?;
    Ok(next)
}

pub fn overlay_geometry<R: Runtime>(app: &AppHandle<R>) -> Option<OverlayGeometry> {
    let w = app.get_webview_window(OVERLAY)?;
    let pos = w.outer_position().ok()?;
    let size = w.inner_size().ok()?;
    let scale = w.scale_factor().ok()?;
    Some(OverlayGeometry { x: pos.x, y: pos.y, width: size.width, height: size.height, scale })
}

// ───────────────────────────── события окон ─────────────────────────────

/// Обработчик `on_window_event`: служебные окна не закрываются, позиция виджета сохраняется.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let app = window.app_handle();
    match (window.label(), event) {
        (HUD, WindowEvent::CloseRequested { api, .. }) => {
            api.prevent_close();
            let _ = set_mode(app, HudMode::Corner);
        }
        (CORNER | OVERLAY, WindowEvent::CloseRequested { api, .. }) => api.prevent_close(),
        (CORNER, WindowEvent::Moved(pos)) => remember_corner(app, *pos, window.is_visible().unwrap_or(false)),
        _ => {}
    }
}

/// Сохранить позицию виджета с задержкой 600 мс (во время перетаскивания событий много).
fn remember_corner<R: Runtime>(app: &AppHandle<R>, pos: PhysicalPosition<i32>, visible: bool) {
    if !visible {
        return;
    }
    let state = app.state::<WindowsState>();
    let generation = state.corner_move_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        if app.state::<WindowsState>().corner_move_gen.load(Ordering::SeqCst) != generation {
            return;
        }
        let store = app.state::<ConfigStore>();
        let hud = store.get().hud;
        if hud.corner_x == Some(pos.x) && hud.corner_y == Some(pos.y) {
            return;
        }
        if let Err(e) = store.update(|c| {
            c.hud.corner_x = Some(pos.x);
            c.hud.corner_y = Some(pos.y);
        }) {
            eprintln!("[jarvis] не удалось сохранить позицию виджета: {e}");
        }
    });
}

// ───────────────────────────── след курсора ─────────────────────────────

#[cfg(windows)]
fn cursor_pos() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    // SAFETY: указатель на локальную переменную.
    unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
}

#[cfg(not(windows))]
fn cursor_pos() -> Option<(i32, i32)> {
    None
}

/// Пока идёт управление ПК — 60 раз в секунду шлём позицию курсора окну `overlay` (только при движении).
fn start_cursor_tracker<R: Runtime>(app: &AppHandle<R>) {
    let running = app.state::<WindowsState>().cursor_running.clone();
    if running.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::Builder::new()
        .name("jarvis-cursor".into())
        .spawn(move || {
            let mut last = None;
            while running.load(Ordering::SeqCst) {
                if let Some(p) = cursor_pos()
                    && last != Some(p)
                {
                    last = Some(p);
                    let _ = app.emit_to(OVERLAY, CURSOR_EVENT, CursorPos { x: p.0, y: p.1 });
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        })
        .ok();
}

fn stop_cursor_tracker<R: Runtime>(app: &AppHandle<R>) {
    app.state::<WindowsState>().cursor_running.store(false, Ordering::SeqCst);
}

/// Побочные эффекты событий в окнах: след курсора во время управления.
pub struct WindowEffects;

impl<R: Runtime> SinkEffects<R> for WindowEffects {
    fn on_event(&self, app: &AppHandle<R>, event: &Event) {
        match event {
            Event::ControlStart { .. } => start_cursor_tracker(app),
            Event::ControlStop => stop_cursor_tracker(app),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Area = Area { x: 0, y: 0, w: 1920, h: 1032 };
    const MON: Area = Area { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn default_corner_is_bottom_right_above_taskbar() {
        assert_eq!(default_corner_position(WORK, 1.0), (1920 - 220 - 24, 1032 - 220 - 24));
        assert_eq!(default_corner_position(WORK, 1.5), (1920 - 330 - 36, 1032 - 330 - 36));
    }

    #[test]
    fn saved_corner_kept_only_if_visible() {
        let mons = [MON, Area { x: 1920, y: 0, w: 2560, h: 1440 }];
        assert_eq!(corner_position((Some(100), Some(200)), &mons, WORK, 1.0), (100, 200));
        assert_eq!(corner_position((Some(3000), Some(50)), &mons, WORK, 1.0), (3000, 50));
        // монитор отключили — возвращаемся в угол
        assert_eq!(corner_position((Some(5000), Some(50)), &mons, WORK, 1.0), default_corner_position(WORK, 1.0));
        assert_eq!(corner_position((None, Some(50)), &mons, WORK, 1.0), default_corner_position(WORK, 1.0));
    }
}
