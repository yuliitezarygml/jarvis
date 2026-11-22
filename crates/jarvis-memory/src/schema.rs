//! Схема `memory.db` и миграции по версиям (`PRAGMA user_version`).
//!
//! Каждая миграция применяется в транзакции `BEGIN IMMEDIATE` с повторной проверкой версии,
//! поэтому одновременное открытие базы из двух мест безопасно, а повторный запуск — no-op.
//! Все `CREATE` дополнительно идут с `IF NOT EXISTS`.
//!
//! FTS5-таблицы — external content поверх основных таблиц, индексируют **стеммированные** колонки
//! `s_*` (их заполняет Rust-код через `text::stem_text`), синхронизация — триггерами.

use rusqlite::{Connection, TransactionBehavior};

use crate::error::Result;

const V1: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    id          INTEGER PRIMARY KEY,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    title       TEXT,
    summary     TEXT
);

CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY,
    session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    ts          INTEGER NOT NULL,
    role        TEXT NOT NULL,
    content     TEXT NOT NULL,
    tool_name   TEXT,
    s_content   TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS messages_session ON messages(session_id, id);

CREATE TABLE IF NOT EXISTS facts (
    id          INTEGER PRIMARY KEY,
    text        TEXT NOT NULL,
    category    TEXT NOT NULL DEFAULT 'general',
    confidence  REAL NOT NULL DEFAULT 0.6,
    source      TEXT NOT NULL DEFAULT 'inferred' CHECK (source IN ('explicit', 'inferred')),
    important   INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    last_used   INTEGER,
    use_count   INTEGER NOT NULL DEFAULT 0,
    decayed_at  INTEGER,
    s_text      TEXT NOT NULL DEFAULT '',
    s_category  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS facts_category ON facts(category);

CREATE TABLE IF NOT EXISTS lessons (
    id                INTEGER PRIMARY KEY,
    signature         TEXT,
    exact_signature   TEXT,
    error_text        TEXT,
    scope             TEXT NOT NULL DEFAULT 'general' CHECK (scope IN ('tool', 'app', 'user_habits', 'general')),
    trigger_text      TEXT NOT NULL DEFAULT '',
    mistake           TEXT NOT NULL DEFAULT '',
    cause             TEXT NOT NULL DEFAULT '',
    fix               TEXT NOT NULL DEFAULT '',
    tool              TEXT,
    app               TEXT,
    app_key           TEXT,
    seen              INTEGER NOT NULL DEFAULT 1,
    first_seen        INTEGER NOT NULL,
    last_seen         INTEGER NOT NULL,
    last_used         INTEGER,
    confidence        REAL NOT NULL DEFAULT 0.6,
    success_after_fix INTEGER NOT NULL DEFAULT 0,
    fail_after_fix    INTEGER NOT NULL DEFAULT 0,
    is_stub           INTEGER NOT NULL DEFAULT 0,
    decayed_at        INTEGER,
    s_trigger         TEXT NOT NULL DEFAULT '',
    s_body            TEXT NOT NULL DEFAULT '',
    s_meta            TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS lessons_signature ON lessons(signature);
CREATE INDEX IF NOT EXISTS lessons_exact ON lessons(exact_signature);
CREATE INDEX IF NOT EXISTS lessons_tool ON lessons(tool);
CREATE INDEX IF NOT EXISTS lessons_app ON lessons(app_key);

CREATE TABLE IF NOT EXISTS playbooks (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    name_key      TEXT NOT NULL UNIQUE,
    when_text     TEXT NOT NULL DEFAULT '',
    steps         TEXT NOT NULL DEFAULT '[]',
    params_schema TEXT NOT NULL DEFAULT '{}',
    success_count INTEGER NOT NULL DEFAULT 0,
    fail_count    INTEGER NOT NULL DEFAULT 0,
    version       INTEGER NOT NULL DEFAULT 1,
    promoted      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    last_used     INTEGER,
    s_name        TEXT NOT NULL DEFAULT '',
    s_when        TEXT NOT NULL DEFAULT '',
    s_steps       TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS actions (
    id             INTEGER PRIMARY KEY,
    ts             INTEGER NOT NULL,
    session_id     INTEGER,
    tool           TEXT NOT NULL,
    args           TEXT NOT NULL DEFAULT '{}',
    tier           TEXT NOT NULL,
    status         TEXT NOT NULL,
    result_summary TEXT NOT NULL DEFAULT '',
    approved_by    TEXT
);
CREATE INDEX IF NOT EXISTS actions_ts ON actions(ts);
"#;

/// FTS5 external-content таблица + триггеры синхронизации.
fn fts_sql(table: &str, cols: &[&str]) -> String {
    let list = cols.join(", ");
    let new_vals = cols.iter().map(|c| format!("new.{c}")).collect::<Vec<_>>().join(", ");
    let old_vals = cols.iter().map(|c| format!("old.{c}")).collect::<Vec<_>>().join(", ");
    let fts = format!("{table}_fts");
    format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {fts} USING fts5({list}, content='{table}', content_rowid='id', \
             tokenize='unicode61 remove_diacritics 0');\n\
         CREATE TRIGGER IF NOT EXISTS {table}_ai AFTER INSERT ON {table} BEGIN\n\
             INSERT INTO {fts}(rowid, {list}) VALUES (new.id, {new_vals});\n\
         END;\n\
         CREATE TRIGGER IF NOT EXISTS {table}_ad AFTER DELETE ON {table} BEGIN\n\
             INSERT INTO {fts}({fts}, rowid, {list}) VALUES ('delete', old.id, {old_vals});\n\
         END;\n\
         CREATE TRIGGER IF NOT EXISTS {table}_au AFTER UPDATE OF {list} ON {table} BEGIN\n\
             INSERT INTO {fts}({fts}, rowid, {list}) VALUES ('delete', old.id, {old_vals});\n\
             INSERT INTO {fts}(rowid, {list}) VALUES (new.id, {new_vals});\n\
         END;\n"
    )
}

pub(crate) const FTS_TABLES: &[(&str, &[&str])] = &[
    ("facts", &["s_text", "s_category"]),
    ("lessons", &["s_trigger", "s_body", "s_meta"]),
    ("playbooks", &["s_name", "s_when", "s_steps"]),
    ("messages", &["s_content"]),
];

fn v1() -> String {
    let mut sql = V1.to_string();
    for (t, cols) in FTS_TABLES {
        sql.push_str(&fts_sql(t, cols));
    }
    sql
}

/// Список миграций: индекс + 1 = номер версии схемы. Новые миграции только добавлять в конец.
fn migrations() -> Vec<String> {
    vec![v1()]
}

/// Текущая (последняя) версия схемы.
pub const SCHEMA_VERSION: u32 = 1;

pub(crate) fn user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?)
}

/// Применить недостающие миграции. Возвращает итоговую версию.
pub(crate) fn migrate(conn: &mut Connection) -> Result<u32> {
    let all = migrations();
    debug_assert_eq!(all.len() as u32, SCHEMA_VERSION);
    for (i, sql) in all.iter().enumerate() {
        let v = i as u32 + 1;
        if user_version(conn)? >= v {
            continue;
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // повторная проверка под блокировкой записи: другой процесс мог успеть раньше
        if user_version(&tx)? < v {
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", v)?;
        }
        tx.commit()?;
    }
    user_version(conn)
}
