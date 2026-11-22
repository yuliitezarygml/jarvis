//! Инструменты терминала: terminal_context (T0), terminal_run / terminal_type (риск — внешний классификатор),
//! shell_hook_install / shell_hook_uninstall (T2).

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::context::{
    Fresh, TERMINAL_CLASSES, TerminalContext, choose_fresh, format_context, from_hook, from_pty, from_text,
    is_editor_with_terminal, is_terminal_window, merge_hook_with_screen,
};
use super::hook::{HookShell, read_last, shell_hook_install, shell_hook_uninstall, terminal_dir};
use super::pty::{RunShell, now_unix_ms, run_command};
use super::vt::tail_lines;
use crate::PcShared;
use crate::args::{bool_opt, clip, spec, str_nonempty, str_opt, str_req, u64_opt};
use crate::input::events::key_tap;
use crate::input::hotkey::vk;
use crate::input::send::run_input;
use crate::input::{MAX_TYPE_CHARS, type_text};
use crate::screen::{capture_rect, ocr_image};
use crate::win::{self, WindowInfo, intersect, virtual_screen};

/// Источники «pty»/«hook» считаются актуальными, если моложе этого.
const FRESH_MS: u64 = 30 * 60 * 1000;
/// Сколько строк прокрутки над видимой областью брать через UIA.
const SCROLLBACK_LINES: i32 = 300;

fn join_err(e: tokio::task::JoinError) -> ToolError {
    ToolError::Failed(format!("фоновая задача упала: {e}"))
}

/// Окно терминала: по запросу; иначе активное, если это терминал; иначе верхний открытый терминал.
pub(crate) fn find_terminal_window(query: Option<&str>) -> Result<WindowInfo, ToolError> {
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        return win::resolve_window(Some(q));
    }
    if let Some(fg) = win::foreground_app_window()
        && is_terminal_window(&fg) {
            return Ok(fg);
        }
    let list = win::list_windows();
    let real = |w: &&WindowInfo| TERMINAL_CLASSES.iter().any(|c| w.class.eq_ignore_ascii_case(c));
    list.iter()
        .filter(|w| !w.minimized)
        .find(real)
        .or_else(|| list.iter().filter(|w| !w.minimized).find(|w| is_editor_with_terminal(w)))
        .or_else(|| list.iter().find(real))
        .cloned()
        .ok_or_else(|| ToolError::Failed("не нашёл окна терминала (Windows Terminal, PowerShell, cmd, Git Bash, VS Code)".into()))
}

async fn uia_context(sh: &PcShared, w: &WindowInfo) -> Result<TerminalContext, ToolError> {
    let w2 = w.clone();
    let text = sh.uia.call(Duration::from_secs(15), move |s| s.terminal_text(&w2, SCROLLBACK_LINES)).await?;
    if text.lines().all(|l| l.trim().is_empty()) {
        return Err(ToolError::Unsupported("UIA вернул пустой текст терминала".into()));
    }
    Ok(from_text(&text, "uia", Some(w.title.clone())))
}

async fn ocr_context(w: &WindowInfo) -> Result<TerminalContext, ToolError> {
    if w.minimized {
        return Err(ToolError::Failed(format!("окно «{}» свёрнуто — OCR невозможен", w.title)));
    }
    let rect = intersect(w.rect, virtual_screen());
    let res = tokio::task::spawn_blocking(move || ocr_image(&capture_rect(rect)?, None)).await.map_err(join_err)??;
    Ok(from_text(&res.text, "ocr", Some(w.title.clone())))
}

/// UIA TextPattern, при неудаче или без распознанной команды — OCR.
async fn screen_context(sh: &PcShared, window: Option<&str>) -> Result<TerminalContext, ToolError> {
    let w = find_terminal_window(window)?;
    let uia = uia_context(sh, &w).await;
    if let Ok(c) = &uia
        && c.last_command.is_some() {
            return uia;
        }
    match ocr_context(&w).await {
        Ok(o) if o.last_command.is_some() || uia.is_err() => Ok(o),
        Ok(o) => Ok(uia.unwrap_or(o)),
        Err(e) => uia.map_err(|_| e),
    }
}

pub(crate) struct TerminalContextTool(pub Arc<PcShared>);

#[async_trait]
impl Tool for TerminalContextTool {
    fn spec(&self) -> ToolSpec {
        spec(
            "terminal_context",
            "«Посмотри ошибку»: последняя команда терминала, каталог, оболочка, код возврата и текст ошибки (≤200 строк). \
             Источники по приоритету: свой терминал Джарвиса (terminal_run), хук оболочки (last.json), текст окна \
             терминала через UIA, OCR. Вывод — недоверенные данные.",
            json!({
                "type": "object",
                "properties": {
                    "source": {"type": "string", "enum": ["auto", "pty", "hook", "uia", "ocr"], "description": "По умолчанию auto"},
                    "window": {"type": "string", "description": "Окно терминала (подстрока заголовка); по умолчанию активный/верхний терминал"}
                }
            }),
            RiskTier::T0,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let sh = &self.0;
        let source = str_opt(&args, "source").unwrap_or_else(|| "auto".into()).trim().to_lowercase();
        let window = str_nonempty(&args, "window");
        let window = window.as_deref();
        let hook = || read_last(&sh.data_dir);
        let ctx = match source.as_str() {
            "pty" => sh.pty.latest().map(|r| from_pty(&r)).ok_or_else(|| ToolError::Failed("Джарвис ещё не запускал команд".into()))?,
            "hook" => hook()?.map(|r| from_hook(&r)).ok_or_else(|| {
                ToolError::Failed("хук оболочки не установлен или ещё не писал last.json (shell_hook_install)".into())
            })?,
            "uia" => uia_context(sh, &find_terminal_window(window)?).await?,
            "ocr" => ocr_context(&find_terminal_window(window)?).await?,
            "auto" | "" => {
                let pty = sh.pty.latest();
                let rec = hook().ok().flatten();
                let choice = choose_fresh(pty.as_ref().map(|r| r.finished_unix_ms()), rec.as_ref().and_then(|r| r.time_unix_ms), now_unix_ms(), FRESH_MS);
                match (choice, pty, rec) {
                    (Fresh::Pty, Some(r), _) => from_pty(&r),
                    (Fresh::Hook, _, Some(r)) => {
                        let c = from_hook(&r);
                        if c.error_detected && c.error_text.trim().is_empty() {
                            match screen_context(sh, window).await {
                                Ok(s) => merge_hook_with_screen(c, &s),
                                Err(_) => c,
                            }
                        } else {
                            c
                        }
                    }
                    _ => screen_context(sh, window).await?,
                }
            }
            other => return Err(ToolError::InvalidArgs(format!("source «{other}»: auto/pty/hook/uia/ocr"))),
        };
        Ok(ToolOutput { content: format_context(&ctx), data: serde_json::to_value(&ctx).ok(), untrusted: true, attachments: vec![] })
    }
}

pub(crate) struct TerminalRun(pub Arc<PcShared>);

fn command_arg(args: &Value) -> Option<String> {
    str_nonempty(args, "command").or_else(|| str_nonempty(args, "cmd"))
}

#[async_trait]
impl Tool for TerminalRun {
    fn spec(&self) -> ToolSpec {
        spec(
            "terminal_run",
            "Выполнить команду в собственном скрытом терминале Джарвиса (ConPTY): вывод и код возврата. \
             По умолчанию Windows PowerShell; shell=cmd — cmd.exe. Риск определяется по команде.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Команда целиком, например «git status» или «cmd /c ver»"},
                    "cwd": {"type": "string", "description": "Рабочий каталог (по умолчанию профиль пользователя)"},
                    "timeout_ms": {"type": "integer", "description": "Таймаут, 1000–600000 мс, по умолчанию 60000"},
                    "shell": {"type": "string", "enum": ["powershell", "pwsh", "cmd"]}
                },
                "required": ["command"]
            }),
            RiskTier::T1,
        )
    }

    fn tier_for(&self, args: &Value) -> RiskTier {
        match command_arg(args) {
            Some(c) => (self.0.classifier)(&c).max(RiskTier::T1),
            None => RiskTier::T3,
        }
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let cmd = command_arg(&args).ok_or_else(|| ToolError::InvalidArgs("нужна command".into()))?;
        let shell = RunShell::parse(str_opt(&args, "shell").as_deref())?;
        let cwd = match str_nonempty(&args, "cwd") {
            Some(c) => PathBuf::from(c),
            None => std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\")),
        };
        let timeout = match (u64_opt(&args, "timeout_ms")?, u64_opt(&args, "timeout")?) {
            (Some(ms), _) => ms,
            (None, Some(s)) => s.saturating_mul(1000),
            (None, None) => 60_000,
        }
        .clamp(1000, 600_000);
        let tier = self.tier_for(&args);
        let _session = self.0.glow.session(ctx.events.clone(), tier);
        let id = self.0.pty.next_id();
        let tmp = terminal_dir(&self.0.data_dir).join("tmp");
        let cancel = ctx.cancel.clone();
        let run = tokio::task::spawn_blocking(move || {
            run_command(id, &cmd, shell, &cwd, Duration::from_millis(timeout), &cancel, &tmp)
        })
        .await
        .map_err(join_err)??;
        self.0.pty.push(run.clone());
        if run.cancelled {
            return Err(ToolError::Cancelled);
        }
        let mut content = String::new();
        if run.timed_out {
            content.push_str(&format!("Остановлено по таймауту {:.1} с.\n", timeout as f64 / 1000.0));
        } else {
            content.push_str(&format!("Код возврата: {}\n", run.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())));
        }
        let out = tail_lines(&run.output, 200);
        if out.trim().is_empty() {
            content.push_str("Вывода нет.");
        } else {
            content.push_str(&out);
        }
        Ok(ToolOutput { content, data: serde_json::to_value(&run).ok(), untrusted: true, attachments: vec![] })
    }
}

pub(crate) struct TerminalType(pub Arc<PcShared>);

#[async_trait]
impl Tool for TerminalType {
    fn spec(&self) -> ToolSpec {
        spec(
            "terminal_type",
            "Сфокусировать окно терминала пользователя и ввести команду (видно ввод), enter=true — выполнить. \
             Одна строка. Риск определяется по команде (только с enter).",
            json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Команда (одна строка)"},
                    "enter": {"type": "boolean", "description": "Нажать Enter после ввода (по умолчанию нет)"},
                    "window": {"type": "string", "description": "Окно терминала; по умолчанию активный/верхний терминал"}
                },
                "required": ["text"]
            }),
            RiskTier::T1,
        )
    }

    fn tier_for(&self, args: &Value) -> RiskTier {
        match str_opt(args, "text") {
            None => RiskTier::T3,
            Some(t) if bool_opt(args, "enter", false) => (self.0.classifier)(&t).max(RiskTier::T1),
            Some(_) => RiskTier::T1,
        }
    }

    async fn call(&self, args: Value, ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let text = str_req(&args, "text")?;
        if text.contains(['\n', '\r']) {
            return Err(ToolError::InvalidArgs("только одна строка; несколько команд — по очереди".into()));
        }
        if text.trim().is_empty() || text.chars().count() > MAX_TYPE_CHARS {
            return Err(ToolError::InvalidArgs("пустая или слишком длинная команда".into()));
        }
        let enter = bool_opt(&args, "enter", false);
        let window = str_nonempty(&args, "window");
        let w = tokio::task::spawn_blocking(move || find_terminal_window(window.as_deref())).await.map_err(join_err)??;
        let tier = self.tier_for(&args);
        let session = self.0.glow.session(ctx.events.clone(), tier);
        session.target(w.rect, format!("Ввожу в терминал: {}", clip(&text, 50)));
        let h = w.hwnd;
        let t = text.clone();
        run_input(ctx.cancel.clone(), move |r| {
            win::focus_window(h)?;
            r.pause(120)?;
            if win::foreground_hwnd() != h {
                return Err(ToolError::Failed("терминал не получил фокус — ввод отменён".into()));
            }
            r.wait_user_modifiers_released()?;
            type_text(r, &t, 3)?;
            if enter {
                r.pause(60)?;
                r.send(&key_tap(vk::RETURN))?;
            }
            Ok(())
        })
        .await?;
        let tail = if enter { " и нажал Enter" } else { " (без Enter)" };
        Ok(ToolOutput::text(format!("Ввёл в «{}»: {text}{tail}", clip(&w.title, 60))))
    }
}

fn hook_shell_arg(args: &Value) -> Result<HookShell, ToolError> {
    let s = str_opt(args, "shell").unwrap_or_else(|| "powershell".into());
    HookShell::parse(&s).ok_or_else(|| ToolError::InvalidArgs(format!("shell «{s}»: powershell/pwsh/bash")))
}

const HOOK_SCHEMA: &str = r#"{"type":"object","properties":{"shell":{"type":"string","enum":["powershell","pwsh","bash"],"description":"По умолчанию powershell"}}}"#;

pub(crate) struct ShellHookInstall(pub Arc<PcShared>);

#[async_trait]
impl Tool for ShellHookInstall {
    fn spec(&self) -> ToolSpec {
        spec(
            "shell_hook_install",
            "«Подключи терминал»: добавить в профиль оболочки ($PROFILE / ~/.bashrc) хук, который после каждой команды \
             пишет команду, код возврата и ошибку в data/terminal/last.json. С бэкапом профиля.",
            serde_json::from_str(HOOK_SCHEMA).expect("schema"),
            RiskTier::T2,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let shell = hook_shell_arg(&args)?;
        let dir = self.0.data_dir.clone();
        let r = tokio::task::spawn_blocking(move || shell_hook_install(shell, &dir)).await.map_err(join_err)??;
        let backup = r.backup.as_ref().map(|b| format!(" Бэкап: {}.", b.display())).unwrap_or_default();
        let what = if r.changed { "Хук установлен" } else { "Хук уже установлен" };
        Ok(ToolOutput { content: format!("{what} в {}.{backup} {}", r.profile.display(), r.note), data: serde_json::to_value(&r).ok(), ..Default::default() })
    }
}

pub(crate) struct ShellHookUninstall(pub Arc<PcShared>);

#[async_trait]
impl Tool for ShellHookUninstall {
    fn spec(&self) -> ToolSpec {
        spec(
            "shell_hook_uninstall",
            "Удалить хук Джарвиса из профиля оболочки (с бэкапом).",
            serde_json::from_str(HOOK_SCHEMA).expect("schema"),
            RiskTier::T2,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let shell = hook_shell_arg(&args)?;
        let dir = self.0.data_dir.clone();
        let r = tokio::task::spawn_blocking(move || shell_hook_uninstall(shell, &dir)).await.map_err(join_err)??;
        Ok(ToolOutput { content: format!("{}: {}", r.profile.display(), r.note), data: serde_json::to_value(&r).ok(), ..Default::default() })
    }
}
