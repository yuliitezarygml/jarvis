//! Экран: снимок (монитор/окно/область) в память, сжатие ≤1280 px, set-of-marks поверх снимка, встроенный OCR Windows.

pub mod capture;
pub mod encode;
pub mod marks;
pub mod ocr;
pub(crate) mod tools;

pub use capture::{Capture, CaptureTarget, capture, capture_rect};
pub use encode::{ImageFormat, MAX_SIDE, downscale, to_image_data};
pub use ocr::{OcrLine, OcrResult, available_languages, ocr_image};
