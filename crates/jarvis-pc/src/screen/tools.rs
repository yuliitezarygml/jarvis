//! Инструменты экрана: screen_capture, screen_marks, screen_ocr (T0 — только чтение).

use async_trait::async_trait;
use jarvis_core::{Attachment, Rect, RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};
use std::sync::Arc;

use super::capture::{CaptureTarget, capture, capture_rect};
use super::encode::{ImageFormat, MAX_SIDE, downscale, to_image_data};
use super::marks::{Mark, draw_marks, to_image_rect};
use super::ocr::ocr_image;
use crate::PcShared;
use crate::args::{bool_opt, clip, i32_opt, spec, str_nonempty, str_opt, u64_opt};
use crate::uia::element::{MAX_ELEMENTS, legend_line};
use crate::win::{intersect, virtual_screen};

fn target_from(args: &Value, default: CaptureTarget) -> Result<CaptureTarget, ToolError> {
    let window = str_nonempty(args, "window");
    let monitor = u64_opt(args, "monitor")?.map(|m| m as usize);
    let area = || -> Result<CaptureTarget, ToolError> {
        let need = |k: &str| i32_opt(args, k)?.ok_or_else(|| ToolError::InvalidArgs(format!("для area нужен «{k}»")));
        Ok(CaptureTarget::Area(Rect { x: need("x")?, y: need("y")?, w: need("w")?, h: need("h")? }))
    };
    let t = str_opt(args, "target").unwrap_or_default().trim().to_lowercase();
    Ok(match t.as_str() {
        "" if window.is_some() => CaptureTarget::Window(window),
        "" if monitor.is_some() => CaptureTarget::Monitor(monitor),
        "" if args.get("w").is_some() => area()?,
        "" => default,
        "screen" | "all" | "desktop" | "экран" => CaptureTarget::Screen,
        "monitor" | "монитор" => CaptureTarget::Monitor(monitor),
        "window" | "active_window" | "окно" => CaptureTarget::Window(window),
        "area" | "rect" | "region" => area()?,
        other => return Err(ToolError::InvalidArgs(format!("target «{other}»: monitor/screen/window/area"))),
    })
}

const TARGET_PROPS: &str = r#"{
    "target": {"type": "string", "enum": ["monitor", "screen", "window", "area"], "description": "monitor — монитор (по умолчанию монитор активного окна), screen — все мониторы, window — окно, area — x,y,w,h"},
    "monitor": {"type": "integer", "description": "Индекс монитора, 0 — основной"},
    "window": {"type": "string", "description": "Окно: подстрока заголовка или процесс; пусто — активное"},
    "x": {"type": "integer"}, "y": {"type": "integer"}, "w": {"type": "integer"}, "h": {"type": "integer"}
}"#;

fn schema(extra: Value) -> Value {
    let mut props: serde_json::Map<String, Value> = serde_json::from_str(TARGET_PROPS).expect("schema");
    if let Value::Object(e) = extra {
        props.extend(e);
    }
    json!({"type": "object", "properties": props})
}

fn join_err(e: tokio::task::JoinError) -> ToolError {
    ToolError::Failed(format!("фоновая задача упала: {e}"))
}

fn mapping_note(origin: Rect, scale: f64) -> String {
    if (scale - 1.0).abs() < 1e-9 {
        format!("экранная точка = ({} + x, {} + y)", origin.x, origin.y)
    } else {
        format!("экранная точка = ({} + x / {scale:.4}, {} + y / {scale:.4})", origin.x, origin.y)
    }
}

pub(crate) struct ScreenCapture(pub Arc<PcShared>);

#[async_trait]
impl Tool for ScreenCapture {
    fn spec(&self) -> ToolSpec {
        spec(
            "screen_capture",
            "Скриншот (монитор/все мониторы/окно/область), уменьшенный до ≤1280 px, картинкой для vision-модели. \
             Окна Джарвиса на снимок не попадают. Для кликов лучше screen_marks (номера элементов).",
            schema(json!({
                "format": {"type": "string", "enum": ["jpeg", "png"], "description": "По умолчанию jpeg"},
                "max_side": {"type": "integer", "description": "Длинная сторона, 200–1280"}
            })),
            RiskTier::T0,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let target = target_from(&args, CaptureTarget::Monitor(None))?;
        let format = ImageFormat::parse(str_opt(&args, "format").as_deref(), ImageFormat::Jpeg(80));
        let max_side = u64_opt(&args, "max_side")?.map(|v| v as u32).unwrap_or(MAX_SIDE).clamp(200, MAX_SIDE);
        let (what, origin, w, h, scale, image) = tokio::task::spawn_blocking(move || -> Result<_, ToolError> {
            let cap = capture(&target)?;
            let (small, scale) = downscale(&cap.image, max_side);
            let data = to_image_data(&small, format)?;
            Ok((cap.what, cap.origin, small.width(), small.height(), scale, data))
        })
        .await
        .map_err(join_err)??;
        Ok(ToolOutput {
            content: format!("Снимок: {what}, {w}x{h} (масштаб {scale:.3}); {}", mapping_note(origin, scale)),
            data: Some(json!({"origin": origin, "scale": scale, "width": w, "height": h, "mime": image.mime})),
            untrusted: true,
            attachments: vec![Attachment::Image(image)],
        })
    }
}

pub(crate) struct ScreenMarks(pub Arc<PcShared>);

#[async_trait]
impl Tool for ScreenMarks {
    fn spec(&self) -> ToolSpec {
        spec(
            "screen_marks",
            "Скриншот окна с пронумерованными цветными рамками по элементам UIA (set-of-marks) и легендой «[3] Кнопка «Сохранить»». \
             Номер = id для ui_invoke/mouse_click. По умолчанию заново снимает UIA активного окна.",
            json!({
                "type": "object",
                "properties": {
                    "window": {"type": "string", "description": "Окно: подстрока заголовка или процесс; пусто — активное"},
                    "refresh": {"type": "boolean", "description": "Сделать новый ui_snapshot (по умолчанию да); false — метки по последнему снимку"},
                    "format": {"type": "string", "enum": ["jpeg", "png"]}
                }
            }),
            RiskTier::T0,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let refresh = bool_opt(&args, "refresh", true);
        let snap = match (refresh, self.0.uia.last_snapshot()) {
            (false, Some(s)) => s,
            _ => self.0.uia.snapshot(str_nonempty(&args, "window"), MAX_ELEMENTS, false).await?,
        };
        let format = ImageFormat::parse(str_opt(&args, "format").as_deref(), ImageFormat::Jpeg(85));
        let s2 = snap.clone();
        let (image, legend, origin, scale, w, h) = tokio::task::spawn_blocking(move || -> Result<_, ToolError> {
            let origin = intersect(s2.window.rect, virtual_screen());
            if origin.w < 1 || origin.h < 1 {
                return Err(ToolError::Failed("окно вне экрана или свёрнуто".into()));
            }
            let mut img = capture_rect(origin)?;
            let long = img.width().max(img.height()).max(1);
            let scale = (MAX_SIDE as f64 / long as f64).min(1.0);
            // Рисуем в полном размере, но с толщиной, которая останется видимой после уменьшения.
            let stroke = (2.0 / scale).round().max(2.0) as i32;
            let digit = (2.0 / scale).round().max(2.0) as u32;
            let mut marks = Vec::new();
            let mut legend = Vec::new();
            for e in &s2.elements {
                if let Some(r) = to_image_rect(e.rect, origin, 1.0) {
                    marks.push(Mark { id: e.id, rect: r });
                    legend.push(legend_line(e));
                }
            }
            draw_marks(&mut img, &marks, stroke, digit);
            let (small, scale) = downscale(&img, MAX_SIDE);
            let data = to_image_data(&small, format)?;
            Ok((data, legend, origin, scale, small.width(), small.height()))
        })
        .await
        .map_err(join_err)??;
        let mut content = format!(
            "Окно «{}» с метками: {} элементов, снимок {w}x{h}. Действие по номеру: ui_invoke {{id}} или mouse_click {{id}}.\n",
            clip(&snap.window.title, 80),
            legend.len()
        );
        for l in &legend {
            content.push_str(l);
            content.push('\n');
        }
        Ok(ToolOutput {
            content,
            data: Some(json!({"origin": origin, "scale": scale, "generation": snap.generation, "count": legend.len()})),
            untrusted: true,
            attachments: vec![Attachment::Image(image)],
        })
    }
}

pub(crate) struct ScreenOcr(pub Arc<PcShared>);

/// Строк OCR в ответе модели не больше этого.
const MAX_OCR_LINES: usize = 300;

#[async_trait]
impl Tool for ScreenOcr {
    fn spec(&self) -> ToolSpec {
        spec(
            "screen_ocr",
            "Распознать текст на экране встроенным офлайн-OCR Windows (русский/английский): строки с экранными координатами. \
             По умолчанию — активное окно.",
            schema(json!({"lang": {"type": "string", "description": "ru, en или auto (языки профиля), по умолчанию auto"}})),
            RiskTier::T0,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let target = target_from(&args, CaptureTarget::Window(None))?;
        let lang = str_nonempty(&args, "lang");
        let (what, res) = tokio::task::spawn_blocking(move || -> Result<_, ToolError> {
            let cap = capture(&target)?;
            let res = ocr_image(&cap.image, lang.as_deref())?.to_screen(cap.origin, 1.0);
            Ok((cap.what, res))
        })
        .await
        .map_err(join_err)??;
        let mut content = format!("OCR ({}), {}: {} строк\n", res.language, what, res.lines.len());
        for l in res.lines.iter().take(MAX_OCR_LINES) {
            let Rect { x, y, w, h } = l.rect;
            content.push_str(&format!("[{x},{y} {w}x{h}] {}\n", l.text));
        }
        if res.lines.len() > MAX_OCR_LINES {
            content.push_str(&format!("… ещё {} строк\n", res.lines.len() - MAX_OCR_LINES));
        }
        Ok(ToolOutput { content, data: serde_json::to_value(&res).ok(), untrusted: true, attachments: vec![] })
    }
}
