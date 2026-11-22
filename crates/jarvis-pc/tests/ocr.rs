//! OCR встроенным Windows.Media.Ocr на картинке с программно нарисованным текстом (Arial).

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::{Rgba, RgbaImage};
use jarvis_pc::screen::{available_languages, ocr_image};

const ARIAL: &str = "C:\\Windows\\Fonts\\arial.ttf";

/// Нарисовать строки текста чёрным по белому. Возвращает картинку и левые верхние углы строк.
fn render(lines: &[&str], size: f32) -> (RgbaImage, Vec<(f32, f32)>) {
    let data = std::fs::read(ARIAL).expect("arial.ttf");
    let font = FontRef::try_from_slice(&data).expect("шрифт");
    let scale = PxScale::from(size);
    let sf = font.as_scaled(scale);
    let margin = 30.0;
    let line_h = sf.height() + sf.line_gap() + size * 0.6;
    let mut glyphs = Vec::new();
    let mut origins = Vec::new();
    let mut max_x: f32 = 0.0;
    for (i, text) in lines.iter().enumerate() {
        let top = margin + i as f32 * line_h;
        let baseline = top + sf.ascent();
        origins.push((margin, top));
        let mut x = margin;
        let mut prev = None;
        for c in text.chars() {
            let id = sf.glyph_id(c);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            glyphs.push(id.with_scale_and_position(scale, point(x, baseline)));
            x += sf.h_advance(id);
            prev = Some(id);
        }
        max_x = max_x.max(x);
    }
    let w = (max_x + margin) as u32;
    let h = (margin * 2.0 + lines.len() as f32 * line_h) as u32;
    let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    for g in glyphs {
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    let v = (255.0 * (1.0 - cov.clamp(0.0, 1.0))) as u8;
                    let p = img.get_pixel_mut(px as u32, py as u32);
                    let m = p[0].min(v);
                    *p = Rgba([m, m, m, 255]);
                }
            });
        }
    }
    (img, origins)
}

#[test]
fn ocr_languages_installed() {
    let langs = available_languages();
    println!("языки OCR: {langs:?}");
    assert!(!langs.is_empty(), "нет ни одного языка Windows OCR");
}

#[test]
fn ocr_english_text_with_line_rects() {
    let (img, origins) = render(&["Hello World 2026", "Save changes"], 40.0);
    let res = ocr_image(&img, Some("en")).expect("OCR");
    println!("{:?}", res);
    let t = res.text.to_lowercase();
    assert!(t.contains("hello"), "{t}");
    assert!(t.contains("world"), "{t}");
    assert!(t.contains("2026"), "{t}");
    assert!(t.contains("save"), "{t}");
    assert_eq!(res.lines.len(), 2, "две строки: {:?}", res.lines);
    // рамка первой строки — около места, где рисовали (координаты исходного изображения)
    let r = res.lines[0].rect;
    assert!((r.x - origins[0].0 as i32).abs() <= 12, "x={} ожидали ≈{}", r.x, origins[0].0);
    assert!(r.h >= 15 && r.h <= 60, "высота строки {}", r.h);
    assert!(res.lines[1].rect.y > r.y + r.h / 2, "вторая строка ниже первой");
}

#[test]
fn ocr_russian_text() {
    if !available_languages().iter().any(|l| l.starts_with("ru")) {
        eprintln!("русский OCR не установлен — пропуск");
        return;
    }
    let (img, _) = render(&["Привет, мир! Сохранить файл"], 42.0);
    let res = ocr_image(&img, Some("ru")).expect("OCR");
    println!("{:?}", res);
    assert!(res.language.starts_with("ru"));
    let t = res.text.to_lowercase();
    assert!(t.contains("привет"), "{t}");
    assert!(t.contains("сохранить"), "{t}");
}

#[test]
fn ocr_unknown_language_is_clear_error() {
    let (img, _) = render(&["x"], 30.0);
    let e = ocr_image(&img, Some("xx-YY")).unwrap_err();
    assert!(e.to_string().contains("не установлен"), "{e}");
}
