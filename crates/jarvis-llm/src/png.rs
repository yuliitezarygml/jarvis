//! Крошечный PNG-кодер без зависимостей: картинка с цифрой для проверки зрения модели.
//! Серое 8 бит, deflate без сжатия (stored-блоки) — этого достаточно для ~20 КБ.

use base64::Engine;
use jarvis_core::ImageData;

/// Глифы 5×7 для цифр 0–9 (строки сверху вниз, младшие 5 бит, старший бит — левый пиксель).
const GLYPHS: [[u8; 7]; 10] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], // 2
    [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E], // 3
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], // 6
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
];

/// Пиксели (0 — чёрный, 255 — белый) цифры `digit`, масштаб `scale`, поля `margin`.
pub fn digit_pixels(digit: u8, scale: u32, margin: u32) -> (u32, u32, Vec<u8>) {
    let g = &GLYPHS[(digit % 10) as usize];
    let (w, h) = (5 * scale + 2 * margin, 7 * scale + 2 * margin);
    let mut px = vec![255u8; (w * h) as usize];
    for (row, bits) in g.iter().enumerate() {
        for col in 0..5u32 {
            if bits & (0x10 >> col) == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let x = margin + col * scale + dx;
                    let y = margin + row as u32 * scale + dy;
                    px[(y * w + x) as usize] = 0;
                }
            }
        }
    }
    (w, h, px)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// zlib-поток из stored-блоков.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = if raw.is_empty() { vec![&[][..]] } else { raw.chunks(65_535).collect() };
    let n = blocks.len();
    for (i, b) in blocks.into_iter().enumerate() {
        z.push(u8::from(i + 1 == n)); // BFINAL, BTYPE=00
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(raw).to_be_bytes());
    z
}

/// Закодировать серое 8-битное изображение в PNG.
pub fn encode_gray(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
    assert_eq!(px.len(), (w * h) as usize);
    let mut raw = Vec::with_capacity(((w + 1) * h) as usize);
    for row in px.chunks(w as usize) {
        raw.push(0); // фильтр None
        raw.extend_from_slice(row);
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8 бит, grayscale, deflate, adaptive, без interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// PNG с крупной чёрной цифрой на белом фоне (128×160 px).
pub fn digit_png(digit: u8) -> Vec<u8> {
    let (w, h, px) = digit_pixels(digit, 16, 24);
    encode_gray(w, h, &px)
}

/// То же, готовое для `ChatMessage.images`.
pub fn digit_image(digit: u8) -> ImageData {
    ImageData { mime: "image/png".into(), base64: base64::engine::general_purpose::STANDARD.encode(digit_png(digit)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_checksums() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn png_decodes_back() {
        let bytes = digit_png(7);
        let dec = png::Decoder::new(std::io::Cursor::new(&bytes));
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (128, 160));
        assert_eq!(info.color_type, png::ColorType::Grayscale);
        let (_, _, px) = digit_pixels(7, 16, 24);
        assert_eq!(&buf[..info.buffer_size()], &px[..]);
        // Верхняя черта семёрки: чёрная; левый нижний угол: белый.
        assert_eq!(px[(30 * 128 + 40) as usize], 0);
        assert_eq!(px[(150 * 128 + 5) as usize], 255);
        let img = digit_image(7);
        assert_eq!(img.mime, "image/png");
        assert!(img.base64.starts_with("iVBORw0KGgo"));
    }
}
