//! Общие помощники интеграционных тестов.
#![allow(dead_code)]

use jarvis_core::{CancellationToken, CollectSink, Tool, ToolCtx};
use jarvis_pc::{PcOptions, PcToolkit};
use std::path::PathBuf;
use std::sync::Arc;

pub fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("jarvis-pc-it-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn toolkit(tag: &str) -> PcToolkit {
    PcToolkit::new(PcOptions::new(temp_dir(tag)))
}

pub fn tool(tk: &PcToolkit, name: &str) -> Arc<dyn Tool> {
    tk.tools().into_iter().find(|t| t.spec().name == name).unwrap_or_else(|| panic!("нет инструмента {name}"))
}

pub fn ctx() -> (ToolCtx, Arc<CollectSink>) {
    let sink = Arc::new(CollectSink::default());
    (ToolCtx { cancel: CancellationToken::new(), events: sink.clone() }, sink)
}
