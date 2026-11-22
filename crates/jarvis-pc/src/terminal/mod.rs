//! Терминал (план, раздел 3б): контекст ошибки из 4 источников, свой ConPTY, ввод команды в окно терминала,
//! хук оболочки.

pub mod context;
pub mod hook;
pub mod parse;
pub mod pty;
pub(crate) mod tools;
pub mod vt;

pub use context::{TerminalContext, format_context};
pub use hook::{HookRecord, HookReport, HookShell, read_last, shell_hook_install, shell_hook_script, shell_hook_uninstall};
pub use parse::{Block, ShellKind, last_block};
pub use pty::{PtyRun, PtySessions, RunShell, run_command};
