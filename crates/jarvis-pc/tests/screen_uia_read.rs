//! Только чтение: снимок UIA панели задач/рабочего стола, список окон, скриншот в память, метки, OCR экрана.
//! Ничего не нажимает и не двигает.

mod common;

use base64::Engine;
use common::{ctx, tool, toolkit};
use jarvis_core::{Attachment, Event};
use jarvis_pc::screen::{CaptureTarget, capture};
use jarvis_pc::win;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn uia_snapshot_of_taskbar() {
    let tk = toolkit("uia");
    let snap = tk.uia().snapshot(Some("taskbar".into()), 150, true).await.expect("снимок панели задач");
    println!("{}", jarvis_pc::uia::element::format_snapshot(&snap));
    assert_eq!(snap.window.class, "Shell_TrayWnd");
    assert!(snap.elements.len() <= 150);
    if snap.elements.is_empty() {
        // Автоскрытие панели задач: элементы «вне экрана» — ищем через ui_find (без фильтра видимости).
        let (c, _) = ctx();
        let out = tool(&tk, "ui_find").call(json!({"role": "button", "window": "taskbar"}), &c).await.unwrap();
        assert!(out.content.contains("button"), "{}", out.content);
        return;
    }
    assert!(snap.elements.iter().any(|e| e.role == "button"), "на панели задач есть кнопки");
    // id последовательны с 1
    for (i, e) in snap.elements.iter().enumerate() {
        assert_eq!(e.id as usize, i + 1);
    }
    // копия снимка доступна без потока UIA
    assert_eq!(tk.uia().last_snapshot().unwrap().generation, snap.generation);
    assert_eq!(tk.uia().element(1).unwrap().id, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn ui_snapshot_tool_and_find_on_desktop() {
    let tk = toolkit("uia-tool");
    let (c, sink) = ctx();
    let out = tool(&tk, "ui_snapshot").call(json!({"window": "class:Shell_TrayWnd", "limit": 40}), &c).await.unwrap();
    assert!(out.untrusted, "имена элементов — чужой текст");
    assert!(out.content.starts_with("Окно «"), "{}", out.content);
    let data = out.data.unwrap();
    assert!(data["elements"].as_array().unwrap().len() <= 40);
    // T0 — без свечения
    assert!(sink.take().is_empty());

    // Рабочий стол (Progman) — чтение
    let desk = tool(&tk, "ui_snapshot").call(json!({"window": "class:Progman", "include_text": false}), &c).await;
    println!("рабочий стол: {:?}", desk.as_ref().map(|o| o.content.lines().next().unwrap_or("").to_string()));

    let unknown = tool(&tk, "ui_snapshot").call(json!({"window": "окно-которого-точно-нет-42"}), &c).await.unwrap_err();
    assert!(unknown.to_string().contains("не найдено"), "{unknown}");
}

#[tokio::test(flavor = "multi_thread")]
async fn window_list_tool() {
    let (c, _) = ctx();
    let out = tool(&toolkit("wl"), "ui_window_list").call(json!({}), &c).await.unwrap();
    assert!(out.content.starts_with("Окон: "));
    let windows = out.data.unwrap()["windows"].as_array().unwrap().clone();
    assert_eq!(out.content.lines().count(), windows.len() + 1, "строка на окно");
    for w in &windows {
        assert!(!w["title"].as_str().unwrap().is_empty());
        assert!(w["hwnd"].as_i64().unwrap() != 0);
    }
}

#[test]
fn screenshot_primary_monitor_into_memory() {
    let mons = win::monitors();
    let cap = capture(&CaptureTarget::Monitor(Some(0))).expect("снимок");
    assert_eq!((cap.image.width() as i32, cap.image.height() as i32), (mons[0].rect.w, mons[0].rect.h));
    assert_eq!(cap.origin, mons[0].rect);
    // пиксели непрозрачные (BGRA→RGBA, альфа 255)
    assert!(cap.image.pixels().take(1000).all(|p| p[3] == 255));
    let whole = capture(&CaptureTarget::Screen).unwrap();
    assert_eq!(whole.origin, win::virtual_screen());
    assert!(capture(&CaptureTarget::Area(jarvis_core::Rect { x: -100000, y: 0, w: 10, h: 10 })).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn screen_capture_tool_returns_small_image() {
    let (c, sink) = ctx();
    let out = tool(&toolkit("cap"), "screen_capture").call(json!({"target": "monitor", "monitor": 0, "format": "png"}), &c).await.unwrap();
    let Attachment::Image(img) = &out.attachments[0];
    assert_eq!(img.mime, "image/png");
    let bytes = base64::engine::general_purpose::STANDARD.decode(&img.base64).unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert!(decoded.width().max(decoded.height()) <= 1280);
    let d = out.data.unwrap();
    assert!(d["scale"].as_f64().unwrap() <= 1.0);
    assert!(!sink.take().iter().any(|e| matches!(e, Event::ControlStart { .. })), "скриншот — T0, без свечения");
}

#[tokio::test(flavor = "multi_thread")]
async fn screen_marks_on_taskbar() {
    let (c, _) = ctx();
    let out = tool(&toolkit("marks"), "screen_marks").call(json!({"window": "taskbar"}), &c).await.unwrap();
    println!("{}", out.content);
    assert!(out.content.contains("с метками"));
    assert_eq!(out.attachments.len(), 1);
    let count = out.data.unwrap()["count"].as_u64().unwrap();
    if count > 0 {
        assert!(out.content.contains("[1] "), "легенда с номерами");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn screen_ocr_tool_reads_taskbar_area() {
    let (c, _) = ctx();
    let tray = win::resolve_window(Some("taskbar")).unwrap();
    let r = tray.rect;
    let out = tool(&toolkit("ocr"), "screen_ocr").call(json!({"target": "area", "x": r.x, "y": r.y, "w": r.w, "h": r.h}), &c).await.unwrap();
    println!("{}", out.content);
    assert!(out.untrusted);
    assert!(out.content.starts_with("OCR ("));
}
