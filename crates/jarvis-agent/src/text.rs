//! Мелкие текстовые утилиты: обрезка вывода, краткие описания вызовов, канонизация аргументов.

use serde_json::Value;

/// Обрезать длинный вывод до `max` символов: начало (¾) + конец (¼) с пометкой посередине.
/// Конец важен — в выводе терминала ошибка обычно внизу.
pub fn truncate_output(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let head = max * 3 / 4;
    let tail = max - head;
    let head_s: String = s.chars().take(head).collect();
    let tail_s: String = s.chars().skip(n - tail).collect();
    format!("{head_s}\n…[вывод обрезан: показано {max} из {n} символов]…\n{tail_s}")
}

/// Первые `n` символов одной строкой (переводы строк → пробелы).
pub fn preview(s: &str, n: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n {
        flat
    } else {
        let mut p: String = flat.chars().take(n).collect();
        p.push('…');
        p
    }
}

/// Короткое описание вызова для ленты действий и подтверждения: `powershell_run: command=Get-Process`.
pub fn summarize_call(name: &str, args: &Value) -> String {
    let body = match args {
        Value::Object(m) if m.is_empty() => String::new(),
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            keys.iter()
                .map(|k| {
                    let v = match &m[k.as_str()] {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    format!("{k}={v}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        Value::Null => String::new(),
        other => other.to_string(),
    };
    if body.is_empty() { name.to_string() } else { preview(&format!("{name}: {body}"), 200) }
}

/// JSON с отсортированными ключами — чтобы одинаковые вызовы с разным порядком ключей совпадали.
pub fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical_json(&m[k.as_str()])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// Аргументы от провайдера: `null` → `{}`, строка с JSON-объектом → объект.
pub fn normalize_args(v: Value) -> Value {
    match v {
        Value::Null => Value::Object(Default::default()),
        Value::String(s) => match serde_json::from_str::<Value>(&s) {
            Ok(parsed @ Value::Object(_)) => parsed,
            _ => Value::String(s),
        },
        other => other,
    }
}

/// Применить редактор секретов ко всем строкам внутри JSON.
pub fn redact_value(v: &Value, redact: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(redact(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| redact_value(x, redact)).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), redact_value(x, redact))).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncation_keeps_head_and_tail() {
        let s = format!("{}{}", "а".repeat(10_000), "ОШИБКА");
        let t = truncate_output(&s, 8_000);
        assert!(t.starts_with("ааа"));
        assert!(t.ends_with("ОШИБКА"));
        assert!(t.contains("вывод обрезан: показано 8000 из 10006"));
        assert!(t.chars().count() < 8_100);
        assert_eq!(truncate_output("коротко", 8_000), "коротко");
    }

    #[test]
    fn summaries() {
        assert_eq!(summarize_call("sys_snapshot", &json!({})), "sys_snapshot");
        assert_eq!(
            summarize_call("service_stop", &json!({"name": "Spooler", "force": true})),
            "service_stop: force=true, name=Spooler"
        );
        assert!(summarize_call("x", &json!({"t": "я".repeat(500)})).chars().count() <= 201);
        assert_eq!(preview("a\n  b\tc", 10), "a b c");
        assert_eq!(preview("абвгд", 3), "абв…");
    }

    #[test]
    fn canonical_ignores_key_order() {
        let a: Value = serde_json::from_str(r#"{"b":1,"a":{"y":[1,"x"],"x":null}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":{"x":null,"y":[1,"x"]},"b":1}"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_ne!(canonical_json(&a), canonical_json(&json!({"b":2})));
    }

    #[test]
    fn args_normalization_and_redaction() {
        assert_eq!(normalize_args(Value::Null), json!({}));
        assert_eq!(normalize_args(json!("{\"q\":1}")), json!({"q":1}));
        assert_eq!(normalize_args(json!("plain")), json!("plain"));
        let r = redact_value(&json!({"cmd": "echo gsk_1", "n": [1, "gsk_2"]}), &|s: &str| s.replace("gsk_", "***"));
        assert_eq!(r, json!({"cmd": "echo ***1", "n": [1, "***2"]}));
    }
}
