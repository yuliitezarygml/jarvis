//! Плейбуки — удачные последовательности вызовов инструментов как именованные рецепты.

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::Value;

use crate::db::{Memory, fts_hits, now, write_tx};
use crate::error::{MemoryError, Result};
use crate::model::{NewPlaybook, Playbook, PlaybookStep, SaveStatus, Saved};
use crate::text::{clean_text, dice, fold, fts_query, fts_query_exact, stem_set, stem_text, truncate_chars};

/// Порог похожести «имя + условия» для признания плейбука той же сущностью (→ новая версия).
pub const PLAYBOOK_DUP_THRESHOLD: f64 = 0.75;
pub(crate) const PLAYBOOK_WEIGHTS: &str = ", 3.0, 2.0, 1.0";
const MAX_STEPS: usize = 50;

pub(crate) const PLAYBOOK_COLS: &str = "id, name, when_text, steps, params_schema, success_count, fail_count, version, \
     promoted, created_at, updated_at, last_used";

fn json_col<T: serde::de::DeserializeOwned>(r: &Row, idx: usize) -> rusqlite::Result<T> {
    let s: String = r.get(idx)?;
    serde_json::from_str(&s).map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

pub(crate) fn row_playbook(r: &Row) -> rusqlite::Result<Playbook> {
    Ok(Playbook {
        id: r.get(0)?,
        name: r.get(1)?,
        when: r.get(2)?,
        steps: json_col(r, 3)?,
        params_schema: json_col(r, 4)?,
        success_count: r.get(5)?,
        fail_count: r.get(6)?,
        version: r.get(7)?,
        promoted: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
        last_used: r.get(11)?,
    })
}

pub(crate) fn get_playbook_conn(conn: &Connection, id: i64) -> Result<Option<Playbook>> {
    Ok(conn
        .query_row(&format!("SELECT {PLAYBOOK_COLS} FROM playbooks WHERE id = ?1"), [id], row_playbook)
        .optional()?)
}

fn query_playbooks(conn: &Connection, where_order: &str, p: impl rusqlite::Params) -> Result<Vec<Playbook>> {
    let mut st = conn.prepare_cached(&format!("SELECT {PLAYBOOK_COLS} FROM playbooks {where_order}"))?;
    let rows = st.query_map(p, row_playbook)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn name_key(name: &str) -> String {
    fold(&clean_text(name))
}

/// Имя инструмента: snake_case латиницей.
pub fn is_tool_name(t: &str) -> bool {
    let mut chars = t.chars();
    matches!(chars.next(), Some('a'..='z'))
        && t.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn collect_strings(v: &Value, out: &mut String, depth: usize) {
    if depth > 4 || out.len() > 2000 {
        return;
    }
    match v {
        Value::String(s) => {
            out.push(' ');
            out.push_str(s);
        }
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out, depth + 1)),
        Value::Object(m) => m.values().for_each(|x| collect_strings(x, out, depth + 1)),
        _ => {}
    }
}

pub(crate) fn steps_stem(steps: &[PlaybookStep]) -> String {
    let mut s = String::new();
    for st in steps {
        s.push(' ');
        s.push_str(&st.tool);
        if let Some(n) = &st.note {
            s.push(' ');
            s.push_str(n);
        }
        collect_strings(&st.args, &mut s, 0);
    }
    stem_text(&s)
}

fn is_empty_schema(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

struct Prepared {
    name: String,
    when: String,
    steps: Vec<PlaybookStep>,
    params: Value,
}

fn prepare(p: NewPlaybook) -> Result<Prepared> {
    let name = truncate_chars(&clean_text(&p.name), 200);
    if name.is_empty() {
        return Err(MemoryError::Invalid("у плейбука нет имени".into()));
    }
    if p.steps.is_empty() || p.steps.len() > MAX_STEPS {
        return Err(MemoryError::Invalid(format!("в плейбуке должно быть 1..{MAX_STEPS} шагов")));
    }
    if let Some(bad) = p.steps.iter().find(|s| !is_tool_name(&s.tool)) {
        return Err(MemoryError::Invalid(format!("неверное имя инструмента в плейбуке: {:?}", bad.tool)));
    }
    let params = if p.params_schema.is_null() { Value::Object(Default::default()) } else { p.params_schema };
    Ok(Prepared { name, when: truncate_chars(&clean_text(&p.when), 1000), steps: p.steps, params })
}

fn find_duplicate(conn: &Connection, p: &Prepared) -> Result<Option<Playbook>> {
    if let Some(pb) =
        query_playbooks(conn, "WHERE name_key = ?1", [name_key(&p.name)])?.pop()
    {
        return Ok(Some(pb));
    }
    let set = stem_set(&format!("{} {}", p.name, p.when));
    let Some(expr) = fts_query_exact(&set) else { return Ok(None) };
    let mut best: Option<(Playbook, f64)> = None;
    for (id, _) in fts_hits(conn, "playbooks", PLAYBOOK_WEIGHTS, &expr, 20)? {
        let Some(cand) = get_playbook_conn(conn, id)? else { continue };
        let sim = dice(&set, &stem_set(&format!("{} {}", cand.name, cand.when)));
        if sim >= PLAYBOOK_DUP_THRESHOLD && best.as_ref().is_none_or(|(_, b)| sim > *b) {
            best = Some((cand, sim));
        }
    }
    Ok(best.map(|(p, _)| p))
}

fn map_unique(e: rusqlite::Error) -> MemoryError {
    match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
            MemoryError::Invalid("плейбук с таким именем уже есть".into())
        }
        other => other.into(),
    }
}

impl Memory {
    /// Записать плейбук. Тот же (по имени или похожести «имя + условия») с другими шагами → версия + 1;
    /// с теми же шагами → обновляются условия/параметры (`Merged`) или ничего (`Unchanged`).
    pub fn add_playbook(&self, p: NewPlaybook) -> Result<Saved> {
        let p = prepare(p)?;
        let ts = now();
        let mut c = self.conn();
        let tx = write_tx(&mut c)?;
        let saved = if let Some(old) = find_duplicate(&tx, &p)? {
            let when = if p.when.is_empty() { old.when.clone() } else { p.when.clone() };
            let params_schema = if is_empty_schema(&p.params) { old.params_schema.clone() } else { p.params.clone() };
            let same_steps = old.steps == p.steps;
            let status = if !same_steps {
                SaveStatus::NewVersion
            } else if when != old.when || params_schema != old.params_schema {
                SaveStatus::Merged
            } else {
                SaveStatus::Unchanged
            };
            if status != SaveStatus::Unchanged {
                let steps = if same_steps { &old.steps } else { &p.steps };
                tx.execute(
                    "UPDATE playbooks SET when_text = ?1, steps = ?2, params_schema = ?3, version = ?4, updated_at = ?5,
                            s_when = ?6, s_steps = ?7 WHERE id = ?8",
                    params![
                        when,
                        serde_json::to_string(steps)?,
                        serde_json::to_string(&params_schema)?,
                        old.version + i64::from(!same_steps),
                        ts,
                        stem_text(&when),
                        steps_stem(steps),
                        old.id
                    ],
                )?;
            }
            Saved { id: old.id, status }
        } else {
            tx.execute(
                "INSERT INTO playbooks (name, name_key, when_text, steps, params_schema, created_at, updated_at,
                                        s_name, s_when, s_steps)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9)",
                params![
                    p.name,
                    name_key(&p.name),
                    p.when,
                    serde_json::to_string(&p.steps)?,
                    serde_json::to_string(&p.params)?,
                    ts,
                    stem_text(&p.name),
                    stem_text(&p.when),
                    steps_stem(&p.steps)
                ],
            )?;
            Saved { id: tx.last_insert_rowid(), status: SaveStatus::Inserted }
        };
        tx.commit()?;
        Ok(saved)
    }

    /// Итог выполнения плейбука.
    pub fn record_playbook_result(&self, id: i64, success: bool) -> Result<()> {
        let sql = if success {
            "UPDATE playbooks SET success_count = success_count + 1, last_used = ?2 WHERE id = ?1"
        } else {
            "UPDATE playbooks SET fail_count = fail_count + 1, last_used = ?2 WHERE id = ?1"
        };
        if self.conn().execute(sql, params![id, now()])? == 0 {
            return Err(MemoryError::NotFound("playbook", id));
        }
        Ok(())
    }

    pub fn get_playbook(&self, id: i64) -> Result<Option<Playbook>> {
        get_playbook_conn(&self.conn(), id)
    }

    pub fn playbook_by_name(&self, name: &str) -> Result<Option<Playbook>> {
        Ok(query_playbooks(&self.conn(), "WHERE name_key = ?1", [name_key(name)])?.pop())
    }

    pub fn list_playbooks(&self, limit: i64, offset: i64) -> Result<Vec<Playbook>> {
        query_playbooks(&self.conn(), "ORDER BY updated_at DESC, id DESC LIMIT ?1 OFFSET ?2", params![limit, offset])
    }

    /// Правка из экрана «Память» (имя, условия, шаги, параметры, счётчики).
    pub fn update_playbook(&self, p: &Playbook) -> Result<()> {
        let prep = prepare(NewPlaybook {
            name: p.name.clone(),
            when: p.when.clone(),
            steps: p.steps.clone(),
            params_schema: p.params_schema.clone(),
        })?;
        let n = self
            .conn()
            .execute(
                "UPDATE playbooks SET name = ?1, name_key = ?2, when_text = ?3, steps = ?4, params_schema = ?5,
                        success_count = ?6, fail_count = ?7, version = ?8, promoted = ?9, updated_at = ?10,
                        s_name = ?11, s_when = ?12, s_steps = ?13
                 WHERE id = ?14",
                params![
                    prep.name,
                    name_key(&prep.name),
                    prep.when,
                    serde_json::to_string(&prep.steps)?,
                    serde_json::to_string(&prep.params)?,
                    p.success_count,
                    p.fail_count,
                    p.version.max(1),
                    p.promoted,
                    now(),
                    stem_text(&prep.name),
                    stem_text(&prep.when),
                    steps_stem(&prep.steps),
                    p.id
                ],
            )
            .map_err(map_unique)?;
        if n == 0 {
            return Err(MemoryError::NotFound("playbook", p.id));
        }
        Ok(())
    }

    pub fn delete_playbook(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM playbooks WHERE id = ?1", [id])? > 0)
    }

    pub fn search_playbooks(&self, query: &str, k: usize) -> Result<Vec<Playbook>> {
        Ok(search_playbooks_scored(&self.conn(), query, k)?.into_iter().map(|(p, _)| p).collect())
    }
}

pub(crate) fn search_playbooks_scored(conn: &Connection, query: &str, k: usize) -> Result<Vec<(Playbook, f64)>> {
    let Some(expr) = fts_query(query) else { return Ok(vec![]) };
    let mut out = Vec::new();
    for (id, score) in fts_hits(conn, "playbooks", PLAYBOOK_WEIGHTS, &expr, k)? {
        if let Some(p) = get_playbook_conn(conn, id)? {
            out.push((p, score));
        }
    }
    Ok(out)
}

pub(crate) fn all_playbooks(conn: &Connection) -> Result<Vec<Playbook>> {
    query_playbooks(conn, "ORDER BY id", [])
}
