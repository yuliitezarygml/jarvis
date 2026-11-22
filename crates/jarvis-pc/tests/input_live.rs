//! ЖИВОЙ ВВОД: двигает мышь, кликает, печатает, жмёт клавиши.
//! Все тесты `#[ignore]` — НЕ ЗАПУСКАТЬ, пока пользователь работает за ПК.
//! Ручной запуск: `cargo test --test input_live -- --ignored --test-threads=1`, открыв пустой Блокнот в фокусе.

mod common;

use common::{ctx, tool, toolkit};
use jarvis_core::Event;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "двигает мышь пользователя"]
async fn live_mouse_move_to_screen_center() {
    let vs = jarvis_pc::win::virtual_screen();
    let (c, sink) = ctx();
    let (x, y) = (vs.x + vs.w / 2, vs.y + vs.h / 2);
    tool(&toolkit("live"), "mouse_move").call(json!({"x": x, "y": y}), &c).await.unwrap();
    let ev = sink.take();
    assert!(matches!(ev[0], Event::ControlStart { .. }));
    assert!(matches!(ev[1], Event::ControlTarget { .. }));
    assert_eq!(ev.last(), Some(&Event::ControlStop));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "кликает мышью"]
async fn live_mouse_click_and_double_click() {
    let vs = jarvis_pc::win::virtual_screen();
    let (c, _) = ctx();
    let t = tool(&toolkit("live"), "mouse_click");
    t.call(json!({"x": vs.x + vs.w / 2, "y": vs.y + vs.h / 2, "label": "тест"}), &c).await.unwrap();
    t.call(json!({"x": vs.x + vs.w / 2, "y": vs.y + vs.h / 2, "double": true}), &c).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "печатает текст в активное окно"]
async fn live_key_type_cyrillic_and_newlines() {
    let (c, _) = ctx();
    tool(&toolkit("live"), "key_type").call(json!({"text": "Привет, сэр!\nВторая строка 😀"}), &c).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "нажимает сочетания клавиш"]
async fn live_hotkey_select_all() {
    let (c, _) = ctx();
    tool(&toolkit("live"), "key_hotkey").call(json!({"keys": "ctrl+a"}), &c).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "меняет громкость"]
async fn live_media_volume() {
    let (c, _) = ctx();
    let t = tool(&toolkit("live"), "media_key");
    t.call(json!({"key": "volume_up"}), &c).await.unwrap();
    t.call(json!({"key": "volume_down"}), &c).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "перетаскивает мышью"]
async fn live_drag_and_scroll() {
    let vs = jarvis_pc::win::virtual_screen();
    let (c, _) = ctx();
    let tk = toolkit("live");
    let (cx, cy) = (vs.x + vs.w / 2, vs.y + vs.h / 2);
    tool(&tk, "mouse_drag").call(json!({"from_x": cx, "from_y": cy, "to_x": cx + 100, "to_y": cy}), &c).await.unwrap();
    tool(&tk, "mouse_scroll").call(json!({"direction": "down", "amount": 2}), &c).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "отмена посреди печати: проверка, что модификаторы отпущены"]
async fn live_cancel_releases_keys() {
    let (mut c, _) = ctx();
    let token = jarvis_core::CancellationToken::new();
    c.cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        token.cancel();
    });
    let r = tool(&toolkit("live"), "key_type").call(json!({"text": "x".repeat(300), "delay_ms": 20}), &c).await;
    assert_eq!(r.unwrap_err(), jarvis_core::ToolError::Cancelled);
    assert_eq!(jarvis_pc::release_all_held(), 0, "ничего не осталось зажатым");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "переключает фокус окна и вводит текст в терминал"]
async fn live_terminal_type() {
    let (c, _) = ctx();
    tool(&toolkit("live"), "terminal_type").call(json!({"text": "echo jarvis", "enter": false}), &c).await.unwrap();
}
