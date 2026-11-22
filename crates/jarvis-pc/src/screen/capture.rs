//! Захват экрана в память через GDI (BitBlt + CAPTUREBLT). Окна с WDA_EXCLUDEFROMCAPTURE (HUD/overlay) не попадают.

use image::RgbaImage;
use jarvis_core::{Rect, ToolError};
use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
};

use crate::dpi::DpiScope;
use crate::win::{self, intersect, virtual_screen};

/// Что снимать.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureTarget {
    /// Весь виртуальный рабочий стол (все мониторы).
    Screen,
    /// Монитор по индексу (0 — основной); `None` — монитор активного окна.
    Monitor(Option<usize>),
    /// Окно по запросу ([`win::resolve_window`]); `None` — активное окно.
    Window(Option<String>),
    /// Произвольная область.
    Area(Rect),
}

/// Снимок: изображение и прямоугольник экрана, которому оно соответствует.
pub struct Capture {
    pub image: RgbaImage,
    pub origin: Rect,
    /// Что снято — для текста ответа.
    pub what: String,
}

/// Прямоугольник экрана для цели (с обрезкой по виртуальному столу).
pub fn resolve_rect(target: &CaptureTarget) -> Result<(Rect, String), ToolError> {
    let vs = virtual_screen();
    let (r, what) = match target {
        CaptureTarget::Screen => (vs, "весь рабочий стол".to_string()),
        CaptureTarget::Monitor(idx) => {
            let mons = win::monitors();
            let m = match idx {
                Some(i) => mons.get(*i).cloned().ok_or_else(|| {
                    ToolError::InvalidArgs(format!("нет монитора {i}; мониторов: {}", mons.len()))
                })?,
                None => {
                    let fg = win::foreground_app_window().map(|w| w.rect);
                    fg.and_then(win::monitor_for).or_else(|| mons.first().cloned()).ok_or_else(|| ToolError::Failed("нет мониторов".into()))?
                }
            };
            (m.rect, format!("монитор {}{}", m.index, if m.primary { " (основной)" } else { "" }))
        }
        CaptureTarget::Window(q) => {
            let w = win::resolve_window(q.as_deref())?;
            if w.minimized {
                return Err(ToolError::Failed(format!("окно «{}» свёрнуто — сначала ui_focus_window", w.title)));
            }
            (w.rect, format!("окно «{}»", w.title))
        }
        CaptureTarget::Area(r) => (*r, "область".to_string()),
    };
    let clipped = intersect(r, vs);
    if clipped.w < 1 || clipped.h < 1 {
        return Err(ToolError::InvalidArgs(format!("область {r:?} вне экрана")));
    }
    Ok((clipped, what))
}

/// Снять прямоугольник экрана (физические пиксели).
pub fn capture_rect(r: Rect) -> Result<RgbaImage, ToolError> {
    let _dpi = DpiScope::per_monitor();
    if r.w <= 0 || r.h <= 0 {
        return Err(ToolError::InvalidArgs("пустая область снимка".into()));
    }
    let (w, h) = (r.w, r.h);
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    // SAFETY: стандартная последовательность GDI; все объекты освобождаются в конце.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(ToolError::Failed("GetDC(экран) не удался".into()));
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let blt = BitBlt(mem, 0, 0, w, h, Some(screen), r.x, r.y, SRCCOPY | CAPTUREBLT);
        SelectObject(mem, old);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // сверху вниз
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut c_void), &mut bmi, DIB_RGB_COLORS);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        blt.map_err(|e| ToolError::Failed(format!("BitBlt: {e}")))?;
        if lines == 0 {
            return Err(ToolError::Failed("GetDIBits не вернул данных".into()));
        }
    }
    // BGRA → RGBA, непрозрачный.
    for px in buf.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = 255;
    }
    RgbaImage::from_raw(w as u32, h as u32, buf).ok_or_else(|| ToolError::Failed("неверный размер буфера".into()))
}

pub fn capture(target: &CaptureTarget) -> Result<Capture, ToolError> {
    let (origin, what) = resolve_rect(target)?;
    Ok(Capture { image: capture_rect(origin)?, origin, what })
}
