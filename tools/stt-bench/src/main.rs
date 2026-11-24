//! `stt-bench` — замер распознавания речи J.A.R.V.I.S. по папке WAV + `refs.tsv`.
//! Логика замера — `jarvis_voice::stt::bench` (её же позовёт мастер «Проверить на моём голосе»).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use jarvis_core::CancellationToken;
use jarvis_core::config::TtsConfig;
use jarvis_voice::stt::bench::{self, BenchOptions, CHECK_PHRASES, EngineScore};
use jarvis_voice::{SpeechNormalizer, TtsClient};

const HELP: &str = "\
stt-bench — замер распознавания речи J.A.R.V.I.S.

  stt-bench <папка> [--engines local,groq,groq-turbo,hybrid] [--config <config.json>]
            [--pause-ms N] [--no-warmup] [--json <файл>] [-v]
      Прогнать движки по фразам из <папка>/refs.tsv (строки «имя<TAB>текст») и <папка>/<имя>.wav
      (любая частота, моно/стерео) → таблица: WER %, CER %, ошибки в названиях, медиана/максимум мс,
      и разбор фраз с ошибками.
      Движки: local (GigaAM на этом ПК), groq (Groq Whisper, модель config.stt.cloud_model),
      groq-turbo (whisper-large-v3-turbo), hybrid (гонка облако vs локальный), server (Xeon).
      --config   по умолчанию <корень проекта>/config.json (ключи Groq — провайдеры groq, groq-2…)
      --pause-ms пауза между фразами (у Groq ~20 запросов/мин на модель), по умолчанию 0
      --no-warmup не прогонять первую фразу «вхолостую» (загрузка модели, TLS)
      -v         журнал (какой движок гибрида дал текст и за сколько мс)

  stt-bench synth <папка> [--tts-url <адрес>] [--voice jarvis-ru] [--config <config.json>]
      Синтезировать 20 проверочных фраз голосом Джарвиса (POST <адрес>/tts/stream) в <папка>/01.wav…20.wav
      и записать <папка>/refs.tsv. Адрес по умолчанию — config.tts.url.

Нормализация для WER: нижний регистр, ё→е, без знаков препинания, числа как есть; разговорные написания
названий из словаря («вс код», «гитхаб») считаются тем же словом, что «VS Code», «GitHub».

Локальный движок (local, hybrid) — sherpa-onnx: рядом с stt-bench.exe должны лежать onnxruntime.dll,
onnxruntime_providers_shared.dll, sherpa-onnx-c-api.dll, sherpa-onnx-cxx-api.dll (cargo кладёт их в
target\\debug и target\\release — там же, где stt-bench.exe; если exe скопирован в другое место — скопируйте
DLL рядом). Без sherpa-onnx-c-api.dll Windows не запустит exe вовсе; без onnxruntime.dll подхватится
системная (старая) из System32 и модель упадёт — stt-bench проверяет это заранее.
Модель GigaAM — <корень проекта>/<data_dir>/models/stt/gigaam-v3-ctc.
";

const DLLS: &[&str] = &["onnxruntime.dll", "onnxruntime_providers_shared.dll", "sherpa-onnx-c-api.dll", "sherpa-onnx-cxx-api.dll"];

struct Args {
    synth: bool,
    dir: PathBuf,
    engines: Vec<String>,
    config: Option<PathBuf>,
    pause_ms: u64,
    warmup: bool,
    json: Option<PathBuf>,
    verbose: bool,
    tts_url: Option<String>,
    voice: Option<String>,
}

fn parse_args(raw: Vec<String>) -> Result<Args, String> {
    let mut it = raw.into_iter();
    let mut a = Args {
        synth: false,
        dir: PathBuf::new(),
        engines: bench::BENCH_ENGINES.iter().map(|s| s.to_string()).collect(),
        config: None,
        pause_ms: 0,
        warmup: true,
        json: None,
        verbose: false,
        tts_url: None,
        voice: None,
    };
    let mut dir = None;
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name}: нужно значение"));
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--engines" => a.engines = val("--engines")?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            "--config" => a.config = Some(val("--config")?.into()),
            "--pause-ms" => a.pause_ms = val("--pause-ms")?.parse().map_err(|_| "--pause-ms: нужно число".to_string())?,
            "--no-warmup" => a.warmup = false,
            "--json" => a.json = Some(val("--json")?.into()),
            "-v" | "--verbose" => a.verbose = true,
            "--tts-url" => a.tts_url = Some(val("--tts-url")?),
            "--voice" => a.voice = Some(val("--voice")?),
            "synth" if dir.is_none() && !a.synth => a.synth = true,
            s if s.starts_with('-') => return Err(format!("неизвестный ключ {s}")),
            s if dir.is_none() => dir = Some(PathBuf::from(s)),
            s => return Err(format!("лишний аргумент {s}")),
        }
    }
    a.dir = dir.ok_or("не указана папка")?;
    Ok(a)
}

/// Нужные DLL рядом с exe (только для локального движка).
fn check_dlls() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().unwrap_or(Path::new("."));
    let missing: Vec<&str> = DLLS.iter().copied().filter(|d| !dir.join(d).is_file()).collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "рядом с {} нет {} — локальный движок не загрузится. Скопируйте DLL sherpa-onnx из target\\debug или \
         target\\release (после сборки приложения) в {}",
        exe.display(),
        missing.join(", "),
        dir.display()
    ))
}

fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w { s.to_string() } else { format!("{s}{}", " ".repeat(w - n)) }
}

fn print_table(scores: &[EngineScore], phrases: usize) {
    let head = ["Движок", "WER %", "CER %", "Названия: ошибок/всего", "Медиана мс", "Макс мс", "Сбоев"];
    let rows: Vec<[String; 7]> = scores
        .iter()
        .map(|s| match &s.error {
            Some(e) => [s.engine.clone(), "—".into(), "—".into(), format!("не запустился: {e}"), String::new(), String::new(), String::new()],
            None => [
                s.engine.clone(),
                format!("{:.1}", s.wer),
                format!("{:.1}", s.cer),
                format!("{}/{}", s.name_errors, s.name_total),
                s.median_ms.to_string(),
                s.max_ms.to_string(),
                format!("{}/{phrases}", s.failed),
            ],
        })
        .collect();
    let mut w = head.map(|h| h.chars().count());
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            if i != 3 || !c.starts_with("не запустился") {
                w[i] = w[i].max(c.chars().count());
            }
        }
    }
    let line = |cells: Vec<String>| println!("| {} |", cells.join(" | "));
    line(head.iter().enumerate().map(|(i, h)| pad(h, w[i])).collect());
    line(w.iter().map(|n| "-".repeat(*n)).collect());
    for r in &rows {
        line(r.iter().enumerate().map(|(i, c)| pad(c, w[i])).collect());
    }
}

fn print_diffs(scores: &[EngineScore]) {
    for s in scores.iter().filter(|s| s.error.is_none()) {
        let bad: Vec<_> = s.phrases.iter().filter(|p| p.word_errors > 0 || p.error.is_some()).collect();
        if bad.is_empty() {
            continue;
        }
        println!("\n[{}] фразы с ошибками ({} из {}):", s.engine, bad.len(), s.phrases.len());
        for p in bad {
            println!("  {}  эталон:     {}", p.name, p.reference);
            match &p.error {
                Some(e) => println!("      сбой:       {e}"),
                None if p.raw_hypothesis != p.hypothesis => {
                    println!("      движок:     {}", p.raw_hypothesis);
                    println!("      после слов: {}", p.hypothesis);
                }
                None => println!("      распознано: {}", p.hypothesis),
            }
            let names = if p.wrong_names.is_empty() { String::new() } else { format!("; названия: {}", p.wrong_names.join(", ")) };
            println!("      ошибок слов: {}/{}, {} мс{names}", p.word_errors, p.words, p.latency_ms);
        }
    }
}

fn load_config(path: Option<&Path>) -> Result<(jarvis_core::Config, PathBuf), String> {
    let path = path.map(Path::to_path_buf).unwrap_or_else(|| jarvis_voice::tls::default_project_root().join("config.json"));
    let cfg = jarvis_core::Config::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let root = path.parent().map(Path::to_path_buf).filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| PathBuf::from("."));
    Ok((cfg, root))
}

async fn synth(a: &Args) -> Result<(), String> {
    let (cfg, root) = load_config(a.config.as_deref())?;
    let tts_cfg = TtsConfig {
        url: a.tts_url.clone().unwrap_or(cfg.tts.url.clone()),
        voice: a.voice.clone().unwrap_or(cfg.tts.voice.clone()),
        ..cfg.tts.clone()
    };
    let client = TtsClient::new(&tts_cfg, &root).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&a.dir).map_err(|e| format!("{}: {e}", a.dir.display()))?;
    let norm = SpeechNormalizer::new();
    for (i, phrase) in CHECK_PHRASES.iter().enumerate() {
        let spoken = norm.normalize(phrase);
        let stream = client.synthesize_stream(&spoken, None, CancellationToken::new()).await.map_err(|e| format!("«{phrase}»: {e}"))?;
        let rate = stream.sample_rate;
        let bytes = stream.collect_all().await.map_err(|e| format!("«{phrase}»: {e}"))?;
        let pcm: Vec<i16> = bytes.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect();
        let name = format!("{:02}", i + 1);
        std::fs::write(a.dir.join(format!("{name}.wav")), jarvis_voice::pcm::wav_encode(&pcm, rate, 1)).map_err(|e| e.to_string())?;
        println!("{name}  {:>5} мс  {phrase}   (произнесено: {spoken})", pcm.len() as u64 * 1000 / rate.max(1) as u64);
    }
    std::fs::write(a.dir.join("refs.tsv"), bench::check_refs_tsv()).map_err(|e| e.to_string())?;
    println!("готово: {} фраз в {}", CHECK_PHRASES.len(), a.dir.display());
    Ok(())
}

async fn run(a: &Args) -> Result<(), String> {
    let (cfg, root) = load_config(a.config.as_deref())?;
    if a.engines.iter().any(|e| e == "local" || e == "hybrid") {
        check_dlls()?;
    }
    let items = bench::load_dir(&a.dir).map_err(|e| e.to_string())?;
    let opt = BenchOptions { pause: Duration::from_millis(a.pause_ms), warmup: a.warmup };
    let names: Vec<&str> = a.engines.iter().map(String::as_str).collect();
    eprintln!("фраз: {}, движки: {}", items.len(), names.join(", "));
    let scores = bench::bench_from_config(&cfg, &root, &names, &items, &[], &opt).await;
    println!();
    print_table(&scores, items.len());
    print_diffs(&scores);
    if let Some(p) = &a.json {
        let j = serde_json::to_string_pretty(&scores).map_err(|e| e.to_string())?;
        std::fs::write(p, j).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let a = match parse_args(std::env::args().skip(1).collect()) {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("ошибка: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    if a.verbose {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
        tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
    }
    let r = if a.synth { synth(&a).await } else { run(&a).await };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<Args, String> {
        parse_args(s.split_whitespace().map(String::from).collect())
    }

    #[test]
    fn parses_arguments() {
        let a = args("data/stt-check --engines local,groq --pause-ms 3000 -v").unwrap();
        assert_eq!((a.dir, a.engines, a.pause_ms, a.verbose, a.synth), (PathBuf::from("data/stt-check"), vec!["local".into(), "groq".into()], 3000, true, false));
        let a = args("synth out --voice jarvis-ru").unwrap();
        assert!(a.synth && a.dir == Path::new("out") && a.voice.as_deref() == Some("jarvis-ru"));
        assert_eq!(args("x").unwrap().engines, bench::BENCH_ENGINES);
        assert_eq!(args("--help").err().as_deref(), Some(""));
        assert!(args("").is_err() && args("a b").is_err() && args("a --bogus").is_err());
    }

    #[test]
    fn padding_counts_chars() {
        assert_eq!(pad("ёж", 4), "ёж  ");
        assert_eq!(pad("длинно", 2), "длинно");
    }
}
