//! Инструменты агента: `kb_search`, `kb_lookup_command`, `kb_status` (все T0 — только чтение).
//!
//! Вызовы синхронные: запрос к локальной SQLite занимает миллисекунды, отдельный поток не нужен.
//! Текст документации — внешние данные, поэтому `ToolOutput.untrusted = true` (агент обернёт в `<untrusted>`).

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

use crate::{Kb, KbHit};

pub const DEFAULT_TOP_K: usize = 6;
const MAX_TOP_K: usize = 20;
const MAX_QUERIES: usize = 5;

/// Все три инструмента над одной открытой базой.
pub fn tools(kb: Arc<Kb>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(KbSearchTool::new(kb.clone())),
        Arc::new(KbLookupCommandTool::new(kb.clone())),
        Arc::new(KbStatusTool::new(kb)),
    ]
}

fn format_hits(header: &str, hits: &[KbHit]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{header}");
    for (i, h) in hits.iter().enumerate() {
        let heading = if h.heading.is_empty() { String::new() } else { format!(" — {}", h.heading) };
        let _ = writeln!(s, "[{}] {}{}\n    {}\n    {}", i + 1, h.title, heading, h.url, h.snippet);
    }
    s
}

fn hits_json(hits: &[KbHit]) -> Value {
    json!({ "hits": hits })
}

fn failed(e: crate::KbError) -> ToolError {
    ToolError::Failed(e.to_string())
}

pub struct KbSearchTool {
    kb: Arc<Kb>,
}

impl KbSearchTool {
    pub fn new(kb: Arc<Kb>) -> Self {
        Self { kb }
    }
}

/// `queries` — массив строк; терпим и одну строку (модели иногда так делают).
fn parse_queries(args: &Value) -> Result<Vec<String>, ToolError> {
    let q = args.get("queries").or_else(|| args.get("query"));
    let mut out: Vec<String> = match q {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => return Err(ToolError::InvalidArgs("нужен queries: массив строк-запросов".into())),
    };
    out.retain(|s| !s.trim().is_empty());
    out.truncate(MAX_QUERIES);
    if out.is_empty() {
        return Err(ToolError::InvalidArgs("queries пуст".into()));
    }
    Ok(out)
}

#[async_trait]
impl Tool for KbSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "kb_search".into(),
            description: "Поиск по локальной базе знаний Windows: документация Microsoft Learn (команды cmd и \
                          Windows Server, PowerShell 5.1/7.x и модули Windows, Sysinternals, Win32 API, Windows \
                          client: безопасность, Defender, брандмауэр, настройка) и шпаргалки Джарвиса. \
                          Формулируй 2–3 англоязычных запроса по вопросу пользователя (разными словами, с \
                          именами команд, если знаешь: [\"disable startup apps Windows 11\", \"Task Manager \
                          startup impact\"]). Возвращает фрагменты со ссылками на learn.microsoft.com — опирайся \
                          на них и называй источник."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": MAX_QUERIES,
                        "description": "2–3 англоязычных поисковых запроса по вопросу пользователя"
                    },
                    "top_k": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_TOP_K,
                        "default": DEFAULT_TOP_K,
                        "description": "Сколько документов вернуть"
                    }
                },
                "required": ["queries"]
            }),
            tier: RiskTier::T0,
        }
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let queries = parse_queries(&args)?;
        let top_k = args
            .get("top_k")
            .and_then(Value::as_u64)
            .map(|k| (k as usize).clamp(1, MAX_TOP_K))
            .unwrap_or(DEFAULT_TOP_K);
        let hits = self.kb.search(&queries, top_k).map_err(failed)?;
        let qs = queries.iter().map(|q| format!("«{q}»")).collect::<Vec<_>>().join(", ");
        let content = if hits.is_empty() {
            format!("В базе знаний ничего не найдено по запросам {qs}. Попробуй другие формулировки или web_search.")
        } else {
            format_hits(&format!("База знаний Windows: {} результат(ов) по запросам {qs}", hits.len()), &hits)
        };
        Ok(ToolOutput { content, data: Some(hits_json(&hits)), untrusted: true, attachments: vec![] })
    }
}

pub struct KbLookupCommandTool {
    kb: Arc<Kb>,
}

impl KbLookupCommandTool {
    pub fn new(kb: Arc<Kb>) -> Self {
        Self { kb }
    }
}

#[async_trait]
impl Tool for KbLookupCommandTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "kb_lookup_command".into(),
            description: "Справка по конкретной команде Windows или командлету PowerShell по точному имени \
                          (robocopy, sc.exe, netsh, Get-Service, Set-MpPreference): страница команды и примеры. \
                          Для вопросов «как сделать…» используй kb_search."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Имя команды или командлета" }
                },
                "required": ["name"]
            }),
            tier: RiskTier::T0,
        }
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::InvalidArgs("нужен name: имя команды".into()))?;
        let hits = self.kb.lookup_command(name).map_err(failed)?;
        let content = if hits.is_empty() {
            format!("Команда «{name}» в базе знаний не найдена. Попробуй kb_search или Get-Help {name}.")
        } else {
            format_hits(&format!("Команда «{name}»:"), &hits)
        };
        Ok(ToolOutput { content, data: Some(hits_json(&hits)), untrusted: true, attachments: vec![] })
    }
}

pub struct KbStatusTool {
    kb: Arc<Kb>,
}

impl KbStatusTool {
    pub fn new(kb: Arc<Kb>) -> Self {
        Self { kb }
    }
}

#[async_trait]
impl Tool for KbStatusTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "kb_status".into(),
            description: "Состояние локальной базы знаний Windows: сколько фрагментов, какие источники и когда \
                          собрана."
                .into(),
            parameters: json!({ "type": "object", "properties": {} }),
            tier: RiskTier::T0,
        }
    }

    async fn call(&self, _args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let st = self.kb.status().map_err(failed)?;
        let mut s = format!(
            "База знаний: {} фрагментов, {} источников, собрана {} ({:.1} МБ)\n",
            st.chunks,
            st.sources.len(),
            st.built_at,
            st.size_bytes as f64 / 1_048_576.0
        );
        for src in &st.sources {
            let commit = src.commit_id.get(..8).unwrap_or(&src.commit_id);
            let _ = writeln!(s, "- {} ({}): {} файлов, {} фрагментов, {} {}", src.id, src.name, src.files,
                src.chunks, commit, src.commit_date);
        }
        Ok(ToolOutput { content: s, data: Some(serde_json::to_value(&st).unwrap_or_default()), ..Default::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixture;

    fn kb() -> (tempfile::TempDir, Arc<Kb>) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("kb.sqlite");
        test_fixture::build(&p);
        let kb = Arc::new(Kb::open(&p).unwrap());
        (dir, kb)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn specs_are_t0_and_registered() {
        let (_d, kb) = kb();
        let mut reg = jarvis_core::ToolRegistry::new();
        reg.extend(tools(kb));
        let specs = reg.specs();
        let names: Vec<_> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["kb_lookup_command", "kb_search", "kb_status"]);
        assert!(specs.iter().all(|s| s.tier == RiskTier::T0));
        let search = specs.iter().find(|s| s.name == "kb_search").unwrap();
        assert!(search.description.contains("Формулируй 2–3 англоязычных запроса"));
        assert_eq!(search.parameters["properties"]["top_k"]["default"], 6);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_call() {
        let (_d, kb) = kb();
        let t = KbSearchTool::new(kb);
        let out = t
            .call(json!({"queries": ["robocopy mirror", "copy directory tree"], "top_k": 2}), &ToolCtx::for_tests())
            .await
            .unwrap();
        assert!(out.untrusted);
        assert!(out.content.contains("[1] robocopy"), "{}", out.content);
        assert!(out.content.contains("https://learn.microsoft.com/"));
        let hits = out.data.unwrap()["hits"].as_array().unwrap().len();
        assert!(hits <= 2 && hits > 0);
        // строка вместо массива тоже принимается
        let out = t.call(json!({"queries": "Get-Service"}), &ToolCtx::for_tests()).await.unwrap();
        assert!(out.content.contains("Get-Service"));
        assert!(matches!(t.call(json!({}), &ToolCtx::for_tests()).await, Err(ToolError::InvalidArgs(_))));
        assert!(matches!(t.call(json!({"queries": [" "]}), &ToolCtx::for_tests()).await, Err(ToolError::InvalidArgs(_))));
        let out = t.call(json!({"queries": ["zzzzqqq"]}), &ToolCtx::for_tests()).await.unwrap();
        assert!(out.content.contains("ничего не найдено"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn search_respects_cancel() {
        let (_d, kb) = kb();
        let ctx = ToolCtx::for_tests();
        ctx.cancel.cancel();
        let r = KbSearchTool::new(kb).call(json!({"queries": ["x"]}), &ctx).await;
        assert!(matches!(r, Err(ToolError::Cancelled)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lookup_and_status_calls() {
        let (_d, kb) = kb();
        let out = KbLookupCommandTool::new(kb.clone())
            .call(json!({"name": "robocopy"}), &ToolCtx::for_tests())
            .await
            .unwrap();
        assert!(out.content.starts_with("Команда «robocopy»:"));
        let out = KbLookupCommandTool::new(kb.clone())
            .call(json!({"name": "zzzz"}), &ToolCtx::for_tests())
            .await
            .unwrap();
        assert!(out.content.contains("не найдена"));
        let out = KbStatusTool::new(kb).call(json!({}), &ToolCtx::for_tests()).await.unwrap();
        assert!(out.content.contains("11 фрагментов"), "{}", out.content);
        assert!(!out.untrusted);
        assert_eq!(out.data.unwrap()["chunks"], 11);
    }
}
