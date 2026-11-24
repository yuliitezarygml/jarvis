//! Инструменты памяти для агента: `memory_remember` (T0), `memory_recall` (T0), `memory_forget` (T2).

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::db::Memory;
use crate::error::MemoryError;
use crate::model::{MemoryKind, NewFact, SaveStatus, category};
use crate::text::{coverage, stem_set, truncate_chars};

/// Доля слов запроса, которая должна быть в факте, чтобы `memory_forget` удалил его без id.
pub const FORGET_COVERAGE: f64 = 0.6;
const FORGET_MAX: usize = 5;

fn mem_err(e: MemoryError) -> ToolError {
    ToolError::Failed(e.to_string())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// Все инструменты памяти.
pub fn memory_tools(mem: Memory) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(RememberTool { mem: mem.clone() }),
        Arc::new(RecallTool { mem: mem.clone() }),
        Arc::new(ForgetTool { mem }),
    ]
}

/// «Запомни, что …» — явный факт от пользователя.
pub struct RememberTool {
    pub mem: Memory,
}

#[async_trait]
impl Tool for RememberTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_remember".into(),
            description: "Запомнить факт о пользователе, когда он явно просит («запомни, что…»). \
                          text — сам факт коротко (например «Пьёт чёрный кофе без сахара»); \
                          category — preference|personal|alias|habit|contact|general|style \
                          (style — как пользователь общается: мат, юмор, что злит, обращение); \
                          important — учитывать всегда."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Факт о пользователе"},
                    "category": {"type": "string", "enum": category::ALL},
                    "important": {"type": "boolean"}
                },
                "required": ["text"]
            }),
            tier: RiskTier::T0,
        }
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let text = str_arg(&args, "text").ok_or_else(|| ToolError::InvalidArgs("нужен text".into()))?.to_string();
        let cat = str_arg(&args, "category").unwrap_or(category::GENERAL).to_string();
        let important = args.get("important").and_then(Value::as_bool).unwrap_or(false);
        let fact = NewFact::explicit(text.clone()).with_category(cat).with_important(important);
        let saved = self.mem.run(move |m| m.upsert_fact(fact)).await.map_err(mem_err)?;
        let verb = if saved.status == SaveStatus::Inserted { "Запомнил" } else { "Обновил в памяти" };
        Ok(ToolOutput {
            content: format!("{verb}: «{text}»."),
            data: Some(json!({"id": saved.id, "status": saved.status})),
            ..Default::default()
        })
    }
}

/// Поиск в памяти: факты, уроки, плейбуки.
pub struct RecallTool {
    pub mem: Memory,
}

#[async_trait]
impl Tool for RecallTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_recall".into(),
            description: "Найти в памяти Джарвиса факты о пользователе, уроки прошлых ошибок и проверенные сценарии \
                          по словам запроса (с учётом словоформ)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Что вспомнить"},
                    "k": {"type": "integer", "minimum": 1, "maximum": 20}
                },
                "required": ["query"]
            }),
            tier: RiskTier::T0,
        }
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let query = str_arg(&args, "query").ok_or_else(|| ToolError::InvalidArgs("нужен query".into()))?.to_string();
        let k = args.get("k").and_then(Value::as_u64).unwrap_or(5).clamp(1, 20) as usize;
        let q = query.clone();
        let (facts, lessons, playbooks) = self
            .mem
            .run(move |m| Ok((m.search_facts(&q, k)?, m.search_lessons(&q, k)?, m.search_playbooks(&q, k)?)))
            .await
            .map_err(mem_err)?;
        if facts.is_empty() && lessons.is_empty() && playbooks.is_empty() {
            return Ok(ToolOutput::text(format!("В памяти ничего не нашлось по запросу «{query}».")));
        }
        let mut out = String::new();
        if !facts.is_empty() {
            out.push_str("Факты:\n");
            for f in &facts {
                out.push_str(&format!("- [факт #{}] {}\n", f.id, f.text));
            }
        }
        if !lessons.is_empty() {
            out.push_str("Уроки:\n");
            for l in &lessons {
                out.push_str(&format!("- [урок #{}] {}\n", l.id, truncate_chars(&l.warning(), 300)));
            }
        }
        if !playbooks.is_empty() {
            out.push_str("Сценарии:\n");
            for p in &playbooks {
                let tools: Vec<&str> = p.steps.iter().map(|s| s.tool.as_str()).collect();
                out.push_str(&format!("- [плейбук #{}] «{}»: {}\n", p.id, p.name, tools.join(" → ")));
            }
        }
        Ok(ToolOutput {
            content: out.trim_end().to_string(),
            data: Some(json!({"facts": facts, "lessons": lessons, "playbooks": playbooks})),
            ..Default::default()
        })
    }
}

/// «Забудь, что …» — удалить запись по id или по запросу (только уверенные совпадения).
pub struct ForgetTool {
    pub mem: Memory,
}

#[async_trait]
impl Tool for ForgetTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_forget".into(),
            description: "Удалить из памяти то, что пользователь просит забыть. Либо id + kind (fact|lesson|playbook) \
                          из memory_recall, либо query — тогда удаляются только факты, явно совпадающие с запросом; \
                          при неуверенности возвращаются кандидаты с id."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "id": {"type": "integer"},
                    "kind": {"type": "string", "enum": ["fact", "lesson", "playbook"]}
                }
            }),
            tier: RiskTier::T2,
        }
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        if let Some(id) = args.get("id").and_then(Value::as_i64) {
            let kind = match str_arg(&args, "kind") {
                Some(k) => MemoryKind::parse(k).ok_or_else(|| ToolError::InvalidArgs(format!("неизвестный kind: {k}")))?,
                None => MemoryKind::Fact,
            };
            let deleted = self.mem.run(move |m| m.delete(kind, id)).await.map_err(mem_err)?;
            let content = if deleted { format!("Забыл запись #{id}.") } else { format!("Записи #{id} в памяти нет.") };
            return Ok(ToolOutput {
                content,
                data: Some(json!({"deleted": if deleted { vec![id] } else { vec![] }})),
                ..Default::default()
            });
        }
        let query =
            str_arg(&args, "query").ok_or_else(|| ToolError::InvalidArgs("нужен id или query".into()))?.to_string();
        let q = query.clone();
        let (deleted, candidates) = self
            .mem
            .run(move |m| {
                let qset = stem_set(&q);
                let hits = m.search_facts(&q, 10)?;
                let mut deleted = Vec::new();
                let mut candidates = Vec::new();
                for f in hits {
                    if !qset.is_empty() && coverage(&qset, &stem_set(&f.text)) >= FORGET_COVERAGE && deleted.len() < FORGET_MAX {
                        m.delete_fact(f.id)?;
                        deleted.push(f);
                    } else {
                        candidates.push(f);
                    }
                }
                Ok((deleted, candidates))
            })
            .await
            .map_err(mem_err)?;
        let content = if !deleted.is_empty() {
            let list: Vec<String> = deleted.iter().map(|f| format!("«{}»", f.text)).collect();
            format!("Забыл: {}.", list.join(", "))
        } else if !candidates.is_empty() {
            let list: Vec<String> = candidates.iter().take(5).map(|f| format!("#{} «{}»", f.id, f.text)).collect();
            format!("Точного совпадения нет, ничего не удалено. Похожие факты: {}. Уточни id.", list.join("; "))
        } else {
            format!("В памяти нет ничего про «{query}».")
        };
        let ids: Vec<i64> = deleted.iter().map(|f| f.id).collect();
        let cand: Vec<i64> = candidates.iter().map(|f| f.id).collect();
        Ok(ToolOutput { content, data: Some(json!({"deleted": ids, "candidates": cand})), ..Default::default() })
    }
}
