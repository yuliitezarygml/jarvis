//! Общий контекст инструментов модуля: настройки, журнал отката, «открывалка», последние результаты поиска.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::open::{Opener, ShellOpener, SystemSources, TargetSources};
use crate::policy::{Classification, PathOp, PolicyCtx, Shell, classify_command_in, classify_path_op};
use crate::undo::{SystemTrash, TrashBackend, UndoJournal};
use crate::util::opt_str;

#[derive(Debug, Clone)]
pub struct SysConfig {
    /// `{data_dir}/undo/` — журнал отката.
    pub data_dir: PathBuf,
    /// Рабочие папки (проект и т.п.) — приравнены к профилю (T2).
    pub work_dirs: Vec<PathBuf>,
    /// Корни `file_search` по умолчанию.
    pub search_roots: Vec<PathBuf>,
    /// Вырезать секреты из вывода команд/файлов/буфера обмена перед отдачей модели.
    pub redact_output: bool,
    /// Папка по умолчанию для команд (обычно профиль).
    pub default_cwd: Option<PathBuf>,
}

fn expand_env(s: &str) -> PathBuf {
    let mut out = s.to_string();
    for (k, v) in std::env::vars() {
        let pat = format!("%{k}%");
        if out.to_lowercase().contains(&pat.to_lowercase()) {
            let i = out.to_lowercase().find(&pat.to_lowercase()).unwrap();
            out.replace_range(i..i + pat.len(), &v);
        }
    }
    PathBuf::from(out)
}

impl SysConfig {
    /// Из общего конфига. `project_root` — корень проекта (`data_dir` в конфиге относительный).
    pub fn from_core(cfg: &jarvis_core::Config, project_root: &Path) -> Self {
        let data = PathBuf::from(&cfg.data_dir);
        let data_dir = if data.is_absolute() { data } else { project_root.join(data) };
        Self {
            work_dirs: vec![project_root.to_path_buf(), data_dir.clone()],
            data_dir,
            search_roots: cfg.search_roots.iter().map(|s| expand_env(s)).collect(),
            redact_output: true,
            default_cwd: std::env::var_os("USERPROFILE").map(PathBuf::from),
        }
    }

    pub fn for_tests(dir: &Path) -> Self {
        Self {
            data_dir: dir.join("data"),
            work_dirs: vec![dir.to_path_buf()],
            search_roots: vec![dir.to_path_buf()],
            redact_output: true,
            default_cwd: Some(dir.to_path_buf()),
        }
    }
}

pub struct Sys {
    pub cfg: SysConfig,
    pub undo: Arc<UndoJournal>,
    pub opener: Arc<dyn Opener>,
    pub sources: Arc<dyn TargetSources>,
    /// Результаты последнего `file_search` — для «открой второй».
    pub last_results: Mutex<Vec<PathBuf>>,
}

impl Sys {
    /// Боевая конфигурация: Корзина Windows, ShellExecuteW, реальные источники open_target.
    pub fn new(cfg: SysConfig) -> Arc<Self> {
        let sources = Arc::new(SystemSources::with_roots(cfg.search_roots.clone()));
        Self::with(cfg, Arc::new(ShellOpener), Arc::new(SystemTrash), sources)
    }

    pub fn with(cfg: SysConfig, opener: Arc<dyn Opener>, trash: Arc<dyn TrashBackend>, sources: Arc<dyn TargetSources>) -> Arc<Self> {
        let undo = Arc::new(UndoJournal::new(&cfg.data_dir, trash));
        Arc::new(Self { cfg, undo, opener, sources, last_results: Mutex::new(Vec::new()) })
    }

    /// Окружение политики: профиль, TEMP, рабочие папки, `data_dir`, текущий каталог команды.
    pub fn policy_ctx(&self, cwd: Option<PathBuf>) -> PolicyCtx {
        let mut c = PolicyCtx::from_env();
        c.cwd = cwd.or_else(|| self.cfg.default_cwd.clone());
        c.work_dirs = self.cfg.work_dirs.clone();
        c.work_dirs.push(self.cfg.data_dir.clone());
        c
    }

    /// Абсолютный путь из аргумента (относительный — от папки по умолчанию).
    pub fn abs(&self, p: &str) -> PathBuf {
        let p = PathBuf::from(p.trim().trim_matches('"'));
        if p.is_absolute() {
            p
        } else if let Some(c) = &self.cfg.default_cwd {
            c.join(p)
        } else {
            p
        }
    }

    /// Полная классификация вызова (с `protected_path`) для инструментов, где она есть.
    /// Агент использует её, чтобы запросить второе подтверждение для защищённых путей.
    pub fn classify_call(&self, tool: &str, args: &Value) -> Option<Classification> {
        let path = |k: &str| opt_str(args, k).map(|p| self.abs(&p).to_string_lossy().to_string());
        let ctx = self.policy_ctx(None);
        Some(match tool {
            "powershell_run" | "cmd_run" => {
                let shell = if tool == "cmd_run" { Shell::Cmd } else { Shell::PowerShell };
                let cwd = opt_str(args, "cwd").map(PathBuf::from);
                classify_command_in(shell, &opt_str(args, "command")?, &self.policy_ctx(cwd))
            }
            "file_write" | "file_copy" => classify_path_op(&path(if tool == "file_copy" { "to" } else { "path" })?, PathOp::Write, &ctx),
            "file_move" => {
                let a = classify_path_op(&path("from")?, PathOp::Write, &ctx);
                let b = classify_path_op(&path("to")?, PathOp::Write, &ctx);
                let mut r = if b.tier > a.tier { b.clone() } else { a.clone() };
                r.protected_path = a.protected_path || b.protected_path;
                r
            }
            "file_delete" => {
                let p = path("path")?;
                let recursive = Path::new(&p).is_dir();
                classify_path_op(&p, PathOp::Delete { recursive }, &ctx)
            }
            _ => return None,
        })
    }
}
