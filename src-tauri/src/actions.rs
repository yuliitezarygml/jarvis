use crate::{
    core::Core,
    events::Kind,
    files::{Change, Files},
    tools::{risk, ToolRegistry},
};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use uuid::Uuid;

struct Ticket {
    turn: String,
    digest: String,
    until: Instant,
    sender: oneshot::Sender<bool>,
}
type WindowAction=Box<dyn Fn(bool)->Result<bool>+Send+Sync>;
pub struct Actions {
    pub files: Files,
    pub window: OnceLock<WindowAction>,
    pending: Mutex<Option<Ticket>>,
}
impl Actions {
    pub fn recover(&self,store:&mut crate::storage::Store)->Result<()>{
        let _guard=self.files.mutation_lock()?;
        store.0.execute_batch("CREATE TABLE IF NOT EXISTS action_undo(change_id INTEGER PRIMARY KEY,original INTEGER NOT NULL);")?;
        let tx=store.0.transaction()?;
        {
            let mut stmt=tx.prepare("SELECT a.id,a.snapshot,u.original FROM action_log a LEFT JOIN action_undo u ON u.change_id=a.id WHERE a.state='prepared' ORDER BY a.id")?;
            let mut rows=stmt.query([])?;
            while let Some(row)=rows.next()?{
                let id:i64=row.get(0)?;
                let plan:Change=serde_json::from_str(&row.get::<_,String>(1)?)?;
                let undo:Option<i64>=row.get(2)?;
                let state=if self.files.matches_snapshot(&plan,undo.is_none()).unwrap_or(false) {
                    if let Some(original)=undo{tx.execute("UPDATE action_log SET undone=1 WHERE id=?",[original])?;}
                    "verified"
                }else if self.files.matches_snapshot(&plan,undo.is_some()).unwrap_or(false){"interrupted"}else{"conflict"};
                tx.execute("UPDATE action_log SET state=?,result=? WHERE id=?",params![state,"Проверено после перезапуска",id])?;
            }
        }
        tx.commit()?; Ok(())
    }
    pub fn new(dir: &Path) -> Result<Self> {
        Ok(Self {
            files: Files::open(&dir.join("files"))?,
            window: OnceLock::new(),
            pending: Mutex::new(None),
        })
    }
    pub fn decide(&self, turn: &str, digest: &str, allow: bool) -> Result<()> {
        let mut p = self.pending.lock().unwrap();
        ensure!(
            p.as_ref()
                .is_some_and(|t| t.turn == turn && t.digest == digest && Instant::now() < t.until),
            "Подтверждение недействительно"
        );
        p.take()
            .unwrap()
            .sender
            .send(allow)
            .map_err(|_| anyhow::anyhow!("Запрос уже отменён"))
    }
}
struct Log<'a> {
    core: &'a Core,
    id: i64,
}
impl Drop for Log<'_> {
    fn drop(&mut self) {
        let _ = self.core.db.lock().unwrap().0.execute(
            "UPDATE action_log SET state='cancelled' WHERE id=? AND state='started'",
            [self.id],
        );
    }
}
async fn change(
    core: &Core,
    turn: &str,
    session: &str,
    tool: &str,
    args: &Value,
    id: i64,
) -> Result<Value> {
    let (plan, original) = if tool == "undo_last" {
        ensure!(args == &json!({}), "Ожидается пустой объект");
        let row:Option<(i64,String)>=core.db.lock().unwrap().0.query_row(
            "SELECT id,snapshot FROM action_log WHERE session=? AND state='verified' AND undone=0 AND tool IN ('fs_write','fs_copy','fs_move') ORDER BY id DESC LIMIT 1",
            [session],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (original, snapshot) = row.context("Нет действия для отката")?;
        (serde_json::from_str::<Change>(&snapshot)?, Some(original))
    } else {
        (core.actions.files.plan(tool, args)?, None)
    };
    let detail = core.actions.files.describe(&plan, original.is_some());
    let snapshot = serde_json::to_string(&plan)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            Uuid::new_v4(),
            turn,
            id,
            tool,
            args,
            &snapshot
        ))?)
    );
    let (sender, rx) = oneshot::channel();
    *core.actions.pending.lock().unwrap() = Some(Ticket {
        turn: turn.into(),
        digest: digest.clone(),
        until: Instant::now() + Duration::from_secs(60),
        sender,
    });
    core.event(
        turn,
        Kind::Confirmation,
        json!({"turn":turn,"digest":digest,"tool":tool,"detail":detail}),
    );
    let decision = tokio::time::timeout(Duration::from_secs(60), rx).await;
    core.event(turn, Kind::Confirmation, Value::Null);
    ensure!(matches!(decision, Ok(Ok(true))), "Действие не подтверждено");
    core.for_turn(turn, || -> Result<()> {
        let _guard=core.actions.files.mutation_lock()?;
        let mut db = core.db.lock().unwrap();
        ensure!(!db.settings()?.read_only, "Включён режим только чтения");
        // Commit the before-image before touching the filesystem.
        let journal=db.0.transaction()?;
        if let Some(original)=original{journal.execute("INSERT INTO action_undo VALUES(?,?)",params![id,original])?;}
        journal.execute(
            "UPDATE action_log SET confirmation=?,snapshot=?,state='prepared' WHERE id=?",
            params![digest, snapshot, id],
        )?;
        journal.commit()?;
        core.actions.files.apply(&plan, original.is_some())?;
        let tx = db.0.transaction()?;
        if let Some(original) = original {
            tx.execute("UPDATE action_log SET undone=1 WHERE id=?", [original])?;
        }
        tx.execute(
            "UPDATE action_log SET state='verified',exit_code=0,result='{}' WHERE id=?",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    })
    .context("Действие отменено")??;
    Ok(
        json!({"verified":true,"file":detail["file"],"destination":detail["destination"],"undo":original.is_some()}),
    )
}
pub async fn dispatch(
    core: &Core,
    turn: &str,
    session: &str,
    tool: &str,
    args: &Value,
) -> Result<Value> {
    let level = risk(tool);
    let id = {
        let db = core.db.lock().unwrap();
        ensure!(
            level == 0 || !db.settings()?.read_only,
            "Включён режим только чтения"
        );
        db.0.execute("INSERT INTO action_log(session,request,tool,args,risk) VALUES(?1,COALESCE((SELECT content FROM messages WHERE session=?1 AND role='user' ORDER BY id DESC LIMIT 1),''),?2,?3,?4)",params![session,tool,args.to_string(),level])?;
        db.0.last_insert_rowid()
    };
    let _log = Log { core, id };
    let result = match level {
        0 => ToolRegistry::run(core, tool, args).await,
        1 => core
            .for_turn(turn, || -> Result<Value> {
                ensure!(args == &json!({}), "Ожидается пустой объект");
                let w = core
                    .actions
                    .window
                    .get()
                    .context("Окно Jarvis недоступно")?;
                let minimize = tool == "window_minimize";
                Ok(json!({"verified":w(minimize)?,"window":"Jarvis"}))
            })
            .context("Действие отменено")?,
        2 => change(core, turn, session, tool, args, id).await,
        _ => bail!("Действие не поддерживается"),
    };
    let (state, output, code) = match &result {
        Ok(v) if v.get("verified") == Some(&Value::Bool(false)) => ("unverified", v.to_string(), 0),
        Ok(v) => ("verified", v.to_string(), 0),
        Err(e) => ("error", e.to_string(), 1),
    };
    core.db.lock().unwrap().0.execute(
        "UPDATE action_log SET state=?,result=?,exit_code=? WHERE id=?",
        params![state, output, code, id],
    )?;
    result
}
