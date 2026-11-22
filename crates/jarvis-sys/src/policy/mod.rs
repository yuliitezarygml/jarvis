//! Политика рисков (план, раздел 3): классификатор команд PowerShell/cmd и путей.
//!
//! Главный принцип — **fail-closed**: всё неизвестное или неразборчивое получает T3.
//! Защищённые пути (`C:\Windows`, `Program Files`, `ProgramData`, `System32`, корень диска) дают T3 и
//! `protected_path = true` — агент должен запросить подтверждение повторно.

mod lexer;
mod paths;
mod rules;
#[cfg(test)]
mod tests;

pub use paths::{PathOp, PathZone, path_tier, path_zone};
pub(crate) use paths::{Hive, RegPath, parse_reg_path};

use jarvis_core::RiskTier;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Оболочка, в которой будет выполнена команда.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    PowerShell,
    Cmd,
}

/// Результат классификации команды/действия.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classification {
    pub tier: RiskTier,
    /// Почему такой уровень (по-русски, для озвучивания и ленты действий).
    pub reasons: Vec<String>,
    /// Затронут защищённый путь — нужно повторное подтверждение.
    pub protected_path: bool,
}

impl Classification {
    pub fn new(tier: RiskTier, reason: impl Into<String>) -> Self {
        Self { tier, reasons: vec![reason.into()], protected_path: false }
    }
    /// Нужно ли второе «да» (защищённый путь).
    pub fn needs_double_confirm(&self) -> bool {
        self.protected_path
    }
    /// Краткое пояснение одной строкой.
    pub fn summary(&self) -> String {
        self.reasons.join("; ")
    }
}

/// Окружение, относительно которого оцениваются пути.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyCtx {
    /// Текущий каталог команды (для относительных путей). `None` — относительные пути считаются неизвестными.
    pub cwd: Option<PathBuf>,
    /// `%USERPROFILE%`.
    pub profile: Option<PathBuf>,
    /// Рабочие папки (проект, `data/`), приравниваются к профилю (T2).
    pub work_dirs: Vec<PathBuf>,
    /// `%TEMP%`.
    pub temp: Option<PathBuf>,
    /// Буква системного диска.
    pub system_drive: char,
}

impl Default for PolicyCtx {
    fn default() -> Self {
        Self { cwd: None, profile: None, work_dirs: Vec::new(), temp: None, system_drive: 'C' }
    }
}

impl PolicyCtx {
    /// Профиль, TEMP, системный диск и текущий каталог из окружения процесса.
    pub fn from_env() -> Self {
        let system_drive = std::env::var("SystemDrive")
            .ok()
            .and_then(|s| s.chars().next())
            .filter(|c| c.is_ascii_alphabetic())
            .unwrap_or('C');
        Self {
            cwd: std::env::current_dir().ok(),
            profile: paths::env_path("USERPROFILE"),
            work_dirs: Vec::new(),
            temp: paths::env_path("TEMP"),
            system_drive,
        }
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_work_dirs(mut self, dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        self.work_dirs.extend(dirs);
        self
    }

    /// Зона литерального пути.
    pub fn zone(&self, path: &str) -> PathZone {
        path_zone(path, self)
    }
}

/// Классифицировать команду в окружении текущего процесса (профиль, TEMP, текущий каталог).
pub fn classify_command(shell: Shell, cmd: &str) -> Classification {
    classify_command_in(shell, cmd, &PolicyCtx::from_env())
}

/// Классифицировать команду в заданном окружении (для тестов и для `cwd` из аргументов инструмента).
pub fn classify_command_in(shell: Shell, cmd: &str, ctx: &PolicyCtx) -> Classification {
    let mut acc = Acc::default();
    rules::classify_script(shell, cmd, ctx, &mut acc, 0, false);
    acc.finish()
}

/// Уровень операции над литеральным путём (файловые инструменты).
pub fn classify_path_op(path: &str, op: PathOp, ctx: &PolicyCtx) -> Classification {
    let zone = path_zone(path, ctx);
    let (tier, protected, why) = path_tier(&zone, op);
    Classification { tier, reasons: vec![format!("{why}: {path}")], protected_path: protected }
}

/// Накопитель: максимум уровней по всем частям команды.
#[derive(Debug, Default)]
pub(crate) struct Acc {
    pub tier: Option<RiskTier>,
    pub reasons: Vec<String>,
    pub protected: bool,
    /// Смена текущего каталога внутри команды (`cd`, `Set-Location`, `pushd`):
    /// `Some(Some(p))` — известный каталог, `Some(None)` — неизвестный (относительные пути → T3).
    pub cwd: Option<Option<PathBuf>>,
}

impl Acc {
    /// Окружение с учётом `cd` внутри команды.
    pub fn effective<'a>(&self, ctx: &'a PolicyCtx) -> std::borrow::Cow<'a, PolicyCtx> {
        match &self.cwd {
            None => std::borrow::Cow::Borrowed(ctx),
            Some(c) => std::borrow::Cow::Owned(PolicyCtx { cwd: c.clone(), ..ctx.clone() }),
        }
    }

    pub fn raise(&mut self, tier: RiskTier, why: impl Into<String>) {
        let why = why.into();
        if tier > RiskTier::T0 && !self.reasons.contains(&why) {
            self.reasons.push(why);
        }
        self.tier = Some(self.tier.map_or(tier, |t| t.max(tier)));
    }

    pub fn protect(&mut self, why: impl Into<String>) {
        self.protected = true;
        self.raise(RiskTier::T3, why);
    }

    fn finish(self) -> Classification {
        let tier = self.tier.unwrap_or(RiskTier::T0);
        let mut reasons = self.reasons;
        if reasons.is_empty() {
            reasons.push("только чтение".into());
        }
        Classification { tier, reasons, protected_path: self.protected }
    }
}
