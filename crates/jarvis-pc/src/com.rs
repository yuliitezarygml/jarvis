//! Инициализация COM (MTA) на время работы потока.

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

/// RAII: `CoInitializeEx(MTA)` → `CoUninitialize` при выходе (только если инициализация удалась).
pub(crate) struct ComInit(bool);

impl ComInit {
    pub(crate) fn mta() -> Self {
        // SAFETY: инициализация COM текущего потока; парный CoUninitialize в Drop.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self(hr.is_ok())
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: парный вызов к успешному CoInitializeEx в этом же потоке.
            unsafe { CoUninitialize() };
        }
    }
}
