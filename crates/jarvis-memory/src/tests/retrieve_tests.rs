use serde_json::json;

use super::temp_mem;
use crate::*;

fn pb(name: &str, when: &str, tools: &[&str]) -> NewPlaybook {
    NewPlaybook {
        name: name.into(),
        when: when.into(),
        steps: tools.iter().map(|t| PlaybookStep::new(*t, json!({"target": "{{name}}"}))).collect(),
        params_schema: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
    }
}

fn tool_lesson(tool: &str, trigger: &str, fix: &str) -> NewLesson {
    NewLesson {
        scope: LessonScope::Tool,
        trigger: trigger.into(),
        mistake: format!("{trigger}: не сработало"),
        fix: fix.into(),
        tool: Some(tool.into()),
        ..Default::default()
    }
}

#[test]
fn retrieve_context_picks_facts_lessons_and_playbooks() {
    let (_d, mem) = temp_mem();
    let sir = mem.upsert_fact(NewFact::explicit("Обращаться к пользователю «сэр»").with_category("preference")).unwrap().id;
    let coffee = mem.upsert_fact(NewFact::explicit("Пьёт чёрный кофе без сахара")).unwrap().id;
    mem.upsert_fact(NewFact::explicit("Кошку зовут Мурка")).unwrap();

    let mut ps_ids = Vec::new();
    for (i, t) in ["удаление каталога", "запуск скрипта", "чтение журнала событий", "остановка процесса", "архивация файлов", "очистка корзины"]
        .iter()
        .enumerate()
    {
        let l = tool_lesson("powershell_run", t, &format!("исправление {i}"));
        ps_ids.push(mem.add_lesson(l).unwrap().id);
    }
    let chrome = mem
        .add_lesson(NewLesson {
            scope: LessonScope::App,
            app: Some("Chrome".into()),
            trigger: "открыть вкладку".into(),
            mistake: "Ctrl+N открывает окно".into(),
            fix: "Ctrl+T".into(),
            ..Default::default()
        })
        .unwrap()
        .id;
    let coffee_lesson = mem
        .add_lesson(NewLesson {
            trigger: "заказ кофе в приложении доставки".into(),
            mistake: "выбрал капучино".into(),
            fix: "пользователь пьёт только чёрный кофе".into(),
            ..Default::default()
        })
        .unwrap()
        .id;
    for (name, when) in [
        ("Заказать кофе через Яндекс Еду", "хочет кофе с доставкой"),
        ("Заказать кофе в Старбакс", "просит кофе из кофейни"),
        ("Включить кофемашину", "хочет сварить кофе дома"),
        ("Найти рецепт кофе", "спрашивает как приготовить кофе"),
        ("Купить зёрна кофе", "кончился кофе"),
    ] {
        assert!(mem.add_playbook(pb(name, when, &["open_target", "ui_click"])).unwrap().is_new(), "{name}");
    }
    mem.add_playbook(pb("Открыть настройки Bluetooth", "просит блютуз", &["open_target"])).unwrap();

    // запрос про кофе без инструмента
    let ctx = mem.retrieve_context("закажи мне кофе", None, None).unwrap();
    let fact_ids: Vec<i64> = ctx.facts.iter().map(|f| f.id).collect();
    assert!(fact_ids.contains(&sir), "предпочтения всегда в контексте: {fact_ids:?}");
    assert!(fact_ids.contains(&coffee));
    assert_eq!(fact_ids[0], sir, "важное — первым");
    assert!(ctx.lessons.iter().any(|l| l.id == coffee_lesson));
    assert!(ctx.lessons.len() <= 5);
    assert_eq!(ctx.playbooks.len(), 3);
    assert!(ctx.playbooks.iter().all(|p| p.name.contains("коф") || p.when.contains("коф")), "{:?}", ctx.playbooks);
    assert!(ctx.playbooks.iter().any(|p| p.name.starts_with("Заказать кофе")));

    // по инструменту: уроки этого инструмента вперёд, k = 5
    let ctx = mem.retrieve_context("сделай что-нибудь", Some("powershell_run"), None).unwrap();
    assert_eq!(ctx.lessons.len(), 5);
    assert!(ctx.lessons.iter().all(|l| l.tool.as_deref() == Some("powershell_run")));
    // по приложению (регистр и кириллица не важны)
    let ctx = mem.retrieve_context("", None, Some("CHROME")).unwrap();
    assert_eq!(ctx.lessons.first().map(|l| l.id), Some(chrome));
    assert!(ctx.playbooks.is_empty());
    // ключевые слова урока
    let ctx = mem.retrieve_context("очисти корзину", None, None).unwrap();
    assert_eq!(ctx.lessons.first().map(|l| l.id), Some(ps_ids[5]));

    // mark_used
    let ctx = mem.retrieve_context("кофе", None, None).unwrap();
    mem.mark_used(&ctx.refs()).unwrap();
    let f = mem.get_fact(coffee).unwrap().unwrap();
    assert_eq!(f.use_count, 1);
    assert!(f.last_used.is_some());
    assert!(mem.get_playbook(ctx.playbooks[0].id).unwrap().unwrap().last_used.is_some());
}

#[test]
fn acceptance_remembered_fact_survives_restart() {
    // Этап 5: «Запомни, что я пью чёрный кофе» → после перезапуска знает
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let mem = Memory::open(&path).unwrap();
        mem.upsert_fact(NewFact::explicit("я пью чёрный кофе")).unwrap();
    }
    let mem = Memory::open(&path).unwrap();
    let ctx = mem.retrieve_context("что я пью по утрам?", None, None).unwrap();
    assert!(ctx.facts.iter().any(|f| f.text.contains("чёрный кофе")));
    let block = ctx.render(DEFAULT_BUDGET_TOKENS);
    assert!(block.starts_with("Память:\nФакты о пользователе:\n- я пью чёрный кофе"), "{block}");
}

fn big_context(mem: &Memory) -> MemoryContext {
    for topic in ["кофе", "музыка", "погода", "работа", "спорт", "кино"] {
        let s = mem
            .upsert_fact(
                NewFact::explicit(format!("Предпочтение про {topic}: {}", format!("{topic} ").repeat(50)))
                    .with_category("preference")
                    .with_important(true),
            )
            .unwrap();
        assert!(s.is_new());
    }
    for i in 0..5 {
        mem.add_lesson(NewLesson {
            scope: LessonScope::Tool,
            trigger: format!("ситуация {} {}", i, "с подробностями ".repeat(15)),
            mistake: "ошибка ".repeat(20),
            cause: "причина ".repeat(20),
            fix: "исправление ".repeat(20),
            tool: Some(format!("tool_{i}")),
            ..Default::default()
        })
        .unwrap();
    }
    MemoryContext {
        facts: mem.list_facts(100, 0).unwrap(),
        lessons: mem.list_lessons(100, 0).unwrap(),
        playbooks: (0..3)
            .map(|i| {
                let s = mem.add_playbook(pb(&format!("Сценарий {i}"), &"условие ".repeat(30), &["a_tool", "b_tool", "c_tool"])).unwrap();
                mem.get_playbook(s.id).unwrap().unwrap()
            })
            .collect(),
    }
}

#[test]
fn render_respects_budget_and_priorities() {
    let (_d, mem) = temp_mem();
    let ctx = big_context(&mem);
    assert_eq!(ctx.facts.len(), 6);
    for budget in [0usize, 3, 10, 25, 60, 100, 200, 400, 800, 1500, 5000] {
        let out = ctx.render(budget);
        assert!(estimate_tokens(&out) <= budget, "бюджет {budget}: {} токенов", estimate_tokens(&out));
        if !out.is_empty() {
            assert!(out.starts_with("Память:\n"), "{out}");
            // никаких «висящих» заголовков секций без строк
            let lines: Vec<&str> = out.lines().collect();
            for (i, l) in lines.iter().enumerate().skip(1) {
                if !l.starts_with("- ") {
                    assert!(lines.get(i + 1).is_some_and(|n| n.starts_with("- ")), "пустая секция в: {out}");
                }
            }
        }
    }
    // маленький бюджет: длинные факты не вытесняют уроки, плейбукам места нет
    let small = ctx.render(100);
    assert!(small.contains("Факты о пользователе:"), "{small}");
    assert!(small.contains("Уроки из прошлых ошибок"), "{small}");
    assert!(!small.contains("плейбуки"), "{small}");
    // средний: представлены все группы, важные факты — первыми в своей секции
    let mid = ctx.render(700);
    assert!(mid.contains("Уроки из прошлых ошибок"), "{mid}");
    assert!(mid.contains("Проверенные сценарии"), "{mid}");
    assert!(mid.lines().nth(2).unwrap().starts_with("- Предпочтение про"), "{mid}");
    // обрезанные строки заканчиваются «…»
    assert!(ctx.render(60).lines().last().unwrap().ends_with('…'));
    // по умолчанию 1500 — всё значимое влезает частично, но в пределах бюджета
    let full = ctx.render(DEFAULT_BUDGET_TOKENS);
    assert!(estimate_tokens(&full) <= DEFAULT_BUDGET_TOKENS);
    let huge = ctx.render(100_000);
    assert!(huge.contains("Проверенные сценарии (плейбуки):"));
    assert!(huge.contains("1) a_tool"));
    assert_eq!(huge.lines().filter(|l| l.starts_with("- ")).count(), 6 + 5 + 3);
}

#[test]
fn render_empty_and_lesson_without_fix() {
    assert_eq!(MemoryContext::default().render(1500), "");
    let (_d, mem) = temp_mem();
    let id = mem.add_lesson(lesson_stub_from_tool_error("ui_click", Some("Блокнот"), "element not found")).unwrap().id;
    let ctx = MemoryContext { lessons: vec![mem.get_lesson(id).unwrap().unwrap()], ..Default::default() };
    let out = ctx.render(1500);
    assert!(out.contains("[ui_click · Блокнот]"), "{out}");
    assert!(out.contains("Как правильно: пока неизвестно"), "{out}");
}
