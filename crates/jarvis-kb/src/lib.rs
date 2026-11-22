//! База знаний Windows (`data/kb.sqlite`, план, раздел 7).
//!
//! Базу собирает отдельный бинарь `tools/kb-build` (клоны MicrosoftDocs + `kb/seed/*.md`), этот крейт её читает:
//! * [`Kb::search`] — несколько запросов (LLM формулирует 2–3 англоязычных), каждый даёт списки AND/OR по FTS5
//!   (`bm25`: title ×5, commands ×4, heading_path ×3, body ×1) и список «точная команда», всё сливается RRF;
//!   результат — по одному лучшему фрагменту на документ.
//! * [`Kb::lookup_command`] — точное совпадение имени команды/командлета (страница самой команды — первой).
//! * [`Kb::status`] — сколько фрагментов, какие источники, когда собрано.
//! * [`tools`] — инструменты `kb_search`, `kb_lookup_command`, `kb_status` (`jarvis_core::Tool`, T0).

pub mod commands;
pub mod query;
pub mod schema;
mod search;
pub mod tools;
mod writer;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use search::{SearchParams, rrf_merge};
pub use tools::{KbLookupCommandTool, KbSearchTool, KbStatusTool, tools};
pub use writer::{CarryOver, FinishOutcome, KbWriter, NewChunk, SourceRow};

#[derive(Debug, thiserror::Error)]
pub enum KbError {
    #[error("база знаний не собрана: {0} не найден (запустите tools/kb-build)")]
    NotBuilt(PathBuf),
    #[error("база знаний {path}: версия схемы {found}, нужна {expected} — пересоберите tools/kb-build")]
    SchemaMismatch { path: PathBuf, found: String, expected: u32 },
    #[error("ошибка SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, KbError>;

/// Найденный фрагмент (по одному на документ).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KbHit {
    /// Заголовок документа (`robocopy`, `Get-Service`, `Windows Firewall overview`).
    pub title: String,
    /// Ссылка на learn.microsoft.com (для шпаргалок — путь `kb/seed/…md`).
    pub url: String,
    /// Раздел внутри документа: «H2 > H3» (пусто для вводной части).
    pub heading: String,
    /// Отрывок текста вокруг совпадения (≤ ~400 символов).
    pub snippet: String,
    /// Итоговый балл (RRF; больше — лучше).
    pub score: f64,
    /// id источника из `kb_sources.json` (`windows-commands`, `powershell`, `seed`…).
    pub source: String,
    /// Путь файла внутри репозитория источника.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub commit_id: String,
    pub commit_date: String,
    pub files: u64,
    pub chunks: u64,
    pub built_at: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KbStatus {
    pub chunks: u64,
    pub sources: Vec<SourceInfo>,
    /// Время сборки, RFC 3339 UTC.
    pub built_at: String,
    pub schema_version: u32,
    pub path: String,
    pub size_bytes: u64,
}

/// Открытая (только чтение) база знаний. Потокобезопасна: соединение под мьютексом, запросы — миллисекунды.
pub struct Kb {
    conn: Mutex<Connection>,
    path: PathBuf,
}

/// Файл «отложенной замены»: `kb-build` кладёт его, если `kb.sqlite` занят открытым Джарвисом (Windows не даёт
/// переименовать поверх открытого файла). [`Kb::open`] и [`Kb::reload`] подхватывают его.
pub fn pending_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".new");
    PathBuf::from(s)
}

fn apply_pending(path: &Path) {
    let pending = pending_path(path);
    if pending.exists() {
        // Не получилось — значит файл всё ещё занят; работаем со старой базой.
        let _ = std::fs::rename(&pending, path);
    }
}

fn open_conn(path: &Path) -> Result<Connection> {
    if !path.exists() {
        return Err(KbError::NotBuilt(path.to_path_buf()));
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let found = schema::meta(&conn, "schema_version")?.unwrap_or_default();
    if found != schema::SCHEMA_VERSION.to_string() {
        return Err(KbError::SchemaMismatch {
            path: path.to_path_buf(),
            found,
            expected: schema::SCHEMA_VERSION,
        });
    }
    Ok(conn)
}

impl Kb {
    /// Открыть собранную базу (только чтение). Нет файла — [`KbError::NotBuilt`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        apply_pending(&path);
        let conn = open_conn(&path)?;
        Ok(Self { conn: Mutex::new(conn), path })
    }

    /// Открыть `<data_dir>/kb.sqlite` по конфигу (`data_dir` относительно корня проекта).
    pub fn open_in_data_dir(project_root: impl AsRef<Path>, config: &jarvis_core::Config) -> Result<Self> {
        Self::open(project_root.as_ref().join(&config.data_dir).join("kb.sqlite"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Переоткрыть базу (после `kb-build --update`): закрывает соединение, применяет отложенную замену.
    pub fn reload(&self) -> Result<()> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Connection::open_in_memory()?; // старое соединение закрыто — файл свободен
        apply_pending(&self.path);
        *guard = open_conn(&self.path)?;
        Ok(())
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Поиск по нескольким запросам со слиянием RRF; по одному фрагменту на документ, не больше `top_k`.
    pub fn search(&self, queries: &[String], top_k: usize) -> Result<Vec<KbHit>> {
        self.search_with(queries, top_k, &SearchParams::default())
    }

    pub fn search_with(&self, queries: &[String], top_k: usize, params: &SearchParams) -> Result<Vec<KbHit>> {
        search::search(&self.conn(), queries, top_k, params)
    }

    /// Точное совпадение имени команды/командлета (`robocopy`, `sc.exe`, `get-service`): сначала страница самой
    /// команды, затем фрагменты, где она встречается в примерах. Если точного нет — поиск по заголовкам.
    pub fn lookup_command(&self, name: &str) -> Result<Vec<KbHit>> {
        search::lookup_command(&self.conn(), name, 5, &SearchParams::default())
    }

    pub fn status(&self) -> Result<KbStatus> {
        let conn = self.conn();
        let chunks = conn.query_row("SELECT count(*) FROM chunks", [], |r| r.get::<_, i64>(0))? as u64;
        let built_at = schema::meta(&conn, "built_at")?.unwrap_or_default();
        let mut st = conn.prepare(
            "SELECT id, name, repo, commit_id, commit_date, files, chunks, built_at, note FROM sources ORDER BY id",
        )?;
        let sources = st
            .query_map([], |r| {
                Ok(SourceInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    repo: r.get(2)?,
                    commit_id: r.get(3)?,
                    commit_date: r.get(4)?,
                    files: r.get::<_, i64>(5)? as u64,
                    chunks: r.get::<_, i64>(6)? as u64,
                    built_at: r.get(7)?,
                    note: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let size_bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        Ok(KbStatus {
            chunks,
            sources,
            built_at,
            schema_version: schema::SCHEMA_VERSION,
            path: self.path.display().to_string(),
            size_bytes,
        })
    }
}

/// Текущее время UTC в RFC 3339 (`2026-11-22T16:05:00Z`) без внешних зависимостей.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_rfc3339(secs)
}

pub fn format_rfc3339(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
pub(crate) mod test_fixture;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_rfc3339(1_795_305_600 + 3661), "2026-11-22T01:01:01Z");
        assert!(now_rfc3339().ends_with('Z'));
    }

    #[test]
    fn open_missing_is_not_built() {
        let dir = tempfile::tempdir().unwrap();
        match Kb::open(dir.path().join("kb.sqlite")) {
            Err(KbError::NotBuilt(p)) => assert!(p.ends_with("kb.sqlite")),
            other => panic!("ожидали NotBuilt, получили {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn status_and_pending_swap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kb.sqlite");
        test_fixture::build(&path);
        let kb = Kb::open(&path).unwrap();
        let st = kb.status().unwrap();
        assert!(st.chunks >= 8, "{st:?}");
        assert_eq!(st.sources.len(), 4);
        assert!(st.built_at.ends_with('Z'));
        assert!(st.size_bytes > 0);

        // Пересборка, пока база открыта: на Windows rename поверх открытого файла падает → .new + reload().
        let tmp = dir.path().join("kb2.sqlite");
        test_fixture::build_small(&tmp);
        std::fs::rename(&tmp, pending_path(&path)).unwrap();
        kb.reload().unwrap();
        assert!(!pending_path(&path).exists());
        assert_eq!(kb.status().unwrap().chunks, 1);
    }
}
