//! Набор инструментов: имена уникальны, схемы корректны, уровни риска как в плане.

mod common;

use jarvis_core::{RiskTier, ToolRegistry};
use std::collections::BTreeMap;

#[test]
fn all_tools_registered_with_expected_tiers() {
    let tk = common::toolkit("registry");
    let mut reg = ToolRegistry::new();
    reg.extend(tk.tools()); // паникует на дубликатах
    let tiers: BTreeMap<String, RiskTier> = reg.specs().into_iter().map(|s| (s.name, s.tier)).collect();
    use RiskTier::*;
    let expected = [
        ("mouse_move", T1),
        ("mouse_click", T1),
        ("mouse_drag", T1),
        ("mouse_scroll", T1),
        ("key_type", T1),
        ("key_hotkey", T1),
        ("media_key", T1),
        ("ui_snapshot", T0),
        ("ui_find", T0),
        ("ui_window_list", T0),
        ("ui_invoke", T1),
        ("ui_set_value", T1),
        ("ui_toggle", T1),
        ("ui_expand", T1),
        ("ui_select", T1),
        ("ui_scroll", T1),
        ("ui_focus_window", T1),
        ("screen_capture", T0),
        ("screen_marks", T0),
        ("screen_ocr", T0),
        ("terminal_context", T0),
        ("terminal_run", T1),
        ("terminal_type", T1),
        ("shell_hook_install", T2),
        ("shell_hook_uninstall", T2),
    ];
    assert_eq!(tiers.len(), expected.len(), "{tiers:?}");
    for (name, tier) in expected {
        assert_eq!(tiers.get(name), Some(&tier), "{name}");
    }
}

#[test]
fn schemas_are_objects_with_descriptions() {
    for t in common::toolkit("schemas").tools() {
        let s = t.spec();
        assert_eq!(s.parameters["type"], "object", "{}", s.name);
        assert!(s.parameters["properties"].is_object(), "{}", s.name);
        assert!(s.description.chars().count() > 20, "{}", s.name);
        assert!(s.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}", s.name);
        if let Some(req) = s.parameters.get("required") {
            for r in req.as_array().unwrap() {
                assert!(s.parameters["properties"].get(r.as_str().unwrap()).is_some(), "{}: {r}", s.name);
            }
        }
    }
}
