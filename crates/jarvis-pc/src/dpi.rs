//! DPI: все координаты модуля — физические пиксели виртуального рабочего стола.
//! Процесс Tauri может быть не DPI-aware, поэтому на время операции переводим поток в Per-Monitor v2.

use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};

/// RAII: поток — Per-Monitor-Aware v2, при выходе — прежний контекст. Создавать и ронять в одном потоке.
pub(crate) struct DpiScope(DPI_AWARENESS_CONTEXT);

impl DpiScope {
    pub(crate) fn per_monitor() -> Self {
        // SAFETY: функция без побочных эффектов кроме контекста текущего потока.
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiScope {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: восстанавливаем контекст, полученный в этом же потоке.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}
