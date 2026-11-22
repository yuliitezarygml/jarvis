//! Элементы снимка UIA, роли и компактный текст для LLM (чистые функции — без COM).

use jarvis_core::Rect;
use serde::{Deserialize, Serialize};

use crate::args::clip;
use crate::win::WindowInfo;

/// Предел элементов в снимке (план, раздел 3).
pub const MAX_ELEMENTS: usize = 150;
/// Сколько статических надписей (`text`) максимум добавлять к интерактивным.
pub const MAX_TEXT_ELEMENTS: usize = 40;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiElement {
    /// Номер в последнем снимке (с 1). Стабилен до следующего `ui_snapshot`.
    pub id: u32,
    /// Роль по-английски в snake_case: `button`, `edit`, `menu_item`…
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub automation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub class_name: String,
    pub rect: Rect,
    pub enabled: bool,
    pub focused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// `on` / `off` / `indeterminate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// `expanded` / `collapsed` / `partial`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<String>,
    /// Доступные действия: `invoke`, `value`, `toggle`, `expand`, `select`, `scroll`, `scroll_item`, `text`, `range`.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Элемент за пределами видимой области (бывает только в результатах `ui_find`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offscreen: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub generation: u64,
    pub window: WindowInfo,
    pub elements: Vec<UiElement>,
    /// Сколько подходящих элементов было до обрезки.
    pub total_candidates: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Interactive,
    Text,
    Skip,
}

/// UIA ControlTypeId (50000…50040) → роль.
pub fn role_for_control_type(id: i32) -> &'static str {
    const ROLES: [&str; 41] = [
        "button",
        "calendar",
        "checkbox",
        "combobox",
        "edit",
        "hyperlink",
        "image",
        "list_item",
        "list",
        "menu",
        "menu_bar",
        "menu_item",
        "progress_bar",
        "radio_button",
        "scroll_bar",
        "slider",
        "spinner",
        "status_bar",
        "tab",
        "tab_item",
        "text",
        "tool_bar",
        "tool_tip",
        "tree",
        "tree_item",
        "custom",
        "group",
        "thumb",
        "data_grid",
        "data_item",
        "document",
        "split_button",
        "window",
        "pane",
        "header",
        "header_item",
        "table",
        "title_bar",
        "separator",
        "semantic_zoom",
        "app_bar",
    ];
    id.checked_sub(50000).and_then(|i| ROLES.get(i as usize)).copied().unwrap_or("unknown")
}

/// Роль по-русски — для легенды меток и подсветки.
pub fn role_ru(role: &str) -> &'static str {
    match role {
        "button" => "Кнопка",
        "calendar" => "Календарь",
        "checkbox" => "Флажок",
        "combobox" => "Список выбора",
        "edit" => "Поле ввода",
        "hyperlink" => "Ссылка",
        "image" => "Изображение",
        "list_item" => "Элемент списка",
        "list" => "Список",
        "menu" => "Меню",
        "menu_bar" => "Строка меню",
        "menu_item" => "Пункт меню",
        "progress_bar" => "Индикатор",
        "radio_button" => "Переключатель",
        "scroll_bar" => "Полоса прокрутки",
        "slider" => "Ползунок",
        "spinner" => "Счётчик",
        "status_bar" => "Строка состояния",
        "tab" => "Вкладки",
        "tab_item" => "Вкладка",
        "text" => "Текст",
        "tool_bar" => "Панель инструментов",
        "tool_tip" => "Подсказка",
        "tree" => "Дерево",
        "tree_item" => "Узел дерева",
        "group" => "Группа",
        "thumb" => "Бегунок",
        "data_grid" | "table" => "Таблица",
        "data_item" => "Строка таблицы",
        "document" => "Документ",
        "split_button" => "Кнопка-меню",
        "window" => "Окно",
        "pane" => "Панель",
        "header" => "Заголовок",
        "header_item" => "Заголовок столбца",
        "title_bar" => "Заголовок окна",
        "separator" => "Разделитель",
        "app_bar" => "Панель приложения",
        _ => "Элемент",
    }
}

/// Роль из запроса пользователя/LLM (англ. или рус.) → каноническая.
pub fn normalize_role(s: &str) -> String {
    let s = s.trim().to_lowercase().replace([' ', '-'], "_");
    let mapped = match s.as_str() {
        "кнопка" | "btn" => "button",
        "поле" | "поле_ввода" | "textbox" | "text_box" | "input" => "edit",
        "флажок" | "check_box" => "checkbox",
        "ссылка" | "link" => "hyperlink",
        "пункт_меню" | "menuitem" => "menu_item",
        "вкладка" | "tab_item" | "tabitem" => "tab_item",
        "список" => "list",
        "элемент_списка" | "listitem" => "list_item",
        "combo" | "combo_box" | "dropdown" => "combobox",
        "текст" | "label" => "text",
        "переключатель" | "radio" | "radiobutton" => "radio_button",
        "документ" => "document",
        other => other,
    };
    mapped.to_string()
}

/// Роли, интерактивные сами по себе (даже без паттернов).
pub fn is_interactive_role(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "checkbox"
            | "combobox"
            | "edit"
            | "hyperlink"
            | "list_item"
            | "menu_item"
            | "radio_button"
            | "slider"
            | "spinner"
            | "tab_item"
            | "tree_item"
            | "data_item"
            | "document"
            | "split_button"
            | "header_item"
    )
}

/// Контейнеры: фокусируемость сама по себе не делает их интересными.
pub fn is_container_role(role: &str) -> bool {
    matches!(
        role,
        "pane"
            | "window"
            | "group"
            | "custom"
            | "text"
            | "tool_bar"
            | "menu_bar"
            | "title_bar"
            | "status_bar"
            | "tab"
            | "list"
            | "tree"
            | "table"
            | "data_grid"
            | "menu"
            | "header"
            | "separator"
            | "image"
            | "scroll_bar"
            | "thumb"
    )
}

/// Классификация элемента для снимка.
pub fn classify(role: &str, name: &str, rect: Rect, focusable: bool, has_action: bool) -> Kind {
    if rect.w <= 1 || rect.h <= 1 {
        return Kind::Skip;
    }
    if is_interactive_role(role) || has_action || (focusable && !is_container_role(role)) {
        Kind::Interactive
    } else if role == "text" && !name.trim().is_empty() {
        Kind::Text
    } else {
        Kind::Skip
    }
}

/// Какие кандидаты попадут в снимок (индексы в исходном порядке дерева).
/// Интерактивные в приоритете; надписям — остаток бюджета, но не меньше 10 мест, если они есть.
pub fn select(kinds: &[Kind], limit: usize, max_text: usize) -> Vec<usize> {
    let interactive = kinds.iter().filter(|k| **k == Kind::Interactive).count();
    let texts = kinds.iter().filter(|k| **k == Kind::Text).count();
    let text_quota = max_text.min(texts).min(limit.saturating_sub(interactive).max(10.min(texts))).min(limit);
    let inter_quota = limit - text_quota;
    let (mut ni, mut nt) = (0usize, 0usize);
    let mut out = Vec::new();
    for (i, k) in kinds.iter().enumerate() {
        match k {
            Kind::Interactive if ni < inter_quota => {
                ni += 1;
                out.push(i);
            }
            Kind::Text if nt < text_quota => {
                nt += 1;
                out.push(i);
            }
            _ => {}
        }
    }
    out
}

fn fmt_rect(r: Rect) -> String {
    format!("{},{} {}x{}", r.x, r.y, r.w, r.h)
}

/// Одна строка элемента: `[3] button «Сохранить» (120,40 80x24) focused`.
pub fn format_element(e: &UiElement) -> String {
    let mut s = format!("[{}] {} «{}»", e.id, e.role, clip(e.name.trim(), 80));
    if e.name.trim().is_empty() && !e.automation_id.is_empty() {
        s.push_str(&format!(" #{}", clip(&e.automation_id, 40)));
    }
    s.push_str(&format!(" ({})", fmt_rect(e.rect)));
    if !e.enabled {
        s.push_str(" disabled");
    }
    if e.focused {
        s.push_str(" focused");
    }
    if let Some(v) = &e.value {
        if !v.is_empty() {
            s.push_str(&format!(" value=«{}»", clip(&v.replace(['\r', '\n'], " "), 80)));
        }
    }
    if let Some(t) = &e.toggle {
        s.push_str(&format!(" toggle={t}"));
    }
    if e.selected == Some(true) {
        s.push_str(" selected");
    }
    if let Some(x) = &e.expanded {
        s.push_str(&format!(" {x}"));
    }
    if e.offscreen {
        s.push_str(" offscreen");
    }
    s
}

/// Компактный текст снимка для LLM.
pub fn format_snapshot(s: &UiSnapshot) -> String {
    let w = &s.window;
    let mut out = format!(
        "Окно «{}» ({}), {}. Элементов: {}",
        clip(&w.title, 100),
        if w.process.is_empty() { w.class.as_str() } else { w.process.as_str() },
        fmt_rect(w.rect),
        s.elements.len()
    );
    if s.truncated {
        out.push_str(&format!(" (из {} подходящих — показаны первые)", s.total_candidates));
    }
    out.push('\n');
    if s.elements.is_empty() {
        out.push_str("UIA не вернул интерактивных элементов — попробуйте screen_marks/screen_ocr.\n");
    }
    for e in &s.elements {
        out.push_str(&format_element(e));
        out.push('\n');
    }
    out
}

/// Строка легенды для set-of-marks: `[3] Кнопка «Сохранить»`.
pub fn legend_line(e: &UiElement) -> String {
    let name = e.name.trim();
    let name = if name.is_empty() { e.automation_id.as_str() } else { name };
    let mut s = format!("[{}] {} «{}»", e.id, role_ru(&e.role), clip(name, 60));
    if !e.enabled {
        s.push_str(" (недоступна)");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: u32, role: &str, name: &str) -> UiElement {
        UiElement {
            id,
            role: role.into(),
            name: name.into(),
            automation_id: String::new(),
            class_name: String::new(),
            rect: Rect { x: 10, y: 20, w: 80, h: 24 },
            enabled: true,
            focused: false,
            value: None,
            toggle: None,
            selected: None,
            expanded: None,
            patterns: vec!["invoke".into()],
            offscreen: false,
        }
    }

    #[test]
    fn roles() {
        assert_eq!(role_for_control_type(50000), "button");
        assert_eq!(role_for_control_type(50004), "edit");
        assert_eq!(role_for_control_type(50040), "app_bar");
        assert_eq!(role_for_control_type(49999), "unknown");
        assert_eq!(role_for_control_type(50041), "unknown");
        assert_eq!(role_ru("button"), "Кнопка");
        assert_eq!(normalize_role("Кнопка"), "button");
        assert_eq!(normalize_role("Menu Item"), "menu_item");
    }

    #[test]
    fn classification() {
        let r = Rect { x: 0, y: 0, w: 10, h: 10 };
        assert_eq!(classify("button", "", r, false, false), Kind::Interactive);
        assert_eq!(classify("pane", "x", r, true, false), Kind::Skip);
        assert_eq!(classify("custom", "x", r, false, true), Kind::Interactive);
        assert_eq!(classify("text", "Сохранить изменения?", r, false, false), Kind::Text);
        assert_eq!(classify("text", " ", r, false, false), Kind::Skip);
        assert_eq!(classify("button", "x", Rect { x: 0, y: 0, w: 0, h: 10 }, true, true), Kind::Skip);
    }

    #[test]
    fn selection_respects_limits_and_order() {
        use Kind::*;
        let kinds = [Text, Interactive, Skip, Interactive, Text, Interactive];
        assert_eq!(select(&kinds, 150, 40), vec![0, 1, 3, 4, 5]);
        assert_eq!(select(&kinds, 150, 0), vec![1, 3, 5]);
        // много интерактивных: всё равно до 10 надписей и ровно limit всего
        let mut big = vec![Interactive; 300];
        big.extend(vec![Text; 20]);
        let picked = select(&big, 150, 40);
        assert_eq!(picked.len(), 150);
        assert_eq!(picked.iter().filter(|&&i| big[i] == Text).count(), 10);
        assert!(picked.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn compact_text() {
        let mut e = el(3, "button", "Сохранить");
        assert_eq!(format_element(&e), "[3] button «Сохранить» (10,20 80x24)");
        e.enabled = false;
        e.focused = true;
        e.value = Some("a\nb".into());
        assert_eq!(format_element(&e), "[3] button «Сохранить» (10,20 80x24) disabled focused value=«a b»");
        assert_eq!(legend_line(&el(3, "button", "Сохранить")), "[3] Кнопка «Сохранить»");
        let mut anon = el(4, "edit", "");
        anon.automation_id = "SearchBox".into();
        assert!(format_element(&anon).contains("#SearchBox"));

        let snap = UiSnapshot {
            generation: 1,
            window: WindowInfo {
                hwnd: 1,
                title: "Безымянный — Блокнот".into(),
                class: "Notepad".into(),
                pid: 1,
                process: "notepad.exe".into(),
                rect: Rect { x: 0, y: 0, w: 800, h: 600 },
                minimized: false,
                foreground: true,
            },
            elements: vec![el(1, "menu_item", "Файл"), el(2, "edit", "Текст")],
            total_candidates: 2,
            truncated: false,
        };
        let t = format_snapshot(&snap);
        assert!(t.starts_with("Окно «Безымянный — Блокнот» (notepad.exe), 0,0 800x600. Элементов: 2\n"));
        assert!(t.contains("[2] edit «Текст»"));
    }
}
