//! Сжатие снимка до ≤1280 px по длинной стороне и кодирование в PNG/JPEG base64 для vision-модели.

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use jarvis_core::{ImageData, ToolError};

/// Предел длинной стороны (план, раздел 3).
pub const MAX_SIDE: u32 = 1280;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    /// Качество 1–100.
    Jpeg(u8),
}

impl ImageFormat {
    pub fn parse(s: Option<&str>, default: ImageFormat) -> ImageFormat {
        match s.map(|s| s.trim().to_lowercase()) {
            Some(s) if s == "png" => ImageFormat::Png,
            Some(s) if s == "jpeg" || s == "jpg" => ImageFormat::Jpeg(80),
            _ => default,
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg(_) => "image/jpeg",
        }
    }
}

/// Уменьшить так, чтобы длинная сторона ≤ `max_side`. Возвращает (картинка, масштаб ≤ 1).
pub fn downscale(img: &RgbaImage, max_side: u32) -> (RgbaImage, f64) {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= max_side || max_side == 0 {
        return (img.clone(), 1.0);
    }
    let scale = max_side as f64 / long as f64;
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    (image::imageops::resize(img, nw, nh, FilterType::Triangle), scale)
}

pub fn encode(img: &RgbaImage, format: ImageFormat) -> Result<Vec<u8>, ToolError> {
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    let err = |e: image::ImageError| ToolError::Failed(format!("кодирование изображения: {e}"));
    match format {
        ImageFormat::Png => PngEncoder::new(&mut out).write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8).map_err(err)?,
        ImageFormat::Jpeg(q) => {
            let rgb: Vec<u8> = img.as_raw().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            JpegEncoder::new_with_quality(&mut out, q.clamp(1, 100))
                .write_image(&rgb, w, h, ExtendedColorType::Rgb8)
                .map_err(err)?
        }
    }
    Ok(out)
}

pub fn to_image_data(img: &RgbaImage, format: ImageFormat) -> Result<ImageData, ToolError> {
    let bytes = encode(img, format)?;
    Ok(ImageData { mime: format.mime().into(), base64: base64::engine::general_purpose::STANDARD.encode(bytes) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn downscale_keeps_aspect() {
        let img = RgbaImage::from_pixel(2560, 1440, Rgba([10, 20, 30, 255]));
        let (small, scale) = downscale(&img, MAX_SIDE);
        assert_eq!(small.dimensions(), (1280, 720));
        assert!((scale - 0.5).abs() < 1e-9);
        let tall = RgbaImage::new(500, 3000);
        let (s2, _) = downscale(&tall, MAX_SIDE);
        assert_eq!(s2.height(), 1280);
        let tiny = RgbaImage::new(100, 50);
        assert_eq!(downscale(&tiny, MAX_SIDE).1, 1.0);
    }

    #[test]
    fn encodes_png_and_jpeg() {
        let img = RgbaImage::from_pixel(64, 32, Rgba([200, 0, 0, 255]));
        let png = encode(&img, ImageFormat::Png).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let jpg = encode(&img, ImageFormat::Jpeg(80)).unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
        let d = to_image_data(&img, ImageFormat::Jpeg(70)).unwrap();
        assert_eq!(d.mime, "image/jpeg");
        let back = base64::engine::general_purpose::STANDARD.decode(&d.base64).unwrap();
        let decoded = image::load_from_memory(&back).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 32));
        assert_eq!(ImageFormat::parse(Some("PNG"), ImageFormat::Jpeg(80)), ImageFormat::Png);
        assert_eq!(ImageFormat::parse(None, ImageFormat::Jpeg(80)), ImageFormat::Jpeg(80));
    }
}
