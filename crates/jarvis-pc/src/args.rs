//! Разбор аргументов инструментов (JSON от LLM). Модели иногда присылают числа строками — принимаем и так.

use jarvis_core::{RiskTier, ToolError, ToolSpec};
use serde_json::{Map, Value};

pub(crate) fn spec(name: &str, description: &str, parameters: Value, tier: RiskTier) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), parameters, tier }
}

fn obj(args: &Value) -> Option<&Map<String, Value>> {
    args.as_object()
}

pub(crate) fn get<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    obj(args).and_then(|m| m.get(key)).filter(|v| !v.is_null())
}

pub(crate) fn str_opt(args: &Value, key: &str) -> Option<String> {
    match get(args, key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub(crate) fn str_req(args: &Value, key: &str) -> Result<String, ToolError> {
    str_opt(args, key).ok_or_else(|| ToolError::InvalidArgs(format!("нужен строковый аргумент «{key}»")))
}

/// Непустая строка (после trim).
pub(crate) fn str_nonempty(args: &Value, key: &str) -> Option<String> {
    str_opt(args, key).filter(|s| !s.trim().is_empty())
}

pub(crate) fn i64_opt(args: &Value, key: &str) -> Result<Option<i64>, ToolError> {
    let Some(v) = get(args, key) else { return Ok(None) };
    let bad = || ToolError::InvalidArgs(format!("«{key}» должно быть числом"));
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)).map(Some).ok_or_else(bad),
        Value::String(s) => {
            let s = s.trim();
            s.parse::<i64>().ok().or_else(|| s.parse::<f64>().ok().map(|f| f.round() as i64)).map(Some).ok_or_else(bad)
        }
        _ => Err(bad()),
    }
}

pub(crate) fn i32_opt(args: &Value, key: &str) -> Result<Option<i32>, ToolError> {
    match i64_opt(args, key)? {
        None => Ok(None),
        Some(v) => i32::try_from(v).map(Some).map_err(|_| ToolError::InvalidArgs(format!("«{key}» вне диапазона"))),
    }
}

pub(crate) fn i32_req(args: &Value, key: &str) -> Result<i32, ToolError> {
    i32_opt(args, key)?.ok_or_else(|| ToolError::InvalidArgs(format!("нужен числовой аргумент «{key}»")))
}

pub(crate) fn u64_opt(args: &Value, key: &str) -> Result<Option<u64>, ToolError> {
    match i64_opt(args, key)? {
        None => Ok(None),
        Some(v) if v >= 0 => Ok(Some(v as u64)),
        Some(_) => Err(ToolError::InvalidArgs(format!("«{key}» не может быть отрицательным"))),
    }
}

pub(crate) fn bool_opt(args: &Value, key: &str, default: bool) -> bool {
    match get(args, key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "true" | "1" | "yes" | "да"),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => default,
    }
}

/// Обрезать строку до `max` символов (по символам, не байтам) с многоточием.
pub(crate) fn clip(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(max).collect();
    if it.next().is_some() { format!("{head}…") } else { head }
}

/// Сравнение строк без учёта регистра и «ё/е».
pub(crate) fn fold(s: &str) -> String {
    s.to_lowercase().replace('ё', "е")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_from_strings_and_floats() {
        let a = json!({"x": "12", "y": 7.6, "z": "abc", "n": null, "b": "да"});
        assert_eq!(i32_opt(&a, "x").unwrap(), Some(12));
        assert_eq!(i32_opt(&a, "y").unwrap(), Some(8));
        assert!(i32_opt(&a, "z").is_err());
        assert_eq!(i32_opt(&a, "n").unwrap(), None);
        assert_eq!(i32_opt(&a, "missing").unwrap(), None);
        assert!(bool_opt(&a, "b", false));
        assert!(i32_req(&a, "missing").is_err());
    }

    #[test]
    fn clip_and_fold() {
        assert_eq!(clip("привет", 3), "при…");
        assert_eq!(clip("ok", 3), "ok");
        assert_eq!(fold("Ёлка СОХРАНИТЬ"), "елка сохранить");
    }
}
