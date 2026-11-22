//! Оценка поиска: `eval/gold.jsonl` — строки `{"q": [...] | "...", "expect": "подстрока пути/URL" | [...], "lang": "en|ru"}`.
//! Попадание — URL или путь найденного документа содержит любую из ожидаемых подстрок (без учёта регистра).
//! Метрики: Recall@k (по умолчанию k = 5) и MRR@10.

use jarvis_kb::{Kb, KbHit, SearchParams};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct GoldLine {
    q: OneOrMany,
    expect: OneOrMany,
    #[serde(default)]
    lang: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GoldItem {
    pub queries: Vec<String>,
    pub expect: Vec<String>,
    pub lang: String,
    pub line: usize,
}

pub fn parse_gold(text: &str) -> Result<Vec<GoldItem>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with("//") {
            continue;
        }
        let g: GoldLine = serde_json::from_str(l).map_err(|e| format!("gold.jsonl:{}: {e}", i + 1))?;
        let queries = g.q.into_vec();
        let expect = g.expect.into_vec();
        if queries.is_empty() || expect.is_empty() {
            return Err(format!("gold.jsonl:{}: пустые q или expect", i + 1));
        }
        out.push(GoldItem { queries, expect, lang: if g.lang.is_empty() { "en".into() } else { g.lang }, line: i + 1 });
    }
    Ok(out)
}

pub fn load_gold(path: &Path) -> Result<Vec<GoldItem>, String> {
    parse_gold(&std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?)
}

pub fn hit_matches(hit: &KbHit, expect: &[String]) -> bool {
    let url = hit.url.to_lowercase();
    let path = hit.path.to_lowercase();
    expect.iter().any(|e| {
        let e = e.to_lowercase();
        url.contains(&e) || path.contains(&e)
    })
}

#[derive(Debug, Clone)]
pub struct ItemResult {
    pub item: GoldItem,
    /// Ранг первого верного документа (с 1) среди первых 10.
    pub rank: Option<usize>,
    pub top: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EvalReport {
    pub k: usize,
    pub n: usize,
    pub hits_at_k: usize,
    pub recall_at_k: f64,
    pub mrr: f64,
    pub results: Vec<ItemResult>,
}

impl EvalReport {
    pub fn by_lang(&self, lang: &str) -> (usize, usize) {
        let items: Vec<_> = self.results.iter().filter(|r| r.item.lang == lang).collect();
        (items.iter().filter(|r| r.rank.is_some_and(|x| x <= self.k)).count(), items.len())
    }
}

/// Универсальная оценка: `search(queries, depth)` — любая функция поиска (база, фикстура).
pub fn evaluate_with(items: &[GoldItem], k: usize, mut search: impl FnMut(&[String], usize) -> Vec<KbHit>) -> EvalReport {
    let depth = k.max(10);
    let mut results = Vec::new();
    let (mut hits, mut rr_sum) = (0usize, 0.0f64);
    for it in items {
        let found = search(&it.queries, depth);
        let rank = found.iter().position(|h| hit_matches(h, &it.expect)).map(|p| p + 1);
        if rank.is_some_and(|r| r <= k) {
            hits += 1;
        }
        if let Some(r) = rank {
            rr_sum += 1.0 / r as f64;
        }
        results.push(ItemResult { item: it.clone(), rank, top: found.iter().take(k).map(|h| h.url.clone()).collect() });
    }
    let n = items.len();
    EvalReport {
        k,
        n,
        hits_at_k: hits,
        recall_at_k: if n == 0 { 0.0 } else { hits as f64 / n as f64 },
        mrr: if n == 0 { 0.0 } else { rr_sum / n as f64 },
        results,
    }
}

pub fn evaluate(kb: &Kb, items: &[GoldItem], k: usize, params: &SearchParams) -> EvalReport {
    evaluate_with(items, k, |q, depth| kb.search_with(q, depth, params).unwrap_or_default())
}

/// `--param name=value` для подбора весов без перекомпиляции.
pub fn apply_param(p: &mut SearchParams, kv: &str) -> Result<(), String> {
    let (k, v) = kv.split_once('=').ok_or_else(|| format!("--param {kv}: нужно имя=значение"))?;
    let f = |v: &str| v.trim().parse::<f64>().map_err(|e| format!("--param {kv}: {e}"));
    match k.trim() {
        "weights" => {
            let w: Vec<f64> = v.split(',').map(f).collect::<Result<_, _>>()?;
            if w.len() != 4 {
                return Err("weights: 4 числа (title,commands,heading_path,body)".into());
            }
            p.weights = [w[0], w[1], w[2], w[3]];
        }
        "rrf_k" => p.rrf_k = f(v)?,
        "w_and" => p.w_and = f(v)?,
        "w_or" => p.w_or = f(v)?,
        "w_cmd" => p.w_cmd = f(v)?,
        "w_cmd_weak" => p.w_cmd_weak = f(v)?,
        "candidates" => p.candidates = f(v)? as usize,
        other => return Err(format!("неизвестный параметр {other}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(url: &str) -> KbHit {
        KbHit { title: url.into(), url: url.into(), heading: String::new(), snippet: String::new(), score: 0.0, source: "s".into(), path: format!("p/{url}") }
    }

    #[test]
    fn parses_gold_lines() {
        let g = parse_gold("# comment\n{\"q\":\"a b\",\"expect\":\"x\"}\n\n{\"q\":[\"c\",\"d\"],\"expect\":[\"y\",\"z\"],\"lang\":\"ru\"}\n").unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].queries, ["a b"]);
        assert_eq!(g[0].lang, "en");
        assert_eq!(g[1].expect, ["y", "z"]);
        assert_eq!(g[1].line, 4);
        assert!(parse_gold("{\"q\":[],\"expect\":\"x\"}").is_err());
        assert!(parse_gold("not json").unwrap_err().contains(":1:"));
    }

    #[test]
    fn metrics_on_fixture() {
        let items = parse_gold(
            "{\"q\":\"one\",\"expect\":\"/robocopy\"}\n{\"q\":\"two\",\"expect\":\"get-service\"}\n{\"q\":\"three\",\"expect\":\"missing\"}\n{\"q\":\"four\",\"expect\":\"deep\",\"lang\":\"ru\"}",
        )
        .unwrap();
        let rep = evaluate_with(&items, 5, |q, depth| {
            assert_eq!(depth, 10);
            match q[0].as_str() {
                "one" => vec![hit("https://x/robocopy")],
                "two" => vec![hit("a"), hit("b"), hit("https://x/Get-Service?view=7.6")],
                "four" => (1..=7).map(|i| hit(if i == 7 { "deep" } else { "zz" })).collect(),
                _ => vec![hit("a")],
            }
        });
        assert_eq!(rep.n, 4);
        assert_eq!(rep.hits_at_k, 2); // «four» найден на 7-м месте — вне top-5
        assert!((rep.recall_at_k - 0.5).abs() < 1e-12);
        assert!((rep.mrr - (1.0 + 1.0 / 3.0 + 1.0 / 7.0) / 4.0).abs() < 1e-12);
        assert_eq!(rep.results[1].rank, Some(3));
        assert_eq!(rep.results[2].rank, None);
        assert_eq!(rep.by_lang("ru"), (0, 1));
        assert_eq!(rep.by_lang("en"), (2, 3));
    }

    /// Харнесс на настоящей маленькой базе (KbWriter → Kb → evaluate).
    #[test]
    fn evaluate_real_fixture_db() {
        use jarvis_kb::{KbWriter, NewChunk};
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("kb.sqlite");
        let mut w = KbWriter::create(&p).unwrap();
        for (url, title, body) in [
            ("https://l/windows-commands/robocopy", "robocopy", "Copies file data. Mirror a directory tree with /mir."),
            ("https://l/windows-commands/ipconfig", "ipconfig", "Displays TCP/IP configuration. /flushdns flushes the DNS resolver cache."),
            ("kb/seed/startup.md", "Автозагрузка (startup apps)", "Как отключить автозагрузку программ: Диспетчер задач."),
        ] {
            w.add_chunk(&NewChunk { source: "t", path: url, url, title, body, ..Default::default() }).unwrap();
        }
        w.finish().unwrap();
        let kb = Kb::open(&p).unwrap();
        let gold = parse_gold(
            "{\"q\":[\"flush DNS cache\",\"clear DNS resolver cache command\"],\"expect\":\"/ipconfig\"}\n\
             {\"q\":\"mirror folder robocopy\",\"expect\":\"robocopy\"}\n\
             {\"q\":\"как отключить автозагрузку\",\"expect\":\"kb/seed/startup\",\"lang\":\"ru\"}\n\
             {\"q\":\"bluetooth pairing\",\"expect\":\"bluetooth\"}",
        )
        .unwrap();
        let rep = evaluate(&kb, &gold, 5, &SearchParams::default());
        assert_eq!(rep.hits_at_k, 3);
        assert_eq!(rep.results[0].rank, Some(1));
        assert_eq!(rep.results[3].rank, None);
        assert!((rep.recall_at_k - 0.75).abs() < 1e-12);
        assert!((rep.mrr - 0.75).abs() < 1e-12);
    }

    #[test]
    fn match_by_path_too() {
        assert!(hit_matches(&hit("u"), &["P/U".into()]));
        assert!(!hit_matches(&hit("u"), &["q".into()]));
    }

    #[test]
    fn params() {
        let mut p = SearchParams::default();
        apply_param(&mut p, "weights=6,4,3,1").unwrap();
        apply_param(&mut p, "rrf_k=30").unwrap();
        assert_eq!(p.weights, [6.0, 4.0, 3.0, 1.0]);
        assert_eq!(p.rrf_k, 30.0);
        assert!(apply_param(&mut p, "weights=1,2").is_err());
        assert!(apply_param(&mut p, "nope=1").is_err());
        assert!(apply_param(&mut p, "rrf_k").is_err());
    }
}
