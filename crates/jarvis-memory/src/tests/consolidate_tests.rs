use serde_json::json;

use super::{DAY, exec, temp_mem};
use crate::db::now;
use crate::*;

/// Вставить копию урока в обход дедупликации (как будто дубль появился из старой версии / импорта).
fn duplicate_lesson(mem: &Memory, id: i64) -> i64 {
    exec(
        mem,
        "INSERT INTO lessons (signature, exact_signature, error_text, scope, trigger_text, mistake, cause, fix, tool,
                              app, app_key, seen, first_seen, last_seen, confidence, is_stub, s_trigger, s_body, s_meta)
         SELECT signature, exact_signature, error_text, scope, trigger_text, mistake, cause, fix, tool,
                app, app_key, seen, first_seen, last_seen, confidence, is_stub, s_trigger, s_body, s_meta
         FROM lessons WHERE id = ?1",
        [id],
    );
    mem.conn().last_insert_rowid()
}

#[test]
fn consolidate_merges_near_duplicate_facts() {
    let (_d, mem) = temp_mem();
    let a = mem.upsert_fact(NewFact::inferred("Любит джаз по вечерам", 0.5)).unwrap().id;
    let b = mem.upsert_fact(NewFact::explicit("Работает из дома по пятницам")).unwrap().id;
    // правка в UI сделала второй факт почти копией первого
    let mut f = mem.get_fact(b).unwrap().unwrap();
    f.text = "любит джаз по вечерам!".into();
    mem.update_fact(&f).unwrap();
    mem.mark_used(&[MemoryRef { kind: MemoryKind::Fact, id: a }]).unwrap();
    let rep = mem.consolidate().unwrap();
    assert_eq!(rep.merged_facts, 1);
    let facts = mem.list_facts(10, 0).unwrap();
    assert_eq!(facts.len(), 1);
    // остаётся явный, счётчики суммируются
    assert_eq!(facts[0].id, b);
    assert_eq!(facts[0].source, FactSource::Explicit);
    assert_eq!(facts[0].use_count, 1);
    assert_eq!(facts[0].confidence, 1.0);
    assert_eq!(mem.consolidate().unwrap().merged_facts, 0);
    mem.check_integrity().unwrap();
}

#[test]
fn consolidate_merges_duplicate_lessons() {
    let (_d, mem) = temp_mem();
    let id = mem
        .add_lesson(NewLesson {
            scope: LessonScope::Tool,
            trigger: "запуск winget".into(),
            mistake: "нет прав".into(),
            fix: "запустить от администратора".into(),
            tool: Some("winget_install".into()),
            error_text: Some("winget: 0x8a150019 installer failed with exit code 5".into()),
            ..Default::default()
        })
        .unwrap()
        .id;
    let dup = duplicate_lesson(&mem, id);
    mem.record_lesson_result(dup, true).unwrap();
    // похожий урок без сигнатуры (описан словами) — тоже сливается
    let wordy = mem
        .add_lesson(NewLesson {
            scope: LessonScope::Tool,
            trigger: "запуск winget".into(),
            mistake: "нет прав администратора".into(),
            fix: "".into(),
            tool: Some("winget_install".into()),
            ..Default::default()
        })
        .unwrap();
    // add_lesson сразу слил его с первым по похожести — проверим и это
    assert_eq!(wordy.status, SaveStatus::Merged);
    assert_eq!(mem.stats().unwrap().lessons, 2);

    let rep = mem.consolidate().unwrap();
    assert_eq!(rep.merged_lessons, 1);
    let all = mem.list_lessons(10, 0).unwrap();
    assert_eq!(all.len(), 1);
    let l = &all[0];
    assert_eq!(l.success_after_fix, 1);
    assert_eq!(l.seen, 2 + 1, "seen суммируется: {l:?}");
    assert_eq!(l.fix, "запустить от администратора");
    mem.check_integrity().unwrap();
}

#[test]
fn consolidate_keeps_different_specific_recipes_apart() {
    let (_d, mem) = temp_mem();
    let gti = "gti : The term 'gti' is not recognized as the name of a cmdlet.";
    let sl = "sl : The term 'sl' is not recognized as the name of a cmdlet.";
    mem.add_lesson(lesson_user_habit("gti", "git", Some(gti))).unwrap();
    mem.add_lesson(lesson_user_habit("sl", "ls", Some(sl))).unwrap();
    assert_eq!(mem.consolidate().unwrap().merged_lessons, 0);
    assert_eq!(mem.stats().unwrap().lessons, 2);
}

#[test]
fn consolidate_decays_unused_and_forgets_weak() {
    let (_d, mem) = temp_mem();
    let t = now();
    let inferred = mem.upsert_fact(NewFact::inferred("Иногда слушает подкасты", 0.6)).unwrap().id;
    let explicit = mem.upsert_fact(NewFact::explicit("Аллергия на орехи")).unwrap().id;
    let fresh = mem.upsert_fact(NewFact::inferred("Смотрит футбол", 0.6)).unwrap().id;
    let lesson = mem.add_lesson(lesson_stub_from_tool_error("ui_click", None, "element not found")).unwrap().id;
    let useful = mem
        .add_lesson(NewLesson { trigger: "печать".into(), fix: "Ctrl+P".into(), confidence: 0.3, ..Default::default() })
        .unwrap()
        .id;
    mem.record_lesson_result(useful, true).unwrap();
    let old = t - 61 * DAY;
    exec(&mem, "UPDATE facts SET updated_at = ?1 WHERE id IN (?2, ?3)", [old, inferred, explicit]);
    exec(&mem, "UPDATE lessons SET last_seen = ?1, last_used = NULL", [old]);

    let rep = mem.consolidate_at(t).unwrap();
    assert_eq!(rep.decayed, 3, "{rep:?}"); // выведенный факт + 2 урока
    let f = mem.get_fact(inferred).unwrap().unwrap();
    assert!((f.confidence - 0.6 * DECAY_FACTOR).abs() < 1e-9);
    assert_eq!(mem.get_fact(explicit).unwrap().unwrap().confidence, 1.0, "явные факты не затухают");
    assert_eq!(mem.get_fact(fresh).unwrap().unwrap().confidence, 0.6);
    // повторный запуск в тот же день не затухает повторно
    assert_eq!(mem.consolidate_at(t).unwrap().decayed, 0);

    // ещё несколько периодов без использования → слабое забывается
    let mut when = t;
    let mut deleted = 0;
    for _ in 0..8 {
        when += 61 * DAY;
        deleted += mem.consolidate_at(when).unwrap().deleted;
    }
    assert!(mem.get_fact(inferred).unwrap().is_none());
    assert!(mem.get_lesson(lesson).unwrap().is_none());
    assert!(mem.get_fact(explicit).unwrap().is_some());
    // урок, который хоть раз помог, не удаляется (только теряет доверие)
    assert!(mem.get_lesson(useful).unwrap().is_some());
    assert!(deleted >= 3, "{deleted}");
    mem.check_integrity().unwrap();
}

#[test]
fn consolidate_promotes_frequent_playbooks_and_merges_same_steps() {
    let (_d, mem) = temp_mem();
    let steps = vec![
        PlaybookStep::new("open_target", json!({"name": "ms-settings:bluetooth"})),
        PlaybookStep::new("ui_click", json!({"name": "Bluetooth"})),
    ];
    let good = mem
        .add_playbook(NewPlaybook { name: "Включить блютуз".into(), when: "".into(), steps: steps.clone(), params_schema: json!({}) })
        .unwrap()
        .id;
    let twin = mem
        .add_playbook(NewPlaybook {
            name: "Bluetooth on".into(),
            when: "просит включить bluetooth".into(),
            steps: steps.clone(),
            params_schema: json!({}),
        })
        .unwrap()
        .id;
    assert_ne!(good, twin);
    let bad = mem
        .add_playbook(NewPlaybook {
            name: "Почистить диск".into(),
            when: "мало места".into(),
            steps: vec![PlaybookStep::new("temp_cleanup", json!({}))],
            params_schema: json!({}),
        })
        .unwrap()
        .id;
    for _ in 0..3 {
        mem.record_playbook_result(good, true).unwrap();
    }
    mem.record_playbook_result(twin, true).unwrap();
    exec(&mem, "UPDATE playbooks SET promoted = 1 WHERE id = ?1", [bad]);
    for _ in 0..3 {
        mem.record_playbook_result(bad, false).unwrap();
    }

    let rep = mem.consolidate().unwrap();
    assert_eq!(rep.merged_playbooks, 1);
    assert_eq!(rep.promoted, 1);
    assert_eq!(rep.demoted, 1);
    let p = mem.get_playbook(good).unwrap().unwrap();
    assert!(p.promoted);
    assert_eq!(p.success_count, 4);
    assert_eq!(p.when, "просит включить bluetooth");
    assert!(mem.get_playbook(twin).unwrap().is_none());
    assert!(!mem.get_playbook(bad).unwrap().unwrap().promoted);

    // давно не использованный повышенный плейбук понижается
    exec(&mem, "UPDATE playbooks SET updated_at = ?1, last_used = ?1", [now() - 70 * DAY]);
    let rep = mem.consolidate().unwrap();
    assert_eq!(rep.demoted, 1);
    assert!(!mem.get_playbook(good).unwrap().unwrap().promoted);
    mem.check_integrity().unwrap();
}

#[test]
fn playbook_versions_and_validation() {
    let (_d, mem) = temp_mem();
    let v1 = vec![PlaybookStep::new("open_target", json!({"name": "notepad"}))];
    let p = NewPlaybook { name: "Открыть Блокнот".into(), when: "просит блокнот".into(), steps: v1.clone(), params_schema: json!({}) };
    let a = mem.add_playbook(p.clone()).unwrap();
    assert_eq!(a.status, SaveStatus::Inserted);
    assert_eq!(mem.add_playbook(p.clone()).unwrap().status, SaveStatus::Unchanged);
    // имя без учёта регистра (в т.ч. кириллицы) и пробелов — тот же плейбук; новые шаги → версия 2
    let mut p2 = p.clone();
    p2.name = "  открыть   БЛОКНОТ ".into();
    p2.steps.push(PlaybookStep::new("ui_wait", json!({"title": "Блокнот"})));
    let b = mem.add_playbook(p2).unwrap();
    assert_eq!((b.id, b.status), (a.id, SaveStatus::NewVersion));
    let got = mem.get_playbook(a.id).unwrap().unwrap();
    assert_eq!(got.version, 2);
    assert_eq!(got.steps.len(), 2);
    assert_eq!(got.name, "Открыть Блокнот");
    // похожее имя + условия → тоже версия, а не новый плейбук
    let mut p3 = p.clone();
    p3.name = "Открыть блокнот".into();
    p3.when = "просит открыть блокнот".into();
    let c = mem.add_playbook(p3).unwrap();
    assert_eq!(c.id, a.id);
    // только условия поменялись → Merged без новой версии
    let mut p4 = p.clone();
    p4.when = "пользователь хочет записать заметку".into();
    let d = mem.add_playbook(p4).unwrap();
    assert_eq!(d.status, SaveStatus::Merged);

    assert_eq!(mem.playbook_by_name("ОТКРЫТЬ блокнот").unwrap().unwrap().id, a.id);
    assert_eq!(mem.search_playbooks("блокнота", 3).unwrap()[0].id, a.id);
    // валидация
    let mut bad = p.clone();
    bad.steps = vec![PlaybookStep::new("Open Target!", json!({}))];
    assert!(matches!(mem.add_playbook(bad), Err(MemoryError::Invalid(_))));
    let mut empty = p.clone();
    empty.steps.clear();
    assert!(matches!(mem.add_playbook(empty), Err(MemoryError::Invalid(_))));
    // переименование в занятое имя — понятная ошибка
    let other = mem
        .add_playbook(NewPlaybook { name: "Скриншот".into(), when: "".into(), steps: v1, params_schema: json!({}) })
        .unwrap()
        .id;
    let mut o = mem.get_playbook(other).unwrap().unwrap();
    o.name = "открыть блокнот".into();
    assert!(matches!(mem.update_playbook(&o), Err(MemoryError::Invalid(_))));
    assert!(mem.delete_playbook(other).unwrap());
}
