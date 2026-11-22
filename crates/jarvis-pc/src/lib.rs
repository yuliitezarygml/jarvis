//! J.A.R.V.I.S. — взаимодействие с ПК: мышь и клавиатура (`SendInput`), UI Automation, экран (снимок, set-of-marks,
//! встроенный OCR Windows), терминал (контекст ошибки, свой ConPTY, ввод команд, хук оболочки).
//!
//! Все инструменты реализуют [`jarvis_core::Tool`]; получить их — [`PcToolkit::tools`].
//! Координаты везде — физические пиксели виртуального рабочего стола (мультимонитор, DPI-aware).

pub mod control;
pub mod input;
pub mod uia;
pub mod win;

mod args;
mod com;
mod dpi;

use jarvis_core::{RiskTier, Tool};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use control::{ControlSession, GlowControl, GlowHold};
pub use input::{INJECTED_TAG, release_all_held};
pub use uia::{UiElement, UiSnapshot, Uia};
pub use win::{exclude_from_capture, include_in_capture};

/// Классификатор риска команды терминала (интегратор подставляет классификатор из `jarvis-sys`).
pub type CommandClassifier = Arc<dyn Fn(&str) -> RiskTier + Send + Sync>;

/// Классификатор по умолчанию: любая команда — T3 (fail-closed, нужно «да, сэр»).
pub fn fail_closed_classifier() -> CommandClassifier {
    Arc::new(|_| RiskTier::T3)
}

/// Настройки модуля.
#[derive(Clone)]
pub struct PcOptions {
    /// Каталог данных (`{data_dir}/terminal/last.json` — файл хука оболочки).
    pub data_dir: PathBuf,
    pub classifier: CommandClassifier,
}

impl PcOptions {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: data_dir.into(), classifier: fail_closed_classifier() }
    }

    /// Из общего конфига: `data_dir` относительно корня проекта.
    pub fn from_config(cfg: &jarvis_core::Config, project_root: &Path) -> Self {
        Self::new(project_root.join(&cfg.data_dir))
    }

    pub fn with_classifier(mut self, classifier: CommandClassifier) -> Self {
        self.classifier = classifier;
        self
    }
}

/// Общее состояние инструментов модуля.
pub(crate) struct PcShared {
    pub(crate) glow: Arc<GlowControl>,
    pub(crate) uia: Arc<Uia>,
    #[allow(dead_code)]
    pub(crate) data_dir: PathBuf,
    #[allow(dead_code)]
    pub(crate) classifier: CommandClassifier,
}

/// Набор инструментов ПК с общим состоянием (поток UIA, свечение, PTY-сессии).
#[derive(Clone)]
pub struct PcToolkit {
    shared: Arc<PcShared>,
}

impl PcToolkit {
    pub fn new(opts: PcOptions) -> Self {
        Self {
            shared: Arc::new(PcShared {
                glow: GlowControl::new(),
                uia: Uia::new(),
                data_dir: opts.data_dir,
                classifier: opts.classifier,
            }),
        }
    }

    /// Все инструменты модуля (для `ToolRegistry::extend`).
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        use input::tools::*;
        use uia::tools::*;
        let s = &self.shared;
        vec![
            Arc::new(MouseMove(s.clone())),
            Arc::new(MouseClick(s.clone())),
            Arc::new(MouseDrag(s.clone())),
            Arc::new(MouseScroll(s.clone())),
            Arc::new(KeyType(s.clone())),
            Arc::new(KeyHotkey(s.clone())),
            Arc::new(MediaKey(s.clone())),
            Arc::new(UiSnapshotTool(s.clone())),
            Arc::new(UiFind(s.clone())),
            Arc::new(UiInvoke(s.clone())),
            Arc::new(UiSetValue(s.clone())),
            Arc::new(UiToggle(s.clone())),
            Arc::new(UiExpand(s.clone())),
            Arc::new(UiSelect(s.clone())),
            Arc::new(UiScroll(s.clone())),
            Arc::new(UiFocusWindow(s.clone())),
            Arc::new(UiWindowList(s.clone())),
        ]
    }

    /// Свечение: агент может удерживать его на время многошаговой задачи ([`GlowControl::hold`]).
    pub fn glow(&self) -> Arc<GlowControl> {
        self.shared.glow.clone()
    }

    /// Поток UI Automation (последний снимок — [`Uia::last_snapshot`]).
    pub fn uia(&self) -> Arc<Uia> {
        self.shared.uia.clone()
    }
}
