use anyhow::{ensure, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use uuid::Uuid;
fn default_silence_ms() -> u32 {
    1000
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub provider: String,
    pub url: String,
    pub model: String,
    pub prompt: String,
    pub speak: bool,
    pub tools: bool,
    #[serde(default = "default_silence_ms")]
    pub silence_ms: u32,
    #[serde(default)]
    pub wake_word: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "Groq".into(),
            url: std::env::var("GROQ_BASE_URL")
                .unwrap_or_else(|_| "https://api.groq.com/openai/v1".into()),
            model: std::env::var("GROQ_MODEL").unwrap_or_else(|_| "openai/gpt-oss-120b".into()),
            prompt: if cfg!(windows) {
                include_str!("../config/personality-windows.txt")
            } else {
                include_str!("../config/personality.txt")
            }
            .trim()
            .into(),
            speak: true,
            tools: true,
            silence_ms: default_silence_ms(),
            wake_word: false,
        }
    }
}
pub struct Store(Connection);
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,title TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS messages(id INTEGER PRIMARY KEY,session TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,role TEXT NOT NULL,content TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS by_session ON messages(session,id);
        CREATE TABLE IF NOT EXISTS config(id INTEGER PRIMARY KEY CHECK(id=1),data TEXT);
        CREATE TABLE IF NOT EXISTS preferences(id INTEGER PRIMARY KEY,body TEXT UNIQUE);")?;
        let db = Self(conn);
        if db.sessions()?.is_empty() {
            db.new_session()?;
        }
        Ok(db)
    }
    pub fn sessions(&self) -> Result<Vec<Value>> {
        self.rows(
            "SELECT id,title FROM sessions ORDER BY rowid DESC",
            &[],
            |r| Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?})),
        )
    }
    fn rows<T>(
        &self,
        sql: &str,
        p: &[&dyn rusqlite::ToSql],
        f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        let mut stmt = self.0.prepare(sql)?;
        let rows = stmt
            .query_map(p, f)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn new_session(&self) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.0
            .execute("INSERT INTO sessions VALUES(?1,'Новый диалог')", [&id])?;
        Ok(id)
    }
    pub fn delete(&mut self, id: &str) -> Result<()> {
        let tx = self.0.transaction()?;
        tx.execute("DELETE FROM sessions WHERE id=?1", [id])?;
        if !tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions)", [], |r| {
            r.get::<_, bool>(0)
        })? {
            tx.execute(
                "INSERT INTO sessions VALUES(?1,'Новый диалог')",
                [Uuid::new_v4().to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn append(&mut self, id: &str, role: &str, text: &str) -> Result<()> {
        let tx = self.0.transaction()?;
        if role == "user" {
            tx.execute("UPDATE sessions SET title=?2 WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM messages WHERE session=?1)",params![id,text.chars().take(45).collect::<String>()])?;
        }
        tx.execute(
            "INSERT INTO messages(session,role,content) VALUES(?1,?2,?3)",
            params![id, role, text],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn history(&self, id: &str, limit: i64) -> Result<Vec<Value>> {
        self.rows("SELECT role,content FROM (SELECT id,role,content FROM messages WHERE session=?1 ORDER BY id DESC LIMIT ?2) ORDER BY id", &[&id,&limit],|r|Ok(json!({"role":r.get::<_,String>(0)?,"content":r.get::<_,String>(1)?})))
    }
    pub fn settings(&self) -> Result<Settings> {
        use rusqlite::OptionalExtension;
        let val: Option<String> = self
            .0
            .query_row("SELECT data FROM config WHERE id=1", [], |r| r.get(0))
            .optional()?;
        Ok(match val {
            Some(s) => serde_json::from_str(&s)?,
            None => Settings::default(),
        })
    }
    pub fn save(&self, s: &Settings) -> Result<()> {
        ensure!(
            (800..=1200).contains(&s.silence_ms),
            "Пауза после речи должна составлять от 800 до 1200 мс"
        );
        ensure!(
            !s.model.trim().is_empty()
                && s.model.len() <= 200
                && !s.prompt.trim().is_empty()
                && s.prompt.chars().count() <= 4000,
            "Проверьте модель и системный промпт"
        );
        self.0.execute(
            "INSERT OR REPLACE INTO config VALUES(1,?1)",
            [serde_json::to_string(s)?],
        )?;
        Ok(())
    }
    pub fn prefs(&self) -> Result<Vec<Value>> {
        self.rows("SELECT id,body FROM preferences ORDER BY id", &[], |r| {
            Ok(json!({"id":r.get::<_,i64>(0)?,"body":r.get::<_,String>(1)?}))
        })
    }
    pub fn preference(&self, text: &str) -> Result<()> {
        ensure!(
            !text.trim().is_empty() && text.chars().count() <= 250,
            "Допустимо от 1 до 250 символов"
        );
        ensure!(self.prefs()?.len() < 40, "Достигнут лимит предпочтений");
        self.0.execute(
            "INSERT OR IGNORE INTO preferences(body) VALUES(?1)",
            [text.trim()],
        )?;
        Ok(())
    }
    pub fn forget(&self, id: i64) -> Result<()> {
        self.0
            .execute("DELETE FROM preferences WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn context(&self, id: &str, s: &Settings) -> Result<Vec<Value>> {
        let mut rows = self.history(id, 25)?;
        let mut size = 0;
        rows.reverse();
        rows = rows
            .into_iter()
            .take_while(|r| {
                size += r["content"].as_str().unwrap_or("").chars().count();
                size <= 16000
            })
            .collect();
        rows.reverse();
        let mut all = vec![json!({"role":"system","content":s.prompt})];
        let prefs = self.prefs()?;
        if !prefs.is_empty() {
            all.push(json!({"role":"user","content":format!("Ранее подтверждённые предпочтения пользователя: {}",serde_json::to_string(&prefs)?)}));
        }
        all.extend(rows);
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sessions_and_preferences_survive_reopen() {
        let path = std::env::temp_dir().join(format!("jarvis-db-test-{}.sqlite", Uuid::new_v4()));
        let mut db = Store::open(&path).unwrap();
        let a = db.new_session().unwrap();
        db.append(&a, "user", "Добрый вечер").unwrap();
        let b = db.new_session().unwrap();
        db.preference("Ответы коротко").unwrap();
        drop(db);
        let mut db = Store::open(&path).unwrap();
        assert_eq!(db.history(&a, 10).unwrap()[0]["content"], "Добрый вечер");
        assert!(db.history(&b, 10).unwrap().is_empty());
        assert_eq!(db.prefs().unwrap().len(), 1);
        db.delete(&a).unwrap();
        assert!(db.history(&a, 10).unwrap().is_empty());
        db.forget(db.prefs().unwrap()[0]["id"].as_i64().unwrap())
            .unwrap();
        assert!(db.prefs().unwrap().is_empty());
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
