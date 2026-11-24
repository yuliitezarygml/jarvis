use jarvis_core::{CancellationToken, ChatMessage, LlmError, Role, ToolCall};
use serde_json::json;
use std::sync::Arc;

use super::{MockModel, temp_mem};
use crate::*;

const GOOD_JSON: &str = r#"{
  "facts": [
    {"text": "Предпочитает тёмную тему в программах", "category": "preference", "confidence": 0.95},
    {"text": "...", "category": "general"},
    {"text": "", "category": "general"}
  ],
  "lessons": [
    {"scope": "tool", "trigger": "сохранение файла в Блокноте", "mistake": "кнопки «Сохранить» нет в UIA",
     "cause": "в новом Блокноте меню свёрнуто", "fix": "нажать Ctrl+S", "tool": "ui_click", "app": "Блокнот",
     "error_text": null, "confidence": "80%"},
    {"scope": "tool", "trigger": "что-то", "mistake": "string", "fix": ""},
    {"scope": "general", "trigger": "string", "mistake": "...", "fix": "string"}
  ],
  "playbooks": [
    {"name": "Сохранить файл в Блокноте", "when": "просит сохранить текст",
     "steps": [{"tool": "window_focus", "args": {"title": "Блокнот"}}, {"tool": "key_press", "args": {"keys": "ctrl+s"}}],
     "params_schema": {"type": "object", "properties": {}}},
    {"name": "Плохой", "steps": [{"tool": "Open Target!", "args": {}}]},
    {"name": "Без шагов", "steps": []},
    {"name": "Аргументы строкой", "steps": [{"tool": "ui_click", "args": "Сохранить"}]}
  ],
}"#;

fn check_good(r: &ReflectionResult) {
    assert_eq!(r.facts.len(), 1, "{r:?}");
    assert_eq!(r.facts[0].source, FactSource::Inferred);
    assert_eq!(r.facts[0].category, "preference");
    assert!((r.facts[0].confidence - 0.9).abs() < 1e-9, "выведенный факт ограничен 0.9");
    assert_eq!(r.lessons.len(), 1);
    let l = &r.lessons[0];
    assert_eq!(l.tool.as_deref(), Some("ui_click"));
    assert_eq!(l.app.as_deref(), Some("Блокнот"));
    assert!((l.confidence - 0.8).abs() < 1e-9);
    assert!(!l.is_stub);
    assert_eq!(r.playbooks.len(), 1);
    assert_eq!(r.playbooks[0].steps.len(), 2);
    assert_eq!(r.playbooks[0].steps[1].args["keys"], "ctrl+s");
    assert_eq!(r.dropped, 2 + 2 + 3);
}

#[test]
fn parses_plain_fenced_and_noisy_json() {
    // висячая запятая в конце GOOD_JSON — тоже допускается
    check_good(&parse_reflection(GOOD_JSON).unwrap());
    check_good(&parse_reflection(&format!("```json\n{GOOD_JSON}\n```")).unwrap());
    check_good(
        &parse_reflection(&format!(
            "Конечно! Вот результат анализа {{кратко}}:\n```JSON\n{GOOD_JSON}\n```\nЕсли нужно — уточню."
        ))
        .unwrap(),
    );
    check_good(&parse_reflection(&format!("Анализ: {GOOD_JSON} — готово.")).unwrap());
    check_good(&parse_reflection(&format!("```\n{GOOD_JSON}```")).unwrap());

    let empty = parse_reflection(r#"{"facts": [], "lessons": [], "playbooks": []}"#).unwrap();
    assert!(empty.is_empty());
    // частичный ответ: только один ключ, объект вместо массива
    let one = parse_reflection(r#"{"facts": {"text": "Живёт в Казани"}}"#).unwrap();
    assert_eq!(one.facts.len(), 1);
    assert_eq!(one.facts[0].category, "general");

    assert!(matches!(parse_reflection("Ничего полезного не нашёл."), Err(ReflectError::Parse(_))));
    assert!(matches!(parse_reflection(r#"{"answer": 42}"#), Err(ReflectError::Parse(_))));
    assert!(matches!(parse_reflection("```json\n{\"facts\": [\n```"), Err(ReflectError::Parse(_))));
}

#[test]
fn truncated_reply_keeps_complete_items() {
    // Как в журнале: reasoning-модель упёрлась в лимит токенов посреди урока.
    let cut = r#"{"facts":[{"text":"Пользователь предпочитает Google Chrome","category":"preference","confidence":0.9},{"text":"Пользователь хранит заметки в Documents","category":"habit"}],"lessons":[{"scope":"tool","trigger":"запись текста в бло"#;
    let r = parse_reflection(cut).expect("оборванный JSON чинится");
    assert_eq!(r.facts.len(), 2, "оба полных факта сохранены");
    assert!(r.facts[0].text.contains("Chrome"));
    assert!(r.lessons.is_empty(), "недописанный урок без fix отброшен");
    // Совсем без JSON — по-прежнему ошибка.
    assert!(parse_reflection("не могу ответить").is_err());
}

#[test]
fn extract_json_ignores_braces_inside_strings() {
    let v = extract_json(r#"мусор { "facts": [{"text": "скобка } внутри \" строки {"}] } хвост"#).unwrap();
    assert_eq!(v["facts"][0]["text"], "скобка } внутри \" строки {");
}

fn failing_transcript(err: &str) -> TaskTranscript {
    TaskTranscript {
        user_request: "Сохрани заметку в блокноте".into(),
        steps: vec![
            TranscriptStep { tool: "window_focus".into(), args: json!({"title": "Блокнот"}), ok: true, output: "ok".into(), error: None },
            TranscriptStep { tool: "ui_click".into(), args: json!({"name": "Сохранить"}), ok: false, output: String::new(), error: Some(err.into()) },
            TranscriptStep { tool: "key_press".into(), args: json!({"keys": "ctrl+s"}), ok: true, output: "ok".into(), error: None },
        ],
        final_answer: Some("Сохранил, сэр.".into()),
        success: Some(true),
        user_feedback: None,
        app: Some("Блокнот".into()),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reflector_builds_prompt_and_parses_mock_reply() {
    let model = Arc::new(MockModel::ok(format!("Вот JSON:\n```json\n{GOOD_JSON}\n```")));
    let r = Reflector::new(model.clone());
    let t = failing_transcript("element 'Сохранить' not found");
    let res = r.reflect(&t).await.unwrap();
    check_good(&res);
    // ошибка шага подставлена в урок того же инструмента → сольётся с заготовкой по сигнатуре
    assert_eq!(res.lessons[0].error_text.as_deref(), Some("element 'Сохранить' not found"));
    assert_eq!(model.calls(), 1);
    let req = model.last.lock().unwrap().clone().unwrap();
    assert_eq!(req.messages[0].role, Role::System);
    assert!(req.messages[0].content.contains("строго один JSON-объект"));
    let user = &req.messages[1].content;
    assert!(user.contains("Сохрани заметку в блокноте"));
    assert!(user.contains("<untrusted source=\"tool_outputs\">"), "вывод инструментов — недоверенные данные");
    assert!(user.contains("2. ui_click"));
    assert!(user.contains("ОШИБКА"));
    assert_eq!(req.temperature, Some(0.1));
    assert!(req.tools.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn reflector_errors_propagate() {
    let r = Reflector::new(Arc::new(MockModel::err(LlmError::Timeout)));
    assert!(matches!(r.reflect(&TaskTranscript::default()).await, Err(ReflectError::Llm(LlmError::Timeout))));
    let r = Reflector::new(Arc::new(MockModel::ok("не знаю")));
    assert!(matches!(r.reflect(&TaskTranscript::default()).await, Err(ReflectError::Parse(_))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reflect_and_apply_refines_stub_and_dedups() {
    let (_d, mem) = temp_mem();
    let err = "element 'Сохранить' not found in window 'Безымянный — Блокнот'";
    // во время задачи агент сразу записал заготовку
    let stub = mem.add_lesson(lesson_stub_from_tool_error("ui_click", Some("Блокнот"), err)).unwrap().id;
    let model = Arc::new(MockModel::ok(GOOD_JSON));
    let r = Reflector::new(model.clone());
    let t = failing_transcript(err);

    let (res, rep) = r.reflect_and_apply(&mem, &t, CancellationToken::new()).await.unwrap();
    assert_eq!(res.lessons.len(), 1);
    assert_eq!(rep.lessons[0].id, stub, "урок рефлектора слит с заготовкой");
    let l = mem.get_lesson(stub).unwrap().unwrap();
    assert!(!l.is_stub);
    assert_eq!(l.fix, "нажать Ctrl+S");
    assert_eq!(l.seen, 1, "уточнение не считается повтором");
    assert_eq!(rep.facts[0].status, SaveStatus::Inserted);
    assert_eq!(rep.playbooks[0].status, SaveStatus::Inserted);
    // повторная рефлексия той же задачи ничего не дублирует
    let (_, rep2) = r.reflect_and_apply(&mem, &t, CancellationToken::new()).await.unwrap();
    assert_eq!(rep2.facts[0].status, SaveStatus::Merged);
    assert_eq!(rep2.playbooks[0].status, SaveStatus::Unchanged);
    assert_eq!(rep2.lessons[0].id, stub);
    let s = mem.stats().unwrap();
    assert_eq!((s.facts, s.lessons, s.playbooks), (1, 1, 1));
    // во второй раз модель видела уже известное
    let req = model.last.lock().unwrap().clone().unwrap();
    assert!(req.messages[1].content.contains("Уже есть в памяти"), "{}", req.messages[1].content);
    // и теперь та же ошибка решается из памяти мгновенно
    let m = mem.lesson_for_error(err).unwrap().unwrap();
    assert!(m.exact);
    assert_eq!(m.lesson.fix, "нажать Ctrl+S");
}

#[test]
fn transcript_from_chat_history() {
    let mut a1 = ChatMessage::assistant("");
    a1.tool_calls = vec![
        ToolCall { id: "c1".into(), name: "open_target".into(), arguments: json!({"name": "хром"}) },
        ToolCall { id: "c2".into(), name: "ui_click".into(), arguments: json!({"name": "Вкладка"}) },
    ];
    let msgs = vec![
        ChatMessage::system("ты Джарвис"),
        ChatMessage::user("открой хром"),
        a1,
        ChatMessage::tool_result("c1", "окно открыто"),
        ChatMessage::tool_result("c2", "Ошибка: элемент не найден"),
        ChatMessage::assistant("Открыл хром, сэр."),
        ChatMessage::user("нет, не так"),
    ];
    let t = TaskTranscript::from_chat(&msgs);
    assert_eq!(t.user_request, "открой хром");
    assert_eq!(t.steps.len(), 2);
    assert!(t.steps[0].ok);
    assert_eq!(t.steps[0].output, "окно открыто");
    assert!(!t.steps[1].ok);
    assert_eq!(t.steps[1].error.as_deref(), Some("Ошибка: элемент не найден"));
    assert_eq!(t.final_answer.as_deref(), Some("Открыл хром, сэр."));
    assert_eq!(t.user_feedback.as_deref(), Some("нет, не так"));
    assert!(t.has_errors());
    let text = t.render(10_000);
    assert!(text.contains("Реплика пользователя после ответа: нет, не так"));
    assert!(t.render(50).chars().count() <= 50);
}

#[tokio::test]
async fn empty_reply_is_retried_once_with_bigger_budget() {
    // gpt-oss тратит бюджет на рассуждения и отдаёт пустой текст — один повтор с бюджетом ×2.
    let model = Arc::new(MockModel::ok("   "));
    let r = Reflector::new(model.clone());
    let t = TaskTranscript { user_request: "Разговор без задач:\nПользователь: да пошёл ты".into(), ..Default::default() };
    let e = r.reflect(&t).await.unwrap_err();
    assert!(e.to_string().contains("пустой ответ"), "{e}");
    assert_eq!(model.calls(), 2);
    assert_eq!(model.last.lock().unwrap().as_ref().unwrap().max_tokens, Some(3000));
}
