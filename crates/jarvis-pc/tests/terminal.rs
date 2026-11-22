//! Терминал: свой ConPTY (дочерние процессы cmd/powershell), код возврата, таймаут, отмена, контекст из pty/хука.
//! В окна пользователя ничего не вводится.

mod common;

use common::{ctx, temp_dir, tool, toolkit};
use jarvis_core::{CancellationToken, Event, RiskTier, ToolError};
use jarvis_pc::terminal::hook::{hook_file, terminal_dir};
use jarvis_pc::{PcOptions, PcToolkit};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn terminal_run_echo_privet_exit_code_and_glow() {
    let tk = toolkit("run");
    let (c, sink) = ctx();
    let out = tool(&tk, "terminal_run").call(json!({"command": "cmd /c echo привет"}), &c).await.unwrap();
    println!("{}", out.content);
    assert!(out.content.contains("привет"), "{}", out.content);
    assert!(out.content.starts_with("Код возврата: 0"));
    assert!(out.untrusted);
    let d = out.data.unwrap();
    assert_eq!(d["exit_code"], 0);
    assert_eq!(d["timed_out"], false);
    // свечение: классификатор по умолчанию fail-closed → T3
    let ev = sink.take();
    assert_eq!(ev.first(), Some(&Event::ControlStart { tier: RiskTier::T3 }));
    assert_eq!(ev.last(), Some(&Event::ControlStop));
    // запуск запомнен — источник №1 для terminal_context
    assert_eq!(tk.pty_sessions().latest().unwrap().command, "cmd /c echo привет");
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_run_nonzero_exit_codes() {
    let tk = toolkit("exit");
    let (c, _) = ctx();
    let t = tool(&tk, "terminal_run");
    let out = t.call(json!({"command": "cmd /c exit 3"}), &c).await.unwrap();
    assert_eq!(out.data.unwrap()["exit_code"], 3);
    // ошибка командлета PowerShell → 1
    let out = t.call(json!({"command": "Get-Item 'C:\\нет\\такого\\файла'"}), &c).await.unwrap();
    let d = out.data.unwrap();
    assert_eq!(d["exit_code"], 1, "{}", out.content);
    // cmd.exe с кавычками и кодом возврата
    let out = t.call(json!({"command": "echo \"a b\" & exit /b 2", "shell": "cmd"}), &c).await.unwrap();
    assert!(out.content.contains("\"a b\""), "{}", out.content);
    assert_eq!(out.data.unwrap()["exit_code"], 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_run_timeout_kills_process_tree() {
    let tk = toolkit("timeout");
    let (c, _) = ctx();
    let start = Instant::now();
    let out = tool(&tk, "terminal_run").call(json!({"command": "ping -n 30 127.0.0.1", "timeout_ms": 1500}), &c).await.unwrap();
    let took = start.elapsed();
    println!("{} ({took:?})", out.content);
    assert!(took < Duration::from_secs(8), "таймаут не сработал: {took:?}");
    assert!(out.content.contains("таймаут"));
    let d = out.data.unwrap();
    assert_eq!(d["timed_out"], true);
    assert!(d["exit_code"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_run_cancel() {
    let tk = toolkit("cancel");
    let (mut c, _) = ctx();
    let token = CancellationToken::new();
    c.cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        token.cancel();
    });
    let start = Instant::now();
    let r = tool(&tk, "terminal_run").call(json!({"command": "ping -n 30 127.0.0.1"}), &c).await;
    assert_eq!(r.unwrap_err(), ToolError::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(8));
}

#[test]
fn tiers_come_from_classifier() {
    let classifier: jarvis_pc::CommandClassifier = Arc::new(|cmd: &str| {
        if cmd.starts_with("git status") {
            RiskTier::T0
        } else if cmd.starts_with("git commit") {
            RiskTier::T2
        } else {
            RiskTier::T3
        }
    });
    let tk = PcToolkit::new(PcOptions::new(temp_dir("tiers")).with_classifier(classifier));
    let run = tool(&tk, "terminal_run");
    assert_eq!(run.tier_for(&json!({"command": "git status"})), RiskTier::T1, "не ниже T1");
    assert_eq!(run.tier_for(&json!({"command": "git commit -m x"})), RiskTier::T2);
    assert_eq!(run.tier_for(&json!({"command": "Remove-Item -Recurse C:\\x"})), RiskTier::T3);
    assert_eq!(run.tier_for(&json!({})), RiskTier::T3, "без команды — fail-closed");
    let typ = tool(&tk, "terminal_type");
    assert_eq!(typ.tier_for(&json!({"text": "Remove-Item -Recurse C:\\x"})), RiskTier::T1, "без Enter не выполняется");
    assert_eq!(typ.tier_for(&json!({"text": "Remove-Item -Recurse C:\\x", "enter": true})), RiskTier::T3);
    assert_eq!(typ.tier_for(&json!({"text": "git commit -m x", "enter": true})), RiskTier::T2);
    // по умолчанию — всё T3
    let default = toolkit("tiers-default");
    assert_eq!(tool(&default, "terminal_run").tier_for(&json!({"command": "echo 1"})), RiskTier::T3);
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_context_from_own_pty() {
    let tk = toolkit("ctx-pty");
    let (c, _) = ctx();
    tool(&tk, "terminal_run").call(json!({"command": "cmd /c \"echo boom 1>&2 & exit 5\""}), &c).await.unwrap();
    let out = tool(&tk, "terminal_context").call(json!({}), &c).await.unwrap();
    println!("{}", out.content);
    assert!(out.untrusted);
    let d = out.data.unwrap();
    assert_eq!(d["source"], "pty");
    assert_eq!(d["exit_code"], 5);
    assert_eq!(d["error_detected"], true);
    assert!(d["error_text"].as_str().unwrap().contains("boom"));
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_context_from_hook_file() {
    let dir = temp_dir("ctx-hook");
    std::fs::create_dir_all(terminal_dir(&dir)).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let rec = json!({
        "version": 1, "shell": "powershell", "command": "git status", "exit_code": 0, "success": true,
        "error_text": null, "cwd": "C:\\proj", "time_unix_ms": now, "pid": 1
    });
    std::fs::write(hook_file(&dir), rec.to_string()).unwrap();
    let tk = PcToolkit::new(PcOptions::new(&dir));
    let (c, _) = ctx();
    let out = tool(&tk, "terminal_context").call(json!({}), &c).await.unwrap();
    let d = out.data.unwrap();
    assert_eq!(d["source"], "hook");
    assert_eq!(d["last_command"], "git status");
    assert_eq!(d["cwd"], "C:\\proj");
    assert_eq!(d["exit_code"], 0);

    // явный источник hook + ошибка PowerShell в error_text
    let rec = json!({
        "version": 1, "shell": "powershell", "command": "Get-Foo", "exit_code": 1, "success": false,
        "error_text": "Get-Foo : Имя \"Get-Foo\" не распознано как имя командлета", "cwd": "C:\\proj", "time_unix_ms": now
    });
    std::fs::write(hook_file(&dir), rec.to_string()).unwrap();
    let out = tool(&tk, "terminal_context").call(json!({"source": "hook"}), &c).await.unwrap();
    assert!(out.content.contains("не распознано"));
    assert!(out.content.contains("Код возврата: 1"));

    // устаревший хук игнорируется в auto → экран (UIA/OCR) или понятная ошибка
    let old = json!({"version": 1, "shell": "powershell", "command": "old", "exit_code": 0, "time_unix_ms": 1000});
    std::fs::write(hook_file(&dir), old.to_string()).unwrap();
    match tool(&tk, "terminal_context").call(json!({}), &c).await {
        Ok(o) => assert_ne!(o.data.unwrap()["source"], "hook"),
        Err(e) => println!("нет окна терминала: {e}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_context_reads_open_terminal_window_if_any() {
    // Только чтение UIA/OCR окна терминала пользователя, если оно открыто.
    let (c, _) = ctx();
    match tool(&toolkit("ctx-uia"), "terminal_context").call(json!({"source": "uia"}), &c).await {
        Ok(o) => println!("UIA терминала:\n{}", o.content),
        Err(e) => println!("UIA терминала недоступен: {e}"),
    }
}

#[test]
fn hook_script_generation_does_not_install() {
    let dir = temp_dir("hook-gen");
    let ps = jarvis_pc::shell_hook_script(jarvis_pc::HookShell::Powershell, &dir);
    assert!(ps.contains(&terminal_dir(&dir).to_string_lossy().to_string()));
    let bash = jarvis_pc::shell_hook_script(jarvis_pc::HookShell::Bash, &dir);
    assert!(bash.contains("PROMPT_COMMAND"));
}
