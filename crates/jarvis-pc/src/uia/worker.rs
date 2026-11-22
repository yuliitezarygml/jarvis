//! Работа с IUIAutomation. Всё здесь выполняется только в выделенном потоке UIA (COM MTA).

use jarvis_core::{Rect, ToolError};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR};
use windows::Win32::UI::Accessibility::*;
use windows::core::{BOOL, BSTR, Interface};

use super::element::{Kind, MAX_TEXT_ELEMENTS, UiElement, UiSnapshot, classify, normalize_role, role_for_control_type, select};
use crate::args::{clip, fold};
use crate::win::{WindowInfo, hwnd, rect_from};

/// Свойства, которые забираем одним запросом для всех элементов (кэш UIA).
const PROPS: &[UIA_PROPERTY_ID] = &[
    UIA_ControlTypePropertyId,
    UIA_NamePropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsKeyboardFocusablePropertyId,
    UIA_AutomationIdPropertyId,
    UIA_ClassNamePropertyId,
    UIA_IsOffscreenPropertyId,
    UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId,
    UIA_IsScrollPatternAvailablePropertyId,
    UIA_IsScrollItemPatternAvailablePropertyId,
    UIA_IsTextPatternAvailablePropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId,
    UIA_ValueValuePropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
];

/// Не разбирать больше стольких элементов дерева (огромные списки/страницы).
const MAX_SCAN: i32 = 6000;

pub(crate) fn uia_err(e: windows::core::Error) -> ToolError {
    match e.code().0 as u32 {
        UIA_E_ELEMENTNOTAVAILABLE => ToolError::Failed("элемент больше не существует — сделайте новый ui_snapshot".into()),
        UIA_E_ELEMENTNOTENABLED => ToolError::Failed("элемент недоступен (disabled)".into()),
        UIA_E_TIMEOUT => ToolError::Timeout,
        _ => ToolError::Failed(format!("UIA: {}", e.message())),
    }
}

fn b(r: windows::core::Result<BOOL>, default: bool) -> bool {
    r.map(|v| v.as_bool()).unwrap_or(default)
}

fn s(r: windows::core::Result<BSTR>) -> String {
    r.map(|v| v.to_string()).unwrap_or_default()
}

fn variant_string(v: &VARIANT) -> Option<String> {
    // SAFETY: читаем bstrVal только при vt == VT_BSTR.
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        (inner.vt == VT_BSTR).then(|| inner.Anonymous.bstrVal.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDir {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "up" | "вверх" => Some(Self::Up),
            "" | "down" | "вниз" => Some(Self::Down),
            "left" | "влево" => Some(Self::Left),
            "right" | "вправо" => Some(Self::Right),
            _ => None,
        }
    }
}

/// Итог `set_value`: через ValuePattern или элемент только сфокусирован (дальше — клавиатура).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetValueOutcome {
    Pattern,
    NeedsKeyboard,
}

pub(crate) struct UiaState {
    auto: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    elements: Vec<IUIAutomationElement>,
    infos: Vec<UiElement>,
    window: Option<WindowInfo>,
    generation: u64,
}

impl UiaState {
    pub(crate) fn new() -> windows::core::Result<Self> {
        // SAFETY: COM уже инициализирован в этом потоке (MTA).
        unsafe {
            let auto: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))?;
            if let Ok(a2) = auto.cast::<IUIAutomation2>() {
                let _ = a2.SetConnectionTimeout(5_000);
                let _ = a2.SetTransactionTimeout(10_000);
            }
            let cache = auto.CreateCacheRequest()?;
            for &p in PROPS {
                cache.AddProperty(p)?;
            }
            Ok(Self { auto, cache, elements: Vec::new(), infos: Vec::new(), window: None, generation: 0 })
        }
    }

    fn root(&self, win: &WindowInfo) -> Result<IUIAutomationElement, ToolError> {
        // SAFETY: вызов COM в потоке-владельце.
        unsafe { self.auto.ElementFromHandle(hwnd(win.hwnd)) }.map_err(uia_err)
    }

    /// Прочитать кэшированные свойства элемента.
    fn read_cached(el: &IUIAutomationElement) -> (UiElement, Kind, bool) {
        // SAFETY: только чтение кэша элемента.
        unsafe {
            let role = role_for_control_type(el.CachedControlType().map(|c| c.0).unwrap_or(0));
            let name = s(el.CachedName());
            let rect = el.CachedBoundingRectangle().map(rect_from).unwrap_or_default();
            let focusable = b(el.CachedIsKeyboardFocusable(), false);
            let flag = |p: UIA_PROPERTY_ID| {
                el.GetCachedPropertyValue(p).ok().and_then(|v| bool::try_from(&v).ok()).unwrap_or(false)
            };
            let int = |p: UIA_PROPERTY_ID| el.GetCachedPropertyValue(p).ok().and_then(|v| i32::try_from(&v).ok());
            let mut patterns: Vec<String> = Vec::new();
            for (p, n) in [
                (UIA_IsInvokePatternAvailablePropertyId, "invoke"),
                (UIA_IsValuePatternAvailablePropertyId, "value"),
                (UIA_IsTogglePatternAvailablePropertyId, "toggle"),
                (UIA_IsExpandCollapsePatternAvailablePropertyId, "expand"),
                (UIA_IsSelectionItemPatternAvailablePropertyId, "select"),
                (UIA_IsRangeValuePatternAvailablePropertyId, "range"),
                (UIA_IsScrollPatternAvailablePropertyId, "scroll"),
                (UIA_IsScrollItemPatternAvailablePropertyId, "scroll_item"),
                (UIA_IsTextPatternAvailablePropertyId, "text"),
            ] {
                if flag(p) {
                    patterns.push(n.into());
                }
            }
            let has = |n: &str| patterns.iter().any(|p| p == n);
            let has_action = ["invoke", "value", "toggle", "expand", "select", "range"].iter().any(|n| has(n));
            let value = if has("value") {
                el.GetCachedPropertyValue(UIA_ValueValuePropertyId).ok().and_then(|v| variant_string(&v)).map(|v| clip(&v, 200))
            } else {
                None
            };
            let toggle = if has("toggle") {
                int(UIA_ToggleToggleStatePropertyId).map(|t| match t {
                    0 => "off".to_string(),
                    1 => "on".to_string(),
                    _ => "indeterminate".to_string(),
                })
            } else {
                None
            };
            let selected = if has("select") { Some(flag(UIA_SelectionItemIsSelectedPropertyId)) } else { None };
            let expanded = if has("expand") {
                int(UIA_ExpandCollapseExpandCollapseStatePropertyId).and_then(|t| match t {
                    0 => Some("collapsed".to_string()),
                    1 => Some("expanded".to_string()),
                    2 => Some("partial".to_string()),
                    _ => None,
                })
            } else {
                None
            };
            let kind = classify(role, &name, rect, focusable, has_action);
            let offscreen = b(el.CachedIsOffscreen(), false);
            let info = UiElement {
                id: 0,
                role: role.into(),
                name: clip(&name, 300),
                automation_id: s(el.CachedAutomationId()),
                class_name: s(el.CachedClassName()),
                rect,
                enabled: b(el.CachedIsEnabled(), true),
                focused: b(el.CachedHasKeyboardFocus(), false),
                value,
                toggle,
                selected,
                expanded,
                patterns,
                offscreen,
            };
            (info, kind, offscreen)
        }
    }

    fn find_all(&self, root: &IUIAutomationElement, only_visible: bool) -> Result<Vec<IUIAutomationElement>, ToolError> {
        // SAFETY: вызовы COM в потоке-владельце.
        unsafe {
            let control = self.auto.ControlViewCondition().map_err(uia_err)?;
            let cond = if only_visible {
                let visible = self.auto.CreatePropertyCondition(UIA_IsOffscreenPropertyId, &VARIANT::from(false)).map_err(uia_err)?;
                self.auto.CreateAndCondition(&control, &visible).map_err(uia_err)?
            } else {
                control
            };
            let arr = root.FindAllBuildCache(TreeScope_Descendants, &cond, &self.cache).map_err(uia_err)?;
            let n = arr.Length().unwrap_or(0).min(MAX_SCAN);
            Ok((0..n).filter_map(|i| arr.GetElement(i).ok()).collect())
        }
    }

    fn snapshot_value(&self) -> UiSnapshot {
        UiSnapshot {
            generation: self.generation,
            window: self.window.clone().expect("окно снимка"),
            elements: self.infos.clone(),
            total_candidates: self.infos.len(),
            truncated: false,
        }
    }

    /// Снимок интерактивных элементов окна. Заменяет кэш id.
    pub(crate) fn snapshot(&mut self, win: WindowInfo, limit: usize, include_text: bool) -> Result<UiSnapshot, ToolError> {
        let root = self.root(&win)?;
        let all = self.find_all(&root, true)?;
        let read: Vec<(UiElement, Kind, bool)> = all.iter().map(Self::read_cached).collect();
        let kinds: Vec<Kind> = read.iter().map(|r| r.1).collect();
        let candidates = kinds.iter().filter(|k| **k != Kind::Skip && (include_text || **k == Kind::Interactive)).count();
        let picked = select(&kinds, limit, if include_text { MAX_TEXT_ELEMENTS } else { 0 });
        self.generation += 1;
        self.elements = picked.iter().map(|&i| all[i].clone()).collect();
        self.infos = picked
            .iter()
            .enumerate()
            .map(|(n, &i)| UiElement { id: n as u32 + 1, ..read[i].0.clone() })
            .collect();
        self.window = Some(win);
        let mut snap = self.snapshot_value();
        snap.total_candidates = candidates;
        snap.truncated = candidates > snap.elements.len();
        Ok(snap)
    }

    /// Поиск по имени (подстрока, без регистра) и роли. Найденное добавляется в кэш с новыми id.
    pub(crate) fn find(
        &mut self,
        win: WindowInfo,
        query: &str,
        role: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<UiElement>, UiSnapshot), ToolError> {
        let root = self.root(&win)?;
        let all = self.find_all(&root, false)?;
        let q = fold(query.trim());
        let role = role.map(normalize_role).filter(|r| !r.is_empty());
        let mut hits: Vec<(u8, usize, UiElement)> = Vec::new();
        for (i, el) in all.iter().enumerate() {
            let (info, _, offscreen) = Self::read_cached(el);
            if info.rect.w <= 0 && !offscreen {
                continue;
            }
            if let Some(r) = &role {
                if &info.role != r {
                    continue;
                }
            }
            let name = fold(&info.name);
            let exact = !q.is_empty() && (name == q || fold(&info.automation_id) == q);
            if !q.is_empty() && !exact && !name.contains(&q) {
                continue;
            }
            let rank = match (exact, offscreen) {
                (true, false) => 0,
                (false, false) => 1,
                (true, true) => 2,
                (false, true) => 3,
            };
            hits.push((rank, i, info));
        }
        hits.sort_by_key(|h| (h.0, h.1));
        hits.truncate(limit);
        let same_window = self.window.as_ref().is_some_and(|w| w.hwnd == win.hwnd);
        if !same_window {
            self.generation += 1;
            self.elements.clear();
            self.infos.clear();
            self.window = Some(win);
        }
        let mut found = Vec::new();
        for (_, i, info) in hits {
            // Уже есть в кэше (тот же прямоугольник, роль и имя) — возвращаем существующий id.
            if let Some(existing) =
                self.infos.iter().find(|e| e.rect == info.rect && e.role == info.role && e.name == info.name)
            {
                found.push(existing.clone());
                continue;
            }
            let el = UiElement { id: self.infos.len() as u32 + 1, ..info };
            self.elements.push(all[i].clone());
            self.infos.push(el.clone());
            found.push(el);
        }
        Ok((found, self.snapshot_value()))
    }

    fn element(&self, id: u32) -> Result<&IUIAutomationElement, ToolError> {
        id.checked_sub(1)
            .and_then(|i| self.elements.get(i as usize))
            .ok_or_else(|| ToolError::InvalidArgs(format!("нет элемента [{id}] в последнем снимке — сделайте ui_snapshot")))
    }

    /// Текущий прямоугольник, имя и роль элемента (для подсветки перед действием).
    pub(crate) fn describe(&self, id: u32) -> Result<(Rect, String, String), ToolError> {
        let el = self.element(id)?;
        let info = &self.infos[(id - 1) as usize];
        // SAFETY: чтение свойства.
        let rect = unsafe { el.CurrentBoundingRectangle() }.map(rect_from).map_err(uia_err)?;
        Ok((rect, info.name.clone(), info.role.clone()))
    }

    fn pattern<T: Interface>(el: &IUIAutomationElement, id: UIA_PATTERN_ID) -> Option<T> {
        // SAFETY: запрос паттерна; null → None.
        unsafe { el.GetCurrentPatternAs::<T>(id) }.ok()
    }

    /// «Нажать» элемент: Invoke → Toggle → Select → Expand/Collapse → LegacyIAccessible.DoDefaultAction.
    pub(crate) fn invoke(&self, id: u32) -> Result<&'static str, ToolError> {
        let el = self.element(id)?;
        // SAFETY: вызовы паттернов UIA в потоке-владельце.
        unsafe {
            if let Some(p) = Self::pattern::<IUIAutomationInvokePattern>(el, UIA_InvokePatternId) {
                return p.Invoke().map(|_| "invoke").map_err(uia_err);
            }
            if let Some(p) = Self::pattern::<IUIAutomationTogglePattern>(el, UIA_TogglePatternId) {
                return p.Toggle().map(|_| "toggle").map_err(uia_err);
            }
            if let Some(p) = Self::pattern::<IUIAutomationSelectionItemPattern>(el, UIA_SelectionItemPatternId) {
                return p.Select().map(|_| "select").map_err(uia_err);
            }
            if let Some(p) = Self::pattern::<IUIAutomationExpandCollapsePattern>(el, UIA_ExpandCollapsePatternId) {
                let collapsed = p.CurrentExpandCollapseState().map(|s| s == ExpandCollapseState_Collapsed).unwrap_or(true);
                return if collapsed { p.Expand().map(|_| "expand") } else { p.Collapse().map(|_| "collapse") }.map_err(uia_err);
            }
            if let Some(p) = Self::pattern::<IUIAutomationLegacyIAccessiblePattern>(el, UIA_LegacyIAccessiblePatternId) {
                if p.DoDefaultAction().is_ok() {
                    return Ok("default_action");
                }
            }
        }
        Err(ToolError::Unsupported(format!("элемент [{id}] не поддерживает Invoke — используйте mouse_click с id={id}")))
    }

    pub(crate) fn set_value(&self, id: u32, text: &str) -> Result<SetValueOutcome, ToolError> {
        let el = self.element(id)?;
        // SAFETY: вызовы паттернов UIA в потоке-владельце.
        unsafe {
            if let Some(p) = Self::pattern::<IUIAutomationValuePattern>(el, UIA_ValuePatternId) {
                if b(p.CurrentIsReadOnly(), false) {
                    return Err(ToolError::Failed(format!("поле [{id}] только для чтения")));
                }
                if p.SetValue(&BSTR::from(text)).is_ok() {
                    return Ok(SetValueOutcome::Pattern);
                }
            }
            el.SetFocus().map_err(uia_err)?;
        }
        Ok(SetValueOutcome::NeedsKeyboard)
    }

    pub(crate) fn toggle(&self, id: u32) -> Result<String, ToolError> {
        let el = self.element(id)?;
        let p = Self::pattern::<IUIAutomationTogglePattern>(el, UIA_TogglePatternId)
            .ok_or_else(|| ToolError::Unsupported(format!("элемент [{id}] не переключается (нет TogglePattern)")))?;
        // SAFETY: вызовы паттерна.
        unsafe {
            p.Toggle().map_err(uia_err)?;
            Ok(match p.CurrentToggleState() {
                Ok(s) if s == ToggleState_On => "on".into(),
                Ok(s) if s == ToggleState_Off => "off".into(),
                Ok(_) => "indeterminate".into(),
                Err(_) => "unknown".into(),
            })
        }
    }

    pub(crate) fn expand(&self, id: u32, expand: bool) -> Result<String, ToolError> {
        let el = self.element(id)?;
        let p = Self::pattern::<IUIAutomationExpandCollapsePattern>(el, UIA_ExpandCollapsePatternId)
            .ok_or_else(|| ToolError::Unsupported(format!("элемент [{id}] не раскрывается (нет ExpandCollapsePattern)")))?;
        // SAFETY: вызовы паттерна.
        unsafe {
            if expand { p.Expand() } else { p.Collapse() }.map_err(uia_err)?;
            Ok(match p.CurrentExpandCollapseState() {
                Ok(s) if s == ExpandCollapseState_Expanded => "expanded".into(),
                Ok(s) if s == ExpandCollapseState_Collapsed => "collapsed".into(),
                Ok(_) => "partial".into(),
                Err(_) => "unknown".into(),
            })
        }
    }

    pub(crate) fn select_item(&self, id: u32) -> Result<(), ToolError> {
        let el = self.element(id)?;
        let p = Self::pattern::<IUIAutomationSelectionItemPattern>(el, UIA_SelectionItemPatternId)
            .ok_or_else(|| ToolError::Unsupported(format!("элемент [{id}] нельзя выбрать (нет SelectionItemPattern)")))?;
        // SAFETY: вызов паттерна.
        unsafe { p.Select() }.map_err(uia_err)
    }

    /// Прокрутить: элемент или ближайшего прокручиваемого предка; без id — самую большую прокручиваемую область окна.
    pub(crate) fn scroll(
        &self,
        id: Option<u32>,
        win: Option<WindowInfo>,
        dir: ScrollDir,
        page: bool,
        count: u32,
        into_view: bool,
    ) -> Result<String, ToolError> {
        // SAFETY: вызовы COM в потоке-владельце.
        unsafe {
            if let (Some(id), true) = (id, into_view) {
                let el = self.element(id)?;
                let p = Self::pattern::<IUIAutomationScrollItemPattern>(el, UIA_ScrollItemPatternId)
                    .ok_or_else(|| ToolError::Unsupported(format!("элемент [{id}] не умеет ScrollIntoView")))?;
                p.ScrollIntoView().map_err(uia_err)?;
                return Ok(format!("элемент [{id}] прокручен в видимую область"));
            }
            let target = match id {
                Some(id) => {
                    let walker = self.auto.ControlViewWalker().map_err(uia_err)?;
                    let mut cur = self.element(id)?.clone();
                    let mut found = None;
                    for _ in 0..16 {
                        if Self::pattern::<IUIAutomationScrollPattern>(&cur, UIA_ScrollPatternId).is_some() {
                            found = Some(cur.clone());
                            break;
                        }
                        match walker.GetParentElement(&cur) {
                            Ok(p) => cur = p,
                            Err(_) => break,
                        }
                    }
                    found.ok_or_else(|| ToolError::Unsupported(format!("ни [{id}], ни его родители не прокручиваются")))?
                }
                None => {
                    let win = win.ok_or_else(|| ToolError::InvalidArgs("нужен id или окно".into()))?;
                    let root = self.root(&win)?;
                    let cond = self
                        .auto
                        .CreatePropertyCondition(UIA_IsScrollPatternAvailablePropertyId, &VARIANT::from(true))
                        .map_err(uia_err)?;
                    let arr = root.FindAllBuildCache(TreeScope_Subtree, &cond, &self.cache).map_err(uia_err)?;
                    let n = arr.Length().unwrap_or(0);
                    (0..n)
                        .filter_map(|i| arr.GetElement(i).ok())
                        .filter(|e| !b(e.CachedIsOffscreen(), false))
                        .max_by_key(|e| {
                            let r = e.CachedBoundingRectangle().map(rect_from).unwrap_or_default();
                            r.w as i64 * r.h as i64
                        })
                        .ok_or_else(|| ToolError::Unsupported("в окне нет прокручиваемых областей UIA — используйте mouse_scroll".into()))?
                }
            };
            let p = Self::pattern::<IUIAutomationScrollPattern>(&target, UIA_ScrollPatternId)
                .ok_or_else(|| ToolError::Unsupported("ScrollPattern недоступен".into()))?;
            let (inc, dec) = if page {
                (ScrollAmount_LargeIncrement, ScrollAmount_LargeDecrement)
            } else {
                (ScrollAmount_SmallIncrement, ScrollAmount_SmallDecrement)
            };
            let (h, v) = match dir {
                ScrollDir::Up => (ScrollAmount_NoAmount, dec),
                ScrollDir::Down => (ScrollAmount_NoAmount, inc),
                ScrollDir::Left => (dec, ScrollAmount_NoAmount),
                ScrollDir::Right => (inc, ScrollAmount_NoAmount),
            };
            for _ in 0..count.max(1) {
                if let Err(e) = p.Scroll(h, v) {
                    // упёрлись в край — не ошибка
                    if e.code().0 as u32 == UIA_E_INVALIDOPERATION {
                        break;
                    }
                    return Err(uia_err(e));
                }
            }
            let vp = p.CurrentVerticalScrollPercent().unwrap_or(-1.0);
            let hp = p.CurrentHorizontalScrollPercent().unwrap_or(-1.0);
            let fmt = |x: f64| if x < 0.0 { "—".to_string() } else { format!("{x:.0}%") };
            Ok(format!("прокрутка: по вертикали {}, по горизонтали {}", fmt(vp), fmt(hp)))
        }
    }

    /// Текст окна терминала через TextPattern: видимая область + `scrollback` строк выше неё.
    pub(crate) fn terminal_text(&self, win: &WindowInfo, scrollback: i32) -> Result<String, ToolError> {
        let root = self.root(win)?;
        // SAFETY: вызовы COM в потоке-владельце.
        unsafe {
            let text_el = if Self::pattern::<IUIAutomationTextPattern>(&root, UIA_TextPatternId).is_some() {
                root.clone()
            } else {
                let has_text = self
                    .auto
                    .CreatePropertyCondition(UIA_IsTextPatternAvailablePropertyId, &VARIANT::from(true))
                    .map_err(uia_err)?;
                let arr = root.FindAllBuildCache(TreeScope_Descendants, &has_text, &self.cache).map_err(uia_err)?;
                let n = arr.Length().unwrap_or(0);
                let els: Vec<IUIAutomationElement> = (0..n).filter_map(|i| arr.GetElement(i).ok()).collect();
                let visible: Vec<&IUIAutomationElement> = els.iter().filter(|e| !b(e.CachedIsOffscreen(), false)).collect();
                visible
                    .iter()
                    .find(|e| b(e.CachedHasKeyboardFocus(), false))
                    .or_else(|| {
                        visible.iter().max_by_key(|e| {
                            let r = e.CachedBoundingRectangle().map(rect_from).unwrap_or_default();
                            r.w as i64 * r.h as i64
                        })
                    })
                    .map(|e| (*e).clone())
                    .ok_or_else(|| ToolError::Unsupported("окно не отдаёт текст через UIA TextPattern".into()))?
            };
            let tp: IUIAutomationTextPattern = text_el.GetCurrentPatternAs(UIA_TextPatternId).map_err(uia_err)?;
            let range = match tp.GetVisibleRanges() {
                Ok(arr) if arr.Length().unwrap_or(0) > 0 => {
                    let n = arr.Length().unwrap_or(1);
                    let first = arr.GetElement(0).map_err(uia_err)?.Clone().map_err(uia_err)?;
                    if n > 1 {
                        if let Ok(last) = arr.GetElement(n - 1) {
                            let _ = first.MoveEndpointByRange(TextPatternRangeEndpoint_End, &last, TextPatternRangeEndpoint_End);
                        }
                    }
                    first
                }
                _ => {
                    // Нет видимых диапазонов: берём хвост документа.
                    let doc = tp.DocumentRange().map_err(uia_err)?;
                    let _ = doc.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &doc, TextPatternRangeEndpoint_End);
                    doc
                }
            };
            if scrollback > 0 {
                let _ = range.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Line, -scrollback);
            }
            let text = range.GetText(400_000).map_err(uia_err)?.to_string();
            Ok(text)
        }
    }
}
