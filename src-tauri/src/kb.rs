use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OpenFlags, Params};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const FIND: &str = include_str!("../../scripts/kb/search.sql");
const LOOKUP: &str = include_str!("../../scripts/kb/lookup.sql");
pub struct Knowledge(Arc<Mutex<State>>);
enum State {
    Unavailable(String),
    Ready {
        connection: Connection,
        _file: File,
        manifest: Value,
        last_answer: bool,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    query: String,
    version: Option<String>,
    limit: Option<u8>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lookup {
    name: String,
}
fn open(path: &Path, pinned: &str) -> Result<State> {
    use std::os::windows::fs::OpenOptionsExt;
    ensure!(
        pinned.len() == 64 && pinned.bytes().all(|c| c.is_ascii_hexdigit()),
        "Некорректный SHA-256"
    );
    // The open handle denies writes and renames between validation and SQLite reads.
    let mut file = OpenOptions::new().read(true).share_mode(1).open(path)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    ensure!(
        format!("{:x}", hash.finalize()).eq_ignore_ascii_case(pinned),
        "SHA-256 локальной копии не совпадает"
    );
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")?;
    ensure!(
        connection.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? == 1,
        "Неизвестная схема базы знаний"
    );
    let manifest =
        connection.query_row("SELECT value FROM metadata WHERE key='manifest'", [], |r| {
            r.get::<_, String>(0)
        })?;
    Ok(State::Ready {
        connection,
        _file: file,
        manifest: serde_json::from_str(&manifest)?,
        last_answer: false,
    })
}
fn load(dir: &Path, pin: &str) -> Result<State> {
    let current = open(&dir.join("windows_kb.sqlite3"), pin);
    if current.is_err() && dir.join("windows_kb.previous.sqlite3").is_file() {
        if let Ok(previous) = open(&dir.join("windows_kb.previous.sqlite3"), pin) {
            return Ok(previous);
        }
    }
    current
}
fn rows(conn: &Connection, sql: &str, p: impl Params) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(sql)?;
    let result = stmt.query_map(p, |r| Ok(json!({
        "command":r.get::<_,String>(0)?,"heading":r.get::<_,String>(1)?,
        "excerpt":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?,
        "powershell":r.get::<_,String>(4)?,"applies":r.get::<_,String>(5)?,
        "risk_ceiling":r.get::<_,i64>(6)?,"repo":r.get::<_,String>(7)?,"path":r.get::<_,String>(8)?,
        "commit":r.get::<_,String>(9)?,"chunk":r.get::<_,i64>(10)?
    })))?.collect::<rusqlite::Result<_>>()?;
    Ok(result)
}
fn expression(query: &str) -> Result<String> {
    ensure!(
        !query.is_empty()
            && query.len() <= 160
            && query
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b" /._-".contains(&c)),
        "Нужны английские поисковые слова или имя команды"
    );
    let terms: Vec<_> = query
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{s}\"*"))
        .collect();
    ensure!(
        !terms.is_empty() && terms.len() <= 16,
        "Недопустимое число поисковых слов"
    );
    Ok(terms.join(" AND "))
}
impl Knowledge {
    pub fn start(dir: PathBuf) -> Self {
        let state = Arc::new(Mutex::new(State::Unavailable(
            "Идёт проверка локальной копии".into(),
        )));
        let handle = state.clone();
        std::thread::spawn(move || {
            let result = std::env::var("JARVIS_KB_LOCAL_SHA256")
                .context("Локальная база не подключена")
                .and_then(|pin| load(&dir, &pin));
            *handle.lock().unwrap() =
                result.unwrap_or_else(|e| State::Unavailable(format!("{e:#}")));
        });
        Self(state)
    }
    pub fn manifest(&self) -> String {
        match &*self.0.lock().unwrap() {
            State::Unavailable(reason) => reason.chars().take(600).collect(),
            State::Ready { manifest, .. } => {
                let sources = manifest["sources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["repo"].as_str())
                    .collect::<Vec<_>>();
                format!("{} документов, {} фрагментов. {}. {}. PowerShell, модули Windows, справка команд и tldr. Локальная сборка, SHA-256 проверен, без подписи. Уточняй состав через kb_status.",manifest["documents"],manifest["chunks"],manifest["date"],sources.join(", ")).chars().take(600).collect()
            }
        }
    }
    pub fn run(&self, name: &str, args: &Value) -> Result<Value> {
        let mut state = self.0.lock().unwrap();
        if name == "kb_status" {
            ensure!(args == &json!({}), "Ожидается пустой объект");
            return Ok(match &*state {
                State::Unavailable(reason) => {
                    json!({"available":false,"reason":reason,"server":null})
                }
                State::Ready {
                    manifest,
                    last_answer,
                    ..
                } => {
                    json!({"available":true,"local":manifest,"server":null,"sha256_verified":true,"signature":"local_unsigned","last_answer":last_answer.then_some("local")})
                }
            });
        }
        let State::Ready {
            connection,
            last_answer,
            ..
        } = &mut *state
        else {
            bail!("Локальная база знаний недоступна");
        };
        let result = match name {
            "kb_lookup_command" => {
                let a: Lookup = serde_json::from_value(args.clone())?;
                ensure!(
                    !a.name.is_empty()
                        && a.name.len() <= 100
                        && a.name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
                    "Нужно точное имя команды"
                );
                rows(connection, LOOKUP, [a.name])?
            }
            "kb_search" => {
                let a: Search = serde_json::from_value(args.clone())?;
                let query = expression(&a.query)?;
                let version = a.version.as_deref();
                ensure!(
                    version.is_none_or(
                        |s| s.len() < 16 && s.bytes().all(|c| c.is_ascii_digit() || c == b'.')
                    ),
                    "Некорректная версия PowerShell"
                );
                let limit = a.limit.unwrap_or(5);
                ensure!((3..=5).contains(&limit), "Лимит от 3 до 5");
                rows(
                    connection,
                    FIND,
                    params![query, version, a.query.split_whitespace().next(), limit],
                )?
            }
            _ => bail!("Неизвестный инструмент базы знаний"),
        };
        *last_answer = !result.is_empty();
        Ok(json!({"source":"local","fragments":result}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_fts_queries_only() {
        assert_eq!(
            expression("Get-NetAdapter /all").unwrap(),
            "\"Get\"* AND \"NetAdapter\"* AND \"all\"*"
        );
        for q in ["*", "foo\" OR 1", "a;b", "/"] {
            assert!(expression(q).is_err());
        }
    }
    #[test]
    fn tampered_or_truncated_snapshot_is_rejected() {
        let p = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let c = Connection::open(&p).unwrap();
        c.execute_batch(include_str!("../../scripts/kb/schema.sql"))
            .unwrap();
        c.execute("INSERT INTO metadata VALUES('manifest','{}')", [])
            .unwrap();
        drop(c);
        let bytes = std::fs::read(&p).unwrap();
        let pin = format!("{:x}", Sha256::digest(&bytes));
        assert!(open(&p, &pin).is_ok());
        assert!(open(&p, &"0".repeat(64)).is_err());
        std::fs::write(&p, &bytes[..bytes.len() / 2]).unwrap();
        assert!(open(&p, &pin).is_err());
        std::fs::remove_file(p).unwrap();
    }
}
