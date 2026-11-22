//! Запись базы (используется `kb-build`). Всё пишется во временный файл рядом с целевым, в конце —
//! перестройка FTS и атомарное переименование. Если целевой файл занят (Джарвис держит его открытым), база
//! кладётся как `kb.sqlite.new` и подхватывается при следующем [`crate::Kb::open`] / [`crate::Kb::reload`].

use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

use crate::{KbError, Result, pending_path, schema};

#[derive(Debug, Clone, Default)]
pub struct NewChunk<'a> {
    pub source: &'a str,
    pub path: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub heading: &'a str,
    pub heading_path: &'a str,
    pub commands: &'a [String],
    pub body: &'a str,
    pub description: &'a str,
    pub ms_date: &'a str,
    pub ord: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceRow {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub commit_id: String,
    pub commit_date: String,
    pub files: u64,
    pub chunks: u64,
    pub skipped: u64,
    pub errors: u64,
    pub built_at: String,
    pub note: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CarryOver {
    pub chunks: u64,
    pub commands: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishOutcome {
    /// Где база в итоге лежит: целевой путь или `<target>.new` (отложенная замена).
    pub path: PathBuf,
    pub pending: bool,
    pub size_bytes: u64,
}

pub struct KbWriter {
    conn: Option<Connection>,
    tmp: PathBuf,
    target: PathBuf,
    in_tx: bool,
}

impl KbWriter {
    pub fn create(target: impl AsRef<Path>) -> Result<Self> {
        let target = target.as_ref().to_path_buf();
        if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = target.as_os_str().to_owned();
        tmp.push(format!(".tmp-{}", std::process::id()));
        let tmp = PathBuf::from(tmp);
        if tmp.exists() {
            std::fs::remove_file(&tmp)?;
        }
        let conn = Connection::open(&tmp)?;
        conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA temp_store = MEMORY;")?;
        schema::create(&conn)?;
        Ok(Self { conn: Some(conn), tmp, target, in_tx: false })
    }

    fn c(&self) -> &Connection {
        self.conn.as_ref().expect("KbWriter уже завершён")
    }

    fn begin(&mut self) -> Result<()> {
        if !self.in_tx {
            self.c().execute_batch("BEGIN")?;
            self.in_tx = true;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<()> {
        if self.in_tx {
            self.c().execute_batch("COMMIT")?;
            self.in_tx = false;
        }
        Ok(())
    }

    /// Перенести фрагменты источников из прежней базы (для `--only` и для источника, который сейчас недоступен).
    /// Идентификаторы сдвигаются, так что порядок вызовов с [`Self::add_chunk`] не важен.
    pub fn carry_over(&mut self, old_db: &Path, sources: &[String]) -> Result<CarryOver> {
        let mut total = CarryOver::default();
        if sources.is_empty() || !old_db.exists() {
            return Ok(total);
        }
        self.commit()?; // ATTACH нельзя внутри транзакции
        let c = self.c();
        c.execute("ATTACH DATABASE ?1 AS old", [old_db.to_string_lossy()])?;
        let res = (|| -> Result<CarryOver> {
            let ver: Option<String> = c
                .query_row("SELECT value FROM old.meta WHERE key = 'schema_version'", [], |r| r.get(0))
                .ok();
            if ver.as_deref() != Some(&schema::SCHEMA_VERSION.to_string()) {
                return Ok(CarryOver::default());
            }
            let mut t = CarryOver::default();
            for s in sources {
                let next: i64 = c.query_row("SELECT coalesce(max(id), 0) + 1 FROM main.chunks", [], |r| r.get(0))?;
                let min_old: Option<i64> =
                    c.query_row("SELECT min(id) FROM old.chunks WHERE source = ?1", [s], |r| r.get(0))?;
                let Some(min_old) = min_old else { continue };
                let off = next - min_old;
                t.chunks += c.execute(
                    "INSERT INTO main.chunks SELECT id + ?2, source, path, url, title, heading, heading_path, commands,
                            body, description, ms_date, ord
                     FROM old.chunks WHERE source = ?1",
                    params![s, off],
                )? as u64;
                t.commands += c.execute(
                    "INSERT OR IGNORE INTO main.commands SELECT m.name, m.chunk_id + ?2, m.is_primary
                     FROM old.commands m JOIN old.chunks k ON k.id = m.chunk_id WHERE k.source = ?1",
                    params![s, off],
                )? as u64;
                c.execute("INSERT OR REPLACE INTO main.sources SELECT * FROM old.sources WHERE id = ?1", [s])?;
            }
            Ok(t)
        })();
        c.execute_batch("DETACH DATABASE old")?;
        total = res?;
        Ok(total)
    }

    pub fn add_chunk(&mut self, ch: &NewChunk<'_>) -> Result<i64> {
        self.begin()?;
        let c = self.c();
        c.prepare_cached(
            "INSERT INTO chunks(source, path, url, title, heading, heading_path, commands, body, description, ms_date, ord)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?
        .execute(params![
            ch.source,
            ch.path,
            ch.url,
            ch.title,
            ch.heading,
            ch.heading_path,
            ch.commands.join(" "),
            ch.body,
            ch.description,
            ch.ms_date,
            ch.ord
        ])?;
        Ok(c.last_insert_rowid())
    }

    /// Привязать имя команды к фрагменту; `primary` — это страница самой команды.
    pub fn add_command(&mut self, name: &str, chunk_id: i64, primary: bool) -> Result<()> {
        self.begin()?;
        self.c()
            .prepare_cached(
                "INSERT INTO commands(name, chunk_id, is_primary) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name, chunk_id) DO UPDATE SET is_primary = max(is_primary, excluded.is_primary)",
            )?
            .execute(params![name, chunk_id, primary as i64])?;
        Ok(())
    }

    pub fn set_source(&mut self, s: &SourceRow) -> Result<()> {
        self.begin()?;
        self.c().execute(
            "INSERT OR REPLACE INTO sources(id, name, repo, branch, commit_id, commit_date, files, chunks, skipped,
                                            errors, built_at, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                s.id,
                s.name,
                s.repo,
                s.branch,
                s.commit_id,
                s.commit_date,
                s.files,
                s.chunks,
                s.skipped,
                s.errors,
                s.built_at,
                s.note
            ],
        )?;
        Ok(())
    }

    pub fn set_meta(&mut self, key: &str, value: &str) -> Result<()> {
        self.begin()?;
        self.c().execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![key, value])?;
        Ok(())
    }

    /// Имена «exe»-команд (основные страницы без дефиса командлета) — чтобы `kb-build --only` знал команды
    /// источников, перенесённых из старой базы.
    pub fn primary_command_names(&self) -> Result<Vec<String>> {
        let mut st = self.c().prepare("SELECT DISTINCT lower(name) FROM commands WHERE is_primary = 1")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(v)
    }

    pub fn chunk_count(&self) -> Result<u64> {
        Ok(self.c().query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))?)
    }

    /// Перестроить FTS, записать время сборки, закрыть и атомарно переименовать.
    pub fn finish(mut self) -> Result<FinishOutcome> {
        self.set_meta("built_at", &crate::now_rfc3339())?;
        self.commit()?;
        {
            let c = self.c();
            c.execute_batch(
                "INSERT INTO chunks_fts(chunks_fts) VALUES ('rebuild');
                 INSERT INTO chunks_fts(chunks_fts) VALUES ('optimize');
                 ANALYZE;",
            )?;
            c.execute_batch("PRAGMA journal_mode = DELETE; VACUUM;")?;
        }
        let conn = self.conn.take().expect("ещё открыт");
        conn.close().map_err(|(_, e)| KbError::Sqlite(e))?;
        let size_bytes = std::fs::metadata(&self.tmp).map(|m| m.len()).unwrap_or(0);
        match std::fs::rename(&self.tmp, &self.target) {
            Ok(()) => Ok(FinishOutcome { path: self.target.clone(), pending: false, size_bytes }),
            Err(_) => {
                let pending = pending_path(&self.target);
                if pending.exists() {
                    std::fs::remove_file(&pending)?;
                }
                std::fs::rename(&self.tmp, &pending)?;
                Ok(FinishOutcome { path: pending, pending: true, size_bytes })
            }
        }
    }
}

impl Drop for KbWriter {
    fn drop(&mut self) {
        // Не завершили (ошибка сборки) — временный файл не оставляем, прежняя база цела.
        if let Some(c) = self.conn.take() {
            drop(c);
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kb;

    fn chunk<'a>(source: &'a str, title: &'a str, body: &'a str, cmds: &'a [String]) -> NewChunk<'a> {
        NewChunk { source, path: title, url: title, title, heading: "", heading_path: "", commands: cmds, body, ..Default::default() }
    }

    #[test]
    fn atomic_write_and_carry_over() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("kb.sqlite");
        let cmds = vec!["robocopy".to_string()];

        let mut w = KbWriter::create(&target).unwrap();
        let id = w.add_chunk(&chunk("a", "robocopy", "copies files", &cmds)).unwrap();
        w.add_command("robocopy", id, true).unwrap();
        w.add_command("robocopy", id, false).unwrap(); // не понижает primary
        w.add_chunk(&chunk("b", "Get-Service", "gets services", &[])).unwrap();
        w.set_source(&SourceRow { id: "a".into(), name: "A".into(), chunks: 1, ..Default::default() }).unwrap();
        w.set_source(&SourceRow { id: "b".into(), name: "B".into(), chunks: 1, ..Default::default() }).unwrap();
        assert!(!target.exists(), "до finish целевой файл не трогаем");
        let out = w.finish().unwrap();
        assert!(!out.pending && out.path == target && out.size_bytes > 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "временных файлов не осталось");

        // Новая сборка: источник «b» пересобираем, «a» переносим из старой базы — после новых вставок.
        let mut w = KbWriter::create(&target).unwrap();
        w.add_chunk(&chunk("b", "Stop-Service", "stops services", &[])).unwrap();
        let co = w.carry_over(&target, &["a".to_string(), "missing".to_string()]).unwrap();
        assert_eq!(co, CarryOver { chunks: 1, commands: 1 });
        assert_eq!(w.primary_command_names().unwrap(), ["robocopy"]);
        assert_eq!(w.chunk_count().unwrap(), 2);
        w.finish().unwrap();

        let kb = Kb::open(&target).unwrap();
        let st = kb.status().unwrap();
        assert_eq!(st.chunks, 2);
        assert_eq!(st.sources.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(kb.lookup_command("ROBOCOPY.EXE").unwrap()[0].title, "robocopy");
        assert_eq!(kb.search(&["stops".into()], 5).unwrap()[0].title, "Stop-Service");
    }

    #[test]
    fn dropped_writer_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("kb.sqlite");
        {
            let mut w = KbWriter::create(&target).unwrap();
            w.add_chunk(&chunk("a", "x", "y", &[])).unwrap();
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
