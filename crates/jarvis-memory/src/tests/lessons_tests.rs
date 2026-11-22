use super::{exec, temp_mem};
use crate::*;

const ACCESS_A: &str = "Remove-Item : Access to the path 'C:\\Users\\bob\\AppData\\Local\\Temp\\x1.tmp' is denied.\nAt line:1 char:1";
const ACCESS_B: &str = "Remove-Item : Access to the path 'D:\\work\\build\\out.log' is denied.\nAt line:7 char:13";

#[test]
fn lessons_merge_by_signature_and_update_fix() {
    let (_d, mem) = temp_mem();
    let s1 = mem.add_lesson(lesson_stub_from_tool_error("powershell_run", None, ACCESS_A)).unwrap();
    assert_eq!(s1.status, SaveStatus::Inserted);
    // та же ошибка с другим путём и строкой → тот же урок, seen = 2
    let s2 = mem.add_lesson(lesson_stub_from_tool_error("powershell_run", None, ACCESS_B)).unwrap();
    assert_eq!((s2.id, s2.status), (s1.id, SaveStatus::Merged));
    let l = mem.get_lesson(s1.id).unwrap().unwrap();
    assert_eq!(l.seen, 2);
    assert!(l.is_stub && l.fix.is_empty());
    assert_eq!(l.signature.as_deref(), Some(error_signature(ACCESS_A).as_str()));
    assert!(l.error_text.as_deref().unwrap().contains("access to the path <q> is denied"));
    // пока нет исправления — мгновенного рецепта нет
    assert!(mem.lesson_for_signature(&error_signature(ACCESS_B)).unwrap().is_none());

    // доработанный урок с исправлением сливается и задаёт fix
    let full = NewLesson {
        scope: LessonScope::Tool,
        trigger: "удаление временного файла".into(),
        mistake: "файл занят другим процессом или нет прав".into(),
        cause: "файл открыт или защищён".into(),
        fix: "закрыть процесс, держащий файл, и повторить Remove-Item -Force".into(),
        tool: Some("powershell_run".into()),
        error_text: Some(ACCESS_B.into()),
        confidence: 0.8,
        ..Default::default()
    };
    let s3 = mem.add_lesson(full).unwrap();
    assert_eq!(s3.id, s1.id);
    let l = mem.get_lesson(s1.id).unwrap().unwrap();
    assert_eq!(l.seen, 3);
    assert!(!l.is_stub);
    assert!(l.fix.contains("Remove-Item -Force"));
    assert_eq!(l.confidence, 0.8);

    // повтор ошибки заготовкой: seen растёт, доработанные поля не затираются
    mem.add_lesson(lesson_stub_from_tool_error("powershell_run", None, ACCESS_A)).unwrap();
    let l = mem.get_lesson(s1.id).unwrap().unwrap();
    assert_eq!(l.seen, 4);
    assert_eq!(l.trigger, "удаление временного файла");
    assert!(!l.is_stub);

    // новый fix от доработанного урока заменяет старый
    mem.add_lesson(NewLesson {
        fix: "запустить от администратора".into(),
        trigger: "удаление".into(),
        error_text: Some(ACCESS_A.into()),
        ..Default::default()
    })
    .unwrap();
    let recipe = mem.lesson_for_signature(&error_signature(ACCESS_B)).unwrap().unwrap();
    assert_eq!(recipe.id, s1.id);
    assert_eq!(recipe.fix, "запустить от администратора");
    assert_eq!(mem.stats().unwrap().lessons, 1);
}

#[test]
fn exact_signature_protects_specific_recipes() {
    let (_d, mem) = temp_mem();
    let gti = "gti : The term 'gti' is not recognized as the name of a cmdlet, function, script file, or operable program.\nAt line:1 char:1\n+ gti status\n+ ~~~";
    let pyhton = "pyhton : The term 'pyhton' is not recognized as the name of a cmdlet, function, script file, or operable program.\nAt line:1 char:1\n+ pyhton x.py\n+ ~~~~~~";
    let a = mem.add_lesson(lesson_user_habit("gti status", "git status", Some(gti))).unwrap();
    // другая опечатка с той же общей сигнатурой — отдельный урок, рецепт «gti» не затирается
    let b = mem.add_lesson(lesson_user_habit("pyhton x.py", "python x.py", Some(pyhton))).unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(error_signature(gti), error_signature(pyhton));

    let m = mem.lesson_for_error(gti).unwrap().unwrap();
    assert!(m.exact);
    assert_eq!(m.lesson.fix, "git status");
    let m = mem.lesson_for_error(&pyhton.replace("x.py", "y.py")).unwrap().unwrap();
    assert!(m.exact);
    assert_eq!(m.lesson.fix, "python x.py");
    // незнакомая опечатка: та же ошибка, но совпадение только общее
    let other = gti.replace("gti", "sl");
    let m = mem.lesson_for_error(&other).unwrap().unwrap();
    assert!(!m.exact);
    // повтор той же опечатки — слияние
    let again = mem.add_lesson(lesson_user_habit("gti status", "git status", Some(gti))).unwrap();
    assert_eq!((again.id, again.status), (a.id, SaveStatus::Merged));
    let hints = mem.user_habit_hints(5).unwrap();
    assert_eq!(hints[0].id, a.id);
    assert_eq!(hints[0].seen, 2);
    assert_eq!(hints[0].scope, LessonScope::UserHabits);
}

#[test]
fn lessons_merge_by_fts_similarity_without_signature() {
    let (_d, mem) = temp_mem();
    let a = mem
        .add_lesson(NewLesson {
            scope: LessonScope::App,
            app: Some("Chrome".into()),
            trigger: "открыть новую вкладку в браузере".into(),
            mistake: "нажал Ctrl+N и открылось новое окно".into(),
            fix: "Ctrl+T".into(),
            ..Default::default()
        })
        .unwrap();
    let b = mem
        .add_lesson(NewLesson {
            scope: LessonScope::App,
            app: Some("chrome".into()),
            trigger: "открыть новую вкладку браузера".into(),
            mistake: "нажимал Ctrl+N, открылось новое окно".into(),
            fix: "использовать Ctrl+T вместо Ctrl+N".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!((b.id, b.status), (a.id, SaveStatus::Merged));
    let l = mem.get_lesson(a.id).unwrap().unwrap();
    assert_eq!(l.seen, 2);
    assert_eq!(l.fix, "использовать Ctrl+T вместо Ctrl+N");
    // другое по смыслу в том же приложении — новый урок
    let c = mem
        .add_lesson(NewLesson {
            scope: LessonScope::App,
            app: Some("Chrome".into()),
            trigger: "сохранить страницу как PDF".into(),
            mistake: "искал пункт в меню закладок".into(),
            fix: "Ctrl+P → Сохранить как PDF".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(c.is_new());
    // тот же текст, но другое приложение — не сливается
    let d = mem
        .add_lesson(NewLesson {
            scope: LessonScope::App,
            app: Some("Firefox".into()),
            trigger: "открыть новую вкладку в браузере".into(),
            mistake: "нажал Ctrl+N и открылось новое окно".into(),
            fix: "Ctrl+T".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(d.is_new());
    assert!(matches!(mem.add_lesson(NewLesson::default()), Err(MemoryError::Invalid(_))));
}

#[test]
fn refine_does_not_double_count_the_same_occurrence() {
    let (_d, mem) = temp_mem();
    let err = "ui_click: element 'Сохранить' not found in window 'Блокнот'";
    let stub = mem.add_lesson(lesson_stub_from_tool_error("ui_click", Some("notepad.exe"), err)).unwrap();
    let refined = mem
        .refine_lesson(NewLesson {
            scope: LessonScope::Tool,
            trigger: "сохранение в Блокноте".into(),
            mistake: "кнопки «Сохранить» нет в UIA-дереве".into(),
            fix: "нажать Ctrl+S".into(),
            tool: Some("ui_click".into()),
            error_text: Some(err.into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!((refined.id, refined.status), (stub.id, SaveStatus::Merged));
    let l = mem.get_lesson(stub.id).unwrap().unwrap();
    assert_eq!(l.seen, 1);
    assert!(!l.is_stub);
    assert_eq!(l.fix, "нажать Ctrl+S");
    assert_eq!(l.app.as_deref(), Some("notepad.exe"));
    // уточнение давнего урока (появление не учтено заготовкой) — это повтор
    exec(&mem, "UPDATE lessons SET last_seen = last_seen - ?1", [REFINE_WINDOW_SECS + 10]);
    mem.refine_lesson(NewLesson { fix: "Ctrl+S".into(), trigger: "x".into(), error_text: Some(err.into()), ..Default::default() })
        .unwrap();
    assert_eq!(mem.get_lesson(stub.id).unwrap().unwrap().seen, 2);
    // а обычный add_lesson — всегда повтор
    mem.add_lesson(lesson_stub_from_tool_error("ui_click", None, err)).unwrap();
    assert_eq!(mem.get_lesson(stub.id).unwrap().unwrap().seen, 3);
}

#[test]
fn lesson_outcomes_warnings_search_and_crud() {
    let (_d, mem) = temp_mem();
    let id = mem
        .add_lesson(NewLesson {
            scope: LessonScope::Tool,
            trigger: "остановка службы".into(),
            mistake: "Отказано в доступе".into(),
            cause: "нет прав администратора".into(),
            fix: "перезапустить Джарвиса от администратора".into(),
            tool: Some("service_stop".into()),
            confidence: 0.5,
            ..Default::default()
        })
        .unwrap()
        .id;
    mem.record_lesson_result(id, true).unwrap();
    let l = mem.get_lesson(id).unwrap().unwrap();
    assert_eq!(l.success_after_fix, 1);
    assert!((l.confidence - 0.6).abs() < 1e-9);
    mem.record_lesson_result(id, false).unwrap();
    assert_eq!(mem.get_lesson(id).unwrap().unwrap().fail_after_fix, 1);
    assert!(matches!(mem.record_lesson_result(777, true), Err(MemoryError::NotFound(..))));

    let w = mem.warnings_for_tool("service_stop", None, 3).unwrap();
    assert_eq!(w.len(), 1);
    let text = w[0].warning();
    assert!(text.starts_with("В прошлый раз с service_stop"), "{text}");
    assert!(text.contains("Как правильно: перезапустить"), "{text}");
    assert!(mem.warnings_for_tool("ui_click", None, 3).unwrap().is_empty());

    // поиск с морфологией: «доступа», «служб»
    assert_eq!(mem.search_lessons("ошибки доступа", 5).unwrap()[0].id, id);
    assert_eq!(mem.search_lessons("службы не останавливаются", 5).unwrap()[0].id, id);

    let mut l = mem.get_lesson(id).unwrap().unwrap();
    l.fix = "использовать sc.exe stop от администратора".into();
    mem.update_lesson(&l).unwrap();
    assert_eq!(mem.search_lessons("sc.exe", 5).unwrap().len(), 1);
    assert_eq!(mem.list_lessons(10, 0).unwrap().len(), 1);
    assert!(mem.delete(MemoryKind::Lesson, id).unwrap());
    assert!(mem.search_lessons("доступ", 5).unwrap().is_empty());
    mem.check_integrity().unwrap();
}

#[test]
fn correction_stubs_merge_on_repeat() {
    let (_d, mem) = temp_mem();
    let act = r#"open_target {"name":"хром"}"#;
    let a = mem.add_lesson(lesson_stub_from_correction("нет, не так", Some(act)).unwrap()).unwrap();
    let b = mem.add_lesson(lesson_stub_from_correction("ты ошибся", Some(act)).unwrap()).unwrap();
    assert_eq!(a.id, b.id);
    let l = mem.get_lesson(a.id).unwrap().unwrap();
    assert_eq!(l.seen, 2);
    assert_eq!(l.tool.as_deref(), Some("open_target"));
}
