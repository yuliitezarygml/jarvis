//! Текст в буфере обмена Windows (CF_UNICODETEXT): чтобы вставить длинный текст через Ctrl+V.
//! Посимвольный ввод `SendInput` длинного текста в некоторых программах (новый Блокнот Windows 11 и др.)
//! «залипает»: символы повторяются и теряются.

use jarvis_core::ToolError;
use std::time::Duration;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

const CF_UNICODETEXT: u32 = 13;

/// Открытый буфер обмена; закрывается при `Drop`.
struct Opened;

impl Opened {
    /// Другая программа может держать буфер — несколько попыток.
    fn open() -> Result<Self, ToolError> {
        for _ in 0..20 {
            // SAFETY: открытие буфера без окна-владельца.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(Opened);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Err(ToolError::Failed("буфер обмена занят другой программой".into()))
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: буфер открыт этим потоком.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Текст из буфера (`None` — в буфере не текст или пусто).
pub fn get_text() -> Result<Option<String>, ToolError> {
    let _g = Opened::open()?;
    // SAFETY: чтение данных буфера, пока он открыт; память только читается и разблокируется.
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return Ok(None);
        }
        let Ok(h) = GetClipboardData(CF_UNICODETEXT) else { return Ok(None) };
        let mem = HGLOBAL(h.0);
        let p = GlobalLock(mem) as *const u16;
        if p.is_null() {
            return Ok(None);
        }
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        let _ = GlobalUnlock(mem);
        Ok(Some(s))
    }
}

/// Положить текст в буфер.
pub fn set_text(text: &str) -> Result<(), ToolError> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let _g = Opened::open()?;
    // SAFETY: память выделяется GlobalAlloc и после SetClipboardData принадлежит системе.
    unsafe {
        EmptyClipboard().map_err(|e| ToolError::Failed(format!("буфер обмена: {e}")))?;
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).map_err(|e| ToolError::Failed(format!("память: {e}")))?;
        let p = GlobalLock(mem) as *mut u16;
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err(ToolError::Failed("память буфера обмена недоступна".into()));
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        let _ = GlobalUnlock(mem);
        if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))) {
            let _ = GlobalFree(Some(mem));
            return Err(ToolError::Failed(format!("буфер обмена: {e}")));
        }
    }
    Ok(())
}

/// Очистить буфер (если до вставки там был не текст — его не восстановить, но и чужой текст не останется).
pub fn clear() {
    if let Ok(_g) = Opened::open() {
        // SAFETY: буфер открыт.
        let _ = unsafe { EmptyClipboard() };
    }
}
