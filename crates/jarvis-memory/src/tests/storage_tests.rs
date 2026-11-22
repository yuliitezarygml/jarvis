use jarvis_core::event::ActionStatus;
use jarvis_core::{Config, RiskTier, Role};
use serde_json::json;
use std::sync::Arc;

use super::{exec, temp_mem};
use crate::*;

#[test]
fn migrations_are_idempotent_and_data_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("memory.db");
    let id = {
        let mem = Memory::open(&path).unwrap();
        assert_eq!(mem.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(mem.migrate().unwrap(), SCHEMA_VERSION);
        assert_eq!(mem.migrate().unwrap(), SCHEMA_VERSION);
        let mode: String = mem.conn().query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        mem.upsert_fact(NewFact::explicit("Я пью чёрный кофе")).unwrap().id
    };
    // «перезапуск»: открыть снова — миграции не ломают данные и не дублируют объекты схемы
    let mem = Memory::open(&path).unwrap();
    assert_eq!(mem.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(mem.get_fact(id).unwrap().unwrap().text, "Я пью чёрный кофе");
    let objects: i64 = mem
        .conn()
        .query_row("SELECT count(*) FROM sqlite_master WHERE name = 'facts_fts'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(objects, 1);
    // повторное применение SQL миграции поверх существующей схемы тоже безопасно (IF NOT EXISTS)
    exec(&mem, "PRAGMA user_version = 0", []);
    assert_eq!(mem.migrate().unwrap(), SCHEMA_VERSION);
    assert_eq!(mem.stats().unwrap().facts, 1);
    mem.check_integrity().unwrap();
}

#[test]
fn open_for_config_uses_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config { data_dir: "mydata".into(), ..Config::default() };
    let mem = Memory::open_for_config(dir.path(), &cfg).unwrap();
    let expected = dir.path().join("mydata").join(DB_FILE);
    assert_eq!(mem.path().unwrap(), expected);
    assert!(expected.exists());
    assert_eq!(Memory::db_path(dir.path(), &cfg), expected);
}

#[test]
fn in_memory_works() {
    let mem = Memory::open_in_memory().unwrap();
    mem.upsert_fact(NewFact::explicit("любимый цвет — синий")).unwrap();
    assert_eq!(mem.search_facts("синего цвета", 5).unwrap().len(), 1);
}

#[test]
fn fact_search_handles_russian_morphology() {
    let (_d, mem) = temp_mem();
    let coffee = mem.upsert_fact(NewFact::explicit("я люблю чёрный кофе")).unwrap().id;
    mem.upsert_fact(NewFact::explicit("живу в Москве")).unwrap();
    let work = mem.upsert_fact(NewFact::explicit("Работаю программистом в банке")).unwrap().id;
    let browser = mem.upsert_fact(NewFact::inferred("Любимый браузер — Firefox", 0.7)).unwrap().id;
    let dark = mem.upsert_fact(NewFact::inferred("Prefers dark themes in all apps", 0.7)).unwrap().id;

    let hits = mem.search_facts("какой кофе я любил", 5).unwrap();
    assert_eq!(hits.first().map(|f| f.id), Some(coffee), "{hits:?}");
    assert_eq!(mem.search_facts("где я работал", 5).unwrap().first().map(|f| f.id), Some(work));
    assert_eq!(mem.search_facts("браузеры", 5).unwrap().first().map(|f| f.id), Some(browser));
    assert_eq!(mem.search_facts("dark theme", 5).unwrap().first().map(|f| f.id), Some(dark));
    assert_eq!(mem.search_facts("ЧЁРНОГО КОФЕ", 5).unwrap().first().map(|f| f.id), Some(coffee));
    assert!(mem.search_facts("я и ты", 5).unwrap().is_empty());
    assert!(mem.search_facts("квантовая хромодинамика", 5).unwrap().is_empty());
}

#[test]
fn fact_upsert_merges_near_duplicates() {
    let (_d, mem) = temp_mem();
    let a = mem.upsert_fact(NewFact::explicit("Я пью чёрный кофе")).unwrap();
    assert_eq!(a.status, SaveStatus::Inserted);
    // выведенный почти-дубль не затирает явный текст, но сливается
    let b = mem.upsert_fact(NewFact::inferred("пью черный кофе", 0.5).with_category("preference")).unwrap();
    assert_eq!((b.id, b.status), (a.id, SaveStatus::Merged));
    let f = mem.get_fact(a.id).unwrap().unwrap();
    assert_eq!(f.text, "Я пью чёрный кофе");
    assert_eq!(f.source, FactSource::Explicit);
    assert_eq!(f.category, "preference");
    assert_eq!(f.confidence, 1.0);
    // явный почти-дубль обновляет текст
    let c = mem.upsert_fact(NewFact::explicit("я пью чёрный кофе!")).unwrap();
    assert_eq!(c.id, a.id);
    assert_eq!(mem.get_fact(a.id).unwrap().unwrap().text, "я пью чёрный кофе!");
    // другое — отдельная запись
    let d = mem.upsert_fact(NewFact::explicit("Я пью зелёный чай")).unwrap();
    assert!(d.is_new());
    assert_eq!(mem.stats().unwrap().facts, 2);
    // пустой факт — ошибка валидации
    assert!(matches!(mem.upsert_fact(NewFact::explicit("   ")), Err(MemoryError::Invalid(_))));
}

#[test]
fn fact_crud_keeps_index_in_sync() {
    let (_d, mem) = temp_mem();
    let id = mem.upsert_fact(NewFact::explicit("Ноутбук называется DESKTOP-7Q").with_category("alias")).unwrap().id;
    mem.upsert_fact(NewFact::explicit("Кошку зовут Мурка")).unwrap();
    assert_eq!(mem.list_facts(10, 0).unwrap().len(), 2);
    assert_eq!(mem.list_facts(1, 1).unwrap().len(), 1);

    let mut f = mem.get_fact(id).unwrap().unwrap();
    assert_eq!(f.category, "alias");
    f.text = "Рабочий компьютер называется WORKSTATION".into();
    f.important = true;
    mem.update_fact(&f).unwrap();
    assert!(mem.search_facts("ноутбук", 5).unwrap().is_empty());
    assert_eq!(mem.search_facts("рабочего компьютера", 5).unwrap()[0].id, id);
    assert!(mem.important_facts(5).unwrap().iter().any(|x| x.id == id));

    assert!(mem.delete_fact(id).unwrap());
    assert!(!mem.delete_fact(id).unwrap());
    assert!(mem.get_fact(id).unwrap().is_none());
    assert!(mem.search_facts("компьютер", 5).unwrap().is_empty());
    f.id = 9999;
    assert!(matches!(mem.update_fact(&f), Err(MemoryError::NotFound(..))));
    mem.check_integrity().unwrap();
}

#[test]
fn sessions_messages_and_search() {
    let (_d, mem) = temp_mem();
    let s = mem.start_session(Some("утро")).unwrap();
    mem.log_message(s, Role::User, "Джарвис, найди файлы договора на диске D", None).unwrap();
    mem.log_message(s, Role::Tool, "найдено 3 файла", Some("file_search")).unwrap();
    mem.log_message(s, Role::Assistant, "Нашёл три файла, сэр.", None).unwrap();
    let msgs = mem.session_messages(s).unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[1].role, Role::Tool);
    assert_eq!(msgs[1].tool_name.as_deref(), Some("file_search"));
    assert_eq!(mem.search_messages("договоры", 5).unwrap().len(), 1);
    mem.end_session(s, Some("искали договор")).unwrap();
    let sessions = mem.list_sessions(10, 0).unwrap();
    assert_eq!(sessions[0].summary.as_deref(), Some("искали договор"));
    assert!(sessions[0].ended_at.is_some());
    assert!(matches!(mem.log_message(999, Role::User, "x", None), Err(MemoryError::NotFound(..))));
    // удаление эпизода удаляет сообщения (ON DELETE CASCADE) и их индекс
    assert!(mem.delete_session(s).unwrap());
    assert_eq!(mem.stats().unwrap().messages, 0);
    assert!(mem.search_messages("договор", 5).unwrap().is_empty());
}

#[test]
fn actions_audit_roundtrip() {
    let (_d, mem) = temp_mem();
    let id = mem
        .record_action(NewAction {
            session_id: None,
            tool: "service_stop".into(),
            args: json!({"name": "Spooler"}),
            tier: RiskTier::T3,
            status: ActionStatus::AwaitingConfirm,
            result_summary: String::new(),
            approved_by: None,
        })
        .unwrap();
    mem.update_action(id, ActionStatus::Done, Some("служба остановлена"), Some("voice")).unwrap();
    let a = mem.get_action(id).unwrap().unwrap();
    assert_eq!(a.tier, RiskTier::T3);
    assert_eq!(a.status, ActionStatus::Done);
    assert_eq!(a.approved_by.as_deref(), Some("voice"));
    assert_eq!(a.result_summary, "служба остановлена");
    assert_eq!(a.args["name"], "Spooler");
    assert_eq!(mem.list_actions(10, 0).unwrap().len(), 1);
    // в базе tier/status лежат человекочитаемо
    let (tier, status): (String, String) =
        mem.conn().query_row("SELECT tier, status FROM actions", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((tier.as_str(), status.as_str()), ("T3", "done"));
    assert!(matches!(mem.update_action(42, ActionStatus::Failed, None, None), Err(MemoryError::NotFound(..))));
}

#[test]
fn export_contains_everything() {
    let (_d, mem) = temp_mem();
    mem.upsert_fact(NewFact::explicit("пьёт кофе")).unwrap();
    mem.add_lesson(lesson_stub_from_tool_error("ui_click", None, "element not found")).unwrap();
    mem.add_playbook(NewPlaybook {
        name: "Открыть блокнот".into(),
        when: "просит блокнот".into(),
        steps: vec![PlaybookStep::new("open_target", json!({"name": "notepad"}))],
        params_schema: json!({}),
    })
    .unwrap();
    let s = mem.start_session(None).unwrap();
    mem.log_message(s, Role::User, "привет", None).unwrap();
    mem.record_action(NewAction {
        session_id: Some(s),
        tool: "open_target".into(),
        args: json!({}),
        tier: RiskTier::T1,
        status: ActionStatus::Done,
        result_summary: "ok".into(),
        approved_by: None,
    })
    .unwrap();
    let e = mem.export_json().unwrap();
    assert_eq!(e["format"], "jarvis-memory");
    assert_eq!(e["schema_version"], SCHEMA_VERSION);
    for (k, n) in [("facts", 1), ("lessons", 1), ("playbooks", 1), ("sessions", 1), ("messages", 1), ("actions", 1)] {
        assert_eq!(e[k].as_array().map(Vec::len), Some(n), "{k}");
    }
    // экспорт читается обратно в типы
    let facts: Vec<Fact> = serde_json::from_value(e["facts"].clone()).unwrap();
    assert_eq!(facts[0].text, "пьёт кофе");
    let pbs: Vec<Playbook> = serde_json::from_value(e["playbooks"].clone()).unwrap();
    assert_eq!(pbs[0].steps[0].tool, "open_target");
    let _: Vec<Lesson> = serde_json::from_value(e["lessons"].clone()).unwrap();
    let _: Vec<Action> = serde_json::from_value(e["actions"].clone()).unwrap();
    let _: Vec<Message> = serde_json::from_value(e["messages"].clone()).unwrap();
}

/// Уникальное «слово» для теста конкурентности (чтобы факты не сливались дедупом).
/// Только согласные без `y`: английский стеммер не должен свести два кода к одной основе.
fn code(mut n: usize) -> String {
    const ALPHA: &[u8] = b"bcdfghjklmnpqrtvwxz";
    let mut s = String::new();
    loop {
        s.push(ALPHA[n % ALPHA.len()] as char);
        n /= ALPHA.len();
        if n == 0 {
            break;
        }
    }
    format!("zq{s}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_access_from_many_tasks_and_two_handles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let a = Memory::open(&path).unwrap();
    // второй независимый хэндл на тот же файл (как другой процесс)
    let b = Memory::open(&path).unwrap();
    let session = a.start_session(Some("load")).unwrap();
    const TASKS: usize = 8;
    const PER: usize = 20;
    let mut handles = Vec::new();
    for t in 0..TASKS {
        let mem = if t % 2 == 0 { a.clone() } else { b.clone() };
        handles.push(tokio::spawn(async move {
            for i in 0..PER {
                let n = t * PER + i;
                mem.upsert_fact_async(NewFact::explicit(format!("значение {} хранится", code(n)))).await?;
                mem.record_action_async(NewAction {
                    session_id: Some(session),
                    tool: "test_tool".into(),
                    args: json!({"n": n}),
                    tier: RiskTier::T0,
                    status: ActionStatus::Done,
                    result_summary: String::new(),
                    approved_by: None,
                })
                .await?;
                mem.log_message_async(session, Role::User, format!("сообщение {n}"), None).await?;
                mem.add_lesson_async(lesson_stub_from_tool_error("t", None, &format!("err {}", code(n)))).await?;
                let _ = mem.retrieve_context_async("значение".into(), Some("t".into()), None).await?;
            }
            Ok::<_, MemoryError>(())
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }
    let s = a.stats().unwrap();
    assert_eq!(s.facts as usize, TASKS * PER);
    assert_eq!(s.actions as usize, TASKS * PER);
    assert_eq!(s.messages as usize, TASKS * PER);
    // «err <code>» — разные слова → разные сигнатуры → ни один урок не потерян и не слит
    assert_eq!(s.lessons as usize, TASKS * PER);
    a.check_integrity().unwrap();
    let shared = Arc::new(b);
    assert_eq!(shared.stats().unwrap(), s);
}

/// Регрессия: FTS5-кэш соединения устаревал, когда другое соединение коммитило в ту же FTS-таблицу,
/// пока это читало её («fts5: corruption found reading blob …» при целом файле). Исправлено сверкой
/// `PRAGMA data_version` и переоткрытием соединения после чужих коммитов (см. `Memory::conn`).
#[test]
fn fts_stays_coherent_when_another_connection_writes() {
    for round in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("memory.db");
        let a = Memory::open(&p).unwrap();
        let b = Memory::open(&p).unwrap();
        let mut threads = Vec::new();
        for t in 0..8 {
            let m = if t % 2 == 0 { a.clone() } else { b.clone() };
            threads.push(std::thread::spawn(move || {
                for i in 0..20 {
                    let w = code(t * 20 + i);
                    m.add_lesson(lesson_stub_from_tool_error("t", None, &format!("err {w}"))).unwrap();
                    m.upsert_fact(NewFact::explicit(format!("значение {w} хранится"))).unwrap();
                    m.retrieve_context("значение err", Some("t"), None).unwrap();
                    m.search_lessons(&w, 3).unwrap();
                }
            }));
        }
        for th in threads {
            th.join().unwrap();
        }
        a.check_integrity().unwrap_or_else(|e| panic!("раунд {round}, хэндл a: {e}"));
        b.check_integrity().unwrap_or_else(|e| panic!("раунд {round}, хэндл b: {e}"));
        // запись, сделанная через b, находится поиском через a (и наоборот)
        let from_b = code(20 + 7); // поток t = 1 (хэндл b)
        let from_a = code(2 * 20 + 3); // поток t = 2 (хэндл a)
        assert_eq!(a.search_lessons(&from_b, 3).unwrap().len(), 1, "раунд {round}");
        assert_eq!(b.search_facts(&from_a, 3).unwrap().len(), 1, "раунд {round}");
        assert_eq!(a.stats().unwrap(), b.stats().unwrap());
    }
}
