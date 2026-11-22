//! Ленивая загрузка инструментов: роутер, `tools_load`, вызов неактивного инструмента, «липкие» группы.

mod common;

use std::sync::Arc;

use common::*;
use jarvis_agent::{AgentConfig, StepStatus, StopReason, TOOLS_LOAD, TOOLS_LOAD_NOTE};
use jarvis_core::{CancellationToken, ChatRequest, Event, RiskTier};
use serde_json::json;

fn token() -> CancellationToken {
    CancellationToken::new()
}

/// Набор с настоящими именами из разных групп.
fn tools() -> Vec<Arc<FakeTool>> {
    vec![
        FakeTool::ok("sys_snapshot", RiskTier::T0),
        FakeTool::ok("web_search", RiskTier::T0),
        FakeTool::ok("mouse_click", RiskTier::T1),
        FakeTool::ok("key_type", RiskTier::T1),
        FakeTool::ok("ui_find", RiskTier::T0),
        FakeTool::ok("screen_capture", RiskTier::T0),
        FakeTool::ok("web_open", RiskTier::T0),
        FakeTool::ok("service_stop", RiskTier::T3),
    ]
}

fn names(req: &ChatRequest) -> Vec<String> {
    req.tools.iter().map(|t| t.name.clone()).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn hello_sends_only_core_and_tools_load() {
    let model = ScriptModel::new(vec![text("Добрый день, сэр.")]);
    let h = default_harness(model.clone(), tools());
    h.agent.run("привет", token()).await;

    let req = model.request(0);
    assert_eq!(names(&req), ["sys_snapshot", "web_search", TOOLS_LOAD]);
    let load = req.tools.iter().find(|t| t.name == TOOLS_LOAD).unwrap();
    assert_eq!(load.tier, RiskTier::T0);
    let groups = load.parameters["properties"]["groups"]["items"]["enum"].clone();
    assert_eq!(groups, json!(["pc_input", "ui", "screen", "system", "web"]));
    assert!(req.messages[0].content.contains(TOOLS_LOAD_NOTE), "подсказка в системном промпте");
}

#[tokio::test(flavor = "current_thread")]
async fn router_adds_groups_by_words() {
    let model = ScriptModel::new(vec![text("Готово.")]);
    let h = default_harness(model.clone(), tools());
    h.agent.run("кликни по кнопке ОК", token()).await;
    let n = names(&model.request(0));
    for t in ["sys_snapshot", "mouse_click", "key_type", "ui_find", TOOLS_LOAD] {
        assert!(n.contains(&t.to_string()), "нет {t}: {n:?}");
    }
    assert!(!n.contains(&"screen_capture".to_string()) && !n.contains(&"web_open".to_string()), "{n:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn tools_load_activates_groups_for_rest_of_run() {
    let model = ScriptModel::new(vec![
        call(TOOLS_LOAD, json!({"groups": ["screen"]})),
        call("screen_capture", json!({})),
        text("На экране рабочий стол, сэр."),
    ]);
    let h = default_harness(model.clone(), tools());
    let out = h.agent.run("опиши обстановку", token()).await;

    assert_eq!(out.stop_reason, StopReason::Completed);
    assert!(!names(&model.request(0)).contains(&"screen_capture".to_string()));
    let r1 = model.request(1);
    assert!(names(&r1).contains(&"screen_capture".to_string()));
    let reply = &tool_messages(&r1)[0].content;
    assert!(reply.contains("Подключено: screen (screen_capture)"), "{reply}");
    // Мета-вызов — не действие: нет в шагах протокола, ленте HUD, без подтверждения и свечения.
    let steps: Vec<_> = out.transcript.steps.iter().map(|s| s.tool.as_str()).collect();
    assert_eq!(steps, ["screen_capture"]);
    assert!(h.confirmer.asked().is_empty());
    let ev = h.sink.take();
    assert!(!action_statuses(&ev).iter().any(|(t, _)| t == TOOLS_LOAD));
    assert!(!ev.iter().any(|e| matches!(e, Event::ControlStart { .. })));
    assert!(out.transcript.notes.iter().any(|n| n.contains("tools_load: screen")));
}

#[tokio::test(flavor = "current_thread")]
async fn tools_load_reports_unknown_and_already_loaded_groups() {
    let model = ScriptModel::new(vec![
        call(TOOLS_LOAD, json!({"groups": ["core", "nope", "memory"]})),
        text("Ок."),
    ]);
    let h = default_harness(model.clone(), tools());
    h.agent.run("привет", token()).await;
    let reply = &tool_messages(&model.request(1))[0].content;
    assert!(reply.contains("Уже подключены: core"), "{reply}");
    // memory — существующая группа, но в этом реестре у неё нет инструментов.
    assert!(reply.contains("Нет таких групп: nope, memory"), "{reply}");
    assert!(reply.contains("Можно подключить: pc_input, ui, screen, system, web"), "{reply}");
}

#[tokio::test(flavor = "current_thread")]
async fn inactive_real_tool_runs_and_activates_its_group() {
    let model = ScriptModel::new(vec![call("mouse_click", json!({"x": 1, "y": 2})), text("Нажал, сэр.")]);
    let ts = tools();
    let click = ts[2].clone();
    let h = default_harness(model.clone(), ts);
    let out = h.agent.run("привет", token()).await;

    assert!(!names(&model.request(0)).contains(&"mouse_click".to_string()));
    assert_eq!(click.calls(), 1, "не отказ, а выполнение");
    assert_eq!(out.transcript.steps[0].status, StepStatus::Done);
    let n = names(&model.request(1));
    assert!(n.contains(&"mouse_click".to_string()) && n.contains(&"key_type".to_string()), "{n:?}");
    assert!(!n.contains(&"ui_find".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn inactive_dangerous_tool_still_needs_confirmation() {
    let model = ScriptModel::new(vec![call("service_stop", json!({"name": "Spooler"})), text("Не буду, сэр.")]);
    let ts = tools();
    let stop = ts[7].clone();
    let h = default_harness(model.clone(), ts); // confirmer отвечает «нет»
    let out = h.agent.run("привет", token()).await;
    assert_eq!(stop.calls(), 0);
    assert_eq!(out.transcript.steps[0].status, StepStatus::Denied);
    assert_eq!(h.confirmer.asked().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn groups_used_in_previous_turn_stay_for_next_turn_only() {
    let model = ScriptModel::new(vec![
        call("mouse_click", json!({})),
        text("Нажал."),
        text("Повторяю."), // 2-й ход: pc_input ещё подключён
        text("Пожалуйста."), // 3-й ход: в прошлом ходе инструментов не было
    ]);
    let h = default_harness(model.clone(), tools());
    h.agent.run("привет", token()).await;
    h.agent.run("а теперь ещё раз", token()).await;
    assert!(names(&model.request(2)).contains(&"mouse_click".to_string()));
    h.agent.run("спасибо", token()).await;
    assert!(!names(&model.request(3)).contains(&"mouse_click".to_string()));

    // «новый разговор» тоже сбрасывает
    let model = ScriptModel::new(vec![call("mouse_click", json!({})), text("Нажал."), text("Ок.")]);
    let h = default_harness(model.clone(), tools());
    h.agent.run("привет", token()).await;
    h.agent.clear_history();
    h.agent.run("ещё", token()).await;
    assert!(!names(&model.request(2)).contains(&"mouse_click".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn lazy_tools_off_sends_whole_registry() {
    let model = ScriptModel::new(vec![call(TOOLS_LOAD, json!({"groups": ["ui"]})), text("Ок.")]);
    let config = AgentConfig { lazy_tools: false, ..Default::default() };
    let h = harness(model.clone(), tools(), Opts { config, ..Default::default() });
    let out = h.agent.run("привет", token()).await;
    let req = model.request(0);
    assert_eq!(req.tools.len(), 8);
    assert!(!names(&req).contains(&TOOLS_LOAD.to_string()));
    assert!(!req.messages[0].content.contains(TOOLS_LOAD_NOTE));
    // без ленивой загрузки tools_load — просто неизвестный инструмент
    assert_eq!(out.transcript.steps[0].status, StepStatus::UnknownTool);
}

#[tokio::test(flavor = "current_thread")]
async fn default_request_has_completion_budget() {
    let model = ScriptModel::new(vec![text("Да.")]);
    let h = default_harness(model.clone(), tools());
    h.agent.run("привет", token()).await;
    assert_eq!(model.request(0).max_tokens, Some(jarvis_agent::DEFAULT_MAX_TOKENS));
}
