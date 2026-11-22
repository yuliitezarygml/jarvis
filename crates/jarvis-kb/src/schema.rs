//! Схема `data/kb.sqlite`.
//!
//! * `chunks` — фрагменты документов (по H2/H3, ~700 токенов), содержимое для FTS (external content).
//! * `chunks_fts` — FTS5; **порядок колонок = порядок весов bm25**: title ×5, commands ×4, heading_path ×3, body ×1.
//! * `commands` — точные имена команд/командлетов → фрагменты (`is_primary = 1` — страница самой команды).
//! * `sources` — отчёт сборки по каждому источнику; `meta` — версия схемы, время сборки.

use rusqlite::Connection;

/// Версия схемы. Меняется при несовместимых изменениях — старую базу нужно пересобрать `kb-build`.
pub const SCHEMA_VERSION: u32 = 1;

pub const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sources (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    repo        TEXT NOT NULL DEFAULT '',
    branch      TEXT NOT NULL DEFAULT '',
    commit_id   TEXT NOT NULL DEFAULT '',
    commit_date TEXT NOT NULL DEFAULT '',
    files       INTEGER NOT NULL DEFAULT 0,
    chunks      INTEGER NOT NULL DEFAULT 0,
    skipped     INTEGER NOT NULL DEFAULT 0,
    errors      INTEGER NOT NULL DEFAULT 0,
    built_at    TEXT NOT NULL DEFAULT '',
    note        TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS chunks (
    id           INTEGER PRIMARY KEY,
    source       TEXT NOT NULL,
    path         TEXT NOT NULL,
    url          TEXT NOT NULL,
    title        TEXT NOT NULL,
    heading      TEXT NOT NULL,
    heading_path TEXT NOT NULL,
    commands     TEXT NOT NULL,
    body         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    ms_date      TEXT NOT NULL DEFAULT '',
    ord          INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS chunks_source ON chunks(source);
CREATE INDEX IF NOT EXISTS chunks_url ON chunks(url);
CREATE TABLE IF NOT EXISTS commands (
    name       TEXT NOT NULL COLLATE NOCASE,
    chunk_id   INTEGER NOT NULL,
    is_primary INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (name, chunk_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS commands_chunk ON commands(chunk_id);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    title, commands, heading_path, body,
    content='chunks', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2',
    prefix='2 3 4'
);
"#;

pub fn create(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL)?;
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

pub fn meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_fts5_with_expected_columns() {
        let c = Connection::open_in_memory().unwrap();
        create(&c).unwrap();
        create(&c).unwrap(); // идемпотентно
        let cols: Vec<String> = c
            .prepare("SELECT name FROM pragma_table_info('chunks_fts')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(cols, ["title", "commands", "heading_path", "body"]);
        assert_eq!(meta(&c, "schema_version").unwrap().as_deref(), Some("1"));
        assert_eq!(meta(&c, "nope").unwrap(), None);
    }
}
