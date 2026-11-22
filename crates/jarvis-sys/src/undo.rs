//! Журнал отката (план, раздел 3): удаление → Корзина, перезапись → резервная копия в `{data_dir}/undo/files`,
//! реестр → `reg export` до изменения. Каждая запись — `{data_dir}/undo/entries/<id>.json`. Инструмент `undo_last`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::util::spec;

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Корзина. В тестах подменяется на папку.
pub trait TrashBackend: Send + Sync {
    fn delete(&self, path: &Path) -> io::Result<()>;
    /// Восстановить элемент, удалённый не раньше `deleted_after` (unix-секунды).
    fn restore(&self, original: &Path, deleted_after: i64) -> io::Result<()>;
}

/// Системная Корзина Windows (крейт `trash`).
pub struct SystemTrash;

impl TrashBackend for SystemTrash {
    fn delete(&self, path: &Path) -> io::Result<()> {
        trash::delete(path).map_err(|e| io::Error::other(format!("Корзина: {e}")))
    }

    fn restore(&self, original: &Path, deleted_after: i64) -> io::Result<()> {
        let items = trash::os_limited::list().map_err(|e| io::Error::other(format!("Корзина: {e}")))?;
        let want = original.to_string_lossy().to_lowercase();
        let item = items
            .into_iter()
            .filter(|i| i.original_path().to_string_lossy().to_lowercase() == want && i.time_deleted >= deleted_after - 5)
            .max_by_key(|i| i.time_deleted)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "элемент не найден в Корзине"))?;
        trash::os_limited::restore_all([item]).map_err(|e| io::Error::other(format!("Корзина: {e}")))
    }
}

/// «Корзина» в папке — для тестов.
pub struct DirTrash {
    dir: PathBuf,
    index: Mutex<Vec<(PathBuf, PathBuf)>>,
}

impl DirTrash {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), index: Mutex::new(Vec::new()) }
    }
}

impl TrashBackend for DirTrash {
    fn delete(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut idx = self.index.lock().unwrap();
        let dst = self.dir.join(format!("{}-{}", idx.len(), path.file_name().unwrap_or_default().to_string_lossy()));
        std::fs::rename(path, &dst)?;
        idx.push((path.to_path_buf(), dst));
        Ok(())
    }

    fn restore(&self, original: &Path, _after: i64) -> io::Result<()> {
        let mut idx = self.index.lock().unwrap();
        let i = idx.iter().rposition(|(o, _)| o == original).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "нет в корзине"))?;
        let (o, d) = idx.remove(i);
        std::fs::rename(d, o)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UndoAction {
    /// Удалено в Корзину.
    Trashed { original: PathBuf, at: i64 },
    /// Файл перезаписан; копия прежнего содержимого.
    Backup { original: PathBuf, backup: PathBuf },
    /// Создан новый файл/папка (откат — в Корзину).
    Created { path: PathBuf },
    Moved { from: PathBuf, to: PathBuf },
    /// Экспорт ключа реестра до изменения (откат — `reg import`).
    RegistryExport { key: String, file: PathBuf },
    /// Прежнее значение параметра реестра (`None` — его не было).
    RegValue { key: String, name: String, previous: Option<Vec<u8>>, reg_type: u32 },
    PowerScheme { previous: String },
    /// Необратимое действие — только для журнала.
    Note { text: String },
}

impl UndoAction {
    fn describe(&self) -> String {
        match self {
            Self::Trashed { original, .. } => format!("восстановлен из Корзины: {}", original.display()),
            Self::Backup { original, .. } => format!("восстановлено прежнее содержимое: {}", original.display()),
            Self::Created { path } => format!("созданное убрано в Корзину: {}", path.display()),
            Self::Moved { from, to } => format!("возвращено: {} → {}", to.display(), from.display()),
            Self::RegistryExport { key, .. } => format!("реестр восстановлен из резервной копии: {key}"),
            Self::RegValue { key, name, .. } => format!("параметр реестра восстановлен: {key}\\{name}"),
            Self::PowerScheme { previous } => format!("схема питания возвращена: {previous}"),
            Self::Note { text } => format!("не откатывается: {text}"),
        }
    }

    /// Откат затрагивает HKLM — это T3.
    fn machine_wide(&self) -> bool {
        match self {
            Self::RegistryExport { key, .. } | Self::RegValue { key, .. } => !key.to_uppercase().starts_with("HKCU") && !key.to_uppercase().starts_with("HKEY_CURRENT_USER"),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndoEntry {
    pub id: u64,
    pub time: i64,
    pub tool: String,
    pub summary: String,
    pub actions: Vec<UndoAction>,
    pub undone: bool,
}

pub struct UndoJournal {
    dir: PathBuf,
    trash: Arc<dyn TrashBackend>,
    lock: Mutex<()>,
}

impl UndoJournal {
    pub fn new(data_dir: impl AsRef<Path>, trash: Arc<dyn TrashBackend>) -> Self {
        Self { dir: data_dir.as_ref().join("undo"), trash, lock: Mutex::new(()) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entries_dir(&self) -> PathBuf {
        self.dir.join("entries")
    }

    fn unique(&self, sub: &str, name: &str) -> io::Result<PathBuf> {
        let d = self.dir.join(sub);
        std::fs::create_dir_all(&d)?;
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        Ok(d.join(format!("{nanos}-{name}")))
    }

    /// Копия файла до перезаписи.
    pub fn backup_file(&self, path: &Path) -> io::Result<UndoAction> {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let backup = self.unique("files", &name)?;
        std::fs::copy(path, &backup)?;
        Ok(UndoAction::Backup { original: path.to_path_buf(), backup })
    }

    /// Удалить в Корзину.
    pub fn trash(&self, path: &Path) -> io::Result<UndoAction> {
        let at = now_unix();
        self.trash.delete(path)?;
        Ok(UndoAction::Trashed { original: path.to_path_buf(), at })
    }

    /// `reg export` ключа до изменения. Если ключа нет — `Note`.
    pub fn export_registry(&self, key: &str) -> io::Result<UndoAction> {
        let safe: String = key.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).take(60).collect();
        let file = self.unique("registry", &format!("{safe}.reg"))?;
        let out = std::process::Command::new("reg.exe")
            .args(["export", key, &file.to_string_lossy(), "/y"])
            .output()?;
        if out.status.success() {
            Ok(UndoAction::RegistryExport { key: key.to_string(), file })
        } else {
            Ok(UndoAction::Note { text: format!("ключа {key} не было до изменения") })
        }
    }

    /// Записать действие в журнал. Возвращает id.
    pub fn record(&self, tool: &str, summary: &str, actions: Vec<UndoAction>) -> io::Result<u64> {
        let _g = self.lock.lock().unwrap();
        std::fs::create_dir_all(self.entries_dir())?;
        let id = self.entries_unlocked().iter().map(|e| e.id).max().unwrap_or(0) + 1;
        let e = UndoEntry { id, time: now_unix(), tool: tool.into(), summary: summary.into(), actions, undone: false };
        self.write(&e)?;
        Ok(id)
    }

    fn write(&self, e: &UndoEntry) -> io::Result<()> {
        let p = self.entries_dir().join(format!("{:010}.json", e.id));
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(e).map_err(io::Error::other)?)?;
        std::fs::rename(tmp, p)
    }

    fn entries_unlocked(&self) -> Vec<UndoEntry> {
        let Ok(rd) = std::fs::read_dir(self.entries_dir()) else { return Vec::new() };
        let mut v: Vec<UndoEntry> = rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect();
        v.sort_by_key(|e| e.id);
        v
    }

    pub fn entries(&self) -> Vec<UndoEntry> {
        let _g = self.lock.lock().unwrap();
        self.entries_unlocked()
    }

    /// Последнее неоткаченное действие, которое можно откатить.
    pub fn last(&self) -> Option<UndoEntry> {
        self.entries().into_iter().rev().find(|e| !e.undone && e.actions.iter().any(|a| !matches!(a, UndoAction::Note { .. })))
    }

    pub fn last_is_machine_wide(&self) -> bool {
        self.last().is_some_and(|e| e.actions.iter().any(UndoAction::machine_wide))
    }

    /// Откатить последнее действие. Возвращает запись и описание шагов.
    pub fn undo_last(&self) -> io::Result<(UndoEntry, Vec<String>)> {
        let mut e = self.last().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "нечего откатывать"))?;
        let mut done = Vec::new();
        for a in e.actions.iter().rev() {
            self.revert(a).map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", a.describe())))?;
            done.push(a.describe());
        }
        e.undone = true;
        let _g = self.lock.lock().unwrap();
        self.write(&e)?;
        Ok((e, done))
    }

    fn revert(&self, a: &UndoAction) -> io::Result<()> {
        match a {
            UndoAction::Trashed { original, at } => {
                if original.exists() {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "на месте уже есть файл"));
                }
                self.trash.restore(original, *at)
            }
            UndoAction::Backup { original, backup } => std::fs::copy(backup, original).map(|_| ()),
            UndoAction::Created { path } => {
                if path.exists() {
                    self.trash.delete(path)
                } else {
                    Ok(())
                }
            }
            UndoAction::Moved { from, to } => {
                if from.exists() {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "исходное место занято"));
                }
                if let Some(p) = from.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::rename(to, from).or_else(|_| {
                    std::fs::copy(to, from)?;
                    std::fs::remove_file(to)
                })
            }
            UndoAction::RegistryExport { file, .. } => {
                let out = std::process::Command::new("reg.exe").args(["import", &file.to_string_lossy()]).output()?;
                if out.status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(crate::win::decode_console(&out.stderr)))
                }
            }
            UndoAction::RegValue { key, name, previous, reg_type } => crate::resources::startup::restore_reg_value(key, name, previous.as_deref(), *reg_type),
            UndoAction::PowerScheme { previous } => crate::win::set_power_scheme(previous),
            UndoAction::Note { .. } => Ok(()),
        }
    }
}

/// Инструмент `undo_last`.
pub struct UndoLast(pub Arc<UndoJournal>);

#[async_trait]
impl Tool for UndoLast {
    fn spec(&self) -> ToolSpec {
        spec(
            "undo_last",
            "Откатить последнее изменение Джарвиса из журнала (вернуть файл из Корзины, восстановить перезаписанный файл, вернуть перенос, значение реестра, схему питания).",
            json!({"type": "object", "properties": {}}),
            RiskTier::T2,
        )
    }

    fn tier_for(&self, _args: &Value) -> RiskTier {
        if self.0.last_is_machine_wide() { RiskTier::T3 } else { RiskTier::T2 }
    }

    async fn call(&self, _args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let j = self.0.clone();
        let r = tokio::task::spawn_blocking(move || j.undo_last()).await.map_err(|e| ToolError::Failed(e.to_string()))?;
        match r {
            Ok((e, steps)) => Ok(ToolOutput {
                content: format!("Откачено: {} ({})\n{}", e.summary, e.tool, steps.join("\n")),
                data: Some(json!({"id": e.id, "tool": e.tool})),
                ..Default::default()
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound && e.to_string().contains("нечего") => {
                Ok(ToolOutput::text("Нечего откатывать: журнал пуст."))
            }
            Err(e) => Err(ToolError::Failed(format!("откат не удался: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal(dir: &Path) -> UndoJournal {
        UndoJournal::new(dir.join("data"), Arc::new(DirTrash::new(dir.join("trash"))))
    }

    #[test]
    fn backup_and_restore_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let j = journal(t.path());
        let f = t.path().join("a.txt");
        std::fs::write(&f, "старое").unwrap();
        let a = j.backup_file(&f).unwrap();
        std::fs::write(&f, "новое").unwrap();
        j.record("file_write", "запись a.txt", vec![a]).unwrap();
        assert!(t.path().join("data/undo/entries").read_dir().unwrap().count() == 1);
        let (e, steps) = j.undo_last().unwrap();
        assert_eq!(e.tool, "file_write");
        assert_eq!(steps.len(), 1);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "старое");
        assert!(j.last().is_none());
        assert!(j.undo_last().is_err());
    }

    #[test]
    fn trash_move_created_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        let j = journal(t.path());
        let f = t.path().join("del.txt");
        std::fs::write(&f, "x").unwrap();
        let a = j.trash(&f).unwrap();
        j.record("file_delete", "удаление", vec![a]).unwrap();
        assert!(!f.exists());

        let src = t.path().join("m1.txt");
        let dst = t.path().join("sub").join("m2.txt");
        std::fs::write(&src, "m").unwrap();
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::rename(&src, &dst).unwrap();
        j.record("file_move", "перенос", vec![UndoAction::Moved { from: src.clone(), to: dst.clone() }]).unwrap();

        let c = t.path().join("new.txt");
        std::fs::write(&c, "n").unwrap();
        j.record("file_write", "создание", vec![UndoAction::Created { path: c.clone() }]).unwrap();
        j.record("temp_cleanup", "очистка", vec![UndoAction::Note { text: "временные файлы".into() }]).unwrap();

        // откат идёт с конца, пропуская необратимые записи
        j.undo_last().unwrap();
        assert!(!c.exists());
        j.undo_last().unwrap();
        assert!(src.exists() && !dst.exists());
        j.undo_last().unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "x");
        assert_eq!(j.entries().len(), 4);
    }

    #[test]
    fn registry_export_reads_only() {
        let t = tempfile::tempdir().unwrap();
        let j = journal(t.path());
        match j.export_registry("HKCU\\Console").unwrap() {
            UndoAction::RegistryExport { file, .. } => assert!(std::fs::metadata(file).unwrap().len() > 0),
            other => panic!("{other:?}"),
        }
        assert!(matches!(j.export_registry("HKCU\\Software\\JarvisDefinitelyMissingKey").unwrap(), UndoAction::Note { .. }));
    }

    #[test]
    #[ignore = "использует настоящую Корзину Windows (файл из временной папки)"]
    fn system_recycle_bin_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        let j = UndoJournal::new(t.path(), Arc::new(SystemTrash));
        let f = t.path().join("jarvis-trash-test.txt");
        std::fs::write(&f, "корзина").unwrap();
        let a = j.trash(&f).unwrap();
        assert!(!f.exists());
        j.record("file_delete", "тест", vec![a]).unwrap();
        j.undo_last().unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "корзина");
    }
}
