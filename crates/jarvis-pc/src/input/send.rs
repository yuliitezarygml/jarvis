//! Отправка ввода через `SendInput` с проверкой отмены между шагами и гарантированным отпусканием клавиш/кнопок.

use jarvis_core::{CancellationToken, Rect, ToolError};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, INPUT, SendInput};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use super::events::{HeldTracker, InputEvent, MouseButton, to_input};
use super::hotkey::vk;
use super::trajectory::{CURVE, STEP_MS, distance, duration_for_distance, point_at};
use crate::dpi::DpiScope;
use crate::win::virtual_screen;

/// Всё, что зажато нашими событиями в процессе (для `release_all_held` из обработчика Ctrl+Alt+K).
static GLOBAL_HELD: Mutex<HeldTracker> = Mutex::new(HeldTracker::new());

fn send_raw(events: &[InputEvent], vs: Rect) -> Result<(), ToolError> {
    if events.is_empty() {
        return Ok(());
    }
    let inputs: Vec<INPUT> = events.iter().map(|e| to_input(e, vs)).collect();
    // SAFETY: массив корректных INPUT, размер структуры передаём явно.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(ToolError::Failed(format!(
            "SendInput отправил {sent} из {} событий: ввод заблокирован (UIPI, экран блокировки или UAC)",
            inputs.len()
        )));
    }
    Ok(())
}

/// Отпустить всё, что Джарвис сейчас держит нажатым (клавиши и кнопки мыши). Возвращает число отпущенных.
/// Безопасно вызывать из любого потока (например, из обработчика Ctrl+Alt+K).
pub fn release_all_held() -> usize {
    let events = {
        let mut g = GLOBAL_HELD.lock().unwrap_or_else(|e| e.into_inner());
        let ev = g.release_events();
        g.clear();
        ev
    };
    if events.is_empty() {
        return 0;
    }
    let _dpi = DpiScope::per_monitor();
    let _ = send_raw(&events, virtual_screen());
    events.len()
}

/// Исполнитель одной операции ввода. Живёт в одном потоке (spawn_blocking); при `Drop` отпускает своё.
pub(crate) struct InputRunner {
    cancel: CancellationToken,
    local: HeldTracker,
    vs: Rect,
    _dpi: DpiScope,
}

impl InputRunner {
    pub(crate) fn new(cancel: CancellationToken) -> Self {
        let dpi = DpiScope::per_monitor();
        Self { cancel, local: HeldTracker::new(), vs: virtual_screen(), _dpi: dpi }
    }

    pub(crate) fn virtual_screen(&self) -> Rect {
        self.vs
    }

    pub(crate) fn check(&self) -> Result<(), ToolError> {
        if self.cancel.is_cancelled() { Err(ToolError::Cancelled) } else { Ok(()) }
    }

    pub(crate) fn send(&mut self, events: &[InputEvent]) -> Result<(), ToolError> {
        self.check()?;
        send_raw(events, self.vs)?;
        let mut g = GLOBAL_HELD.lock().unwrap_or_else(|e| e.into_inner());
        for e in events {
            self.local.apply(e);
            g.apply(e);
        }
        Ok(())
    }

    /// Пауза с проверкой отмены каждые 10 мс.
    pub(crate) fn pause(&self, ms: u64) -> Result<(), ToolError> {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            self.check()?;
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            std::thread::sleep((end - now).min(Duration::from_millis(10)));
        }
    }

    pub(crate) fn cursor(&self) -> (i32, i32) {
        let mut p = POINT::default();
        // SAFETY: выходной параметр.
        let _ = unsafe { GetCursorPos(&mut p) };
        (p.x, p.y)
    }

    /// Плавно довести курсор до точки. Позиция считается по часам, поэтому длительность точная при любом шаге сна.
    pub(crate) fn smooth_move(&mut self, to: (i32, i32), duration_ms: Option<u32>) -> Result<(), ToolError> {
        let from = self.cursor();
        let dist = distance(from, to);
        let duration = duration_ms.unwrap_or_else(|| duration_for_distance(dist)).min(3000);
        if dist < 1.0 || duration == 0 {
            return self.send(&[InputEvent::MoveAbs { x: to.0, y: to.1 }]);
        }
        let start = Instant::now();
        let total = Duration::from_millis(duration as u64);
        let mut last = from;
        loop {
            let t = start.elapsed().as_secs_f64() / total.as_secs_f64();
            let p = point_at(from, to, t, CURVE);
            if p != last {
                self.send(&[InputEvent::MoveAbs { x: p.0, y: p.1 }])?;
                last = p;
            }
            if t >= 1.0 {
                break;
            }
            self.pause(STEP_MS as u64)?;
        }
        if last != to {
            self.send(&[InputEvent::MoveAbs { x: to.0, y: to.1 }])?;
        }
        Ok(())
    }

    pub(crate) fn click(&mut self, button: MouseButton, double: bool) -> Result<(), ToolError> {
        let clicks = if double { 2 } else { 1 };
        for i in 0..clicks {
            if i > 0 {
                self.pause(70)?;
            }
            self.send(&[InputEvent::Button { button, up: false }])?;
            self.pause(45)?;
            self.send(&[InputEvent::Button { button, up: true }])?;
        }
        Ok(())
    }

    /// Пользователь держит Ctrl/Alt/Shift/Win — наш ввод превратится в сочетания. Ждём до 2 с.
    pub(crate) fn wait_user_modifiers_released(&self) -> Result<(), ToolError> {
        let stale = !GLOBAL_HELD.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        if stale {
            release_all_held();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let held: Vec<&str> = [(vk::CONTROL, "Ctrl"), (vk::MENU, "Alt"), (vk::SHIFT, "Shift"), (vk::LWIN, "Win"), (vk::RWIN, "Win")]
                .into_iter()
                // SAFETY: чтение состояния клавиши.
                .filter(|(k, _)| unsafe { GetAsyncKeyState(*k as i32) } as u16 & 0x8000 != 0)
                .map(|(_, n)| n)
                .collect();
            if held.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ToolError::Failed(format!("зажаты клавиши {} — отпустите их и повторите", held.join("+"))));
            }
            self.pause(50)?;
        }
    }
}

impl Drop for InputRunner {
    fn drop(&mut self) {
        if self.local.is_empty() {
            return;
        }
        let release = self.local.release_events();
        let _ = send_raw(&release, self.vs);
        let mut g = GLOBAL_HELD.lock().unwrap_or_else(|e| e.into_inner());
        for e in &release {
            g.apply(e);
        }
        self.local.clear();
    }
}

/// Выполнить операцию ввода в блокирующем потоке.
pub(crate) async fn run_input<R, F>(cancel: CancellationToken, f: F) -> Result<R, ToolError>
where
    R: Send + 'static,
    F: FnOnce(&mut InputRunner) -> Result<R, ToolError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut runner = InputRunner::new(cancel);
        f(&mut runner)
    })
    .await
    .map_err(|e| ToolError::Failed(format!("поток ввода упал: {e}")))?
}
