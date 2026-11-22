//! Быстрые заготовки уроков без LLM: ошибка инструмента, «нет, не так», опечатка пользователя.
//! Заготовку потом дорабатывает рефлектор (`refine_lesson` сливает по сигнатуре/похожести).

use crate::model::{LessonScope, NewLesson};
use crate::playbooks::is_tool_name;
use crate::signature::error_signature;
use crate::text::{clean_text, fold, truncate_chars};

const STUB_CONFIDENCE: f64 = 0.3;

fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    truncate_chars(&clean_text(line), 200)
}

/// Заготовка урока из ошибки/таймаута инструмента: сигнатура считается из `error_text`.
pub fn lesson_stub_from_tool_error(tool: &str, app: Option<&str>, error_text: &str) -> NewLesson {
    let tool = tool.trim();
    NewLesson {
        scope: LessonScope::Tool,
        trigger: format!("вызов {tool}"),
        mistake: first_line(error_text),
        cause: String::new(),
        fix: String::new(),
        tool: (!tool.is_empty()).then(|| tool.to_string()),
        app: app.map(str::trim).filter(|a| !a.is_empty()).map(str::to_string),
        error_text: Some(error_text.to_string()),
        signature: None,
        confidence: STUB_CONFIDENCE,
        is_stub: true,
    }
}

/// Фразы-поправки (после `fold`, по словам). «нет» само по себе — не поправка (это ответ на вопрос).
const CORRECTION_PHRASES: &[&[&str]] = &[
    &["не", "так"],
    &["не", "то"],
    &["не", "тот"],
    &["не", "та"],
    &["не", "туда"],
    &["не", "это"],
    &["не", "этот"],
    &["не", "эту"],
    &["ты", "ошибся"],
    &["ты", "ошиблась"],
    &["ошибся"],
    &["неправильно"],
    &["не", "правильно"],
    &["неверно"],
    &["не", "верно"],
    &["я", "не", "просил"],
    &["я", "не", "просила"],
    &["не", "надо", "было"],
    &["зачем", "ты"],
    &["wrong"],
    &["not", "that"],
    &["that's", "not"],
];

fn words(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Похоже ли на поправку ассистента: «нет, не так», «ты ошибся», «не то», «неправильно»…
pub fn is_correction(user_text: &str) -> bool {
    let w = words(user_text);
    // поправка — в начале реплики, а не где-то в длинном рассказе
    let head = &w[..w.len().min(8)];
    CORRECTION_PHRASES.iter().any(|p| head.windows(p.len()).any(|win| win.iter().zip(p.iter()).all(|(a, b)| a == b)))
}

/// Инструмент из описания действия: «open_target {"name":"хром"}» → `open_target`.
fn tool_of(action: &str) -> Option<String> {
    let head: String = action.trim().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    is_tool_name(&head).then_some(head)
}

/// Заготовка урока из поправки пользователя. `None`, если реплика не похожа на поправку.
/// `last_action` — что ассистент сделал перед этим (например, `open_target {"name":"хром"}`).
pub fn lesson_stub_from_correction(user_text: &str, last_action: Option<&str>) -> Option<NewLesson> {
    if !is_correction(user_text) {
        return None;
    }
    let action = last_action.map(|a| truncate_chars(&clean_text(a), 300)).filter(|a| !a.is_empty());
    let tool = action.as_deref().and_then(tool_of);
    let signature = action.as_deref().map(|a| error_signature(&format!("correction: {a}")));
    Some(NewLesson {
        scope: if tool.is_some() { LessonScope::Tool } else { LessonScope::General },
        trigger: action.clone().unwrap_or_else(|| "последнее действие ассистента".into()),
        mistake: format!("пользователь поправил: «{}»", truncate_chars(&clean_text(user_text), 200)),
        cause: String::new(),
        fix: String::new(),
        tool,
        app: None,
        error_text: None,
        signature,
        confidence: STUB_CONFIDENCE,
        is_stub: true,
    })
}

/// Урок о привычной ошибке пользователя (план 3б «Поправляет меня»): что ввёл → как правильно.
/// Сигнатура — по тексту ошибки, если он есть, иначе по самой неверной команде.
pub fn lesson_user_habit(wrong: &str, right: &str, error_text: Option<&str>) -> NewLesson {
    let wrong = clean_text(wrong);
    let right = clean_text(right);
    let (error_text, signature) = match error_text.filter(|e| !e.trim().is_empty()) {
        Some(e) => (Some(e.to_string()), None),
        None => (None, Some(error_signature(&format!("user_habit: {wrong}")))),
    };
    NewLesson {
        scope: LessonScope::UserHabits,
        trigger: format!("пользователь вводит «{wrong}»"),
        mistake: format!("ошибка в команде «{wrong}»"),
        cause: String::new(),
        fix: right,
        tool: None,
        app: None,
        error_text,
        signature,
        confidence: 0.7,
        is_stub: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_corrections() {
        for t in ["Нет, не так", "ты ошибся", "не то", "Неправильно!", "нет, я не просил это закрывать", "не туда"] {
            assert!(is_correction(t), "{t}");
        }
        for t in ["нет", "да, сэр", "не только хром, но и edge", "открой то окно", "всё верно"] {
            assert!(!is_correction(t), "{t}");
        }
    }

    #[test]
    fn tool_error_stub() {
        let l = lesson_stub_from_tool_error("powershell_run", Some("Windows Terminal"), "Access is denied.\nmore");
        assert!(l.is_stub);
        assert_eq!(l.tool.as_deref(), Some("powershell_run"));
        assert_eq!(l.mistake, "Access is denied.");
        assert_eq!(l.scope, LessonScope::Tool);
        assert!(l.fix.is_empty());
    }

    #[test]
    fn correction_stub() {
        let l = lesson_stub_from_correction("нет, не так", Some(r#"open_target {"name":"хром"}"#)).unwrap();
        assert_eq!(l.tool.as_deref(), Some("open_target"));
        assert!(l.signature.is_some());
        assert!(l.mistake.contains("нет, не так"));
        assert!(lesson_stub_from_correction("спасибо", Some("x")).is_none());
        let g = lesson_stub_from_correction("ты ошибся", None).unwrap();
        assert_eq!(g.scope, LessonScope::General);
        assert!(g.signature.is_none());
    }
}
