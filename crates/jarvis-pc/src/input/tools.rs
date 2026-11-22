//! Инструменты ввода: mouse_move, mouse_click, mouse_drag, mouse_scroll, key_type, key_hotkey, media_key (все T1).

use async_trait::async_trait;
use jarvis_core::{Rect, RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};
use std::sync::Arc;

use super::events::{InputEvent, MouseButton, WHEEL_DELTA, key_tap};
use super::hotkey::{describe, parse_hotkey, vk};
use super::send::run_input;
use super::{MAX_TYPE_CHARS, press_chords, type_text};
use crate::PcShared;
use crate::args::{bool_opt, clip, i32_req, spec, str_nonempty, str_opt, str_req, u64_opt};
use crate::control::point_rect;
use crate::uia::element::role_ru;
use crate::win::{self, center, contains, virtual_screen};

pub(crate) fn display_name(name: &str, role: &str) -> String {
    let n = name.trim();
    if n.is_empty() { role_ru(role).to_string() } else { clip(n, 60) }
}

/// Точка действия: по `id` элемента последнего снимка UIA (центр) или по `x`,`y`.
async fn resolve_point(sh: &PcShared, args: &Value, xk: &str, yk: &str) -> Result<(i32, i32, Rect, Option<String>), ToolError> {
    if let Some(id) = u64_opt(args, "id")? {
        let (rect, name, role) = sh.uia.element_rect(id as u32).await?;
        if rect.w <= 0 || rect.h <= 0 {
            return Err(ToolError::Failed(format!("у элемента [{id}] нет видимого прямоугольника")));
        }
        let (x, y) = center(rect);
        return Ok((x, y, rect, Some(display_name(&name, &role))));
    }
    let x = i32_req(args, xk)?;
    let y = i32_req(args, yk)?;
    Ok((x, y, point_rect(x, y), str_nonempty(args, "label")))
}

fn check_on_screen(x: i32, y: i32) -> Result<(), ToolError> {
    let vs = virtual_screen();
    if contains(vs, x, y) {
        Ok(())
    } else {
        Err(ToolError::InvalidArgs(format!(
            "точка ({x}, {y}) вне экрана: рабочий стол x {}…{}, y {}…{}",
            vs.x,
            vs.x + vs.w - 1,
            vs.y,
            vs.y + vs.h - 1
        )))
    }
}

fn foreground_rect() -> Rect {
    win::foreground_app_window().map(|w| w.rect).unwrap_or_else(virtual_screen)
}

fn duration_arg(args: &Value) -> Result<Option<u32>, ToolError> {
    Ok(u64_opt(args, "duration_ms")?.map(|d| d.min(3000) as u32))
}

const POINT_PROPS: &str = r#"{
    "x": {"type": "integer", "description": "X в пикселях виртуального рабочего стола (физические пиксели, как в ui_snapshot)"},
    "y": {"type": "integer", "description": "Y в пикселях виртуального рабочего стола"},
    "id": {"type": "integer", "description": "Вместо x/y: id элемента из последнего ui_snapshot/screen_marks — клик в его центр"},
    "label": {"type": "string", "description": "Что это за цель (для подсветки), например «Сохранить»"}
}"#;

fn point_schema(extra: Value) -> Value {
    let mut props: serde_json::Map<String, Value> = serde_json::from_str(POINT_PROPS).expect("schema");
    if let Value::Object(e) = extra {
        props.extend(e);
    }
    json!({"type": "object", "properties": props})
}

pub(crate) struct MouseMove(pub Arc<PcShared>);

#[async_trait]
impl Tool for MouseMove {
    fn spec(&self) -> ToolSpec {
        spec(
            "mouse_move",
            "Плавно переместить курсор (150–400 мс, видно движение) в точку или к элементу по id.",
            point_schema(json!({"duration_ms": {"type": "integer", "description": "Длительность движения, по умолчанию по расстоянию"}})),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let (x, y, rect, label) = resolve_point(&self.0, &args, "x", "y").await?;
        check_on_screen(x, y)?;
        let dur = duration_arg(&args)?;
        let session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        session.target(rect, label.map(|l| format!("Навожу: {l}")).unwrap_or_else(|| format!("Курсор → {x}, {y}")));
        run_input(ctx.cancel.clone(), move |r| r.smooth_move((x, y), dur)).await?;
        Ok(ToolOutput { content: format!("Курсор в ({x}, {y})"), data: Some(json!({"x": x, "y": y})), ..Default::default() })
    }
}

pub(crate) struct MouseClick(pub Arc<PcShared>);

#[async_trait]
impl Tool for MouseClick {
    fn spec(&self) -> ToolSpec {
        spec(
            "mouse_click",
            "Клик мышью: курсор плавно подъезжает к точке (или центру элемента по id) и нажимает. \
             Предпочитайте ui_invoke, если элемент поддерживает Invoke.",
            point_schema(json!({
                "button": {"type": "string", "enum": ["left", "right", "middle"], "description": "Кнопка, по умолчанию left"},
                "double": {"type": "boolean", "description": "Двойной клик"}
            })),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let (x, y, rect, label) = resolve_point(&self.0, &args, "x", "y").await?;
        check_on_screen(x, y)?;
        let button_s = str_opt(&args, "button").unwrap_or_default();
        let button = MouseButton::parse(&button_s)
            .ok_or_else(|| ToolError::InvalidArgs(format!("неизвестная кнопка «{button_s}»: left/right/middle")))?;
        let double = bool_opt(&args, "double", false);
        let verb = match (button, double) {
            (_, true) => "Дважды нажимаю",
            (MouseButton::Right, _) => "Правый клик",
            _ => "Нажимаю",
        };
        let session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        session.target(rect, format!("{verb}: {}", label.clone().unwrap_or_else(|| format!("{x}, {y}"))));
        run_input(ctx.cancel.clone(), move |r| {
            r.smooth_move((x, y), None)?;
            r.pause(40)?;
            r.click(button, double)
        })
        .await?;
        let what = label.map(|l| format!(" по «{l}»")).unwrap_or_default();
        Ok(ToolOutput {
            content: format!("{verb}{what} в ({x}, {y})"),
            data: Some(json!({"x": x, "y": y, "button": format!("{button:?}").to_lowercase(), "double": double})),
            ..Default::default()
        })
    }
}

pub(crate) struct MouseDrag(pub Arc<PcShared>);

#[async_trait]
impl Tool for MouseDrag {
    fn spec(&self) -> ToolSpec {
        spec(
            "mouse_drag",
            "Перетащить: нажать кнопку в (from_x, from_y), плавно провести до (to_x, to_y), отпустить.",
            json!({
                "type": "object",
                "properties": {
                    "from_x": {"type": "integer"}, "from_y": {"type": "integer"},
                    "to_x": {"type": "integer"}, "to_y": {"type": "integer"},
                    "button": {"type": "string", "enum": ["left", "right", "middle"]},
                    "duration_ms": {"type": "integer", "description": "Длительность перетаскивания, по умолчанию 300–600 мс"},
                    "label": {"type": "string"}
                },
                "required": ["from_x", "from_y", "to_x", "to_y"]
            }),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let (fx, fy) = (i32_req(&args, "from_x")?, i32_req(&args, "from_y")?);
        let (tx, ty) = (i32_req(&args, "to_x")?, i32_req(&args, "to_y")?);
        check_on_screen(fx, fy)?;
        check_on_screen(tx, ty)?;
        let button_s = str_opt(&args, "button").unwrap_or_default();
        let button = MouseButton::parse(&button_s).ok_or_else(|| ToolError::InvalidArgs("button: left/right/middle".into()))?;
        let dur = duration_arg(&args)?;
        let label = str_nonempty(&args, "label").unwrap_or_else(|| format!("{fx},{fy} → {tx},{ty}"));
        let session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        let bounds = Rect { x: fx.min(tx), y: fy.min(ty), w: (tx - fx).abs().max(1), h: (ty - fy).abs().max(1) };
        session.target(bounds, format!("Перетаскиваю: {label}"));
        run_input(ctx.cancel.clone(), move |r| {
            r.smooth_move((fx, fy), None)?;
            r.pause(60)?;
            r.send(&[InputEvent::Button { button, up: false }])?;
            r.pause(90)?;
            let d = dur.unwrap_or_else(|| {
                (super::trajectory::duration_for_distance(super::trajectory::distance((fx, fy), (tx, ty))) + 200).min(600)
            });
            r.smooth_move((tx, ty), Some(d))?;
            r.pause(90)?;
            r.send(&[InputEvent::Button { button, up: true }])
        })
        .await?;
        Ok(ToolOutput::text(format!("Перетащил из ({fx}, {fy}) в ({tx}, {ty})")))
    }
}

pub(crate) struct MouseScroll(pub Arc<PcShared>);

#[async_trait]
impl Tool for MouseScroll {
    fn spec(&self) -> ToolSpec {
        spec(
            "mouse_scroll",
            "Прокрутить колесом мыши (в точке x,y или под текущим курсором). Для UIA-окон есть ui_scroll.",
            json!({
                "type": "object",
                "properties": {
                    "direction": {"type": "string", "enum": ["up", "down", "left", "right"], "description": "По умолчанию down"},
                    "amount": {"type": "integer", "description": "Щелчков колеса, 1–50, по умолчанию 3"},
                    "x": {"type": "integer"}, "y": {"type": "integer"},
                    "id": {"type": "integer", "description": "Прокрутить над элементом из ui_snapshot"}
                }
            }),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let point = if u64_opt(&args, "id")?.is_some() || args.get("x").is_some() {
            let (x, y, rect, _) = resolve_point(&self.0, &args, "x", "y").await?;
            check_on_screen(x, y)?;
            Some((x, y, rect))
        } else {
            None
        };
        let dir = str_opt(&args, "direction").unwrap_or_else(|| "down".into()).to_lowercase();
        let (sign, horizontal) = match dir.as_str() {
            "up" | "вверх" => (1, false),
            "down" | "вниз" => (-1, false),
            "left" | "влево" => (-1, true),
            "right" | "вправо" => (1, true),
            other => return Err(ToolError::InvalidArgs(format!("direction «{other}»: up/down/left/right"))),
        };
        let amount = u64_opt(&args, "amount")?.unwrap_or(3).clamp(1, 50) as u32;
        let session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        let rect = point.map(|p| p.2).unwrap_or_else(foreground_rect);
        session.target(rect, format!("Прокручиваю {dir}"));
        run_input(ctx.cancel.clone(), move |r| {
            if let Some((x, y, _)) = point {
                r.smooth_move((x, y), None)?;
                r.pause(40)?;
            }
            for _ in 0..amount {
                r.send(&[InputEvent::Wheel { delta: sign * WHEEL_DELTA, horizontal }])?;
                r.pause(35)?;
            }
            Ok(())
        })
        .await?;
        Ok(ToolOutput::text(format!("Прокрутил {dir} на {amount}")))
    }
}

pub(crate) struct KeyType(pub Arc<PcShared>);

#[async_trait]
impl Tool for KeyType {
    fn spec(&self) -> ToolSpec {
        spec(
            "key_type",
            "Напечатать текст в активное окно (любой язык, кириллица, переводы строк = Enter). \
             Сначала убедитесь, что фокус в нужном поле.",
            json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Текст (до 5000 символов)"},
                    "delay_ms": {"type": "integer", "description": "Пауза между символами, 0–200 мс, по умолчанию 5"}
                },
                "required": ["text"]
            }),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let text = str_req(&args, "text")?;
        if text.is_empty() {
            return Err(ToolError::InvalidArgs("пустой текст".into()));
        }
        if text.chars().count() > MAX_TYPE_CHARS {
            return Err(ToolError::InvalidArgs(format!("текст длиннее {MAX_TYPE_CHARS} символов — используйте буфер обмена")));
        }
        let delay = u64_opt(&args, "delay_ms")?.unwrap_or(5).min(200);
        let session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        session.target(foreground_rect(), format!("Печатаю: «{}»", clip(&text.replace(['\r', '\n'], "⏎"), 40)));
        let n = run_input(ctx.cancel.clone(), move |r| {
            r.wait_user_modifiers_released()?;
            type_text(r, &text, delay)
        })
        .await?;
        Ok(ToolOutput { content: format!("Напечатал {n} символов"), data: Some(json!({"chars": n})), ..Default::default() })
    }
}

pub(crate) struct KeyHotkey(pub Arc<PcShared>);

#[async_trait]
impl Tool for KeyHotkey {
    fn spec(&self) -> ToolSpec {
        spec(
            "key_hotkey",
            "Нажать сочетание клавиш в активном окне: «ctrl+s», «alt+f4», «win+r», «ctrl+shift+esc», «enter»; \
             последовательность через пробел: «ctrl+k ctrl+c».",
            json!({
                "type": "object",
                "properties": {
                    "keys": {"type": "string", "description": "Сочетание, например ctrl+shift+s"},
                    "repeat": {"type": "integer", "description": "Сколько раз, 1–20"}
                },
                "required": ["keys"]
            }),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let keys = str_req(&args, "keys")?;
        let chords = parse_hotkey(&keys).map_err(ToolError::InvalidArgs)?;
        let repeat = u64_opt(&args, "repeat")?.unwrap_or(1).clamp(1, 20) as usize;
        let name = describe(&chords);
        let session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        session.target(foreground_rect(), format!("Нажимаю {name}"));
        run_input(ctx.cancel.clone(), move |r| {
            r.wait_user_modifiers_released()?;
            for i in 0..repeat {
                if i > 0 {
                    r.pause(50)?;
                }
                press_chords(r, &chords)?;
            }
            Ok(())
        })
        .await?;
        let times = if repeat > 1 { format!(" ×{repeat}") } else { String::new() };
        Ok(ToolOutput::text(format!("Нажал {name}{times}")))
    }
}

pub(crate) struct MediaKey(pub Arc<PcShared>);

fn media_vk(name: &str) -> Option<(u16, &'static str)> {
    let n = name.trim().to_lowercase().replace(['-', ' '], "_");
    Some(match n.as_str() {
        "play_pause" | "play" | "pause" | "playpause" => (vk::MEDIA_PLAY_PAUSE, "воспроизведение/пауза"),
        "next" | "next_track" => (vk::MEDIA_NEXT_TRACK, "следующий трек"),
        "prev" | "previous" | "prev_track" | "previous_track" => (vk::MEDIA_PREV_TRACK, "предыдущий трек"),
        "stop" => (vk::MEDIA_STOP, "стоп"),
        "volume_up" | "vol_up" | "louder" => (vk::VOLUME_UP, "громче"),
        "volume_down" | "vol_down" | "quieter" => (vk::VOLUME_DOWN, "тише"),
        "mute" | "volume_mute" => (vk::VOLUME_MUTE, "звук вкл/выкл"),
        _ => return None,
    })
}

#[async_trait]
impl Tool for MediaKey {
    fn spec(&self) -> ToolSpec {
        spec(
            "media_key",
            "Медиаклавиша: play_pause, next, prev, stop, volume_up, volume_down, mute (громкость — шагами по 2%).",
            json!({
                "type": "object",
                "properties": {
                    "key": {"type": "string", "enum": ["play_pause", "next", "prev", "stop", "volume_up", "volume_down", "mute"]},
                    "count": {"type": "integer", "description": "Сколько раз нажать, 1–50 (для громкости)"}
                },
                "required": ["key"]
            }),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let key = str_req(&args, "key")?;
        let (code, what) = media_vk(&key).ok_or_else(|| ToolError::InvalidArgs(format!("неизвестная медиаклавиша «{key}»")))?;
        let count = u64_opt(&args, "count")?.unwrap_or(1).clamp(1, 50) as usize;
        let _session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        run_input(ctx.cancel.clone(), move |r| {
            for i in 0..count {
                if i > 0 {
                    r.pause(30)?;
                }
                r.send(&key_tap(code))?;
            }
            Ok(())
        })
        .await?;
        Ok(ToolOutput::text(format!("Медиаклавиша: {what} ×{count}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_names() {
        assert_eq!(media_vk("Volume Up").unwrap().0, vk::VOLUME_UP);
        assert_eq!(media_vk("play").unwrap().0, vk::MEDIA_PLAY_PAUSE);
        assert!(media_vk("louderer").is_none());
    }

    #[test]
    fn schema_merges() {
        let s = point_schema(json!({"double": {"type": "boolean"}}));
        assert!(s["properties"]["x"].is_object());
        assert!(s["properties"]["double"].is_object());
    }
}
