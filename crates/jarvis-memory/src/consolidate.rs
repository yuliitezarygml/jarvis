//! Консолидация (ночью / раз в неделю): слияние почти-дублей, затухание неиспользуемого,
//! повышение частых плейбуков.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::db::{Memory, fts_hits, now, write_tx};
use crate::error::Result;
use crate::facts::{FACT_COLS, find_similar_fact, get_fact_conn, row_fact};
use crate::lessons::{LESSON_DUP_THRESHOLD, LESSON_WEIGHTS, get_lesson_conn, lesson_stems, similarity_set};
use crate::model::{Fact, FactSource, Lesson};
use crate::playbooks::{all_playbooks, steps_stem};
use crate::schema::FTS_TABLES;
use crate::text::{clean_text, dice, fold, fts_query_exact, stem_set, stem_text};

/// Запись, не использовавшаяся столько дней, теряет доверие.
pub const DECAY_AFTER_DAYS: i64 = 60;
/// Множитель доверия за каждый период неиспользования.
pub const DECAY_FACTOR: f64 = 0.7;
/// Ниже этого доверия затухшие выведенные факты и уроки без успехов удаляются.
pub const FORGET_BELOW: f64 = 0.1;
/// Плейбук повышается при ≥3 успехах и доле успехов ≥80 %.
pub const PROMOTE_MIN_SUCCESS: i64 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidateReport {
    pub merged_facts: usize,
    pub merged_lessons: usize,
    pub merged_playbooks: usize,
    pub decayed: usize,
    pub deleted: usize,
    pub promoted: usize,
    pub demoted: usize,
}

impl Memory {
    pub fn consolidate(&self) -> Result<ConsolidateReport> {
        self.consolidate_at(now())
    }

    /// Консолидация относительно момента `now` (секунды Unix) — для тестов и пересчёта.
    pub fn consolidate_at(&self, now: i64) -> Result<ConsolidateReport> {
        let mut rep = ConsolidateReport::default();
        let mut c = self.conn();
        {
            let tx = write_tx(&mut c)?;
            merge_facts(&tx, &mut rep)?;
            merge_lessons(&tx, &mut rep)?;
            merge_playbooks(&tx, &mut rep)?;
            decay(&tx, now, &mut rep)?;
            promote(&tx, now, &mut rep)?;
            tx.commit()?;
        }
        for (t, _) in FTS_TABLES {
            let fts = format!("{t}_fts");
            c.execute(&format!("INSERT INTO {fts}({fts}) VALUES ('optimize')"), [])?;
        }
        Ok(rep)
    }
}

// ---------------------------------------------------------------- факты

/// Кто из двух фактов остаётся: явный > выведенный, затем новее, затем старше по id.
fn fact_keeper_first(a: &Fact, b: &Fact) -> bool {
    let rank = |f: &Fact| (f.source == FactSource::Explicit, f.updated_at, -f.id);
    rank(a) >= rank(b)
}

fn merge_facts(tx: &Connection, rep: &mut ConsolidateReport) -> Result<()> {
    let ids: Vec<i64> = {
        let mut st = tx.prepare(&format!("SELECT {FACT_COLS} FROM facts ORDER BY id"))?;
        st.query_map([], row_fact)?.map(|r| r.map(|f| f.id)).collect::<rusqlite::Result<_>>()?
    };
    let mut gone = HashSet::new();
    for id in ids {
        if gone.contains(&id) {
            continue;
        }
        loop {
            let Some(cur) = get_fact_conn(tx, id)? else { break };
            let set = stem_set(&cur.text);
            let Some((other, _)) = find_similar_fact(tx, &cur.text, &set, Some(cur.id))? else { break };
            let (keep, drop) = if fact_keeper_first(&cur, &other) { (cur, other) } else { (other, cur) };
            let source = if keep.source == FactSource::Explicit || drop.source == FactSource::Explicit {
                FactSource::Explicit
            } else {
                FactSource::Inferred
            };
            tx.execute(
                "UPDATE facts SET confidence = ?1, important = ?2, use_count = ?3, last_used = ?4, source = ?5,
                        created_at = ?6 WHERE id = ?7",
                params![
                    keep.confidence.max(drop.confidence),
                    keep.important || drop.important,
                    keep.use_count + drop.use_count,
                    keep.last_used.max(drop.last_used),
                    source.as_str(),
                    keep.created_at.min(drop.created_at),
                    keep.id
                ],
            )?;
            tx.execute("DELETE FROM facts WHERE id = ?1", [drop.id])?;
            gone.insert(drop.id);
            rep.merged_facts += 1;
            if drop.id == id {
                break;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- уроки

fn lesson_rank(l: &Lesson) -> (bool, i64, i64, i64) {
    (!l.is_stub, l.success_after_fix - l.fail_after_fix, l.seen, l.last_seen)
}

fn same_fix(a: &Lesson, b: &Lesson) -> bool {
    let fa = fold(&clean_text(&a.fix));
    !fa.is_empty() && fa == fold(&clean_text(&b.fix))
}

/// Партнёры для слияния: та же точная сигнатура; та же общая сигнатура без конфликта имён
/// (или с одинаковым рецептом); похожее описание без сигнатурного конфликта.
fn lesson_partners(tx: &Connection, l: &Lesson) -> Result<Vec<i64>> {
    let mut out = Vec::new();
    if let Some(ex) = &l.exact_signature {
        let mut st = tx.prepare_cached("SELECT id FROM lessons WHERE exact_signature = ?1 AND id != ?2")?;
        out.extend(st.query_map(params![ex, l.id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    if let Some(sig) = &l.signature {
        let mut st = tx.prepare_cached("SELECT id FROM lessons WHERE signature = ?1 AND id != ?2")?;
        let ids = st.query_map(params![sig, l.id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            if out.contains(&id) {
                continue;
            }
            if let Some(o) = get_lesson_conn(tx, id)?
                && (l.exact_signature.is_none() || o.exact_signature.is_none() || same_fix(l, &o))
            {
                out.push(id);
            }
        }
    }
    let set = similarity_set(&l.trigger, &l.mistake);
    if let Some(expr) = fts_query_exact(&set) {
        for (id, _) in fts_hits(tx, "lessons", LESSON_WEIGHTS, &expr, 20)? {
            if id == l.id || out.contains(&id) {
                continue;
            }
            let Some(o) = get_lesson_conn(tx, id)? else { continue };
            if l.signature.is_some() && o.signature.is_some() {
                continue;
            }
            let compatible = |a: &Option<String>, b: &Option<String>| match (a, b) {
                (Some(x), Some(y)) => fold(x) == fold(y),
                _ => true,
            };
            if o.scope == l.scope
                && compatible(&o.tool, &l.tool)
                && compatible(&o.app, &l.app)
                && dice(&set, &similarity_set(&o.trigger, &o.mistake)) >= LESSON_DUP_THRESHOLD
            {
                out.push(id);
            }
        }
    }
    Ok(out)
}

fn or_take(keep: &str, other: &str) -> String {
    if keep.is_empty() { other.to_string() } else { keep.to_string() }
}

fn merge_lesson_pair(tx: &Connection, keep: &Lesson, drop: &Lesson) -> Result<()> {
    let trigger = or_take(&keep.trigger, &drop.trigger);
    let mistake = or_take(&keep.mistake, &drop.mistake);
    let cause = or_take(&keep.cause, &drop.cause);
    let fix = or_take(&keep.fix, &drop.fix);
    let tool = keep.tool.clone().or(drop.tool.clone());
    let app = keep.app.clone().or(drop.app.clone());
    let error_text = keep.error_text.clone().or(drop.error_text.clone());
    let (s_trigger, s_body, s_meta) =
        lesson_stems(&trigger, &mistake, &cause, &fix, tool.as_deref(), app.as_deref(), error_text.as_deref());
    tx.execute(
        "UPDATE lessons SET signature = ?1, exact_signature = ?2, error_text = ?3, trigger_text = ?4, mistake = ?5,
                cause = ?6, fix = ?7, tool = ?8, app = ?9, app_key = ?10, seen = ?11, first_seen = ?12, last_seen = ?13,
                last_used = ?14, confidence = ?15, success_after_fix = ?16, fail_after_fix = ?17, is_stub = ?18,
                s_trigger = ?19, s_body = ?20, s_meta = ?21
         WHERE id = ?22",
        params![
            keep.signature.clone().or(drop.signature.clone()),
            keep.exact_signature.clone().or(drop.exact_signature.clone()),
            error_text,
            trigger,
            mistake,
            cause,
            fix,
            tool,
            app,
            crate::lessons::app_key(app.as_deref()),
            keep.seen + drop.seen,
            keep.first_seen.min(drop.first_seen),
            keep.last_seen.max(drop.last_seen),
            keep.last_used.max(drop.last_used),
            keep.confidence.max(drop.confidence),
            keep.success_after_fix + drop.success_after_fix,
            keep.fail_after_fix + drop.fail_after_fix,
            keep.is_stub && drop.is_stub,
            s_trigger,
            s_body,
            s_meta,
            keep.id
        ],
    )?;
    tx.execute("DELETE FROM lessons WHERE id = ?1", [drop.id])?;
    Ok(())
}

fn merge_lessons(tx: &Connection, rep: &mut ConsolidateReport) -> Result<()> {
    let ids: Vec<i64> = {
        let mut st = tx.prepare("SELECT id FROM lessons ORDER BY id")?;
        st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let mut gone = HashSet::new();
    for id in ids {
        if gone.contains(&id) {
            continue;
        }
        let Some(start) = get_lesson_conn(tx, id)? else { continue };
        for pid in lesson_partners(tx, &start)? {
            if gone.contains(&pid) {
                continue;
            }
            let (Some(cur), Some(other)) = (get_lesson_conn(tx, id)?, get_lesson_conn(tx, pid)?) else { continue };
            let (keep, drop) = if lesson_rank(&cur) >= lesson_rank(&other) { (cur, other) } else { (other, cur) };
            merge_lesson_pair(tx, &keep, &drop)?;
            gone.insert(drop.id);
            rep.merged_lessons += 1;
            if drop.id == id {
                break;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- плейбуки

/// Плейбуки с одинаковыми шагами под разными именами сливаются в более успешный.
fn merge_playbooks(tx: &Connection, rep: &mut ConsolidateReport) -> Result<()> {
    let all = all_playbooks(tx)?;
    let mut gone = HashSet::new();
    for i in 0..all.len() {
        if gone.contains(&all[i].id) {
            continue;
        }
        let mut keep = all[i].clone();
        for other in &all[i + 1..] {
            if gone.contains(&other.id) || other.steps != keep.steps {
                continue;
            }
            let (mut k, d) = if keep.success_count >= other.success_count { (keep.clone(), other) } else { (other.clone(), &keep) };
            k.success_count += d.success_count;
            k.fail_count += d.fail_count;
            k.version = k.version.max(d.version);
            k.last_used = k.last_used.max(d.last_used);
            k.promoted |= d.promoted;
            if k.when.is_empty() {
                k.when = d.when.clone();
            }
            let drop_id = d.id;
            tx.execute(
                "UPDATE playbooks SET success_count = ?1, fail_count = ?2, version = ?3, last_used = ?4, promoted = ?5,
                        when_text = ?6, s_when = ?7, s_steps = ?8 WHERE id = ?9",
                params![
                    k.success_count,
                    k.fail_count,
                    k.version,
                    k.last_used,
                    k.promoted,
                    k.when,
                    stem_text(&k.when),
                    steps_stem(&k.steps),
                    k.id
                ],
            )?;
            tx.execute("DELETE FROM playbooks WHERE id = ?1", [drop_id])?;
            gone.insert(drop_id);
            rep.merged_playbooks += 1;
            keep = k;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- затухание и повышение

fn decay(tx: &Connection, now: i64, rep: &mut ConsolidateReport) -> Result<()> {
    let cutoff = now - DECAY_AFTER_DAYS * 86_400;
    // явные и важные факты не затухают: пользователь сам попросил их помнить
    rep.decayed += tx.execute(
        "UPDATE facts SET confidence = confidence * ?1, decayed_at = ?2
         WHERE source = 'inferred' AND important = 0
           AND max(updated_at, coalesce(last_used, 0), coalesce(decayed_at, 0)) < ?3",
        params![DECAY_FACTOR, now, cutoff],
    )?;
    rep.decayed += tx.execute(
        "UPDATE lessons SET confidence = confidence * ?1, decayed_at = ?2
         WHERE max(last_seen, coalesce(last_used, 0), coalesce(decayed_at, 0)) < ?3",
        params![DECAY_FACTOR, now, cutoff],
    )?;
    rep.deleted += tx.execute(
        "DELETE FROM facts WHERE source = 'inferred' AND important = 0 AND decayed_at IS NOT NULL AND confidence < ?1",
        [FORGET_BELOW],
    )?;
    rep.deleted += tx.execute(
        "DELETE FROM lessons WHERE decayed_at IS NOT NULL AND success_after_fix = 0 AND confidence < ?1",
        [FORGET_BELOW],
    )?;
    rep.demoted += tx.execute(
        "UPDATE playbooks SET promoted = 0 WHERE promoted = 1 AND max(updated_at, coalesce(last_used, 0)) < ?1",
        [cutoff],
    )?;
    Ok(())
}

fn promote(tx: &Connection, now: i64, rep: &mut ConsolidateReport) -> Result<()> {
    let cutoff = now - DECAY_AFTER_DAYS * 86_400;
    rep.promoted += tx.execute(
        "UPDATE playbooks SET promoted = 1
         WHERE promoted = 0 AND success_count >= ?1 AND success_count >= 4 * fail_count
           AND max(updated_at, coalesce(last_used, 0)) >= ?2",
        params![PROMOTE_MIN_SUCCESS, cutoff],
    )?;
    rep.demoted += tx.execute(
        "UPDATE playbooks SET promoted = 0 WHERE promoted = 1 AND fail_count >= 3 AND fail_count > success_count",
        [],
    )?;
    Ok(())
}
