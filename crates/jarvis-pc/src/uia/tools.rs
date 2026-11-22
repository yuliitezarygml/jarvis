//! Инструменты UIA: ui_snapshot, ui_find, ui_window_list (T0); ui_invoke, ui_set_value, ui_toggle, ui_expand,
//! ui_select, ui_scroll, ui_focus_window (T1).

use async_trait::async_trait;
use jarvis_core::{Rect, RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

use super::DEFAULT_TIMEOUT;
use super::element::{MAX_ELEMENTS, format_element, format_snapshot};
use super::worker::{ScrollDir, SetValueOutcome, UiaState};
use crate::PcShared;
use crate::args::{bool_opt, clip, spec, str_nonempty, str_opt, str_req, u64_opt};
use crate::input::send::run_input;
use crate::input::tools::display_name;
use crate::input::{MAX_TYPE_CHARS, hotkey::parse_hotkey, press_chords, type_text};
use crate::win;

/// Пауза, чтобы человек успел увидеть подсветку цели до действия.
const SHOW_TARGET_MS: u64 = 150;

async fn pause_visible(ctx: &ToolCtx) -> Result<(), ToolError> {
    tokio::select! {
        _ = ctx.cancel.cancelled() => Err(ToolError::Cancelled),
        _ = tokio::time::sleep(Duration::from_millis(SHOW_TARGET_MS)) => Ok(()),
    }
}

fn id_arg(args: &Value) -> Result<u32, ToolError> {
    u64_opt(args, "id")?
        .map(|v| v as u32)
        .ok_or_else(|| ToolError::InvalidArgs("нужен id элемента из последнего ui_snapshot".into()))
}

fn id_schema(extra: Value) -> Value {
    let mut props = serde_json::Map::new();
    props.insert("id".into(), json!({"type": "integer", "description": "id элемента из последнего ui_snapshot/ui_find/screen_marks"}));
    if let Value::Object(e) = extra {
        props.extend(e);
    }
    json!({"type": "object", "properties": props, "required": ["id"]})
}

/// Подсветить элемент, подождать, выполнить действие в потоке UIA.
async fn act<R, F>(sh: &PcShared, ctx: &ToolCtx, id: u32, verb: &str, f: F) -> Result<(R, String), ToolError>
where
    R: Send + 'static,
    F: FnOnce(&mut UiaState) -> Result<R, ToolError> + Send + 'static,
{
    let (rect, name, role) = sh.uia.element_rect(id).await?;
    let shown = display_name(&name, &role);
    let session = sh.glow.session(ctx.events.clone(), RiskTier::T1);
    session.target(rect, format!("{verb}: {shown}"));
    pause_visible(ctx).await?;
    let r = sh.uia.call(DEFAULT_TIMEOUT, f).await?;
    Ok((r, shown))
}

const WINDOW_PROP: &str = "Окно: подстрока заголовка или имя процесса (`notepad`), `class:Имя`, `taskbar`. По умолчанию — активное окно";

pub(crate) struct UiSnapshotTool(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiSnapshotTool {
    fn spec(&self) -> ToolSpec {
        spec(
            "ui_snapshot",
            "Снимок интерактивных элементов окна через UI Automation (до 150): [id] роль «имя» (x,y wxh) состояние. \
             id действуют до следующего снимка: ui_invoke/ui_set_value/ui_toggle/mouse_click {id}. Начинайте с этого, скриншот — если UIA мало.",
            json!({
                "type": "object",
                "properties": {
                    "window": {"type": "string", "description": WINDOW_PROP},
                    "limit": {"type": "integer", "description": "Максимум элементов, 1–150"},
                    "include_text": {"type": "boolean", "description": "Добавить статические надписи (по умолчанию да)"}
                }
            }),
            RiskTier::T0,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let window = str_nonempty(&args, "window");
        let limit = u64_opt(&args, "limit")?.map(|v| v as usize).unwrap_or(MAX_ELEMENTS).clamp(1, MAX_ELEMENTS);
        let include_text = bool_opt(&args, "include_text", true);
        let snap = self.0.uia.snapshot(window, limit, include_text).await?;
        Ok(ToolOutput {
            content: format_snapshot(&snap),
            data: serde_json::to_value(&*snap).ok(),
            untrusted: true,
            attachments: vec![],
        })
    }
}

pub(crate) struct UiFind(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiFind {
    fn spec(&self) -> ToolSpec {
        spec(
            "ui_find",
            "Найти элементы окна по имени (подстрока без учёта регистра) и/или роли (button, edit, menu_item…), \
             включая невидимые без прокрутки. Найденные получают id (добавляются к последнему снимку).",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Имя или его часть: «Сохранить»"},
                    "role": {"type": "string", "description": "Роль: button, edit, checkbox, menu_item, list_item, hyperlink…"},
                    "window": {"type": "string", "description": WINDOW_PROP},
                    "limit": {"type": "integer", "description": "1–30, по умолчанию 10"}
                }
            }),
            RiskTier::T0,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let name = str_opt(&args, "name").unwrap_or_default();
        let role = str_nonempty(&args, "role");
        if name.trim().is_empty() && role.is_none() {
            return Err(ToolError::InvalidArgs("нужно name и/или role".into()));
        }
        let window = str_nonempty(&args, "window");
        let limit = u64_opt(&args, "limit")?.unwrap_or(10).clamp(1, 30) as usize;
        let (found, snap) = self
            .0
            .uia
            .call(DEFAULT_TIMEOUT, move |s| {
                let win = win::resolve_window(window.as_deref())?;
                s.find(win, &name, role.as_deref(), limit)
            })
            .await?;
        self.0.uia.set_last(snap);
        let content = if found.is_empty() {
            "Ничего не найдено. Проверьте имя или сделайте ui_snapshot / screen_ocr.".to_string()
        } else {
            let mut s = format!("Найдено: {}\n", found.len());
            for e in &found {
                s.push_str(&format_element(e));
                s.push('\n');
            }
            s
        };
        Ok(ToolOutput { content, data: Some(json!({"elements": found})), untrusted: true, attachments: vec![] })
    }
}

pub(crate) struct UiInvoke(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiInvoke {
    fn spec(&self) -> ToolSpec {
        spec(
            "ui_invoke",
            "Нажать элемент через UIA без мыши (Invoke; для флажков — Toggle, для пунктов — Select/Expand). \
             Если не поддерживается — используйте mouse_click {id}.",
            id_schema(json!({})),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let id = id_arg(&args)?;
        let (how, name) = act(&self.0, ctx, id, "Нажимаю", move |s| s.invoke(id)).await?;
        Ok(ToolOutput { content: format!("Нажал «{name}» [{id}] ({how})"), data: Some(json!({"id": id, "method": how})), ..Default::default() })
    }
}

pub(crate) struct UiSetValue(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiSetValue {
    fn spec(&self) -> ToolSpec {
        spec(
            "ui_set_value",
            "Записать текст в поле ввода (ValuePattern; если не поддерживается — фокус, Ctrl+A и ввод с клавиатуры). Заменяет содержимое.",
            id_schema(json!({"text": {"type": "string", "description": "Новое значение"}})),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let id = id_arg(&args)?;
        let text = str_req(&args, "text")?;
        if text.chars().count() > MAX_TYPE_CHARS {
            return Err(ToolError::InvalidArgs(format!("текст длиннее {MAX_TYPE_CHARS} символов")));
        }
        let t = text.clone();
        let (outcome, name) = act(&self.0, ctx, id, "Ввожу", move |s| s.set_value(id, &t)).await?;
        let via = match outcome {
            SetValueOutcome::Pattern => "ValuePattern",
            SetValueOutcome::NeedsKeyboard => {
                let chords = parse_hotkey("ctrl+a").map_err(ToolError::Failed)?;
                let _session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
                run_input(ctx.cancel.clone(), move |r| {
                    r.wait_user_modifiers_released()?;
                    r.pause(60)?;
                    press_chords(r, &chords)?;
                    r.pause(40)?;
                    if text.is_empty() {
                        r.send(&crate::input::events::key_tap(crate::input::hotkey::vk::DELETE))?;
                        Ok(0)
                    } else {
                        type_text(r, &text, 3)
                    }
                })
                .await?;
                "клавиатура"
            }
        };
        Ok(ToolOutput { content: format!("Записал значение в «{name}» [{id}] ({via})"), data: Some(json!({"id": id, "via": via})), ..Default::default() })
    }
}

pub(crate) struct UiToggle(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiToggle {
    fn spec(&self) -> ToolSpec {
        spec("ui_toggle", "Переключить флажок/тумблер (TogglePattern). Возвращает новое состояние on/off.", id_schema(json!({})), RiskTier::T1)
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let id = id_arg(&args)?;
        let (state, name) = act(&self.0, ctx, id, "Переключаю", move |s| s.toggle(id)).await?;
        Ok(ToolOutput { content: format!("«{name}» [{id}] теперь {state}"), data: Some(json!({"id": id, "state": state})), ..Default::default() })
    }
}

pub(crate) struct UiExpand(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiExpand {
    fn spec(&self) -> ToolSpec {
        spec(
            "ui_expand",
            "Раскрыть или свернуть список/меню/узел дерева (ExpandCollapsePattern).",
            id_schema(json!({"expand": {"type": "boolean", "description": "true — раскрыть (по умолчанию), false — свернуть"}})),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let id = id_arg(&args)?;
        let expand = bool_opt(&args, "expand", true);
        let verb = if expand { "Раскрываю" } else { "Сворачиваю" };
        let (state, name) = act(&self.0, ctx, id, verb, move |s| s.expand(id, expand)).await?;
        Ok(ToolOutput { content: format!("«{name}» [{id}]: {state}"), data: Some(json!({"id": id, "state": state})), ..Default::default() })
    }
}

pub(crate) struct UiSelect(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiSelect {
    fn spec(&self) -> ToolSpec {
        spec("ui_select", "Выбрать элемент списка/вкладку/переключатель (SelectionItemPattern).", id_schema(json!({})), RiskTier::T1)
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let id = id_arg(&args)?;
        let ((), name) = act(&self.0, ctx, id, "Выбираю", move |s| s.select_item(id)).await?;
        Ok(ToolOutput::text(format!("Выбрал «{name}» [{id}]")))
    }
}

pub(crate) struct UiScroll(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiScroll {
    fn spec(&self) -> ToolSpec {
        spec(
            "ui_scroll",
            "Прокрутить через UIA (ScrollPattern): элемент/его контейнер по id или главную прокручиваемую область окна. \
             into_view=true — прокрутить так, чтобы элемент id стал видимым.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer"},
                    "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                    "amount": {"type": "string", "enum": ["line", "page"], "description": "По умолчанию line"},
                    "count": {"type": "integer", "description": "Повторов, 1–20"},
                    "into_view": {"type": "boolean"},
                    "window": {"type": "string", "description": WINDOW_PROP}
                }
            }),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let id = u64_opt(&args, "id")?.map(|v| v as u32);
        let dir_s = str_opt(&args, "direction").unwrap_or_default();
        let dir = ScrollDir::parse(&dir_s).ok_or_else(|| ToolError::InvalidArgs(format!("direction «{dir_s}»")))?;
        let page = matches!(str_opt(&args, "amount").unwrap_or_default().to_lowercase().as_str(), "page" | "large" | "страница");
        let count = u64_opt(&args, "count")?.unwrap_or(1).clamp(1, 20) as u32;
        let into_view = bool_opt(&args, "into_view", false);
        let window = str_nonempty(&args, "window");
        let (rect, label) = match id {
            Some(id) => {
                let (r, n, role) = self.0.uia.element_rect(id).await?;
                (r, display_name(&n, &role))
            }
            None => {
                let w = win::resolve_window(window.as_deref())?;
                (w.rect, clip(&w.title, 40))
            }
        };
        let session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        session.target(rect, format!("Прокручиваю: {label}"));
        pause_visible(ctx).await?;
        let msg = self
            .0
            .uia
            .call(DEFAULT_TIMEOUT, move |s| {
                let win = if id.is_none() { Some(win::resolve_window(window.as_deref())?) } else { None };
                s.scroll(id, win, dir, page, count, into_view)
            })
            .await?;
        Ok(ToolOutput::text(msg))
    }
}

pub(crate) struct UiFocusWindow(pub Arc<PcShared>);

#[async_trait]
impl Tool for UiFocusWindow {
    fn spec(&self) -> ToolSpec {
        spec(
            "ui_focus_window",
            "Переключиться на окно (развернуть, если свёрнуто) по подстроке заголовка или имени процесса.",
            json!({
                "type": "object",
                "properties": {"title": {"type": "string", "description": "Подстрока заголовка или процесс: «Блокнот», «chrome»"}},
                "required": ["title"]
            }),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let title = str_req(&args, "title")?;
        let w = win::resolve_window(Some(&title))?;
        let session = self.0.glow.session(ctx.events.clone(), RiskTier::T1);
        session.target(w.rect, format!("Переключаюсь: {}", clip(&w.title, 40)));
        let h = w.hwnd;
        tokio::task::spawn_blocking(move || win::focus_window(h))
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))??;
        Ok(ToolOutput {
            content: format!("Активно окно «{}» ({})", w.title, w.process),
            data: serde_json::to_value(&w).ok(),
            untrusted: true,
            attachments: vec![],
        })
    }
}

pub(crate) struct UiWindowList;

fn fmt_window(w: &win::WindowInfo) -> String {
    let Rect { x, y, w: ww, h } = w.rect;
    let mut s = format!("«{}» ({}) {x},{y} {ww}x{h} hwnd:{}", clip(&w.title, 90), w.process, w.hwnd);
    if w.foreground {
        s.push_str(" [активно]");
    }
    if w.minimized {
        s.push_str(" [свёрнуто]");
    }
    s
}

#[async_trait]
impl Tool for UiWindowList {
    fn spec(&self) -> ToolSpec {
        spec(
            "ui_window_list",
            "Список открытых окон (сверху вниз по Z-порядку): заголовок, процесс, положение, активно/свёрнуто.",
            json!({"type": "object", "properties": {}}),
            RiskTier::T0,
        )
    }

    async fn call(&self, _args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let list = tokio::task::spawn_blocking(win::list_windows).await.map_err(|e| ToolError::Failed(e.to_string()))?;
        let mut content = format!("Окон: {}\n", list.len());
        for w in &list {
            content.push_str(&fmt_window(w));
            content.push('\n');
        }
        Ok(ToolOutput { content, data: Some(json!({"windows": list})), untrusted: true, attachments: vec![] })
    }
}
