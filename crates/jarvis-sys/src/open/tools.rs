//! Инструменты: `open_target`, `reveal_in_explorer`, `url_open`, `clipboard_get`, `clipboard_set` (T1).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use super::{OpenRequest, TargetKind, is_confident, resolve};
use crate::context::Sys;
use crate::util::{opt_str, opt_u64, req_str, spec, truncate_middle};

fn result_path(sys: &Sys, args: &Value) -> Result<Option<PathBuf>, ToolError> {
    let Some(n) = opt_u64(args, "result") else { return Ok(None) };
    let r = sys.last_results.lock().unwrap();
    r.get((n as usize).wrapping_sub(1))
        .cloned()
        .map(Some)
        .ok_or_else(|| ToolError::InvalidArgs(format!("нет результата №{n} последнего поиска (всего {})", r.len())))
}

pub struct OpenTarget(pub Arc<Sys>);

#[async_trait]
impl Tool for OpenTarget {
    fn spec(&self) -> ToolSpec {
        spec(
            "open_target",
            "Открыть программу, окно, папку, страницу настроек Windows или файл по названию (по-русски или по-английски): «блокнот», «настройки bluetooth», «загрузки». Если вариантов несколько — вернёт список, затем вызовите снова с pick=N. result=N — открыть N-й результат последнего file_search.",
            json!({"type":"object","properties":{
                "name":{"type":"string","description":"Что открыть"},
                "pick":{"type":"integer","description":"Номер варианта из предыдущего ответа"},
                "result":{"type":"integer","description":"Номер результата file_search"}
            }}),
            RiskTier::T1,
        )
    }

    fn tier_for(&self, args: &Value) -> RiskTier {
        let n = opt_str(args, "name").unwrap_or_default().to_lowercase();
        let exec = [".exe", ".bat", ".cmd", ".ps1", ".vbs", ".msi", ".reg", ".scr", ".js", ".hta"];
        if exec.iter().any(|e| n.trim_matches('"').ends_with(e)) && (n.contains('\\') || n.contains('/')) {
            return RiskTier::T3;
        }
        RiskTier::T1
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let sys = self.0.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(p) = result_path(&sys, &args)? {
                let kind = TargetKind::File { path: p.clone() };
                if kind.is_executable_file() {
                    return Err(ToolError::Denied(format!("{} — исполняемый файл; запуск требует подтверждения", p.display())));
                }
                sys.opener.open(&kind.request()).map_err(|e| ToolError::Failed(e.to_string()))?;
                return Ok(ToolOutput::text(format!("Открываю {}", p.display())));
            }
            let name = req_str(&args, "name")?;
            let cands = resolve(sys.sources.as_ref(), &name);
            let data = json!({"candidates": cands});
            let chosen = match opt_u64(&args, "pick") {
                Some(n) => Some(cands.get((n as usize).wrapping_sub(1)).cloned().ok_or_else(|| ToolError::InvalidArgs(format!("нет варианта №{n}")))?),
                None if is_confident(&cands) => cands.first().cloned(),
                None => None,
            };
            let Some(c) = chosen else {
                if cands.is_empty() {
                    return Ok(ToolOutput { content: format!("Не нашёл «{name}»."), data: Some(data), ..Default::default() });
                }
                let list: Vec<String> = cands.iter().take(5).enumerate().map(|(i, c)| format!("{}. {} ({})", i + 1, c.title, c.source)).collect();
                return Ok(ToolOutput { content: format!("Уточните, что открыть:\n{}", list.join("\n")), data: Some(data), ..Default::default() });
            };
            if c.kind.is_executable_file() {
                return Err(ToolError::Denied(format!("«{}» — исполняемый файл из поиска; запуск требует подтверждения", c.title)));
            }
            sys.opener.open(&c.kind.request()).map_err(|e| ToolError::Failed(format!("не удалось открыть «{}»: {e}", c.title)))?;
            let verb = if matches!(c.kind, TargetKind::Window { .. }) { "Переключаюсь на" } else { "Открываю" };
            Ok(ToolOutput { content: format!("{verb}: {}", c.title), data: Some(json!({"opened": c, "candidates": cands})), ..Default::default() })
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
    }
}

pub struct RevealInExplorer(pub Arc<Sys>);

#[async_trait]
impl Tool for RevealInExplorer {
    fn spec(&self) -> ToolSpec {
        spec(
            "reveal_in_explorer",
            "Показать файл в Проводнике (выделить). path или result=N из последнего file_search.",
            json!({"type":"object","properties":{"path":{"type":"string"},"result":{"type":"integer"}}}),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let p = match result_path(&self.0, &args)? {
            Some(p) => p,
            None => self.0.abs(&req_str(&args, "path")?),
        };
        if !p.exists() {
            return Err(ToolError::Failed(format!("не найдено: {}", p.display())));
        }
        self.0.opener.open(&OpenRequest::Reveal { path: p.clone() }).map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok(ToolOutput::text(format!("Показываю в Проводнике: {}", p.display())))
    }
}

pub struct UrlOpen(pub Arc<Sys>);

/// Разрешённые схемы: веб, почта, настройки.
pub fn url_allowed(u: &str) -> bool {
    let l = u.trim().to_lowercase();
    (l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:") || l.starts_with("ms-settings:"))
        && !l.contains(char::is_whitespace)
}

#[async_trait]
impl Tool for UrlOpen {
    fn spec(&self) -> ToolSpec {
        spec(
            "url_open",
            "Открыть ссылку в браузере по умолчанию (http/https/mailto/ms-settings).",
            json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let url = req_str(&args, "url")?;
        if !url_allowed(&url) {
            return Err(ToolError::InvalidArgs(format!("недопустимая ссылка: {url} (только http, https, mailto, ms-settings)")));
        }
        self.0.opener.open(&OpenRequest::Uri { uri: url.trim().to_string() }).map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok(ToolOutput::text(format!("Открываю {url}")))
    }
}

pub struct ClipboardGet(pub Arc<Sys>);

#[async_trait]
impl Tool for ClipboardGet {
    fn spec(&self) -> ToolSpec {
        spec("clipboard_get", "Прочитать текст из буфера обмена (секреты маскируются).", json!({"type":"object","properties":{}}), RiskTier::T1)
    }

    async fn call(&self, _args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let text = tokio::task::spawn_blocking(crate::win::clipboard_get)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let Some(text) = text else { return Ok(ToolOutput::text("В буфере обмена нет текста.")) };
        let (text, n) = if self.0.cfg.redact_output { crate::redact::redact_secrets(&text) } else { (text, 0) };
        let (mut text, _) = truncate_middle(&text, 20_000);
        if n > 0 {
            text.push_str(&format!("\n(скрыто секретов: {n})"));
        }
        Ok(ToolOutput::untrusted(text))
    }
}

pub struct ClipboardSet;

#[async_trait]
impl Tool for ClipboardSet {
    fn spec(&self) -> ToolSpec {
        spec(
            "clipboard_set",
            "Положить текст в буфер обмена.",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
            RiskTier::T1,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let text = args.get("text").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("нужен text".into()))?.to_string();
        let n = text.chars().count();
        tokio::task::spawn_blocking(move || crate::win::clipboard_set(&text))
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok(ToolOutput::text(format!("Скопировано в буфер обмена ({n} символов)")))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{MockOpener, sources};
    use super::*;
    use crate::context::{Sys, SysConfig};
    use crate::undo::DirTrash;

    fn sys(opener: Arc<MockOpener>) -> (Arc<Sys>, tempfile::TempDir) {
        let t = tempfile::tempdir().unwrap();
        let s = Sys::with(SysConfig::for_tests(t.path()), opener, Arc::new(DirTrash::new(t.path().join("trash"))), Arc::new(sources()));
        (s, t)
    }

    #[tokio::test]
    async fn opens_via_injected_opener() {
        let op = Arc::new(MockOpener::default());
        let (s, _t) = sys(op.clone());
        let ctx = ToolCtx::for_tests();
        OpenTarget(s.clone()).call(json!({"name": "настройки bluetooth"}), &ctx).await.unwrap();
        OpenTarget(s.clone()).call(json!({"name": "загрузки"}), &ctx).await.unwrap();
        let opened = op.0.lock().unwrap().clone();
        assert_eq!(opened[0], OpenRequest::Uri { uri: "ms-settings:bluetooth".into() });
        assert_eq!(opened[1], OpenRequest::Path { path: "C:\\Users\\test\\Downloads".into() });
        // исполняемый файл из поиска не запускается
        assert!(matches!(OpenTarget(s.clone()).call(json!({"name": "setup"}), &ctx).await, Err(ToolError::Denied(_))));
        assert!(matches!(UrlOpen(s.clone()).call(json!({"url": "file:///C:/x.exe"}), &ctx).await, Err(ToolError::InvalidArgs(_))));
        UrlOpen(s.clone()).call(json!({"url": "https://ya.ru"}), &ctx).await.unwrap();
        assert_eq!(op.0.lock().unwrap().len(), 3);
        assert_eq!(OpenTarget(s).tier_for(&json!({"name": "C:\\x\\evil.exe"})), RiskTier::T3);
    }

    #[test]
    fn url_rules() {
        assert!(url_allowed("https://example.com/a?b=1"));
        assert!(url_allowed("ms-settings:sound"));
        assert!(!url_allowed("javascript:alert(1)"));
        assert!(!url_allowed("file:///C:/Windows"));
    }
}
