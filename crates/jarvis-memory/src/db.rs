//! `Memory` — потокобезопасный доступ к `memory.db`.
//!
//! Одно соединение под `Mutex` (операции короткие), WAL, `busy_timeout` 5 с, запись — в транзакциях
//! `BEGIN IMMEDIATE` (без SQLITE_BUSY при апгрейде чтения до записи, если базу открыл ещё кто-то).
//! Все методы синхронные и быстрые; из async-кода звать через [`Memory::run`] (spawn_blocking)
//! или готовые `*_async`-обёртки.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use jarvis_core::Config;

use crate::error::{MemoryError, Result};
use crate::model::{MemoryKind, MemoryRef, MemoryStats};
use crate::schema;

/// Имя файла базы внутри `Config.data_dir`.
pub const DB_FILE: &str = "memory.db";

#[derive(Clone)]
pub struct Memory {
    inner: Arc<Inner>,
}

struct Inner {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Memory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Memory").field("path", &self.inner.path).finish()
    }
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn write_tx(conn: &mut Connection) -> rusqlite::Result<Transaction<'_>> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
}

/// Поиск по FTS5: `(rowid, релевантность)`, релевантность = −bm25 (больше — лучше).
pub(crate) fn fts_hits(
    conn: &Connection,
    table: &str,
    weights: &str,
    expr: &str,
    limit: usize,
) -> Result<Vec<(i64, f64)>> {
    let fts = format!("{table}_fts");
    let sql = format!(
        "SELECT rowid, -bm25({fts}{weights}) AS score FROM {fts} WHERE {fts} MATCH ?1 ORDER BY score DESC LIMIT ?2"
    );
    let mut st = conn.prepare_cached(&sql)?;
    let rows = st.query_map(params![expr, limit as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn clamp01(x: f64) -> f64 {
    if x.is_nan() { 0.5 } else { x.clamp(0.0, 1.0) }
}

impl Memory {
    /// Открыть (создать) базу по пути; каталоги создаются; миграции применяются.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn, Some(path.to_path_buf()))
    }

    /// База в памяти (для тестов других модулей).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    /// `{project_root}/{config.data_dir}/memory.db`.
    pub fn db_path(project_root: impl AsRef<Path>, cfg: &Config) -> PathBuf {
        project_root.as_ref().join(&cfg.data_dir).join(DB_FILE)
    }

    /// Открыть базу по конфигу проекта.
    pub fn open_for_config(project_root: impl AsRef<Path>, cfg: &Config) -> Result<Self> {
        Self::open(Self::db_path(project_root, cfg))
    }

    fn init(mut conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        if path.is_some() {
            let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        }
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        schema::migrate(&mut conn)?;
        Ok(Self { inner: Arc::new(Inner { conn: Mutex::new(conn), path }) })
    }

    pub fn path(&self) -> Option<&Path> {
        self.inner.path.as_deref()
    }

    /// Повторно применить миграции (идемпотентно). Возвращает версию схемы.
    pub fn migrate(&self) -> Result<u32> {
        schema::migrate(&mut self.conn())
    }

    pub fn schema_version(&self) -> Result<u32> {
        schema::user_version(&self.conn())
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        // отравленный мьютекс не делает соединение невалидным: SQLite сам откатит незавершённое
        self.inner.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Выполнить синхронную операцию с памятью в пуле блокирующих задач tokio.
    ///
    /// ```ignore
    /// let saved = mem.run(|m| m.upsert_fact(NewFact::explicit("пьёт чёрный кофе"))).await?;
    /// ```
    pub async fn run<R, F>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Memory) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let me = self.clone();
        tokio::task::spawn_blocking(move || f(&me)).await.map_err(|e| MemoryError::Join(e.to_string()))?
    }

    /// Отметить записи как использованные (попали в промпт и помогли): `last_used`, счётчики.
    pub fn mark_used(&self, refs: &[MemoryRef]) -> Result<()> {
        if refs.is_empty() {
            return Ok(());
        }
        let ts = now();
        let mut c = self.conn();
        let tx = write_tx(&mut c)?;
        for r in refs {
            let sql = match r.kind {
                MemoryKind::Fact => "UPDATE facts SET last_used = ?1, use_count = use_count + 1 WHERE id = ?2",
                MemoryKind::Lesson => "UPDATE lessons SET last_used = ?1 WHERE id = ?2",
                MemoryKind::Playbook => "UPDATE playbooks SET last_used = ?1 WHERE id = ?2",
            };
            tx.execute(sql, params![ts, r.id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Удалить запись любого вида. `true`, если она была.
    pub fn delete(&self, kind: MemoryKind, id: i64) -> Result<bool> {
        let sql = format!("DELETE FROM {} WHERE id = ?1", kind.table());
        Ok(self.conn().execute(&sql, [id])? > 0)
    }

    pub fn stats(&self) -> Result<MemoryStats> {
        let c = self.conn();
        let count = |t: &str| -> Result<i64> { Ok(c.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))?) };
        Ok(MemoryStats {
            facts: count("facts")?,
            lessons: count("lessons")?,
            playbooks: count("playbooks")?,
            actions: count("actions")?,
            sessions: count("sessions")?,
            messages: count("messages")?,
        })
    }

    /// Полный экспорт памяти в JSON (экран «Память» → «Экспорт»).
    pub fn export_json(&self) -> Result<Value> {
        let facts = self.list_facts(i64::MAX, 0)?;
        let lessons = self.list_lessons(i64::MAX, 0)?;
        let playbooks = self.list_playbooks(i64::MAX, 0)?;
        let actions = self.list_actions(i64::MAX, 0)?;
        let sessions = self.list_sessions(i64::MAX, 0)?;
        let messages = self.all_messages()?;
        Ok(json!({
            "format": "jarvis-memory",
            "schema_version": schema::SCHEMA_VERSION,
            "exported_at": now(),
            "facts": facts,
            "lessons": lessons,
            "playbooks": playbooks,
            "sessions": sessions,
            "messages": messages,
            "actions": actions,
        }))
    }

    /// Проверка целостности FTS-индексов (для тестов и диагностики).
    pub fn check_integrity(&self) -> Result<()> {
        let c = self.conn();
        let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if ok != "ok" {
            return Err(MemoryError::Invalid(format!("integrity_check: {ok}")));
        }
        for (t, _) in schema::FTS_TABLES {
            let fts = format!("{t}_fts");
            c.execute(&format!("INSERT INTO {fts}({fts}, rank) VALUES ('integrity-check', 1)"), [])?;
        }
        Ok(())
    }

    pub(crate) fn exists(conn: &Connection, table: &str, id: i64) -> Result<bool> {
        Ok(conn
            .query_row(&format!("SELECT 1 FROM {table} WHERE id = ?1"), [id], |_| Ok(()))
            .optional()?
            .is_some())
    }
}
