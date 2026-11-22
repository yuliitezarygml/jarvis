//! Сборка: клоны (параллельно) → файлы → front matter/docfx/Markdown → фрагменты + команды → `kb.sqlite`
//! (временный файл → rename) + отчёт по каждому источнику.

use jarvis_kb::{KbWriter, NewChunk, SourceRow};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::chunker::{self, ChunkOpts};
use crate::config::{self, CommandsFrom, SourceCfg, SourceKind};
use crate::markdown::{Block, Section};
use crate::{docfx, extract, frontmatter, git, glob, markdown, url};

#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Корень проекта (`d:\jarvis`).
    pub root: PathBuf,
    pub sources_file: PathBuf,
    /// Абсолютный каталог данных (`<root>/data`).
    pub data_dir: PathBuf,
    pub update: bool,
    pub only: Option<String>,
    pub offline: bool,
    pub chunk: ChunkOpts,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SourceReport {
    pub id: String,
    pub name: String,
    /// `ok`, `warning` (собран, но были проблемы), `carried` (недоступен — перенесён из прежней базы),
    /// `failed` (недоступен и переносить нечего), `kept` (не выбран `--only`, перенесён).
    pub status: String,
    pub repo_used: String,
    pub commit: String,
    pub commit_date: String,
    pub files_seen: u64,
    pub files_indexed: u64,
    pub chunks: u64,
    pub commands: u64,
    pub skipped: BTreeMap<String, u64>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
    pub clone_bytes: u64,
    pub seconds: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BuildReport {
    pub sources: Vec<SourceReport>,
    pub db_path: String,
    pub pending_swap: bool,
    pub db_bytes: u64,
    pub total_chunks: u64,
    pub carried_chunks: u64,
    pub built_at: String,
    pub seconds: f64,
}

#[derive(Debug, Clone)]
pub struct ParsedDoc {
    pub rel: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub ms_date: String,
    pub sections: Vec<Section>,
    pub primary: Vec<String>,
}

pub fn kb_path(data_dir: &Path) -> PathBuf {
    data_dir.join("kb.sqlite")
}

pub fn src_root(data_dir: &Path) -> PathBuf {
    data_dir.join("kb-src")
}

/// Разобрать один файл. `Err(причина)` — файл пропущен.
pub fn parse_doc(src: &SourceCfg, base_dir: &Path, file: &Path, rel: &str) -> Result<ParsedDoc, String> {
    let bytes = std::fs::read(file).map_err(|_| "read-error".to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let (fm, body) = frontmatter::split(&text);
    if fm.is_redirect_or_noindex() {
        return Err("redirect".into());
    }
    let body = docfx::preprocess_file(body, file, base_dir);
    let md = markdown::parse(&body);
    let stem = rel.rsplit('/').next().unwrap_or(rel).trim_end_matches(".md").to_string();
    let title = fm
        .title()
        .map(str::to_string)
        .or_else(|| md.h1.clone())
        .unwrap_or_else(|| stem.clone());
    let text_len: usize = md.sections.iter().flat_map(|s| &s.blocks).map(|b| b.text().len()).sum();
    if text_len < 20 {
        return Err("empty".into());
    }
    let url = url::build_url(&src.url, rel).ok_or_else(|| "no-url".to_string())?;
    let mut primary = extract::primary_commands(src, rel, &title);
    if let Some(h1) = &md.h1
        && primary.is_empty()
        && !h1.eq_ignore_ascii_case(&title)
    {
        primary = extract::primary_commands(src, rel, h1);
    }
    let mut sections = md.sections;
    let description = fm.description().unwrap_or("").to_string();
    // Описание из front matter — в начало документа, если его нет в тексте (часто это лучшая сводка страницы).
    if !description.is_empty() {
        let probe: String = description.chars().take(60).collect();
        let present = sections.iter().flat_map(|s| &s.blocks).take(3).any(|b| b.text().contains(&probe));
        if !present {
            match sections.first_mut() {
                Some(s) if s.level == 1 => s.blocks.insert(0, Block::Text(description.clone())),
                _ => sections.insert(0, Section { level: 1, path: vec![], blocks: vec![Block::Text(description.clone())] }),
            }
        }
    }
    Ok(ParsedDoc { rel: rel.to_string(), url, title, description, ms_date: fm.ms_date().unwrap_or("").to_string(), sections, primary })
}

fn walk_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                if e.file_name() != ".git" {
                    walk_md(&p, out);
                }
            }
            Ok(t) if t.is_file() => {
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")) {
                    out.push(p);
                }
            }
            _ => {}
        }
    }
}

fn rel_of(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Все документы источника из каталога `base` (клон или локальная папка).
pub fn parse_source(src: &SourceCfg, base: &Path, rep: &mut SourceReport) -> Vec<ParsedDoc> {
    let mut files = Vec::new();
    if src.kind == SourceKind::Local || src.sparse.is_empty() {
        walk_md(base, &mut files);
    } else {
        for sp in &src.sparse {
            let d = base.join(sp);
            if !d.exists() {
                rep.warnings.push(format!("подпуть {sp} отсутствует в репозитории"));
            }
            walk_md(&d, &mut files);
        }
    }
    files.sort();
    files.dedup();
    let rels: Vec<String> = files.iter().map(|f| rel_of(base, f)).collect();
    let lower_set: HashSet<String> = rels.iter().map(|r| r.to_lowercase()).collect();
    let mut docs = Vec::new();
    for (f, rel) in files.iter().zip(&rels) {
        rep.files_seen += 1;
        let name = rel.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
        if name == "toc.md" || !glob::allowed(rel, &src.include, &src.exclude) {
            *rep.skipped.entry("excluded".into()).or_default() += 1;
            continue;
        }
        if is_lower_version_duplicate(rel, &src.dedupe_versions, &lower_set) {
            *rep.skipped.entry("duplicate-version".into()).or_default() += 1;
            continue;
        }
        match parse_doc(src, base, f, rel) {
            Ok(d) => docs.push(d),
            Err(reason) => {
                if reason == "read-error" && rep.errors.len() < 20 {
                    rep.errors.push(format!("{rel}: не удалось прочитать"));
                }
                *rep.skipped.entry(reason).or_default() += 1;
            }
        }
    }
    rep.files_indexed = docs.len() as u64;
    docs
}

/// Файл из менее приоритетной версии (`reference/5.1/…`), если тот же путь есть в более приоритетной (`7.6`).
pub fn is_lower_version_duplicate(rel: &str, versions: &[String], all_lower: &HashSet<String>) -> bool {
    let rl = rel.to_lowercase();
    for (i, v) in versions.iter().enumerate().skip(1) {
        if let Some(tail) = rl.strip_prefix(&v.to_lowercase()) {
            return versions[..i].iter().any(|hi| all_lower.contains(&format!("{}{tail}", hi.to_lowercase())));
        }
    }
    false
}

/// Записать документы источника, вернуть (фрагменты, привязки команд).
pub fn write_docs(w: &mut KbWriter, src_id: &str, docs: &[ParsedDoc], known: &HashSet<String>, opts: ChunkOpts) -> jarvis_kb::Result<(u64, u64)> {
    let (mut n_chunks, mut n_cmds) = (0u64, 0u64);
    for d in docs {
        let chunks = chunker::chunk(&d.sections, opts);
        for (i, ch) in chunks.iter().enumerate() {
            let mut cmds: Vec<String> = d.primary.clone();
            for c in extract::chunk_commands(&ch.code, &ch.prose, known) {
                if !cmds.iter().any(|x| x.eq_ignore_ascii_case(&c)) {
                    cmds.push(c);
                }
            }
            let id = w.add_chunk(&NewChunk {
                source: src_id,
                path: &d.rel,
                url: &d.url,
                title: &d.title,
                heading: &ch.heading,
                heading_path: &ch.heading_path,
                commands: &cmds,
                body: &ch.body,
                description: &d.description,
                ms_date: &d.ms_date,
                ord: i as u32,
            })?;
            n_chunks += 1;
            for c in &cmds {
                let primary = i == 0 && d.primary.iter().any(|p| p.eq_ignore_ascii_case(c));
                w.add_command(c, id, primary)?;
                n_cmds += 1;
            }
        }
    }
    Ok((n_chunks, n_cmds))
}

struct Acquired {
    report: SourceReport,
    docs: Option<Vec<ParsedDoc>>,
    row: SourceRow,
}

fn acquire_and_parse(src: &SourceCfg, opts: &BuildOptions) -> Acquired {
    let t0 = Instant::now();
    let mut rep = SourceReport { id: src.id.clone(), name: src.name.clone(), ..Default::default() };
    let mut row = SourceRow { id: src.id.clone(), name: src.name.clone(), branch: src.branch.clone(), note: src.note.clone(), ..Default::default() };
    let base = match src.kind {
        SourceKind::Local => {
            let p = opts.root.join(&src.path);
            if !p.is_dir() {
                rep.status = "failed".into();
                rep.warnings.push(format!("папка {} не найдена", p.display()));
                rep.seconds = t0.elapsed().as_secs_f64();
                return Acquired { report: rep, docs: None, row };
            }
            row.repo = src.path.clone();
            rep.repo_used = src.path.clone();
            p
        }
        SourceKind::Git => {
            let dir = src_root(&opts.data_dir).join(&src.id);
            match git::ensure_clone(&dir, &src.candidate_urls(), &src.branch, &src.sparse, opts.update, opts.offline) {
                Ok(info) => {
                    for n in &info.notes {
                        if n.contains("недоступен") || n.contains("не удалось") || n.contains("sparse-checkout") {
                            rep.warnings.push(n.clone());
                        } else {
                            rep.notes.push(n.clone());
                        }
                    }
                    if !src.repo.is_empty() && info.url != src.repo {
                        rep.warnings.push(format!("основной адрес {} недоступен, использовано зеркало {}", src.repo, info.url));
                    }
                    rep.repo_used = info.url.clone();
                    rep.commit = info.commit.clone();
                    rep.commit_date = info.commit_date.clone();
                    row.repo = info.url;
                    row.commit_id = info.commit;
                    row.commit_date = info.commit_date;
                    rep.clone_bytes = git::dir_size(&dir);
                    dir
                }
                Err(e) => {
                    rep.status = "failed".into();
                    rep.warnings.push(format!("источник недоступен: {e}"));
                    rep.seconds = t0.elapsed().as_secs_f64();
                    return Acquired { report: rep, docs: None, row };
                }
            }
        }
    };
    let docs = parse_source(src, &base, &mut rep);
    if docs.is_empty() {
        rep.warnings.push("не найдено ни одного документа".into());
    }
    rep.seconds = t0.elapsed().as_secs_f64();
    Acquired { report: rep, docs: Some(docs), row }
}

pub fn run(opts: &BuildOptions) -> Result<BuildReport, String> {
    let t0 = Instant::now();
    let cfg = config::load(&opts.sources_file)?;
    let enabled: Vec<&SourceCfg> = cfg.sources.iter().filter(|s| s.enabled).collect();
    let selected: Vec<&SourceCfg> = match &opts.only {
        Some(id) => {
            let s = enabled.iter().find(|s| &s.id == id).ok_or_else(|| {
                format!("нет источника «{id}»; есть: {}", enabled.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", "))
            })?;
            vec![*s]
        }
        None => enabled.clone(),
    };
    std::fs::create_dir_all(src_root(&opts.data_dir)).map_err(|e| e.to_string())?;

    // 1. Клоны и разбор — параллельно по источникам.
    let mut acquired: Vec<Acquired> = std::thread::scope(|scope| {
        let handles: Vec<_> = selected.iter().map(|s| scope.spawn(move || acquire_and_parse(s, opts))).collect();
        handles.into_iter().map(|h| h.join().expect("поток источника упал")).collect()
    });

    // 2. Новая база: перенос невыбранных (--only) и недоступных источников из прежней.
    let target = kb_path(&opts.data_dir);
    let old_db = if jarvis_kb::pending_path(&target).exists() { jarvis_kb::pending_path(&target) } else { target.clone() };
    let mut carry: Vec<String> = Vec::new();
    if opts.only.is_some() {
        carry.extend(enabled.iter().filter(|s| Some(&s.id) != opts.only.as_ref()).map(|s| s.id.clone()));
    }
    carry.extend(acquired.iter().filter(|a| a.docs.is_none()).map(|a| a.report.id.clone()));
    // Копия прежней базы — только если есть что переносить (ATTACH читает её, пока пишется новая).
    let old_copy = opts.data_dir.join(format!("kb.sqlite.old-{}", std::process::id()));
    let have_old = !carry.is_empty() && old_db.exists() && std::fs::copy(&old_db, &old_copy).is_ok();

    let mut w = KbWriter::create(&target).map_err(|e| e.to_string())?;
    let carried = if have_old { w.carry_over(&old_copy, &carry).map_err(|e| e.to_string())? } else { Default::default() };
    let carried_ids: HashSet<String> = if have_old && !carry.is_empty() {
        jarvis_kb_sources(&old_copy).into_iter().filter(|s| carry.contains(s)).collect()
    } else {
        HashSet::new()
    };

    // 3. Известные имена exe: встроенные + страницы команд всех источников (и перенесённых).
    let mut known = extract::builtin_exes();
    for a in &acquired {
        if let (Some(docs), Some(src)) = (&a.docs, cfg.sources.iter().find(|s| s.id == a.report.id)) {
            if matches!(src.commands, CommandsFrom::ExeTitle | CommandsFrom::Filename) {
                for d in docs {
                    known.extend(d.primary.iter().filter(|p| !p.contains(' ') && !p.contains('-')).map(|p| p.to_lowercase()));
                }
            }
        }
    }
    known.extend(w.primary_command_names().map_err(|e| e.to_string())?.into_iter().filter(|p| !p.contains(' ') && !p.contains('-')));

    // 4. Запись.
    let built_at = jarvis_kb::now_rfc3339();
    for a in &mut acquired {
        let Some(docs) = &a.docs else {
            a.report.status = if carried_ids.contains(&a.report.id) { "carried".into() } else { "failed".into() };
            continue;
        };
        let (n, c) = write_docs(&mut w, &a.report.id, docs, &known, opts.chunk).map_err(|e| e.to_string())?;
        a.report.chunks = n;
        a.report.commands = c;
        a.report.status = if a.report.warnings.is_empty() && a.report.errors.is_empty() { "ok".into() } else { "warning".into() };
        a.row.files = a.report.files_indexed;
        a.row.chunks = n;
        a.row.skipped = a.report.skipped.values().sum();
        a.row.errors = a.report.errors.len() as u64;
        a.row.built_at = built_at.clone();
        if !a.report.warnings.is_empty() {
            let w_note = a.report.warnings.join("; ");
            a.row.note = if a.row.note.is_empty() { w_note } else { format!("{} | {w_note}", a.row.note) };
        }
        w.set_source(&a.row).map_err(|e| e.to_string())?;
    }
    let mut reports: Vec<SourceReport> = acquired.into_iter().map(|a| a.report).collect();
    if opts.only.is_some() {
        for id in carried_ids.iter().filter(|id| !reports.iter().any(|r| &r.id == *id)) {
            reports.push(SourceReport { id: id.clone(), status: "kept".into(), ..Default::default() });
        }
    }
    let total = w.chunk_count().map_err(|e| e.to_string())?;
    let out = w.finish().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&old_copy);
    if !out.pending && jarvis_kb::pending_path(&target).exists() {
        let _ = std::fs::remove_file(jarvis_kb::pending_path(&target));
    }
    let report = BuildReport {
        sources: reports,
        db_path: out.path.display().to_string(),
        pending_swap: out.pending,
        db_bytes: out.size_bytes,
        total_chunks: total,
        carried_chunks: carried.chunks,
        built_at,
        seconds: t0.elapsed().as_secs_f64(),
    };
    let _ = std::fs::write(opts.data_dir.join("kb-build-report.json"), serde_json::to_string_pretty(&report).unwrap_or_default());
    Ok(report)
}

/// id источников в существующей базе (для отчёта о переносе).
fn jarvis_kb_sources(db: &Path) -> Vec<String> {
    jarvis_kb::Kb::open(db).and_then(|kb| kb.status()).map(|s| s.sources.into_iter().map(|s| s.id).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn version_dedupe() {
        let v = vec!["reference/7.6/".to_string(), "reference/5.1/".to_string()];
        let all: HashSet<String> = ["reference/7.6/m/get-service.md", "reference/5.1/m/get-service.md", "reference/5.1/m/get-wmiobject.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(is_lower_version_duplicate("reference/5.1/M/Get-Service.md", &v, &all));
        assert!(!is_lower_version_duplicate("reference/5.1/M/Get-WmiObject.md", &v, &all));
        assert!(!is_lower_version_duplicate("reference/7.6/M/Get-Service.md", &v, &all));
    }

    /// Полная сборка из локальной папки (без git и сети) + перенос при `--only`.
    #[test]
    fn local_build_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("kb/seed/services.md"), "---\ntitle: Службы Windows (services)\ndescription: Как управлять службами.\n---\n# Службы\n\nОстановить службу: `Stop-Service -Name Spooler` или `sc.exe stop spooler`.\n\n## Автозапуск\n\n```powershell\nSet-Service -Name Spooler -StartupType Disabled\n```\n");
        write(&root.join("kb/seed/redirect.md"), "---\ntitle: old\nredirect_url: /x\n---\ntext text text text text");
        write(&root.join("kb/seed/empty.md"), "---\ntitle: e\n---\n");
        write(&root.join("kb/other/net.md"), "---\ntitle: Сеть\n---\nСброс сети: `netsh winsock reset` и `ipconfig /flushdns`.");
        let sources = root.join("kb_sources.json");
        std::fs::write(&sources, r#"{"sources":[
            {"id":"seed","name":"Seed","kind":"local","path":"kb/seed","url":{"rules":[{"from":"","to":"kb/seed/"}],"keep_ext":true}},
            {"id":"other","name":"Other","kind":"local","path":"kb/other","url":{"rules":[{"from":"","to":"kb/other/"}],"keep_ext":true}},
            {"id":"gone","name":"Gone","kind":"local","path":"kb/none","url":{}}
        ]}"#).unwrap();
        let opts = BuildOptions {
            root: root.to_path_buf(),
            sources_file: sources.clone(),
            data_dir: root.join("data"),
            update: false,
            only: None,
            offline: true,
            chunk: ChunkOpts::default(),
        };
        let rep = run(&opts).unwrap();
        let seed = rep.sources.iter().find(|s| s.id == "seed").unwrap();
        assert_eq!(seed.status, "ok");
        assert_eq!(seed.files_seen, 3);
        assert_eq!(seed.files_indexed, 1);
        assert_eq!(seed.skipped.get("redirect"), Some(&1));
        assert_eq!(seed.skipped.get("empty"), Some(&1));
        assert_eq!(rep.sources.iter().find(|s| s.id == "gone").unwrap().status, "failed");
        assert!(!rep.pending_swap && rep.db_bytes > 0);
        assert!(root.join("data/kb-build-report.json").exists());

        let kb = jarvis_kb::Kb::open(root.join("data/kb.sqlite")).unwrap();
        let h = kb.lookup_command("Set-Service").unwrap();
        assert_eq!(h[0].url, "kb/seed/services.md");
        assert!(h[0].snippet.contains("Set-Service -Name Spooler"), "{}", h[0].snippet);
        assert_eq!(kb.lookup_command("netsh").unwrap()[0].url, "kb/other/net.md");
        assert_eq!(kb.search(&["остановить службу".into()], 3).unwrap()[0].title, "Службы Windows (services)");
        drop(kb);

        // --only other: seed переносится из прежней базы, other пересобирается
        std::fs::write(root.join("kb/other/net.md"), "---\ntitle: Сеть v2\n---\nСброс сети: `netsh int ip reset` и перезагрузка.").unwrap();
        let rep2 = run(&BuildOptions { only: Some("other".into()), ..opts.clone() }).unwrap();
        assert_eq!(rep2.sources.iter().find(|s| s.id == "seed").unwrap().status, "kept");
        assert!(rep2.carried_chunks >= 1);
        let kb = jarvis_kb::Kb::open(root.join("data/kb.sqlite")).unwrap();
        assert_eq!(kb.lookup_command("Stop-Service").unwrap()[0].url, "kb/seed/services.md");
        assert_eq!(kb.search(&["сброс сети".into()], 1).unwrap()[0].title, "Сеть v2");
        assert!(run(&BuildOptions { only: Some("nope".into()), ..opts }).unwrap_err().contains("нет источника"));
    }
}
