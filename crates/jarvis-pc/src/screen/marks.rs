//! Set-of-marks: пронумерованные цветные рамки поверх скриншота по элементам UIA.
//! Номера рисуются встроенным растровым шрифтом 5×7 (нужны только цифры) — без файлов шрифтов.

use image::{Rgba, RgbaImage};
use jarvis_core::Rect;

use crate::win::intersect;

/// Цвета рамок: контрастные, белые цифры на них читаются.
pub const PALETTE: [[u8; 3]; 10] = [
    [230, 25, 75],
    [0, 130, 200],
    [60, 160, 60],
    [245, 110, 20],
    [145, 30, 180],
    [200, 30, 200],
    [0, 128, 128],
    [128, 0, 0],
    [0, 0, 160],
    [150, 100, 30],
];

const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;

/// Растровые цифры 5×7 (старший из 5 бит — левый пиксель).
const DIGITS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
];

/// Метка: номер и прямоугольник в координатах изображения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub id: u32,
    pub rect: Rect,
}

pub fn color_for(index: usize) -> Rgba<u8> {
    let [r, g, b] = PALETTE[index % PALETTE.len()];
    Rgba([r, g, b, 255])
}

/// Экранный прямоугольник → прямоугольник на снимке (`origin` — снятая область, `scale` — уменьшение).
/// `None`, если элемент вне снимка или слишком мал.
pub fn to_image_rect(r: Rect, origin: Rect, scale: f64) -> Option<Rect> {
    let clipped = intersect(r, origin);
    if clipped.w < 2 || clipped.h < 2 {
        return None;
    }
    let x = ((clipped.x - origin.x) as f64 * scale).round() as i32;
    let y = ((clipped.y - origin.y) as f64 * scale).round() as i32;
    let w = ((clipped.w as f64 * scale).round() as i32).max(1);
    let h = ((clipped.h as f64 * scale).round() as i32).max(1);
    Some(Rect { x, y, w, h })
}

fn fill(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: Rgba<u8>) {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(iw), (y + h).min(ih));
    for yy in y0..y1 {
        for xx in x0..x1 {
            img.put_pixel(xx as u32, yy as u32, c);
        }
    }
}

/// Контур прямоугольника толщиной `t` (внутрь).
pub fn draw_rect_outline(img: &mut RgbaImage, r: Rect, c: Rgba<u8>, t: i32) {
    let t = t.max(1).min(r.w.max(1)).min(r.h.max(1));
    fill(img, r.x, r.y, r.w, t, c);
    fill(img, r.x, r.y + r.h - t, r.w, t, c);
    fill(img, r.x, r.y, t, r.h, c);
    fill(img, r.x + r.w - t, r.y, t, r.h, c);
}

/// Размер плашки с числом при масштабе цифр `s`.
pub fn label_size(n: u32, s: u32) -> (u32, u32) {
    let digits = n.to_string().len() as u32;
    let pad = s;
    (digits * (GLYPH_W + 1) * s - s + 2 * pad, GLYPH_H * s + 2 * pad)
}

/// Плашка с числом: фон `bg`, цифры `fg`, левый верхний угол (x, y).
pub fn draw_number(img: &mut RgbaImage, n: u32, x: i32, y: i32, s: u32, fg: Rgba<u8>, bg: Rgba<u8>) {
    let (lw, lh) = label_size(n, s);
    fill(img, x, y, lw as i32, lh as i32, bg);
    let pad = s as i32;
    for (i, ch) in n.to_string().bytes().enumerate() {
        let glyph = &DIGITS[(ch - b'0') as usize];
        let gx = x + pad + (i as u32 * (GLYPH_W + 1) * s) as i32;
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..GLYPH_W {
                if bits & (1 << (GLYPH_W - 1 - col)) != 0 {
                    fill(img, gx + (col * s) as i32, y + pad + (row as u32 * s) as i32, s as i32, s as i32, fg);
                }
            }
        }
    }
}

/// Нарисовать метки. `stroke` — толщина рамки, `digit_scale` — размер пикселя цифры.
pub fn draw_marks(img: &mut RgbaImage, marks: &[Mark], stroke: i32, digit_scale: u32) {
    let white = Rgba([255, 255, 255, 255]);
    for (i, m) in marks.iter().enumerate() {
        let c = color_for(i);
        draw_rect_outline(img, m.rect, c, stroke);
        let (lw, lh) = label_size(m.id, digit_scale);
        // Плашка над рамкой, если помещается, иначе — внутри в левом верхнем углу.
        let ly = if m.rect.y >= lh as i32 { m.rect.y - lh as i32 } else { m.rect.y };
        let lx = m.rect.x.clamp(0, (img.width() as i32 - lw as i32).max(0));
        draw_number(img, m.id, lx, ly, digit_scale, white, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_mapping() {
        let origin = Rect { x: 100, y: 50, w: 1000, h: 800 };
        assert_eq!(to_image_rect(Rect { x: 200, y: 150, w: 100, h: 40 }, origin, 0.5), Some(Rect { x: 50, y: 50, w: 50, h: 20 }));
        // частично за краем — обрезается
        assert_eq!(to_image_rect(Rect { x: 50, y: 50, w: 100, h: 100 }, origin, 1.0), Some(Rect { x: 0, y: 0, w: 50, h: 100 }));
        assert_eq!(to_image_rect(Rect { x: 5000, y: 0, w: 10, h: 10 }, origin, 1.0), None);
    }

    #[test]
    fn draws_marks_on_synthetic_image() {
        let bg = Rgba([255, 255, 255, 255]);
        let mut img = RgbaImage::from_pixel(400, 300, bg);
        let marks = [Mark { id: 3, rect: Rect { x: 50, y: 60, w: 120, h: 40 } }, Mark { id: 12, rect: Rect { x: 0, y: 0, w: 80, h: 30 } }];
        draw_marks(&mut img, &marks, 2, 2);
        let c0 = color_for(0);
        // рамка первой метки по периметру
        assert_eq!(*img.get_pixel(50, 80), c0, "левая сторона");
        assert_eq!(*img.get_pixel(169, 80), c0, "правая сторона");
        assert_eq!(*img.get_pixel(100, 99), c0, "низ");
        // внутренность не тронута
        assert_eq!(*img.get_pixel(100, 80), bg);
        // плашка с «3» над рамкой: фон цвета метки, внутри есть белые пиксели цифры
        let (lw, lh) = label_size(3, 2);
        let ly = 60 - lh;
        assert_eq!(*img.get_pixel(50, ly), c0);
        let white = Rgba([255, 255, 255, 255]);
        let whites = (0..lw).flat_map(|dx| (0..lh).map(move |dy| (50 + dx, ly + dy))).filter(|&(x, y)| *img.get_pixel(x, y) == white).count();
        assert!(whites > 10, "цифра не нарисована ({whites})");
        // вторая метка у верхнего края: плашка внутри рамки, цвет второй палитры
        assert_eq!(*img.get_pixel(0, 0), color_for(1));
        let (lw12, _) = label_size(12, 2);
        assert!(lw12 > lw, "двузначная плашка шире");
    }

    #[test]
    fn digit_glyphs_differ() {
        for a in 0..10 {
            for b in (a + 1)..10 {
                assert_ne!(DIGITS[a], DIGITS[b], "{a} и {b}");
            }
        }
    }

    #[test]
    fn label_clamped_to_image() {
        let mut img = RgbaImage::new(50, 50);
        draw_marks(&mut img, &[Mark { id: 999, rect: Rect { x: 45, y: 20, w: 10, h: 10 } }], 1, 2);
        // не паникует; плашка (38x18) прижата к правому краю: x 12..50, y 2..20
        assert_eq!(label_size(999, 2), (38, 18));
        assert_eq!(*img.get_pixel(49, 2), color_for(0));
        assert_eq!(*img.get_pixel(11, 2), Rgba([0, 0, 0, 0]));
    }
}
