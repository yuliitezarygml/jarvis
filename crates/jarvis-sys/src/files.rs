//! Файловые инструменты с журналом отката: `file_read`, `file_write`, `file_move`, `file_copy`, `file_delete`.
//! Уровень по пути: профиль и рабочие папки — T2, вне — T3, защищённые — T3 + `confirm_protected: true`
//! (агент спрашивает второй раз и только тогда передаёт флаг).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::context::Sys;
use crate::redact::{contains_redaction, redact_secrets};
use crate::undo::UndoAction;
use crate::util::{fmt_bytes, opt_bool, opt_str, opt_u64, req_str, spec, truncate_middle};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
    Move,
    Copy,
    Delete,
}

pub struct FileTool {
    sys: Arc<Sys>,
    op: FileOp,
}

impl FileTool {
    pub fn new(sys: Arc<Sys>, op: FileOp) -> Self {
        Self { sys, op }
    }

    fn name(&self) -> &'static str {
        match self.op {
            FileOp::Read => "file_read",
            FileOp::Write => "file_write",
            FileOp::Move => "file_move",
            FileOp::Copy => "file_copy",
            FileOp::Delete => "file_delete",
        }
    }

    fn check_protected(&self, args: &Value) -> Result<(), ToolError> {
        if let Some(c) = self.sys.classify_call(self.name(), args)
            && c.protected_path
            && opt_bool(args, "confirm_protected") != Some(true)
        {
            return Err(ToolError::Denied(format!(
                "{}. Нужно повторное подтверждение пользователя; после второго «да» повторите вызов с confirm_protected=true",
                c.summary()
            )));
        }
        Ok(())
    }
}

/// Первый несуществующий предок (для отката созданных папок).
fn first_missing_ancestor(p: &Path) -> Option<PathBuf> {
    let mut cur = p.parent()?;
    let mut missing = None;
    while !cur.exists() {
        missing = Some(cur.to_path_buf());
        cur = cur.parent()?;
    }
    missing
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            n += copy_dir(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), dst)?;
            n += 1;
        }
    }
    Ok(n)
}

fn io_err(what: &str, p: &Path, e: io::Error) -> ToolError {
    ToolError::Failed(format!("{what} {}: {e}", p.display()))
}

#[async_trait]
impl Tool for FileTool {
    fn spec(&self) -> ToolSpec {
        let protected = json!({"type": "boolean", "description": "true только после повторного подтверждения для защищённого пути"});
        let (desc, params, tier) = match self.op {
            FileOp::Read => (
                "Прочитать текстовый файл (секреты маскируются). Большие файлы обрезаются.",
                json!({"type":"object","properties":{"path":{"type":"string"},"max_chars":{"type":"integer","default":20000}},"required":["path"]}),
                RiskTier::T0,
            ),
            FileOp::Write => (
                "Записать текстовый файл (mode: overwrite|append|create_new). Прежнее содержимое сохраняется для undo_last.",
                json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"mode":{"type":"string","enum":["overwrite","append","create_new"],"default":"overwrite"},"confirm_protected":protected},"required":["path","content"]}),
                RiskTier::T2,
            ),
            FileOp::Move => (
                "Перенести/переименовать файл или папку. Откат — undo_last.",
                json!({"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"overwrite":{"type":"boolean","default":false},"confirm_protected":protected},"required":["from","to"]}),
                RiskTier::T2,
            ),
            FileOp::Copy => (
                "Скопировать файл или папку. Откат — undo_last.",
                json!({"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"overwrite":{"type":"boolean","default":false},"confirm_protected":protected},"required":["from","to"]}),
                RiskTier::T2,
            ),
            FileOp::Delete => (
                "Удалить файл или папку в Корзину (можно вернуть через undo_last).",
                json!({"type":"object","properties":{"path":{"type":"string"},"confirm_protected":protected},"required":["path"]}),
                RiskTier::T2,
            ),
        };
        spec(self.name(), desc, params, tier)
    }

    fn tier_for(&self, args: &Value) -> RiskTier {
        if self.op == FileOp::Read {
            return RiskTier::T0;
        }
        self.sys.classify_call(self.name(), args).map_or(RiskTier::T3, |c| c.tier)
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let sys = self.sys.clone();
        let op = self.op;
        if op != FileOp::Read {
            self.check_protected(&args)?;
        }
        let name = self.name();
        tokio::task::spawn_blocking(move || run(&sys, op, name, &args)).await.map_err(|e| ToolError::Failed(e.to_string()))?
    }
}

fn run(sys: &Sys, op: FileOp, tool: &str, args: &Value) -> Result<ToolOutput, ToolError> {
    match op {
        FileOp::Read => {
            let p = sys.abs(&req_str(args, "path")?);
            let max = opt_u64(args, "max_chars").unwrap_or(20_000).clamp(100, 200_000) as usize;
            let meta = std::fs::metadata(&p).map_err(|e| io_err("не удалось открыть", &p, e))?;
            if meta.is_dir() {
                return Err(ToolError::InvalidArgs(format!("{} — это папка", p.display())));
            }
            let mut buf = Vec::new();
            use std::io::Read;
            std::fs::File::open(&p)
                .and_then(|f| f.take(4 * 1024 * 1024).read_to_end(&mut buf))
                .map_err(|e| io_err("не удалось прочитать", &p, e))?;
            if buf.iter().take(8192).any(|&b| b == 0) {
                return Ok(ToolOutput::text(format!("{} — двоичный файл, {}", p.display(), fmt_bytes(meta.len()))));
            }
            let text = crate::win::decode_console(&buf);
            let (text, n) = if sys.cfg.redact_output { redact_secrets(&text) } else { (text, 0) };
            let (text, cut) = truncate_middle(&text, max);
            let mut content = text;
            if cut {
                content.push_str(&format!("\n(файл {} обрезан)", fmt_bytes(meta.len())));
            }
            if n > 0 {
                content.push_str(&format!("\n(скрыто секретов: {n})"));
            }
            Ok(ToolOutput { content, data: Some(json!({"path": p, "size": meta.len(), "redacted": n})), untrusted: true, attachments: vec![] })
        }
        FileOp::Write => {
            let p = sys.abs(&req_str(args, "path")?);
            let content = args.get("content").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("нужен content".into()))?;
            if contains_redaction(content) {
                return Err(ToolError::Denied(
                    "в тексте есть маркер [REDACTED:…] — это замаскированный секрет; запись затёрла бы настоящее значение".into(),
                ));
            }
            let mode = opt_str(args, "mode").unwrap_or_else(|| "overwrite".into());
            let mut actions = Vec::new();
            if p.is_dir() {
                return Err(ToolError::InvalidArgs(format!("{} — это папка", p.display())));
            }
            if p.exists() {
                if mode == "create_new" {
                    return Err(ToolError::Failed(format!("файл уже существует: {}", p.display())));
                }
                actions.push(sys.undo.backup_file(&p).map_err(|e| io_err("не удалось сделать резервную копию", &p, e))?);
            } else {
                if let Some(dir) = first_missing_ancestor(&p) {
                    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| io_err("не удалось создать папку", &dir, e))?;
                    actions.push(UndoAction::Created { path: dir });
                }
                actions.push(UndoAction::Created { path: p.clone() });
            }
            let r = if mode == "append" {
                use std::io::Write;
                std::fs::OpenOptions::new().append(true).create(true).open(&p).and_then(|mut f| f.write_all(content.as_bytes()))
            } else {
                std::fs::write(&p, content)
            };
            r.map_err(|e| io_err("не удалось записать", &p, e))?;
            // Created записан в порядке «папка, файл»; откат идёт с конца — сначала файл.
            let id = sys.undo.record(tool, &format!("запись {}", p.display()), actions).map_err(|e| ToolError::Failed(e.to_string()))?;
            Ok(ToolOutput { content: format!("Записано {} ({})", p.display(), fmt_bytes(content.len() as u64)), data: Some(json!({"undo_id": id})), ..Default::default() })
        }
        FileOp::Move | FileOp::Copy => {
            let from = sys.abs(&req_str(args, "from")?);
            let mut to = sys.abs(&req_str(args, "to")?);
            if !from.exists() {
                return Err(ToolError::Failed(format!("не найдено: {}", from.display())));
            }
            if to.is_dir() {
                to = to.join(from.file_name().unwrap_or_default());
            }
            let overwrite = opt_bool(args, "overwrite").unwrap_or(false);
            let mut actions = Vec::new();
            if to.exists() {
                if !overwrite {
                    return Err(ToolError::Failed(format!("назначение уже существует: {} (overwrite=true, чтобы заменить)", to.display())));
                }
                if op == FileOp::Copy && to.is_file() {
                    actions.push(sys.undo.backup_file(&to).map_err(|e| io_err("резервная копия", &to, e))?);
                } else {
                    actions.push(sys.undo.trash(&to).map_err(|e| io_err("не удалось убрать в Корзину", &to, e))?);
                }
            }
            if let Some(dir) = first_missing_ancestor(&to) {
                std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| io_err("не удалось создать папку", &dir, e))?;
                actions.push(UndoAction::Created { path: dir });
            }
            let what = if op == FileOp::Move {
                std::fs::rename(&from, &to)
                    .or_else(|e| {
                        if from.is_file() {
                            std::fs::copy(&from, &to)?;
                            std::fs::remove_file(&from)
                        } else {
                            Err(e)
                        }
                    })
                    .map_err(|e| io_err("не удалось перенести", &from, e))?;
                actions.push(UndoAction::Moved { from: from.clone(), to: to.clone() });
                "Перенесено"
            } else {
                let existed = actions.iter().any(|a| matches!(a, UndoAction::Backup { .. }));
                if from.is_dir() {
                    copy_dir(&from, &to).map_err(|e| io_err("не удалось скопировать", &from, e))?;
                } else {
                    std::fs::copy(&from, &to).map_err(|e| io_err("не удалось скопировать", &from, e))?;
                }
                if !existed {
                    actions.push(UndoAction::Created { path: to.clone() });
                }
                "Скопировано"
            };
            let id = sys
                .undo
                .record(tool, &format!("{what}: {} → {}", from.display(), to.display()), actions)
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            Ok(ToolOutput { content: format!("{what}: {} → {}", from.display(), to.display()), data: Some(json!({"undo_id": id, "to": to})), ..Default::default() })
        }
        FileOp::Delete => {
            let p = sys.abs(&req_str(args, "path")?);
            if !p.exists() {
                return Err(ToolError::Failed(format!("не найдено: {}", p.display())));
            }
            let a = sys.undo.trash(&p).map_err(|e| io_err("не удалось удалить в Корзину", &p, e))?;
            let id = sys.undo.record(tool, &format!("удаление {}", p.display()), vec![a]).map_err(|e| ToolError::Failed(e.to_string()))?;
            Ok(ToolOutput { content: format!("Удалено в Корзину: {}", p.display()), data: Some(json!({"undo_id": id})), ..Default::default() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::SysConfig;
    use crate::open::tests_support::{MockOpener, MockSources};
    use crate::undo::{DirTrash, UndoLast};

    fn sys(dir: &Path) -> Arc<Sys> {
        Sys::with(SysConfig::for_tests(dir), Arc::new(MockOpener::default()), Arc::new(DirTrash::new(dir.join("trash"))), Arc::new(MockSources::default()))
    }

    #[tokio::test]
    async fn write_overwrite_undo() {
        let t = tempfile::tempdir().unwrap();
        let s = sys(t.path());
        let w = FileTool::new(s.clone(), FileOp::Write);
        let ctx = ToolCtx::for_tests();
        let f = t.path().join("sub").join("a.txt");
        let args = json!({"path": f, "content": "один"});
        assert_eq!(w.tier_for(&args), RiskTier::T2);
        w.call(args, &ctx).await.unwrap();
        w.call(json!({"path": f, "content": "два"}), &ctx).await.unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "два");
        let undo = UndoLast(s.undo.clone());
        undo.call(json!({}), &ctx).await.unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "один");
        undo.call(json!({}), &ctx).await.unwrap();
        assert!(!f.exists() && !f.parent().unwrap().exists());
        assert!(undo.call(json!({}), &ctx).await.unwrap().content.contains("Нечего"));
    }

    #[tokio::test]
    async fn move_copy_delete_and_read() {
        let t = tempfile::tempdir().unwrap();
        let s = sys(t.path());
        let ctx = ToolCtx::for_tests();
        let a = t.path().join("a.txt");
        std::fs::write(&a, "token=abc123def\nтекст").unwrap();
        let r = FileTool::new(s.clone(), FileOp::Read).call(json!({"path": a}), &ctx).await.unwrap();
        assert!(r.untrusted && !r.content.contains("abc123def") && r.content.contains("текст"));

        let b = t.path().join("b.txt");
        FileTool::new(s.clone(), FileOp::Copy).call(json!({"from": a, "to": b}), &ctx).await.unwrap();
        assert!(b.exists());
        let c = t.path().join("moved").join("c.txt");
        FileTool::new(s.clone(), FileOp::Move).call(json!({"from": b, "to": c}), &ctx).await.unwrap();
        assert!(c.exists() && !b.exists());
        FileTool::new(s.clone(), FileOp::Delete).call(json!({"path": c}), &ctx).await.unwrap();
        assert!(!c.exists());

        let undo = UndoLast(s.undo.clone());
        undo.call(json!({}), &ctx).await.unwrap(); // удаление
        assert!(c.exists());
        undo.call(json!({}), &ctx).await.unwrap(); // перенос
        assert!(b.exists() && !c.exists());
    }

    #[tokio::test]
    async fn refuses_redacted_content_and_protected() {
        let t = tempfile::tempdir().unwrap();
        let s = sys(t.path());
        let ctx = ToolCtx::for_tests();
        let w = FileTool::new(s.clone(), FileOp::Write);
        let e = w.call(json!({"path": t.path().join("x.txt"), "content": "key=[REDACTED:key]"}), &ctx).await.unwrap_err();
        assert!(matches!(e, ToolError::Denied(_)));
        let prot = json!({"path": "C:\\Windows\\jarvis-never-written.txt", "content": "x"});
        assert_eq!(w.tier_for(&prot), RiskTier::T3);
        assert!(s.classify_call("file_write", &prot).unwrap().protected_path);
        assert!(matches!(w.call(prot, &ctx).await.unwrap_err(), ToolError::Denied(_)));
        assert!(!Path::new("C:\\Windows\\jarvis-never-written.txt").exists());
        // вне профиля/рабочих папок — T3 без защиты
        let out = json!({"path": "Z:\\elsewhere\\x.txt", "content": "x"});
        assert_eq!(w.tier_for(&out), RiskTier::T3);
        // удаление папки — T3 (рекурсивно)
        std::fs::create_dir_all(t.path().join("dir")).unwrap();
        assert_eq!(FileTool::new(s.clone(), FileOp::Delete).tier_for(&json!({"path": t.path().join("dir")})), RiskTier::T3);
    }
}
