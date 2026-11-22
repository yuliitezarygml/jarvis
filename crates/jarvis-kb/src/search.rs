//! Поиск: FTS5 bm25 по каждому запросу (списки AND и OR) + список «точная команда», слияние Reciprocal Rank
//! Fusion по документам, на выходе — лучший фрагмент каждого документа со сниппетом.

use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::hash::Hash;

use crate::{KbHit, Result, commands, query};

/// Параметры ранжирования (значения по умолчанию подобраны по `kb-build --eval`).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchParams {
    /// Веса bm25 по колонкам FTS: title, commands, heading_path, body.
    pub weights: [f64; 4],
    /// Константа RRF: вклад списка = вес / (k + ранг).
    pub rrf_k: f64,
    /// Вес списка «все слова запроса» (AND).
    pub w_and: f64,
    /// Вес списка «любое слово» (OR).
    pub w_or: f64,
    /// Вес списка «явная команда из запроса» (`Get-Service`, `sfc /scannow`, `robocopy.exe`).
    pub w_cmd: f64,
    /// Вес списка «слово запроса совпало с именем команды» (`robocopy`).
    pub w_cmd_weak: f64,
    /// Сколько фрагментов брать из FTS на один список (до схлопывания по документам).
    pub candidates: usize,
    /// Длина сниппета в символах.
    pub snippet_chars: usize,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            weights: [5.0, 4.0, 3.0, 1.0],
            rrf_k: 20.0,
            w_and: 1.0,
            w_or: 1.0,
            w_cmd: 1.5,
            w_cmd_weak: 0.7,
            candidates: 200,
            snippet_chars: 400,
        }
    }
}

/// Кандидат: фрагмент и документ, к которому он относится (ключ документа — URL).
#[derive(Debug, Clone)]
struct Cand {
    chunk_id: i64,
    doc: String,
}

/// Reciprocal Rank Fusion: `score(d) = Σ w_l / (k + rank_l(d))`, ранги с 1; повторы внутри списка игнорируются.
/// Возвращает ключи по убыванию балла (при равенстве — в порядке первого появления).
pub fn rrf_merge<K: Clone + Eq + Hash>(lists: &[(f64, Vec<K>)], k: f64) -> Vec<(K, f64)> {
    let mut score: HashMap<K, (f64, usize)> = HashMap::new();
    let mut order = 0usize;
    for (w, list) in lists {
        let mut seen: Vec<&K> = Vec::new();
        let mut rank = 0usize;
        for key in list {
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            rank += 1;
            let e = score.entry(key.clone()).or_insert_with(|| {
                order += 1;
                (0.0, order)
            });
            e.0 += w / (k + rank as f64);
        }
    }
    let mut v: Vec<(K, f64, usize)> = score.into_iter().map(|(k, (s, o))| (k, s, o)).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.2.cmp(&b.2)));
    v.into_iter().map(|(k, s, _)| (k, s)).collect()
}

fn dedupe_docs(rows: Vec<Cand>) -> Vec<Cand> {
    let mut seen = std::collections::HashSet::new();
    rows.into_iter().filter(|c| seen.insert(c.doc.clone())).collect()
}

fn fts_list(conn: &Connection, expr: &str, p: &SearchParams) -> Result<Vec<Cand>> {
    let mut st = conn.prepare_cached(
        "SELECT c.id, c.url FROM chunks_fts JOIN chunks c ON c.id = chunks_fts.rowid
         WHERE chunks_fts MATCH ?1
         ORDER BY bm25(chunks_fts, ?2, ?3, ?4, ?5) LIMIT ?6",
    )?;
    let w = p.weights;
    let rows = st.query_map(params![expr, w[0], w[1], w[2], w[3], p.candidates as i64], |r| {
        Ok(Cand { chunk_id: r.get(0)?, doc: r.get(1)? })
    });
    match rows.and_then(|it| it.collect::<rusqlite::Result<Vec<_>>>()) {
        Ok(v) => Ok(dedupe_docs(v)),
        // Синтаксическая ошибка FTS (не должна случаться — термы в кавычках) не роняет весь поиск.
        Err(e) if e.to_string().contains("fts5") => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

/// Фрагменты для точного имени команды: сначала страница самой команды, затем упоминания.
fn command_rows(conn: &Connection, name: &str, primary_only: bool, limit: usize) -> Result<Vec<(Cand, bool)>> {
    let mut st = conn.prepare_cached(
        "SELECT c.id, c.url, m.is_primary FROM commands m JOIN chunks c ON c.id = m.chunk_id
         WHERE m.name = ?1 AND (?2 = 0 OR m.is_primary = 1)
         ORDER BY m.is_primary DESC, (c.title = ?1 COLLATE NOCASE) DESC, length(c.body) > 200 DESC, c.ord, c.id
         LIMIT ?3",
    )?;
    let v = st
        .query_map(params![name, primary_only as i64, limit as i64], |r| {
            Ok((Cand { chunk_id: r.get(0)?, doc: r.get(1)? }, r.get::<_, i64>(2)? == 1))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

fn cmd_list(conn: &Connection, names: &[String]) -> Result<Vec<Cand>> {
    let mut out = Vec::new();
    for n in names {
        let n = commands::normalize_command(n);
        out.extend(command_rows(conn, &n, true, 10)?.into_iter().map(|(c, _)| c));
    }
    Ok(dedupe_docs(out))
}

struct Row {
    title: String,
    url: String,
    heading_path: String,
    body: String,
    source: String,
    path: String,
}

fn load_chunk(conn: &Connection, id: i64) -> Result<Row> {
    Ok(conn.prepare_cached("SELECT title, url, heading_path, body, source, path FROM chunks WHERE id = ?1")?.query_row(
        [id],
        |r| {
            Ok(Row {
                title: r.get(0)?,
                url: r.get(1)?,
                heading_path: r.get(2)?,
                body: r.get(3)?,
                source: r.get(4)?,
                path: r.get(5)?,
            })
        },
    )?)
}

/// Сниппет: окно `max_chars` символов вокруг первого вхождения любого из термов (иначе — начало текста).
pub(crate) fn snippet(body: &str, stems: &[String], max_chars: usize) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= max_chars {
        return flat;
    }
    let lower: Vec<char> = flat.to_lowercase().chars().collect();
    let lower = if lower.len() == chars.len() { lower } else { chars.clone() };
    let mut best: Option<usize> = None;
    for s in stems.iter().filter(|s| !s.is_empty()) {
        let sc: Vec<char> = s.chars().collect();
        if let Some(pos) = (0..lower.len().saturating_sub(sc.len() - 1)).find(|&i| {
            lower[i..i + sc.len()] == sc[..] && (i == 0 || !lower[i - 1].is_alphanumeric())
        }) {
            best = Some(best.map_or(pos, |b| b.min(pos)));
        }
    }
    let center = best.unwrap_or(0);
    let mut start = center.saturating_sub(max_chars / 4);
    if center < max_chars / 2 {
        start = 0;
    }
    let end = (start + max_chars).min(chars.len());
    let start = end.saturating_sub(max_chars).min(start);
    // по границам слов
    let mut s = start;
    if s > 0 {
        while s < end && !chars[s - 1].is_whitespace() {
            s += 1;
        }
    }
    let mut e = end;
    if e < chars.len() {
        while e > s && !chars[e].is_whitespace() {
            e -= 1;
        }
    }
    let mut out: String = chars[s..e].iter().collect::<String>().trim().to_string();
    if s > 0 {
        out.insert(0, '…');
    }
    if e < chars.len() {
        out.push('…');
    }
    out
}

fn to_hit(row: Row, score: f64, stems: &[String], p: &SearchParams) -> KbHit {
    KbHit {
        snippet: snippet(&row.body, stems, p.snippet_chars),
        title: row.title,
        url: row.url,
        heading: row.heading_path,
        score,
        source: row.source,
        path: row.path,
    }
}

pub(crate) fn search(conn: &Connection, queries: &[String], top_k: usize, p: &SearchParams) -> Result<Vec<KbHit>> {
    let top_k = top_k.clamp(1, 50);
    let mut lists: Vec<(f64, Vec<Cand>)> = Vec::new();
    let mut stems: Vec<String> = Vec::new();
    for q in queries {
        let pq = query::parse(q);
        if pq.is_empty() {
            continue;
        }
        if let Some(e) = pq.and_expr() {
            lists.push((p.w_and, fts_list(conn, &e, p)?));
        }
        if let Some(e) = pq.or_expr() {
            lists.push((p.w_or, fts_list(conn, &e, p)?));
        }
        if !pq.strong_commands.is_empty() {
            lists.push((p.w_cmd, cmd_list(conn, &pq.strong_commands)?));
        }
        if !pq.weak_commands.is_empty() {
            lists.push((p.w_cmd_weak, cmd_list(conn, &pq.weak_commands)?));
        }
        stems.extend(pq.terms.into_iter().map(|t| t.stem));
    }

    // Лучший фрагмент документа — из списка с наибольшим вкладом.
    let mut best_chunk: HashMap<String, (f64, i64)> = HashMap::new();
    for (w, list) in &lists {
        for (i, c) in list.iter().enumerate() {
            let contrib = w / (p.rrf_k + (i + 1) as f64);
            let e = best_chunk.entry(c.doc.clone()).or_insert((contrib, c.chunk_id));
            if contrib > e.0 {
                *e = (contrib, c.chunk_id);
            }
        }
    }
    let doc_lists: Vec<(f64, Vec<String>)> =
        lists.iter().map(|(w, l)| (*w, l.iter().map(|c| c.doc.clone()).collect())).collect();
    let merged = rrf_merge(&doc_lists, p.rrf_k);

    let mut hits = Vec::new();
    for (doc, score) in merged.into_iter().take(top_k) {
        let chunk_id = best_chunk[&doc].1;
        hits.push(to_hit(load_chunk(conn, chunk_id)?, score, &stems, p));
    }
    Ok(hits)
}

pub(crate) fn lookup_command(conn: &Connection, name: &str, limit: usize, p: &SearchParams) -> Result<Vec<KbHit>> {
    let n = commands::normalize_command(name);
    if n.is_empty() {
        return Ok(vec![]);
    }
    let mut rows = command_rows(conn, &n, false, 300)?;
    if rows.is_empty() && n.contains(' ') {
        let first = n.split_whitespace().next().unwrap_or_default().to_string();
        rows = command_rows(conn, &first, false, 300)?;
    }
    let stems = query::words(&n);
    if rows.is_empty() {
        // Точного совпадения нет — обычный поиск, но только по заголовкам и командам.
        let pq = query::parse(&n);
        let Some(expr) = pq.and_expr().or_else(|| pq.or_expr()) else { return Ok(vec![]) };
        let expr = format!("{{title commands}} : ({expr})");
        let docs = fts_list(conn, &expr, p)?;
        return docs
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(i, c)| Ok(to_hit(load_chunk(conn, c.chunk_id)?, 0.5 / (i + 1) as f64, &stems, p)))
            .collect();
    }
    let mut seen = std::collections::HashSet::new();
    let mut hits = Vec::new();
    for (c, primary) in rows {
        if !seen.insert(c.doc.clone()) {
            continue;
        }
        let score = if primary { 1.0 } else { 0.5 / (hits.len() + 1) as f64 };
        hits.push(to_hit(load_chunk(conn, c.chunk_id)?, score, &stems, p));
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kb, test_fixture};

    #[test]
    fn rrf_basic() {
        let lists = vec![(1.0, vec!["a", "b", "c"]), (1.0, vec!["b", "a", "d"]), (2.0, vec!["d"])];
        let m = rrf_merge(&lists, 60.0);
        let keys: Vec<_> = m.iter().map(|(k, _)| *k).collect();
        // d: 2/61 + 1/63; a = b = 1/61 + 1/62 — d первым, a раньше b (появился раньше), c последним
        assert_eq!(keys, ["d", "a", "b", "c"]);
        assert!((m[1].1 - (1.0 / 61.0 + 1.0 / 62.0)).abs() < 1e-12);
    }

    #[test]
    fn rrf_ignores_duplicates_and_empty() {
        let m = rrf_merge(&[(1.0, vec!["x", "x", "y"]), (1.0, vec![])], 1.0);
        assert_eq!(m, vec![("x", 0.5), ("y", 1.0 / 3.0)]);
        assert!(rrf_merge::<&str>(&[], 60.0).is_empty());
    }

    #[test]
    fn snippet_window() {
        let body = format!("{} needle here {}", "word ".repeat(200), "tail ".repeat(200));
        let s = snippet(&body, &["needl".into()], 100);
        assert!(s.contains("needle"), "{s}");
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.chars().count() <= 102);
        assert_eq!(snippet("short  text\n here", &[], 100), "short text here");
        let s = snippet(&"abc ".repeat(100), &["zzz".into()], 50);
        assert!(s.starts_with("abc") && s.ends_with('…'));
    }

    fn kb() -> (tempfile::TempDir, Kb) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("kb.sqlite");
        test_fixture::build(&p);
        let kb = Kb::open(&p).unwrap();
        (dir, kb)
    }

    fn titles(h: &[KbHit]) -> Vec<&str> {
        h.iter().map(|h| h.title.as_str()).collect()
    }

    #[test]
    fn title_weight_beats_body_mentions() {
        let (_d, kb) = kb();
        // «robocopy» встречается в теле xcopy и в заголовке robocopy — заголовок ×5 должен победить
        let h = kb.search(&["robocopy copy files".into()], 5).unwrap();
        assert_eq!(h[0].title, "robocopy");
        assert!(h[0].url.starts_with("https://learn.microsoft.com/"));
        assert_eq!(h[0].source, "windows-commands");
    }

    #[test]
    fn stemmed_prefix_matches() {
        let (_d, kb) = kb();
        let h = kb.search(&["disabling startup apps".into()], 3).unwrap();
        assert_eq!(h[0].title, "Автозагрузка программ (startup apps)");
    }

    #[test]
    fn cmdlet_phrase_and_command_boost() {
        let (_d, kb) = kb();
        let h = kb.search(&["Get-Service".into()], 3).unwrap();
        assert_eq!(h[0].title, "Get-Service");
        let h = kb.search(&["stop a service".into(), "Stop-Service cmdlet".into()], 3).unwrap();
        assert_eq!(h[0].title, "Stop-Service");
    }

    #[test]
    fn one_hit_per_document_and_top_k() {
        let (_d, kb) = kb();
        let h = kb.search(&["robocopy".into(), "robocopy mirror".into()], 10).unwrap();
        let mut urls: Vec<_> = h.iter().map(|h| &h.url).collect();
        let n = urls.len();
        urls.dedup();
        assert_eq!(urls.len(), n);
        assert_eq!(kb.search(&["service".into()], 1).unwrap().len(), 1);
        assert!(kb.search(&[], 5).unwrap().is_empty());
        assert!(kb.search(&["the of".into()], 5).unwrap().is_empty());
    }

    #[test]
    fn best_chunk_is_returned_with_heading() {
        let (_d, kb) = kb();
        let h = kb.search(&["robocopy mirror directory tree /mir".into()], 3).unwrap();
        assert_eq!(h[0].title, "robocopy");
        assert_eq!(h[0].heading, "Examples");
        assert!(h[0].snippet.contains("/mir"), "{}", h[0].snippet);
    }

    #[test]
    fn multi_query_fusion_promotes_consensus() {
        let (_d, kb) = kb();
        // Каждый запрос отдельно слабоват, вместе они сходятся на брандмауэре
        let h = kb
            .search(&["allow app through firewall".into(), "New-NetFirewallRule inbound rule".into()], 3)
            .unwrap();
        assert_eq!(h[0].title, "New-NetFirewallRule");
        assert!(titles(&h).contains(&"Windows Firewall overview"));
    }

    #[test]
    fn russian_query_finds_seed() {
        let (_d, kb) = kb();
        let h = kb.search(&["как отключить автозагрузку".into()], 3).unwrap();
        assert_eq!(h[0].source, "seed");
    }

    #[test]
    fn lookup_exact_and_fallback() {
        let (_d, kb) = kb();
        let h = kb.lookup_command("get-service").unwrap();
        assert_eq!(h[0].title, "Get-Service");
        assert_eq!(h[0].score, 1.0);
        // упоминание в примерах другой страницы — после основной
        let h = kb.lookup_command("Robocopy.exe").unwrap();
        assert_eq!(titles(&h), ["robocopy", "xcopy"]);
        assert!(h[1].score < h[0].score);
        // нет точного — поиск по заголовкам
        let h = kb.lookup_command("firewall").unwrap();
        assert_eq!(h[0].title, "Windows Firewall overview");
        assert!(kb.lookup_command("   ").unwrap().is_empty());
        assert!(kb.lookup_command("nonexistentcmd").unwrap().is_empty());
    }
}
