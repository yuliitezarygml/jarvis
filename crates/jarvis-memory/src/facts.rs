//! Факты о пользователе: запись с дедупликацией, поиск, CRUD.

use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::BTreeSet;

use crate::db::{Memory, clamp01, fts_hits, now, write_tx};
use crate::error::{MemoryError, Result};
use crate::model::{Fact, FactSource, NewFact, SaveStatus, Saved, category};
use crate::text::{clean_text, dice, fold, fts_query, fts_query_exact, set_of, stem_set, stem_text};

/// Порог похожести (Дайс по основам), выше которого новый факт сливается со старым.
pub const FACT_DUP_THRESHOLD: f64 = 0.8;
const MAX_FACT_CHARS: usize = 2000;
const FACT_WEIGHTS: &str = ", 1.0, 0.3";

pub(crate) const FACT_COLS: &str =
    "id, text, category, confidence, source, important, created_at, updated_at, last_used, use_count";

pub(crate) fn row_fact(r: &Row) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id: r.get(0)?,
        text: r.get(1)?,
        category: r.get(2)?,
        confidence: r.get(3)?,
        source: FactSource::parse(&r.get::<_, String>(4)?),
        important: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
        last_used: r.get(8)?,
        use_count: r.get(9)?,
    })
}

pub(crate) fn normalize_category(c: &str) -> String {
    let c = fold(c.trim());
    if c.is_empty() {
        return category::GENERAL.into();
    }
    match c.as_str() {
        "preferences" | "предпочтение" | "предпочтения" => category::PREFERENCE.into(),
        "алиас" | "aliases" | "псевдоним" => category::ALIAS.into(),
        "привычка" | "habits" => category::HABIT.into(),
        "личное" | "person" => category::PERSONAL.into(),
        _ => crate::text::truncate_chars(&c, 32),
    }
}

pub(crate) fn get_fact_conn(conn: &Connection, id: i64) -> Result<Option<Fact>> {
    Ok(conn
        .query_row(&format!("SELECT {FACT_COLS} FROM facts WHERE id = ?1"), [id], row_fact)
        .optional()?)
}

/// Самый похожий существующий факт (Дайс ≥ порога). Кандидаты — из FTS по точным основам.
pub(crate) fn find_similar_fact(
    conn: &Connection,
    text: &str,
    set: &BTreeSet<String>,
    exclude: Option<i64>,
) -> Result<Option<(Fact, f64)>> {
    let mut best: Option<(i64, f64)> = None;
    if let Some(expr) = fts_query_exact(set) {
        for (id, _) in fts_hits(conn, "facts", FACT_WEIGHTS, &expr, 20)? {
            if Some(id) == exclude {
                continue;
            }
            let s_text: String = conn.query_row("SELECT s_text FROM facts WHERE id = ?1", [id], |r| r.get(0))?;
            let sim = dice(set, &set_of(&s_text));
            if sim >= FACT_DUP_THRESHOLD && best.is_none_or(|(_, b)| sim > b) {
                best = Some((id, sim));
            }
        }
    } else {
        // в тексте нет значимых слов — дедуп только по точному совпадению
        let id: Option<i64> = conn
            .query_row("SELECT id FROM facts WHERE text = ?1 AND id IS NOT ?2", params![text, exclude], |r| r.get(0))
            .optional()?;
        best = id.map(|id| (id, 1.0));
    }
    match best {
        Some((id, sim)) => Ok(get_fact_conn(conn, id)?.map(|f| (f, sim))),
        None => Ok(None),
    }
}

pub(crate) fn validate_fact_text(text: &str) -> Result<String> {
    let t = clean_text(text);
    if t.is_empty() {
        return Err(MemoryError::Invalid("пустой факт".into()));
    }
    if t.chars().count() > MAX_FACT_CHARS {
        return Err(MemoryError::Invalid(format!("факт длиннее {MAX_FACT_CHARS} символов")));
    }
    Ok(t)
}

impl Memory {
    /// Записать факт. Почти-дубль (Дайс ≥ [`FACT_DUP_THRESHOLD`]) обновляет существующий:
    /// текст — новый (кроме случая «выведенный поверх явного»), confidence — максимум,
    /// явный источник и `important` побеждают.
    pub fn upsert_fact(&self, f: NewFact) -> Result<Saved> {
        let text = validate_fact_text(&f.text)?;
        let cat = normalize_category(&f.category);
        let conf = clamp01(f.confidence);
        let set = stem_set(&text);
        let ts = now();
        let mut c = self.conn();
        let tx = write_tx(&mut c)?;
        let saved = if let Some((old, _)) = find_similar_fact(&tx, &text, &set, None)? {
            let take_new_text = f.source == FactSource::Explicit || old.source == FactSource::Inferred;
            let new_text = if take_new_text { text } else { old.text.clone() };
            let new_cat = if cat != category::GENERAL { cat } else { old.category.clone() };
            let source = if f.source == FactSource::Explicit || old.source == FactSource::Explicit {
                FactSource::Explicit
            } else {
                FactSource::Inferred
            };
            tx.execute(
                "UPDATE facts SET text = ?1, category = ?2, confidence = ?3, source = ?4, important = ?5,
                        updated_at = ?6, decayed_at = NULL, s_text = ?7, s_category = ?8 WHERE id = ?9",
                params![
                    new_text,
                    new_cat,
                    old.confidence.max(conf),
                    source.as_str(),
                    old.important || f.important,
                    ts,
                    stem_text(&new_text),
                    stem_text(&new_cat),
                    old.id
                ],
            )?;
            Saved { id: old.id, status: SaveStatus::Merged }
        } else {
            tx.execute(
                "INSERT INTO facts (text, category, confidence, source, important, created_at, updated_at, s_text, s_category)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8)",
                params![text, cat, conf, f.source.as_str(), f.important, ts, stem_text(&text), stem_text(&cat)],
            )?;
            Saved { id: tx.last_insert_rowid(), status: SaveStatus::Inserted }
        };
        tx.commit()?;
        Ok(saved)
    }

    pub fn get_fact(&self, id: i64) -> Result<Option<Fact>> {
        get_fact_conn(&self.conn(), id)
    }

    /// Список для экрана «Память»: новые сверху.
    pub fn list_facts(&self, limit: i64, offset: i64) -> Result<Vec<Fact>> {
        let c = self.conn();
        let mut st = c.prepare_cached(&format!(
            "SELECT {FACT_COLS} FROM facts ORDER BY updated_at DESC, id DESC LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = st.query_map(params![limit, offset], row_fact)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Правка из экрана «Память»: все редактируемые поля берутся из `f`.
    pub fn update_fact(&self, f: &Fact) -> Result<()> {
        let text = validate_fact_text(&f.text)?;
        let cat = normalize_category(&f.category);
        let n = self.conn().execute(
            "UPDATE facts SET text = ?1, category = ?2, confidence = ?3, source = ?4, important = ?5,
                    updated_at = ?6, s_text = ?7, s_category = ?8 WHERE id = ?9",
            params![
                text,
                cat,
                clamp01(f.confidence),
                f.source.as_str(),
                f.important,
                now(),
                stem_text(&text),
                stem_text(&cat),
                f.id
            ],
        )?;
        if n == 0 {
            return Err(MemoryError::NotFound("fact", f.id));
        }
        Ok(())
    }

    pub fn delete_fact(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM facts WHERE id = ?1", [id])? > 0)
    }

    /// Поиск фактов с русской/английской морфологией (BM25 по стеммированным колонкам).
    pub fn search_facts(&self, query: &str, k: usize) -> Result<Vec<Fact>> {
        Ok(search_facts_scored(&self.conn(), query, k)?.into_iter().map(|(f, _)| f).collect())
    }

    /// Всегда важные факты: `important` или категория `preference`.
    pub fn important_facts(&self, k: usize) -> Result<Vec<Fact>> {
        important_facts_conn(&self.conn(), k)
    }
}

pub(crate) fn search_facts_scored(conn: &Connection, query: &str, k: usize) -> Result<Vec<(Fact, f64)>> {
    let Some(expr) = fts_query(query) else { return Ok(vec![]) };
    let mut out = Vec::new();
    for (id, score) in fts_hits(conn, "facts", FACT_WEIGHTS, &expr, k)? {
        if let Some(f) = get_fact_conn(conn, id)? {
            out.push((f, score));
        }
    }
    Ok(out)
}

pub(crate) fn important_facts_conn(conn: &Connection, k: usize) -> Result<Vec<Fact>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {FACT_COLS} FROM facts WHERE important = 1 OR category = 'preference'
         ORDER BY important DESC, confidence DESC, use_count DESC, updated_at DESC LIMIT ?1"
    ))?;
    let rows = st.query_map([k as i64], row_fact)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
