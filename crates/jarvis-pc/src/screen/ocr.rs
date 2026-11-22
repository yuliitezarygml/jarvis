//! Встроенный офлайн-OCR Windows (`Windows.Media.Ocr`, WinRT): текст и строки с рамками.

use image::RgbaImage;
use image::imageops::FilterType;
use jarvis_core::{Rect, ToolError};
use serde::{Deserialize, Serialize};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::core::HSTRING;

use crate::com::ComInit;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrLine {
    pub text: String,
    /// В координатах исходного изображения (или экрана — после [`OcrResult::offset`]).
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrResult {
    pub text: String,
    pub lines: Vec<OcrLine>,
    /// Язык распознавания (`ru`, `en-US`…).
    pub language: String,
}

impl OcrResult {
    /// Перевести рамки из координат снимка в экранные: `screen = origin + img / scale`.
    pub fn to_screen(mut self, origin: Rect, scale: f64) -> Self {
        let s = if scale > 0.0 { scale } else { 1.0 };
        for l in &mut self.lines {
            l.rect = Rect {
                x: origin.x + (l.rect.x as f64 / s).round() as i32,
                y: origin.y + (l.rect.y as f64 / s).round() as i32,
                w: (l.rect.w as f64 / s).round() as i32,
                h: (l.rect.h as f64 / s).round() as i32,
            };
        }
        self
    }
}

fn err(ctx: &str) -> impl Fn(windows::core::Error) -> ToolError + '_ {
    move |e| ToolError::Failed(format!("OCR ({ctx}): {}", e.message()))
}

/// Установленные языки распознавания.
pub fn available_languages() -> Vec<String> {
    let _com = ComInit::mta();
    OcrEngine::AvailableRecognizerLanguages()
        .ok()
        .map(|v| v.into_iter().filter_map(|l| l.LanguageTag().ok().map(|t| t.to_string())).collect())
        .unwrap_or_default()
}

fn engine_for(lang: Option<&str>) -> Result<OcrEngine, ToolError> {
    let want = lang.map(str::trim).filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("auto"));
    if let Some(tag) = want {
        let candidates: Vec<String> = match tag.to_lowercase().as_str() {
            "ru" | "rus" | "russian" | "русский" => vec!["ru".into(), "ru-RU".into()],
            "en" | "eng" | "english" | "английский" => vec!["en-US".into(), "en-GB".into(), "en".into()],
            _ => vec![tag.to_string()],
        };
        for t in &candidates {
            if let Ok(l) = Language::CreateLanguage(&HSTRING::from(t.as_str()))
                && OcrEngine::IsLanguageSupported(&l).unwrap_or(false) {
                    return OcrEngine::TryCreateFromLanguage(&l).map_err(err("язык"));
                }
        }
        return Err(ToolError::Unsupported(format!(
            "язык OCR «{tag}» не установлен (доступны: {}). Параметры → Время и язык → Язык → добавить пакет.",
            available_languages().join(", ")
        )));
    }
    OcrEngine::TryCreateFromUserProfileLanguages()
        .or_else(|_| {
            let l = Language::CreateLanguage(&HSTRING::from("en-US"))?;
            OcrEngine::TryCreateFromLanguage(&l)
        })
        .map_err(|_| ToolError::Unsupported("нет установленных языков OCR Windows".into()))
}

/// Распознать текст на изображении. Мелкий текст распознаётся лучше после увеличения — `upscale` делает ×2
/// для небольших снимков. Рамки строк — в координатах исходного `img`.
pub fn ocr_image(img: &RgbaImage, lang: Option<&str>) -> Result<OcrResult, ToolError> {
    let _com = ComInit::mta();
    let engine = engine_for(lang)?;
    let max = OcrEngine::MaxImageDimension().unwrap_or(2600).max(100);
    let (w, h) = img.dimensions();
    let long = w.max(h).max(1);
    // Увеличиваем мелкие снимки (терминал 100% DPI ~ 14 px на строку), уменьшаем слишком большие.
    let k = if long * 2 <= max && long <= 1600 { 2.0 } else if long > max { max as f64 / long as f64 } else { 1.0 };
    let work = if (k - 1.0f64).abs() > f64::EPSILON {
        let nw = ((w as f64 * k).round() as u32).max(1);
        let nh = ((h as f64 * k).round() as u32).max(1);
        image::imageops::resize(img, nw, nh, if k > 1.0 { FilterType::CatmullRom } else { FilterType::Triangle })
    } else {
        img.clone()
    };
    let (ww, wh) = work.dimensions();
    let bgra: Vec<u8> = work.as_raw().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();

    let writer = DataWriter::new().map_err(err("буфер"))?;
    writer.WriteBytes(&bgra).map_err(err("буфер"))?;
    let buffer = writer.DetachBuffer().map_err(err("буфер"))?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, ww as i32, wh as i32, BitmapAlphaMode::Premultiplied)
        .map_err(err("SoftwareBitmap"))?;
    let result = engine.RecognizeAsync(&bitmap).map_err(err("запуск"))?.join().map_err(err("распознавание"))?;
    let language = engine.RecognizerLanguage().and_then(|l| l.LanguageTag()).map(|t| t.to_string()).unwrap_or_default();

    let mut lines = Vec::new();
    for line in result.Lines().map_err(err("строки"))? {
        let text = line.Text().map(|t| t.to_string()).unwrap_or_default();
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        if let Ok(words) = line.Words() {
            for wd in words {
                if let Ok(r) = wd.BoundingRect() {
                    x0 = x0.min(r.X);
                    y0 = y0.min(r.Y);
                    x1 = x1.max(r.X + r.Width);
                    y1 = y1.max(r.Y + r.Height);
                }
            }
        }
        let rect = if x0 <= x1 {
            Rect {
                x: (x0 as f64 / k).round() as i32,
                y: (y0 as f64 / k).round() as i32,
                w: ((x1 - x0) as f64 / k).round() as i32,
                h: ((y1 - y0) as f64 / k).round() as i32,
            }
        } else {
            Rect::default()
        };
        lines.push(OcrLine { text, rect });
    }
    let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    Ok(OcrResult { text, lines, language })
}
