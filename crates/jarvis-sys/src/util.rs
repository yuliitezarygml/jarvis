//! Общие помощники: аргументы инструментов, обрезка вывода, форматирование.

use jarvis_core::{RiskTier, ToolError, ToolSpec};
use serde_json::Value;

pub fn spec(name: &str, description: &str, parameters: Value, tier: RiskTier) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), parameters, tier }
}

pub fn req_str(args: &Value, key: &str) -> Result<String, ToolError> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => Err(ToolError::InvalidArgs(format!("нужен строковый аргумент «{key}»"))),
    }
}

pub fn opt_str(args: &Value, key: &str) -> Option<String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

pub fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    match args.get(key) {
        Some(Value::Number(n)) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    match args.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) => match s.to_lowercase().as_str() {
            "true" | "1" | "yes" | "да" => Some(true),
            "false" | "0" | "no" | "нет" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn opt_str_list(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => s.split([',', ';']).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// Обрезать середину длинного текста. Возвращает (текст, было ли обрезано).
pub fn truncate_middle(s: &str, max_chars: usize) -> (String, bool) {
    let n = s.chars().count();
    if n <= max_chars {
        return (s.to_string(), false);
    }
    let head = max_chars * 6 / 10;
    let tail = max_chars - head;
    let h: String = s.chars().take(head).collect();
    let t: String = s.chars().skip(n - tail).collect();
    (format!("{h}\n…[пропущено {} символов]…\n{t}", n - head - tail), true)
}

/// «1,5 ГБ», «820 МБ», «12 КБ».
pub fn fmt_bytes(b: u64) -> String {
    const K: f64 = 1024.0;
    let f = b as f64;
    let (v, u) = if f >= K * K * K * K {
        (f / (K * K * K * K), "ТБ")
    } else if f >= K * K * K {
        (f / (K * K * K), "ГБ")
    } else if f >= K * K {
        (f / (K * K), "МБ")
    } else if f >= K {
        (f / K, "КБ")
    } else {
        return format!("{b} Б");
    };
    let s = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{} {u}", s.replace('.', ","))
}

/// Нижний регистр, ё→е, без пунктуации, одинарные пробелы.
pub fn norm_text(s: &str) -> String {
    s.to_lowercase()
        .replace('ё', "е")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_and_bytes() {
        let s = "a".repeat(100);
        let (t, cut) = truncate_middle(&s, 20);
        assert!(cut && t.contains("пропущено 80"));
        assert_eq!(truncate_middle("abc", 10), ("abc".into(), false));
        assert_eq!(fmt_bytes(1536 * 1024 * 1024), "1,5 ГБ");
        assert_eq!(fmt_bytes(512), "512 Б");
        assert_eq!(norm_text("Настройки, Bluetooth!"), "настройки bluetooth");
    }
}
