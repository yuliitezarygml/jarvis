use jarvis_core::{RiskTier, ToolCtx, ToolError, ToolRegistry};
use serde_json::json;

use super::temp_mem;
use crate::*;

fn registry(mem: &Memory) -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.extend(memory_tools(mem.clone()));
    r
}

#[test]
fn tool_specs_and_tiers() {
    let (_d, mem) = temp_mem();
    let r = registry(&mem);
    let specs = r.specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["memory_forget", "memory_recall", "memory_remember"]);
    assert_eq!(r.get("memory_remember").unwrap().spec().tier, RiskTier::T0);
    assert_eq!(r.get("memory_recall").unwrap().spec().tier, RiskTier::T0);
    assert_eq!(r.get("memory_forget").unwrap().spec().tier, RiskTier::T2);
    for s in &specs {
        assert_eq!(s.parameters["type"], "object");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remember_recall_forget_flow() {
    let (_d, mem) = temp_mem();
    let r = registry(&mem);
    let ctx = ToolCtx::for_tests();
    let remember = r.get("memory_remember").unwrap();
    let recall = r.get("memory_recall").unwrap();
    let forget = r.get("memory_forget").unwrap();

    let out = remember
        .call(json!({"text": "Я пью чёрный кофе", "category": "preference", "important": true}), &ctx)
        .await
        .unwrap();
    assert!(out.content.starts_with("Запомнил"), "{}", out.content);
    let out = remember.call(json!({"text": "я пью черный кофе"}), &ctx).await.unwrap();
    assert!(out.content.starts_with("Обновил"), "{}", out.content);
    remember.call(json!({"text": "Кошку зовут Мурка"}), &ctx).await.unwrap();
    let f = &mem.search_facts("кофе", 5).unwrap()[0];
    assert_eq!(f.source, FactSource::Explicit);
    assert!(f.important);

    // («пил» ↔ «пью» — разные основы, стеммер такие формы не связывает; «кофе» — связывает)
    let out = recall.call(json!({"query": "какой кофе я пью?"}), &ctx).await.unwrap();
    // текст — от последней явной формулировки («черный» без ё)
    assert!(out.content.contains("я пью черный кофе"), "{}", out.content);
    assert!(!out.untrusted);
    let out = recall.call(json!({"query": "погода на Марсе"}), &ctx).await.unwrap();
    assert!(out.content.starts_with("В памяти ничего не нашлось"));

    // неуверенное совпадение — ничего не удаляется, возвращаются кандидаты
    let out = forget.call(json!({"query": "кофе и погода в Москве"}), &ctx).await.unwrap();
    assert!(out.content.contains("ничего не удалено"), "{}", out.content);
    assert!(out.content.contains("черный кофе"), "кандидаты перечислены: {}", out.content);
    let out = forget.call(json!({"query": "квантовая физика"}), &ctx).await.unwrap();
    assert!(out.content.starts_with("В памяти нет ничего"), "{}", out.content);
    assert_eq!(mem.stats().unwrap().facts, 2);
    // уверенное — удаляется
    let out = forget.call(json!({"query": "что я пью кофе"}), &ctx).await.unwrap();
    assert!(out.content.starts_with("Забыл"), "{}", out.content);
    assert!(mem.search_facts("кофе", 5).unwrap().is_empty());
    assert_eq!(mem.stats().unwrap().facts, 1);

    // по id и виду
    let lesson = mem.add_lesson(lesson_stub_from_tool_error("ui_click", None, "boom")).unwrap().id;
    let out = forget.call(json!({"id": lesson, "kind": "lesson"}), &ctx).await.unwrap();
    assert!(out.content.contains(&format!("#{lesson}")));
    assert!(mem.get_lesson(lesson).unwrap().is_none());
    let out = forget.call(json!({"id": 12345}), &ctx).await.unwrap();
    assert!(out.content.contains("нет"));

    // неверные аргументы
    assert!(matches!(remember.call(json!({}), &ctx).await, Err(ToolError::InvalidArgs(_))));
    assert!(matches!(recall.call(json!({"query": "  "}), &ctx).await, Err(ToolError::InvalidArgs(_))));
    assert!(matches!(forget.call(json!({}), &ctx).await, Err(ToolError::InvalidArgs(_))));
    assert!(matches!(forget.call(json!({"id": 1, "kind": "x"}), &ctx).await, Err(ToolError::InvalidArgs(_))));
}
