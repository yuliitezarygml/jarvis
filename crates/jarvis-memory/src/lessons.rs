//! Уроки — память ошибок: запись с дедупликацией (сигнатура → FTS-похожесть), мгновенный рецепт
//! по сигнатуре, исходы применения, CRUD.

use regex::Regex;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::BTreeSet;
use std::sync::LazyLock;

use crate::db::{Memory, clamp01, fts_hits, now, write_tx};
use crate::error::{MemoryError, Result};
use crate::model::{Lesson, LessonMatch, LessonScope, NewLesson, SaveStatus, Saved};
use crate::signature::ErrorSig;
use crate::text::{clean_text, dice, fold, fts_query, fts_query_exact, stem_set, stem_text, truncate_chars};

/// Порог похожести (Дайс по основам «ситуация + ошибка») для слияния уроков без общей сигнатуры.
pub const LESSON_DUP_THRESHOLD: f64 = 0.6;
/// Уточнение урока рефлектором в пределах этого окна после последнего появления не считается повтором.
pub const REFINE_WINDOW_SECS: i64 = 30 * 60;
const MAX_FIELD_CHARS: usize = 1000;
pub(crate) const LESSON_WEIGHTS: &str = ", 3.0, 1.0, 2.0";

pub(crate) const LESSON_COLS: &str = "id, signature, exact_signature, error_text, scope, trigger_text, mistake, cause, fix, \
     tool, app, seen, first_seen, last_seen, last_used, confidence, success_after_fix, fail_after_fix, is_stub";

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<\w+>").unwrap());

pub(crate) fn row_lesson(r: &Row) -> rusqlite::Result<Lesson> {
    Ok(Lesson {
        id: r.get(0)?,
        signature: r.get(1)?,
        exact_signature: r.get(2)?,
        error_text: r.get(3)?,
        scope: LessonScope::parse(&r.get::<_, String>(4)?).unwrap_or_default(),
        trigger: r.get(5)?,
        mistake: r.get(6)?,
        cause: r.get(7)?,
        fix: r.get(8)?,
        tool: r.get(9)?,
        app: r.get(10)?,
        seen: r.get(11)?,
        first_seen: r.get(12)?,
        last_seen: r.get(13)?,
        last_used: r.get(14)?,
        confidence: r.get(15)?,
        success_after_fix: r.get(16)?,
        fail_after_fix: r.get(17)?,
        is_stub: r.get(18)?,
    })
}

pub(crate) fn get_lesson_conn(conn: &Connection, id: i64) -> Result<Option<Lesson>> {
    Ok(conn
        .query_row(&format!("SELECT {LESSON_COLS} FROM lessons WHERE id = ?1"), [id], row_lesson)
        .optional()?)
}

fn query_lessons(conn: &Connection, where_order: &str, p: impl rusqlite::Params) -> Result<Vec<Lesson>> {
    let mut st = conn.prepare_cached(&format!("SELECT {LESSON_COLS} FROM lessons {where_order}"))?;
    let rows = st.query_map(p, row_lesson)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Набор основ для сравнения похожести уроков.
pub(crate) fn similarity_set(trigger: &str, mistake: &str) -> BTreeSet<String> {
    stem_set(&format!("{trigger} {mistake}"))
}

/// Стеммированные колонки FTS: (s_trigger, s_body, s_meta).
pub(crate) fn lesson_stems(
    trigger: &str,
    mistake: &str,
    cause: &str,
    fix: &str,
    tool: Option<&str>,
    app: Option<&str>,
    error_text: Option<&str>,
) -> (String, String, String) {
    let err = error_text.map(|e| PLACEHOLDER.replace_all(e, " ").into_owned()).unwrap_or_default();
    (
        stem_text(trigger),
        stem_text(&format!("{mistake} {cause} {fix}")),
        stem_text(&format!("{} {} {err}", tool.unwrap_or(""), app.unwrap_or(""))),
    )
}

pub(crate) fn app_key(app: Option<&str>) -> Option<String> {
    app.map(|a| fold(a.trim())).filter(|a| !a.is_empty())
}

fn opt_clean(s: Option<String>) -> Option<String> {
    s.map(|v| clean_text(&v)).filter(|v| !v.is_empty())
}

fn field(s: &str) -> String {
    truncate_chars(&clean_text(s), MAX_FIELD_CHARS)
}

/// Урок после нормализации входа.
struct Prepared {
    scope: LessonScope,
    trigger: String,
    mistake: String,
    cause: String,
    fix: String,
    tool: Option<String>,
    app: Option<String>,
    signature: Option<String>,
    exact: Option<String>,
    normalized: Option<String>,
    confidence: f64,
    is_stub: bool,
}

fn prepare(n: NewLesson) -> Result<Prepared> {
    let raw_error = n.error_text.filter(|e| !e.trim().is_empty());
    let (signature, exact, normalized) = match raw_error {
        Some(raw) => {
            let sig = ErrorSig::of(&raw);
            (opt_clean(n.signature).or(Some(sig.signature)), Some(sig.exact), Some(sig.normalized))
        }
        None => (opt_clean(n.signature), None, None),
    };
    let p = Prepared {
        scope: n.scope,
        trigger: field(&n.trigger),
        mistake: field(&n.mistake),
        cause: field(&n.cause),
        fix: field(&n.fix),
        tool: opt_clean(n.tool),
        app: opt_clean(n.app),
        signature,
        exact,
        normalized,
        confidence: clamp01(n.confidence),
        is_stub: n.is_stub,
    };
    if p.trigger.is_empty() && p.mistake.is_empty() && p.fix.is_empty() && p.normalized.is_none() {
        return Err(MemoryError::Invalid("пустой урок: нет ни ситуации, ни ошибки, ни исправления".into()));
    }
    Ok(p)
}

fn compatible(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => fold(x) == fold(y),
        _ => true,
    }
}

/// Найти существующий урок, с которым надо слить новый.
fn find_duplicate(conn: &Connection, p: &Prepared) -> Result<Option<Lesson>> {
    const ORDER: &str = "ORDER BY is_stub ASC, seen DESC, id ASC LIMIT 1";
    if let Some(exact) = &p.exact
        && let Some(l) = query_lessons(conn, &format!("WHERE exact_signature = ?1 {ORDER}"), [exact])?.pop()
    {
        return Ok(Some(l));
    }
    if let Some(sig) = &p.signature
        && let Some(l) = query_lessons(
            conn,
            &format!("WHERE signature = ?1 AND (exact_signature IS NULL OR ?2 IS NULL) {ORDER}"),
            params![sig, p.exact],
        )?
        .pop()
    {
        return Ok(Some(l));
    }
    let set = similarity_set(&p.trigger, &p.mistake);
    let Some(expr) = fts_query_exact(&set) else { return Ok(None) };
    let mut best: Option<(Lesson, f64)> = None;
    for (id, _) in fts_hits(conn, "lessons", LESSON_WEIGHTS, &expr, 20)? {
        let Some(cand) = get_lesson_conn(conn, id)? else { continue };
        // разные сигнатуры — разные ошибки, даже если описаны похоже
        if p.signature.is_some() && cand.signature.is_some() {
            continue;
        }
        if cand.scope != p.scope
            || !compatible(cand.tool.as_deref(), p.tool.as_deref())
            || !compatible(cand.app.as_deref(), p.app.as_deref())
        {
            continue;
        }
        let sim = dice(&set, &similarity_set(&cand.trigger, &cand.mistake));
        if sim >= LESSON_DUP_THRESHOLD && best.as_ref().is_none_or(|(_, b)| sim > *b) {
            best = Some((cand, sim));
        }
    }
    Ok(best.map(|(l, _)| l))
}

/// Новое значение текстового поля при слиянии: заготовка не затирает доработанный урок,
/// доработанный — затирает всё, пустое не затирает ничего.
fn pick(old: &str, new: &str, old_stub: bool, new_stub: bool) -> String {
    if new.is_empty() {
        return old.to_string();
    }
    if old.is_empty() || old_stub || !new_stub { new.to_string() } else { old.to_string() }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Новое появление ошибки: `seen += 1`.
    Occurrence,
    /// Уточнение рефлектором: `seen` растёт, только если появление ещё не учтено заготовкой.
    Refine,
}

fn save_lesson(conn: &mut Connection, n: NewLesson, mode: Mode) -> Result<Saved> {
    let p = prepare(n)?;
    let ts = now();
    let tx = write_tx(conn)?;
    let saved = if let Some(old) = find_duplicate(&tx, &p)? {
        let trigger = pick(&old.trigger, &p.trigger, old.is_stub, p.is_stub);
        let mistake = pick(&old.mistake, &p.mistake, old.is_stub, p.is_stub);
        let cause = pick(&old.cause, &p.cause, old.is_stub, p.is_stub);
        let fix = pick(&old.fix, &p.fix, old.is_stub, p.is_stub);
        let tool = old.tool.clone().or(p.tool);
        let app = old.app.clone().or(p.app);
        let signature = old.signature.clone().or(p.signature);
        let exact = old.exact_signature.clone().or(p.exact);
        let error_text = old.error_text.clone().or(p.normalized);
        let scope = if old.scope == LessonScope::General { p.scope } else { old.scope };
        let confidence = if p.is_stub { old.confidence } else { old.confidence.max(p.confidence) };
        let is_stub = old.is_stub && p.is_stub;
        let repeat = mode == Mode::Occurrence || ts - old.last_seen > REFINE_WINDOW_SECS;
        let seen = old.seen + i64::from(repeat);
        let (s_trigger, s_body, s_meta) =
            lesson_stems(&trigger, &mistake, &cause, &fix, tool.as_deref(), app.as_deref(), error_text.as_deref());
        tx.execute(
            "UPDATE lessons SET signature = ?1, exact_signature = ?2, error_text = ?3, scope = ?4, trigger_text = ?5,
                    mistake = ?6, cause = ?7, fix = ?8, tool = ?9, app = ?10, app_key = ?11, seen = ?12,
                    last_seen = ?13, confidence = ?14, is_stub = ?15, decayed_at = NULL,
                    s_trigger = ?16, s_body = ?17, s_meta = ?18
             WHERE id = ?19",
            params![
                signature,
                exact,
                error_text,
                scope.as_str(),
                trigger,
                mistake,
                cause,
                fix,
                tool,
                app,
                app_key(app.as_deref()),
                seen,
                ts,
                confidence,
                is_stub,
                s_trigger,
                s_body,
                s_meta,
                old.id
            ],
        )?;
        Saved { id: old.id, status: SaveStatus::Merged }
    } else {
        let (s_trigger, s_body, s_meta) = lesson_stems(
            &p.trigger,
            &p.mistake,
            &p.cause,
            &p.fix,
            p.tool.as_deref(),
            p.app.as_deref(),
            p.normalized.as_deref(),
        );
        tx.execute(
            "INSERT INTO lessons (signature, exact_signature, error_text, scope, trigger_text, mistake, cause, fix,
                                  tool, app, app_key, seen, first_seen, last_seen, confidence, is_stub,
                                  s_trigger, s_body, s_meta)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                p.signature,
                p.exact,
                p.normalized,
                p.scope.as_str(),
                p.trigger,
                p.mistake,
                p.cause,
                p.fix,
                p.tool,
                p.app,
                app_key(p.app.as_deref()),
                ts,
                p.confidence,
                p.is_stub,
                s_trigger,
                s_body,
                s_meta
            ],
        )?;
        Saved { id: tx.last_insert_rowid(), status: SaveStatus::Inserted }
    };
    tx.commit()?;
    Ok(saved)
}

impl Memory {
    /// Записать урок о новом появлении ошибки. Дубликат ищется по точной сигнатуре, затем по общей
    /// (если имена в кавычках не противоречат), затем по FTS-похожести (Дайс ≥ [`LESSON_DUP_THRESHOLD`]);
    /// при слиянии `seen += 1`, `last_seen` обновляется, исправление — из нового урока
    /// (заготовка не затирает доработанный урок).
    pub fn add_lesson(&self, l: NewLesson) -> Result<Saved> {
        save_lesson(&mut self.conn(), l, Mode::Occurrence)
    }

    /// Уточнить урок (рефлектор после заготовки): как [`add_lesson`](Self::add_lesson), но `seen` не растёт,
    /// если появление уже учтено в последние [`REFINE_WINDOW_SECS`].
    pub fn refine_lesson(&self, l: NewLesson) -> Result<Saved> {
        save_lesson(&mut self.conn(), l, Mode::Refine)
    }

    /// Мгновенный рецепт по сигнатуре (общей или точной): лучший урок с непустым исправлением.
    pub fn lesson_for_signature(&self, sig: &str) -> Result<Option<Lesson>> {
        Ok(query_lessons(
            &self.conn(),
            "WHERE (exact_signature = ?1 OR signature = ?1) AND fix != ''
             ORDER BY (exact_signature = ?1) DESC, success_after_fix - fail_after_fix DESC, confidence DESC, seen DESC
             LIMIT 1",
            [sig],
        )?
        .pop())
    }

    /// Рецепт по сырому тексту ошибки: сначала точное совпадение (с именами в кавычках), затем общее.
    pub fn lesson_for_error(&self, error_text: &str) -> Result<Option<LessonMatch>> {
        let sig = ErrorSig::of(error_text);
        let c = self.conn();
        const ORDER: &str =
            "AND fix != '' ORDER BY success_after_fix - fail_after_fix DESC, confidence DESC, seen DESC LIMIT 1";
        if let Some(l) = query_lessons(&c, &format!("WHERE exact_signature = ?1 {ORDER}"), [&sig.exact])?.pop() {
            return Ok(Some(LessonMatch { lesson: l, exact: true }));
        }
        Ok(query_lessons(&c, &format!("WHERE signature = ?1 {ORDER}"), [&sig.signature])?
            .pop()
            .map(|l| LessonMatch { lesson: l, exact: false }))
    }

    /// Результат применения рецепта: успех повышает доверие, неудача — понижает.
    pub fn record_lesson_result(&self, id: i64, success: bool) -> Result<()> {
        let sql = if success {
            "UPDATE lessons SET success_after_fix = success_after_fix + 1, confidence = min(1.0, confidence + 0.1),
                    last_used = ?2, decayed_at = NULL WHERE id = ?1"
        } else {
            "UPDATE lessons SET fail_after_fix = fail_after_fix + 1, confidence = max(0.05, confidence - 0.15),
                    last_used = ?2 WHERE id = ?1"
        };
        if self.conn().execute(sql, params![id, now()])? == 0 {
            return Err(MemoryError::NotFound("lesson", id));
        }
        Ok(())
    }

    pub fn get_lesson(&self, id: i64) -> Result<Option<Lesson>> {
        get_lesson_conn(&self.conn(), id)
    }

    pub fn list_lessons(&self, limit: i64, offset: i64) -> Result<Vec<Lesson>> {
        query_lessons(&self.conn(), "ORDER BY last_seen DESC, id DESC LIMIT ?1 OFFSET ?2", params![limit, offset])
    }

    /// Правка из экрана «Память». Сигнатуры и счётчики берутся из `l` как есть.
    pub fn update_lesson(&self, l: &Lesson) -> Result<()> {
        let (trigger, mistake, cause, fix) = (field(&l.trigger), field(&l.mistake), field(&l.cause), field(&l.fix));
        let tool = opt_clean(l.tool.clone());
        let app = opt_clean(l.app.clone());
        let (s_trigger, s_body, s_meta) =
            lesson_stems(&trigger, &mistake, &cause, &fix, tool.as_deref(), app.as_deref(), l.error_text.as_deref());
        let n = self.conn().execute(
            "UPDATE lessons SET signature = ?1, exact_signature = ?2, error_text = ?3, scope = ?4, trigger_text = ?5,
                    mistake = ?6, cause = ?7, fix = ?8, tool = ?9, app = ?10, app_key = ?11, seen = ?12,
                    confidence = ?13, success_after_fix = ?14, fail_after_fix = ?15, is_stub = ?16,
                    s_trigger = ?17, s_body = ?18, s_meta = ?19
             WHERE id = ?20",
            params![
                l.signature,
                l.exact_signature,
                l.error_text,
                l.scope.as_str(),
                trigger,
                mistake,
                cause,
                fix,
                tool,
                app,
                app_key(app.as_deref()),
                l.seen.max(1),
                clamp01(l.confidence),
                l.success_after_fix,
                l.fail_after_fix,
                l.is_stub,
                s_trigger,
                s_body,
                s_meta,
                l.id
            ],
        )?;
        if n == 0 {
            return Err(MemoryError::NotFound("lesson", l.id));
        }
        Ok(())
    }

    pub fn delete_lesson(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM lessons WHERE id = ?1", [id])? > 0)
    }

    /// Поиск уроков по словам (с морфологией).
    pub fn search_lessons(&self, query: &str, k: usize) -> Result<Vec<Lesson>> {
        Ok(search_lessons_scored(&self.conn(), query, k)?.into_iter().map(|(l, _)| l).collect())
    }

    /// Предупреждения перед вызовом инструмента: «в прошлый раз…» (уроки этого инструмента/приложения).
    pub fn warnings_for_tool(&self, tool: &str, app: Option<&str>, k: usize) -> Result<Vec<Lesson>> {
        query_lessons(
            &self.conn(),
            "WHERE (tool = ?1 OR (?2 IS NOT NULL AND app_key = ?2)) AND confidence >= 0.3
             ORDER BY (tool = ?1) DESC, seen DESC, last_seen DESC LIMIT ?3",
            params![tool, app_key(app), k as i64],
        )
    }

    /// Частые ошибки самого пользователя (scope `user_habits`) — для упреждающих подсказок.
    pub fn user_habit_hints(&self, k: usize) -> Result<Vec<Lesson>> {
        query_lessons(
            &self.conn(),
            "WHERE scope = 'user_habits' AND fix != '' ORDER BY seen DESC, last_seen DESC LIMIT ?1",
            [k as i64],
        )
    }
}

pub(crate) fn search_lessons_scored(conn: &Connection, query: &str, k: usize) -> Result<Vec<(Lesson, f64)>> {
    let Some(expr) = fts_query(query) else { return Ok(vec![]) };
    let mut out = Vec::new();
    for (id, score) in fts_hits(conn, "lessons", LESSON_WEIGHTS, &expr, k)? {
        if let Some(l) = get_lesson_conn(conn, id)? {
            out.push((l, score));
        }
    }
    Ok(out)
}

pub(crate) fn lessons_by_tool_or_app(
    conn: &Connection,
    tool: Option<&str>,
    app: Option<&str>,
    limit: usize,
) -> Result<Vec<Lesson>> {
    if tool.is_none() && app.is_none() {
        return Ok(vec![]);
    }
    query_lessons(
        conn,
        "WHERE (?1 IS NOT NULL AND tool = ?1) OR (?2 IS NOT NULL AND app_key = ?2)
         ORDER BY seen DESC, last_seen DESC LIMIT ?3",
        params![tool, app_key(app), limit as i64],
    )
}
