//! `kb-build` — сборщик базы знаний Windows J.A.R.V.I.S. (см. README.md).

use jarvis_kb::{Kb, SearchParams};
use kb_build::build::{self, BuildOptions, BuildReport};
use kb_build::chunker::ChunkOpts;
use kb_build::{eval, urlcheck};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "kb-build — сборщик базы знаний Windows (data/kb.sqlite)

Использование:
  kb-build [--update] [--only <id>] [--offline]   собрать базу (клоны в data/kb-src/)
  kb-build --eval [--gold FILE] [--k 5] [--param имя=значение]... [--verbose]
                                                  оценить существующую базу: Recall@k и MRR@10
  kb-build --build --eval                         собрать и оценить
  kb-build --check-urls N                         проверить HEAD-запросами по N URL каждого источника
  kb-build --search \"запрос\" [--search ...]       поиск по базе (как kb_search), для отладки
  kb-build --show <часть URL>                     показать фрагменты документа

Флаги:
  --update        обновить имеющиеся клоны (git fetch --depth 1 + reset)
  --only <id>     пересобрать один источник, остальные перенести из прежней базы
  --offline       не обращаться к сети (только имеющиеся клоны)
  --root DIR      корень проекта (по умолчанию ищется вверх от текущего каталога)
  --sources FILE  файл источников (по умолчанию tools/kb-build/kb_sources.json)
  --target N      размер фрагмента в токенах (по умолчанию 700)
  --param k=v     параметры ранжирования для --eval: weights=5,4,3,1 rrf_k w_and w_or w_cmd w_cmd_weak candidates";

#[derive(Debug, Default)]
struct Args {
    build: bool,
    update: bool,
    only: Option<String>,
    offline: bool,
    eval: bool,
    gold: Option<PathBuf>,
    k: usize,
    params: Vec<String>,
    verbose: bool,
    check_urls: Option<usize>,
    root: Option<PathBuf>,
    sources: Option<PathBuf>,
    target: Option<usize>,
    show: Option<String>,
    search: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { k: 5, ..Default::default() };
    let mut it = std::env::args().skip(1);
    let need = |it: &mut dyn Iterator<Item = String>, f: &str| it.next().ok_or_else(|| format!("{f}: нужно значение"));
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--build" => a.build = true,
            "--update" => a.update = true,
            "--offline" => a.offline = true,
            "--only" => a.only = Some(need(&mut it, "--only")?),
            "--eval" => a.eval = true,
            "--verbose" | "-v" => a.verbose = true,
            "--gold" => a.gold = Some(need(&mut it, "--gold")?.into()),
            "--k" => a.k = need(&mut it, "--k")?.parse().map_err(|e| format!("--k: {e}"))?,
            "--param" => a.params.push(need(&mut it, "--param")?),
            "--check-urls" => a.check_urls = Some(need(&mut it, "--check-urls")?.parse().map_err(|e| format!("--check-urls: {e}"))?),
            "--root" => a.root = Some(need(&mut it, "--root")?.into()),
            "--sources" => a.sources = Some(need(&mut it, "--sources")?.into()),
            "--target" => a.target = Some(need(&mut it, "--target")?.parse().map_err(|e| format!("--target: {e}"))?),
            "--show" => a.show = Some(need(&mut it, "--show")?),
            "--search" => a.search.push(need(&mut it, "--search")?),
            other => return Err(format!("неизвестный аргумент {other}")),
        }
    }
    // Без режимов — сборка; --update/--only подразумевают сборку.
    if (!a.eval && a.check_urls.is_none() && a.show.is_none() && a.search.is_empty()) || a.update || a.only.is_some() {
        a.build = true;
    }
    Ok(a)
}

fn mb(b: u64) -> String {
    format!("{:.1} МБ", b as f64 / 1_048_576.0)
}

fn print_build(r: &BuildReport) {
    println!("\n{:<18} {:<8} {:>6} {:>6} {:>7} {:>9} {:>7}  пропущено", "источник", "статус", "файлы", "взято", "фрагм.", "клон", "время");
    for s in &r.sources {
        let skipped = s.skipped.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
        println!(
            "{:<18} {:<8} {:>6} {:>6} {:>7} {:>9} {:>6.1}с  {}",
            s.id,
            s.status,
            s.files_seen,
            s.files_indexed,
            s.chunks,
            mb(s.clone_bytes),
            s.seconds,
            skipped
        );
        if !s.repo_used.is_empty() {
            println!("    {} @ {} {}", s.repo_used, s.commit.get(..10).unwrap_or(&s.commit), s.commit_date);
        }
        for n in &s.notes {
            println!("    · {n}");
        }
        for w in &s.warnings {
            println!("    ! {w}");
        }
        for e in &s.errors {
            println!("    ✗ {e}");
        }
    }
    let clones: u64 = r.sources.iter().map(|s| s.clone_bytes).sum();
    println!(
        "\nИтого: {} фрагментов (перенесено из прежней базы: {}), клоны {}, база {} ({}), {:.1} с",
        r.total_chunks,
        r.carried_chunks,
        mb(clones),
        r.db_path,
        mb(r.db_bytes),
        r.seconds
    );
    if r.pending_swap {
        println!("! kb.sqlite занят (открыт Джарвисом): новая база сохранена как kb.sqlite.new и подхватится при следующем открытии");
    }
}

fn run() -> Result<bool, String> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            println!("{HELP}");
            return Ok(true);
        }
        Err(e) => return Err(format!("{e}\n\n{HELP}")),
    };
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = args.root.clone().unwrap_or_else(|| kb_build::find_project_root(&cwd));
    let cfg = jarvis_core::Config::load(root.join("config.json")).map_err(|e| e.to_string())?;
    let data_dir = root.join(&cfg.data_dir);
    let sources_file = args.sources.clone().unwrap_or_else(|| root.join("tools").join("kb-build").join("kb_sources.json"));
    let db = build::kb_path(&data_dir);
    let mut ok = true;

    if args.build {
        let opts = BuildOptions {
            root: root.clone(),
            sources_file,
            data_dir: data_dir.clone(),
            update: args.update,
            only: args.only.clone(),
            offline: args.offline,
            chunk: ChunkOpts { target_tokens: args.target.unwrap_or(700), ..ChunkOpts::default() },
        };
        println!("Сборка базы знаний: корень {}, данные {}", root.display(), data_dir.display());
        let rep = build::run(&opts)?;
        print_build(&rep);
        if rep.sources.iter().any(|s| s.status == "failed") {
            println!("(недоступные источники — предупреждение, сборка продолжена)");
        }
    }

    if let Some(part) = &args.show {
        let kb = Kb::open(&db).map_err(|e| e.to_string())?;
        for c in kb.chunks_like(part, 40).map_err(|e| e.to_string())? {
            println!("── #{} [{}] {} | {}\n   {}\n   команды: {}\n{}\n", c.id, c.source, c.title, c.heading_path, c.url, c.commands.join(" "), c.body);
        }
    }

    if !args.search.is_empty() {
        let kb = Kb::open(&db).map_err(|e| e.to_string())?;
        let mut params = SearchParams::default();
        for p in &args.params {
            eval::apply_param(&mut params, p)?;
        }
        for (i, h) in kb.search_with(&args.search, args.k.max(6), &params).map_err(|e| e.to_string())?.iter().enumerate() {
            println!("[{}] {:.4} {} — {}\n    {}\n    {}", i + 1, h.score, h.title, h.heading, h.url, h.snippet);
        }
    }

    if let Some(n) = args.check_urls {
        let kb = Kb::open(&db).map_err(|e| e.to_string())?;
        let sample = kb.sample_urls(n).map_err(|e| e.to_string())?;
        println!("\nПроверка URL (HEAD, {} шт.):", sample.iter().filter(|(_, u)| u.starts_with("http")).count());
        let res = urlcheck::check(&sample);
        for c in &res {
            let redirect = if !c.final_url.is_empty() && c.final_url != c.url { format!(" → {}", c.final_url) } else { String::new() };
            let err = if c.error.is_empty() { String::new() } else { format!(" ({})", c.error) };
            println!("  {} {:<18} {}{}{}", c.status, c.source, c.url, redirect, err);
        }
        let good = res.iter().filter(|c| c.ok()).count();
        println!("Итого: {good}/{} отвечают 2xx", res.len());
    }

    if args.eval {
        let gold_path = args.gold.clone().unwrap_or_else(|| root.join("tools").join("kb-build").join("eval").join("gold.jsonl"));
        let gold = eval::load_gold(&gold_path)?;
        let kb = Kb::open(&db).map_err(|e| e.to_string())?;
        let mut params = SearchParams::default();
        for p in &args.params {
            eval::apply_param(&mut params, p)?;
        }
        let t0 = std::time::Instant::now();
        let rep = eval::evaluate(&kb, &gold, args.k, &params);
        let ms = t0.elapsed().as_secs_f64() * 1000.0 / rep.n.max(1) as f64;
        let (en_h, en_n) = rep.by_lang("en");
        let (ru_h, ru_n) = rep.by_lang("ru");
        println!(
            "\nОценка ({}): Recall@{} = {:.1} % ({}/{}) · MRR@10 = {:.3} · en {en_h}/{en_n} · ru {ru_h}/{ru_n} · {:.1} мс/вопрос",
            gold_path.display(),
            rep.k,
            rep.recall_at_k * 100.0,
            rep.hits_at_k,
            rep.n,
            rep.mrr,
            ms
        );
        for r in &rep.results {
            let miss = !r.rank.is_some_and(|x| x <= rep.k);
            if miss || args.verbose {
                let rank = r.rank.map_or("—".to_string(), |x| x.to_string());
                println!("  {} стр.{} ранг {rank}: {:?} → {:?}", if miss { "✗" } else { "✓" }, r.item.line, r.item.queries, r.item.expect);
                if miss {
                    for u in &r.top {
                        println!("        {u}");
                    }
                }
            }
        }
        ok = rep.recall_at_k >= 0.9;
    }
    Ok(ok)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(e) => {
            eprintln!("kb-build: {e}");
            ExitCode::FAILURE
        }
    }
}
