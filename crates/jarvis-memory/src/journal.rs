//! Эпизоды (сессии и сообщения) и аудит действий.

use jarvis_core::event::ActionStatus;
use jarvis_core::{RiskTier, Role};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::db::{Memory, fts_hits, now};
use crate::error::{MemoryError, Result};
use crate::model::{Action, Message, NewAction, Session};
use crate::text::{fts_query, stem_text, truncate_chars};

/// Сколько символов сообщения индексировать для поиска (длинные выводы инструментов обрезаются).
const MAX_INDEXED_CHARS: usize = 4000;
const MAX_SUMMARY_CHARS: usize = 2000;

/// Enum → строка через serde (`T0`, `done`, `user`).
fn enum_str<T: Serialize>(v: &T) -> Result<String> {
    match serde_json::to_value(v)? {
        Value::String(s) => Ok(s),
        other => Err(MemoryError::Invalid(format!("ожидалась строка, получено {other}"))),
    }
}

fn enum_col<T: DeserializeOwned>(r: &Row, idx: usize) -> rusqlite::Result<T> {
    let s: String = r.get(idx)?;
    serde_json::from_value(Value::String(s))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

fn row_session(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session { id: r.get(0)?, started_at: r.get(1)?, ended_at: r.get(2)?, title: r.get(3)?, summary: r.get(4)? })
}

fn row_message(r: &Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: r.get(0)?,
        session_id: r.get(1)?,
        ts: r.get(2)?,
        role: enum_col(r, 3)?,
        content: r.get(4)?,
        tool_name: r.get(5)?,
    })
}

fn row_action(r: &Row) -> rusqlite::Result<Action> {
    let args: String = r.get(4)?;
    Ok(Action {
        id: r.get(0)?,
        ts: r.get(1)?,
        session_id: r.get(2)?,
        tool: r.get(3)?,
        args: serde_json::from_str(&args).unwrap_or(Value::String(args)),
        tier: enum_col::<RiskTier>(r, 5)?,
        status: enum_col::<ActionStatus>(r, 6)?,
        result_summary: r.get(7)?,
        approved_by: r.get(8)?,
    })
}

const MESSAGE_COLS: &str = "id, session_id, ts, role, content, tool_name";
const ACTION_COLS: &str = "id, ts, session_id, tool, args, tier, status, result_summary, approved_by";

fn get_message_conn(conn: &Connection, id: i64) -> Result<Option<Message>> {
    Ok(conn
        .query_row(&format!("SELECT {MESSAGE_COLS} FROM messages WHERE id = ?1"), [id], row_message)
        .optional()?)
}

impl Memory {
    /// Начать эпизод диалога.
    pub fn start_session(&self, title: Option<&str>) -> Result<i64> {
        let c = self.conn();
        c.execute("INSERT INTO sessions (started_at, title) VALUES (?1, ?2)", params![now(), title])?;
        Ok(c.last_insert_rowid())
    }

    /// Закрыть эпизод с кратким итогом.
    pub fn end_session(&self, id: i64, summary: Option<&str>) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE sessions SET ended_at = ?1, summary = coalesce(?2, summary) WHERE id = ?3",
            params![now(), summary, id],
        )?;
        if n == 0 {
            return Err(MemoryError::NotFound("session", id));
        }
        Ok(())
    }

    pub fn list_sessions(&self, limit: i64, offset: i64) -> Result<Vec<Session>> {
        let c = self.conn();
        let mut st = c.prepare_cached(
            "SELECT id, started_at, ended_at, title, summary FROM sessions ORDER BY started_at DESC, id DESC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = st.query_map(params![limit, offset], row_session)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Удалить эпизод вместе с сообщениями.
    pub fn delete_session(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM sessions WHERE id = ?1", [id])? > 0)
    }

    /// Записать реплику в эпизод.
    pub fn log_message(&self, session_id: i64, role: Role, content: &str, tool_name: Option<&str>) -> Result<i64> {
        let c = self.conn();
        if !Memory::exists(&c, "sessions", session_id)? {
            return Err(MemoryError::NotFound("session", session_id));
        }
        let stem = stem_text(&truncate_chars(content, MAX_INDEXED_CHARS));
        c.execute(
            "INSERT INTO messages (session_id, ts, role, content, tool_name, s_content) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, now(), enum_str(&role)?, content, tool_name, stem],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn session_messages(&self, session_id: i64) -> Result<Vec<Message>> {
        let c = self.conn();
        let mut st = c.prepare_cached(&format!("SELECT {MESSAGE_COLS} FROM messages WHERE session_id = ?1 ORDER BY id"))?;
        let rows = st.query_map([session_id], row_message)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn all_messages(&self) -> Result<Vec<Message>> {
        let c = self.conn();
        let mut st = c.prepare_cached(&format!("SELECT {MESSAGE_COLS} FROM messages ORDER BY id"))?;
        let rows = st.query_map([], row_message)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Поиск по истории диалогов.
    pub fn search_messages(&self, query: &str, k: usize) -> Result<Vec<Message>> {
        let Some(expr) = fts_query(query) else { return Ok(vec![]) };
        let c = self.conn();
        let mut out = Vec::new();
        for (id, _) in fts_hits(&c, "messages", "", &expr, k)? {
            if let Some(m) = get_message_conn(&c, id)? {
                out.push(m);
            }
        }
        Ok(out)
    }

    /// Записать действие в аудит. Возвращает id (для последующего [`update_action`](Self::update_action)).
    pub fn record_action(&self, a: NewAction) -> Result<i64> {
        let c = self.conn();
        c.execute(
            "INSERT INTO actions (ts, session_id, tool, args, tier, status, result_summary, approved_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                now(),
                a.session_id,
                a.tool,
                serde_json::to_string(&a.args)?,
                enum_str(&a.tier)?,
                enum_str(&a.status)?,
                truncate_chars(&a.result_summary, MAX_SUMMARY_CHARS),
                a.approved_by
            ],
        )?;
        Ok(c.last_insert_rowid())
    }

    /// Обновить статус/итог действия (running → done/failed, подтверждение T3).
    pub fn update_action(
        &self,
        id: i64,
        status: ActionStatus,
        result_summary: Option<&str>,
        approved_by: Option<&str>,
    ) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE actions SET status = ?1, result_summary = coalesce(?2, result_summary),
                    approved_by = coalesce(?3, approved_by) WHERE id = ?4",
            params![enum_str(&status)?, result_summary.map(|s| truncate_chars(s, MAX_SUMMARY_CHARS)), approved_by, id],
        )?;
        if n == 0 {
            return Err(MemoryError::NotFound("action", id));
        }
        Ok(())
    }

    /// Журнал действий: новые сверху.
    pub fn list_actions(&self, limit: i64, offset: i64) -> Result<Vec<Action>> {
        let c = self.conn();
        let mut st = c.prepare_cached(&format!(
            "SELECT {ACTION_COLS} FROM actions ORDER BY ts DESC, id DESC LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = st.query_map(params![limit, offset], row_action)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get_action(&self, id: i64) -> Result<Option<Action>> {
        Ok(self
            .conn()
            .query_row(&format!("SELECT {ACTION_COLS} FROM actions WHERE id = ?1"), [id], row_action)
            .optional()?)
    }
}
