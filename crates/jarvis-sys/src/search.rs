//! `file_search`: параллельный обход (`ignore::WalkBuilder`) с фильтром по имени/расширению/дате и поиском
//! внутри текстовых файлов (`grep-searcher`). Бюджет времени ≤ 5 с, результаты нумеруются для «открой второй».

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};
use ignore::{WalkBuilder, WalkState};
use jarvis_core::{RiskTier, Tool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use serde_json::{Value, json};

use crate::context::Sys;
use crate::util::{fmt_bytes, norm_text, opt_str, opt_str_list, opt_u64, spec};

#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// Слова из имени файла (можно пусто, если ищем по содержимому/расширению).
    pub name: String,
    /// Расширения без точки, нижний регистр.
    pub ext: Vec<String>,
    pub modified_after: Option<SystemTime>,
    /// Текст внутри файла (без учёта регистра).
    pub content: Option<String>,
    pub limit: usize,
    pub budget: Duration,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self { name: String::new(), ext: Vec::new(), modified_after: None, content: None, limit: 20, budget: Duration::from_secs(5) }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Hit {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<u64>,
    /// Номер строки и сама строка с совпадением по содержимому.
    pub line: Option<(u64, String)>,
    pub score: f32,
}

#[derive(Debug, Clone, Default)]
pub struct SearchResult {
    pub hits: Vec<Hit>,
    /// Остановились по бюджету времени или лимиту.
    pub truncated: bool,
    pub scanned: usize,
    pub elapsed: Duration,
}

const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", ".git", "$recycle.bin", "system volume information", "__pycache__", ".cache", ".venv",
    "windows", "winsxs", "program files", "program files (x86)", "programdata", "appdata", ".cargo", ".rustup", ".npm",
];

/// Грубая русская основа: «договора» → «договор», «отчёты» → «отчет».
fn stem(w: &str) -> String {
    const ENDS: &[&str] = &[
        "ами", "ями", "ого", "его", "ому", "ему", "ов", "ев", "ей", "ой", "ий", "ый", "ая", "яя", "ое", "ее", "ах", "ях", "ам",
        "ям", "ом", "ем", "а", "я", "ы", "и", "у", "ю", "е", "о", "ь",
    ];
    let n = w.chars().count();
    for e in ENDS {
        if w.ends_with(e) && n - e.chars().count() >= 4 {
            return w[..w.len() - e.len()].to_string();
        }
    }
    w.to_string()
}

fn name_score(stems: &[String], q_norm: &str, file_name: &str) -> f32 {
    if stems.is_empty() {
        return 0.5;
    }
    let stem_part = Path::new(file_name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let n = norm_text(&stem_part.replace('_', " "));
    if !stems.iter().all(|s| n.contains(s.as_str())) {
        return 0.0;
    }
    if n == q_norm {
        1.0
    } else if n.starts_with(q_norm) {
        0.9
    } else {
        0.7 + 0.2 * (q_norm.len() as f32 / n.len().max(1) as f32).min(1.0)
    }
}

/// Найти файлы.
pub fn search(roots: &[PathBuf], q: &SearchQuery) -> SearchResult {
    let start = Instant::now();
    let deadline = start + q.budget.min(Duration::from_secs(5));
    let roots: Vec<&PathBuf> = roots.iter().filter(|r| r.exists()).collect();
    let Some(first) = roots.first() else { return SearchResult::default() };
    let q_norm = norm_text(&q.name);
    let stems: Vec<String> = q_norm.split(' ').filter(|w| w.chars().count() >= 2).map(stem).collect();
    let matcher = q.content.as_ref().and_then(|c| RegexMatcherBuilder::new().case_insensitive(true).fixed_strings(true).build(c).ok());

    let mut wb = WalkBuilder::new(first);
    for r in &roots[1..] {
        wb.add(r);
    }
    wb.hidden(true)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .follow_links(false)
        .threads(std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(4))
        .filter_entry(|e| {
            !(e.file_type().is_some_and(|t| t.is_dir()) && e.depth() > 0 && SKIP_DIRS.contains(&e.file_name().to_string_lossy().to_lowercase().as_str()))
        });

    let hits = Arc::new(Mutex::new(Vec::<Hit>::new()));
    let scanned = Arc::new(AtomicUsize::new(0));
    let truncated = Arc::new(AtomicBool::new(false));
    let cap = q.limit.max(1) * 5;
    wb.build_parallel().run(|| {
        let hits = hits.clone();
        let scanned = scanned.clone();
        let truncated = truncated.clone();
        let stems = stems.clone();
        let q_norm = q_norm.clone();
        let matcher = matcher.clone();
        Box::new(move |entry| {
            if Instant::now() > deadline {
                truncated.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            let Ok(e) = entry else { return WalkState::Continue };
            if !e.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            scanned.fetch_add(1, Ordering::Relaxed);
            let name = e.file_name().to_string_lossy().to_string();
            if !q.ext.is_empty() {
                let ext = Path::new(&name).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                if !q.ext.contains(&ext) {
                    return WalkState::Continue;
                }
            }
            let score = name_score(&stems, &q_norm, &name);
            if score == 0.0 && (matcher.is_none() || !stems.is_empty()) {
                return WalkState::Continue;
            }
            let Ok(meta) = e.metadata() else { return WalkState::Continue };
            let modified = meta.modified().ok();
            if let (Some(after), Some(m)) = (q.modified_after, modified)
                && m < after
            {
                return WalkState::Continue;
            }
            let mut line = None;
            if let Some(m) = &matcher {
                if meta.len() > 10 * 1024 * 1024 {
                    return WalkState::Continue;
                }
                let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
                let _ = searcher.search_path(
                    m,
                    e.path(),
                    UTF8(|n, l| {
                        line = Some((n, l.trim_end().chars().take(200).collect()));
                        Ok(false)
                    }),
                );
                if line.is_none() {
                    return WalkState::Continue;
                }
            }
            let mut h = hits.lock().unwrap();
            h.push(Hit {
                path: e.path().to_path_buf(),
                size: meta.len(),
                modified: modified.and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
                line,
                score,
            });
            if h.len() >= cap {
                truncated.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            WalkState::Continue
        })
    });
    let mut hits = Arc::try_unwrap(hits).map(|m| m.into_inner().unwrap()).unwrap_or_default();
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then(b.modified.cmp(&a.modified)));
    hits.truncate(q.limit.max(1));
    SearchResult { hits, truncated: truncated.load(Ordering::Relaxed), scanned: scanned.load(Ordering::Relaxed), elapsed: start.elapsed() }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// «2026-01-31», «2026-01-31T10:00:00», «7d», «24h», «2w».
pub fn parse_since(s: &str, now: SystemTime) -> Option<SystemTime> {
    let s = s.trim().to_lowercase();
    let rel = |n: &str, unit: u64| n.parse::<u64>().ok().map(|v| now - Duration::from_secs(v * unit));
    for (suf, unit) in [("d", 86400), ("д", 86400), ("h", 3600), ("ч", 3600), ("w", 7 * 86400), ("н", 7 * 86400)] {
        if let Some(n) = s.strip_suffix(suf) {
            return rel(n.trim(), unit);
        }
    }
    let date = s.get(..10)?;
    let p: Vec<i64> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    if p.len() != 3 {
        return None;
    }
    let days = days_from_civil(p[0], p[1], p[2]);
    let mut secs = days * 86400;
    if let Some(t) = s.get(11..19) {
        let hms: Vec<i64> = t.split(':').filter_map(|x| x.parse().ok()).collect();
        if hms.len() == 3 {
            secs += hms[0] * 3600 + hms[1] * 60 + hms[2];
        }
    }
    (secs >= 0).then(|| UNIX_EPOCH + Duration::from_secs(secs as u64))
}

fn fmt_date(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    // обратное к days_from_civil
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{d:02}.{m:02}.{y}")
}

pub struct FileSearch(pub Arc<Sys>);

#[async_trait]
impl Tool for FileSearch {
    fn spec(&self) -> ToolSpec {
        spec(
            "file_search",
            "Найти файлы по имени, расширению, дате изменения и/или тексту внутри. По умолчанию ищет в профиле и на D:\\. Результаты пронумерованы — затем open_target/reveal_in_explorer с result=N.",
            json!({"type":"object","properties":{
                "query":{"type":"string","description":"Слова из имени файла"},
                "roots":{"type":"array","items":{"type":"string"}},
                "ext":{"description":"Расширение или список: pdf, docx","oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]},
                "modified_after":{"type":"string","description":"YYYY-MM-DD или 7d/24h/2w"},
                "content":{"type":"string","description":"Текст внутри файла"},
                "limit":{"type":"integer","default":20,"maximum":100}
            }}),
            RiskTier::T0,
        )
    }

    async fn call(&self, args: Value, _ctx: &ToolCtx) -> Result<ToolOutput, ToolError> {
        let name = opt_str(&args, "query").unwrap_or_default();
        let content = opt_str(&args, "content");
        let ext: Vec<String> = opt_str_list(&args, "ext").into_iter().map(|e| e.trim_start_matches('.').to_lowercase()).collect();
        if name.trim().is_empty() && content.is_none() && ext.is_empty() {
            return Err(ToolError::InvalidArgs("нужен query, content или ext".into()));
        }
        let modified_after = match opt_str(&args, "modified_after") {
            Some(s) => Some(parse_since(&s, SystemTime::now()).ok_or_else(|| ToolError::InvalidArgs(format!("не понял дату «{s}»")))?),
            None => None,
        };
        let mut roots: Vec<PathBuf> = opt_str_list(&args, "roots").into_iter().map(PathBuf::from).collect();
        if roots.is_empty() {
            roots = self.0.cfg.search_roots.clone();
        }
        let q = SearchQuery { name, ext, modified_after, content: content.clone(), limit: opt_u64(&args, "limit").unwrap_or(20).clamp(1, 100) as usize, budget: Duration::from_secs(5) };
        let r = tokio::task::spawn_blocking(move || search(&roots, &q)).await.map_err(|e| ToolError::Failed(e.to_string()))?;
        *self.0.last_results.lock().unwrap() = r.hits.iter().map(|h| h.path.clone()).collect();
        if r.hits.is_empty() {
            return Ok(ToolOutput::text(format!("Ничего не найдено (просмотрено файлов: {}{}).", r.scanned, if r.truncated { ", поиск прерван по времени" } else { "" })));
        }
        let mut lines = Vec::new();
        for (i, h) in r.hits.iter().enumerate() {
            let mut s = format!("{}. {} ({}{})", i + 1, h.path.display(), fmt_bytes(h.size), h.modified.map(|m| format!(", {}", fmt_date(m))).unwrap_or_default());
            if let Some((n, l)) = &h.line {
                let l = if self.0.cfg.redact_output { crate::redact::redact_secrets(l).0 } else { l.clone() };
                s.push_str(&format!("\n   строка {n}: {l}"));
            }
            lines.push(s);
        }
        if r.truncated {
            lines.push("(показаны не все результаты)".into());
        }
        Ok(ToolOutput {
            content: lines.join("\n"),
            data: Some(json!({"hits": r.hits, "truncated": r.truncated, "elapsed_ms": r.elapsed.as_millis() as u64})),
            untrusted: content.is_some(),
            attachments: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        std::fs::create_dir_all(p.join("Документы/2026")).unwrap();
        std::fs::create_dir_all(p.join("node_modules/pkg")).unwrap();
        std::fs::write(p.join("Документы/Договор_аренды.pdf"), b"%PDF-1.4 \0 binary").unwrap();
        std::fs::write(p.join("Документы/2026/договор поставки.docx"), b"x").unwrap();
        std::fs::write(p.join("Документы/notes.txt"), "Купить молоко\nПозвонить по договору в пятницу\n").unwrap();
        std::fs::write(p.join("node_modules/pkg/договор.js"), "skip").unwrap();
        std::fs::write(p.join("readme.md"), "hello").unwrap();
        t
    }

    #[test]
    fn by_name_ext_and_content() {
        let t = fixture();
        let roots = vec![t.path().to_path_buf()];
        let r = search(&roots, &SearchQuery { name: "договор".into(), ..Default::default() });
        let names: Vec<String> = r.hits.iter().map(|h| h.path.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(r.hits.len(), 2, "{names:?}");
        assert!(!names.iter().any(|n| n.ends_with(".js")), "node_modules пропускается");

        let r = search(&roots, &SearchQuery { name: "договора".into(), ext: vec!["pdf".into()], ..Default::default() });
        assert_eq!(r.hits.len(), 1);
        assert!(r.hits[0].path.ends_with("Договор_аренды.pdf"));

        let r = search(&roots, &SearchQuery { content: Some("ДОГОВОРУ".into()), ..Default::default() });
        assert_eq!(r.hits.len(), 1, "{:?}", r.hits);
        assert_eq!(r.hits[0].line.as_ref().unwrap().0, 2);

        let future = SystemTime::now() + Duration::from_secs(3600);
        assert!(search(&roots, &SearchQuery { name: "договор".into(), modified_after: Some(future), ..Default::default() }).hits.is_empty());
        assert!(r.elapsed < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn tool_numbers_results() {
        use crate::context::SysConfig;
        use crate::open::tests_support::{MockOpener, MockSources};
        let t = fixture();
        let s = Sys::with(SysConfig::for_tests(t.path()), Arc::new(MockOpener::default()), Arc::new(crate::undo::DirTrash::new(t.path().join("trash"))), Arc::new(MockSources::default()));
        let out = FileSearch(s.clone()).call(json!({"query": "договор"}), &ToolCtx::for_tests()).await.unwrap();
        assert!(out.content.starts_with("1. "), "{}", out.content);
        assert!(out.content.contains("\n2. "));
        assert_eq!(s.last_results.lock().unwrap().len(), 2);
    }

    #[test]
    fn dates() {
        let now = SystemTime::now();
        assert_eq!(parse_since("7d", now), Some(now - Duration::from_secs(7 * 86400)));
        let d = parse_since("2026-11-22", now).unwrap();
        assert_eq!(d.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_795_305_600);
        assert_eq!(fmt_date(1_795_305_600), "22.11.2026");
        assert!(parse_since("вчера", now).is_none());
        assert_eq!(stem("договора"), "договор");
    }
}
