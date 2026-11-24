//! Замер точности распознавания: прогон движков по набору фраз (WAV + эталонный текст) → WER/CER,
//! ошибки в названиях, время ответа. Используется консольным `tools/stt-bench` и (позже) мастером
//! «Проверить на моём голосе» в настройках.
//!
//! Нормализация для подсчёта: нижний регистр, «ё» → «е», знаки препинания → пробел, числа как есть.
//! До этого ответ движка проходит `terms::apply` — ту же обработку, что в конвейере («вс код» → «VS Code»,
//! «конфиг джейсон» → «config.json»): оценивается то, что реально получит Джарвис.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jarvis_core::CancellationToken;
use serde::Serialize;

use super::hints::stt_hints;
use super::terms;
use super::{GroqWhisperStt, SttEngine};
use crate::error::{Result, VoiceError};

/// 20 фраз для проверки на голосе хозяина: команды Джарвиса, названия программ, числа, английские
/// слова, буквы дисков, адреса. Так они и должны распознаться (эталон).
pub const CHECK_PHRASES: &[&str] = &[
    "Джарвис, открой браузер",
    "Джарвис, запусти VS Code",
    "Джарвис, открой GitHub и покажи мои репозитории",
    "Джарвис, включи Discord",
    "Джарвис, запусти Steam",
    "Джарвис, напиши в Telegram, что я буду через 10 минут",
    "Джарвис, найди на YouTube обзор видеокарты RTX 4070",
    "Джарвис, открой Блокнот",
    "Джарвис, запусти PowerShell от имени администратора",
    "Джарвис, выключи Wi-Fi",
    "Джарвис, включи Bluetooth",
    "Джарвис, поставь громкость на 35 процентов",
    "Джарвис, поставь таймер на 15 минут",
    "Джарвис, открой диск D и папку Downloads",
    "Джарвис, сколько свободного места на диске C",
    "Джарвис, открой сайт github.com",
    "Джарвис, переведи на английский good morning",
    "Джарвис, проложи маршрут до улицы Ленина, дом 12",
    "Джарвис, напомни мне в 18:30 позвонить маме",
    "Джарвис, проверь обновления Windows и открой диспетчер задач",
];

/// Имена движков для замера: `local` (GigaAM), `groq` (`config.stt.cloud_model`, по умолчанию large-v3),
/// `groq-turbo` (large-v3-turbo), `hybrid` (гонка), `server` (Xeon).
pub const BENCH_ENGINES: &[&str] = &["local", "groq", "groq-turbo", "hybrid"];

/// Одна фраза набора.
#[derive(Debug, Clone)]
pub struct BenchItem {
    pub name: String,
    /// PCM s16 16 кГц mono.
    pub pcm16k: Vec<i16>,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhraseResult {
    pub name: String,
    pub reference: String,
    /// Текст, который получит конвейер (ответ движка после `terms::apply`).
    pub hypothesis: String,
    /// Сырой ответ движка.
    pub raw_hypothesis: String,
    pub word_errors: usize,
    pub words: usize,
    pub char_errors: usize,
    pub chars: usize,
    pub latency_ms: u64,
    /// Ошибка движка на этой фразе (тогда `hypothesis` пустой и все слова — ошибки).
    pub error: Option<String>,
    /// Названия из словаря, которые есть в эталоне, но не распознаны.
    pub wrong_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngineScore {
    pub engine: String,
    /// Движок не создался (нет модели, нет ключа…) — остальные поля пустые.
    pub error: Option<String>,
    /// % ошибок в словах.
    pub wer: f32,
    /// % ошибок в символах.
    pub cer: f32,
    pub name_errors: usize,
    pub name_total: usize,
    pub median_ms: u64,
    pub max_ms: u64,
    /// Фраз, на которых движок вернул ошибку.
    pub failed: usize,
    pub phrases: Vec<PhraseResult>,
}

impl EngineScore {
    fn failed_to_start(engine: &str, e: &VoiceError) -> Self {
        Self {
            engine: engine.into(),
            error: Some(e.to_string()),
            wer: 0.0,
            cer: 0.0,
            name_errors: 0,
            name_total: 0,
            median_ms: 0,
            max_ms: 0,
            failed: 0,
            phrases: Vec::new(),
        }
    }
}

/// Параметры прогона.
#[derive(Debug, Clone)]
pub struct BenchOptions {
    /// Пауза между фразами (у Groq лимит ~20 запросов в минуту на модель).
    pub pause: Duration,
    /// Перед замером прогнать первую фразу без учёта (загрузка модели, TLS-соединение).
    pub warmup: bool,
}

impl Default for BenchOptions {
    fn default() -> Self {
        Self { pause: Duration::ZERO, warmup: true }
    }
}

/// Нижний регистр, «ё» → «е», всё кроме букв и цифр → пробел; слова через один пробел.
pub fn normalize(text: &str) -> String {
    let s: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'ё' => 'е',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Слова для подсчёта: текст после `terms::apply` (как его получает конвейер: «вс код» → «VS Code»),
/// затем нормализация (регистр не важен).
pub fn score_words(text: &str) -> Vec<String> {
    normalize(&terms::apply(text)).split(' ').filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Расстояние Левенштейна (вставка/удаление/замена).
pub fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(x != y)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// (ошибок, слов в эталоне).
pub fn word_errors(reference: &str, hypothesis: &str) -> (usize, usize) {
    let (r, h) = (score_words(reference), score_words(hypothesis));
    (edit_distance(&r, &h), r.len())
}

/// (ошибок, символов в эталоне) — по нормализованному тексту с пробелами.
pub fn char_errors(reference: &str, hypothesis: &str) -> (usize, usize) {
    let r: Vec<char> = score_words(reference).join(" ").chars().collect();
    let h: Vec<char> = score_words(hypothesis).join(" ").chars().collect();
    (edit_distance(&r, &h), r.len())
}

pub fn wer(reference: &str, hypothesis: &str) -> f32 {
    let (e, n) = word_errors(reference, hypothesis);
    100.0 * e as f32 / n.max(1) as f32
}

pub fn cer(reference: &str, hypothesis: &str) -> f32 {
    let (e, n) = char_errors(reference, hypothesis);
    100.0 * e as f32 / n.max(1) as f32
}

fn contains_seq(hay: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Названия из `names`, которые есть в эталоне, но не распознаны: (не распознаны, всего в эталоне).
pub fn name_errors(reference: &str, hypothesis: &str, names: &[String]) -> (Vec<String>, usize) {
    let (r, h) = (score_words(reference), score_words(hypothesis));
    let mut wrong = Vec::new();
    let mut total = 0;
    for name in names {
        let n = score_words(name);
        if contains_seq(&r, &n) {
            total += 1;
            if !contains_seq(&h, &n) {
                wrong.push(name.clone());
            }
        }
    }
    (wrong, total)
}

/// Оценка одной фразы.
pub fn score_phrase(item_name: &str, reference: &str, hypothesis: &str, latency_ms: u64, names: &[String]) -> PhraseResult {
    let (we, words) = word_errors(reference, hypothesis);
    let (ce, chars) = char_errors(reference, hypothesis);
    let (wrong_names, _) = name_errors(reference, hypothesis, names);
    PhraseResult {
        name: item_name.into(),
        reference: reference.into(),
        hypothesis: hypothesis.into(),
        raw_hypothesis: hypothesis.into(),
        word_errors: we,
        words,
        char_errors: ce,
        chars,
        latency_ms,
        error: None,
        wrong_names,
    }
}

fn median(mut v: Vec<u64>) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2 }
}

/// Прогнать один движок по набору.
pub async fn bench_engine(name: &str, engine: &dyn SttEngine, items: &[BenchItem], names: &[String], opt: &BenchOptions) -> EngineScore {
    let cancel = CancellationToken::new();
    if opt.warmup && let Some(first) = items.first() {
        let _ = engine.transcribe(&first.pcm16k, &cancel).await;
        tokio::time::sleep(opt.pause).await;
    }
    let mut phrases = Vec::with_capacity(items.len());
    let mut name_total = 0;
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(opt.pause).await;
        }
        let t0 = Instant::now();
        let r = engine.transcribe(&it.pcm16k, &cancel).await;
        let ms = t0.elapsed().as_millis() as u64;
        name_total += name_errors(&it.reference, &it.reference, names).1;
        phrases.push(match r {
            // Оценивается то, что получит конвейер (после `terms::apply`); сырой ответ движка — рядом.
            Ok(r) => PhraseResult {
                raw_hypothesis: r.text.clone(),
                ..score_phrase(&it.name, &it.reference, &terms::apply(&r.text), ms, names)
            },
            Err(e) => PhraseResult { error: Some(e.to_string()), ..score_phrase(&it.name, &it.reference, "", ms, names) },
        });
    }
    let sum = |f: fn(&PhraseResult) -> usize| phrases.iter().map(f).sum::<usize>();
    let (we, w, ce, c) = (sum(|p| p.word_errors), sum(|p| p.words), sum(|p| p.char_errors), sum(|p| p.chars));
    let ok_lat: Vec<u64> = phrases.iter().filter(|p| p.error.is_none()).map(|p| p.latency_ms).collect();
    EngineScore {
        engine: name.into(),
        error: None,
        wer: 100.0 * we as f32 / w.max(1) as f32,
        cer: 100.0 * ce as f32 / c.max(1) as f32,
        name_errors: sum(|p| p.wrong_names.len()),
        name_total,
        median_ms: median(ok_lat.clone()),
        max_ms: ok_lat.iter().copied().max().unwrap_or(0),
        failed: phrases.iter().filter(|p| p.error.is_some()).count(),
        phrases,
    }
}

/// Прогнать готовые движки (`Err` — движок не создался, попадёт в таблицу с причиной).
pub async fn bench_engines(
    engines: Vec<(String, Result<Arc<dyn SttEngine>>)>,
    items: &[BenchItem],
    names: &[String],
    opt: &BenchOptions,
) -> Vec<EngineScore> {
    let mut out = Vec::with_capacity(engines.len());
    for (name, e) in engines {
        out.push(match e {
            Ok(e) => bench_engine(&name, e.as_ref(), items, names, opt).await,
            Err(err) => EngineScore::failed_to_start(&name, &err),
        });
    }
    out
}

/// Движок для замера по имени (без запасных вариантов: нет модели/ключа — ошибка).
pub fn bench_engine_named(
    cfg: &jarvis_core::Config,
    project_root: &Path,
    name: &str,
    extra_hints: &[String],
) -> Result<Arc<dyn SttEngine>> {
    Ok(match name {
        "local" => super::local_engine(cfg, project_root)?,
        "groq" | "groq_whisper" => Arc::new(GroqWhisperStt::from_config_with_hints(cfg, extra_hints)?),
        "groq-turbo" => Arc::new(GroqWhisperStt::from_config_with_hints(cfg, extra_hints)?.with_model(GroqWhisperStt::TURBO_MODEL)),
        "groq-v3" => Arc::new(GroqWhisperStt::from_config_with_hints(cfg, extra_hints)?.with_model(GroqWhisperStt::DEFAULT_MODEL)),
        "hybrid" => {
            let h = super::hybrid_engine(cfg, project_root, extra_hints)?;
            if !h.has_local() || !h.has_cloud() {
                return Err(VoiceError::Config("гибрид неполный: нужны и локальная модель, и ключ Groq".into()));
            }
            Arc::new(h)
        }
        "server" => Arc::new(super::ServerStt::new(&cfg.stt, project_root)?),
        other => return Err(VoiceError::Config(format!("неизвестный движок «{other}» (есть: local, groq, groq-turbo, hybrid, server)"))),
    })
}

/// Названия, по которым считаются «ошибки в названиях»: словарь + `config.stt.hints` + переданные.
pub fn bench_names(cfg: &jarvis_core::Config, extra_hints: &[String]) -> Vec<String> {
    stt_hints(&cfg.stt.hints, extra_hints)
}

/// Всё вместе из конфига: создать движки `names` (см. `BENCH_ENGINES`) и прогнать набор.
/// Для будущей команды Tauri «Проверить на моём голосе».
pub async fn bench_from_config(
    cfg: &jarvis_core::Config,
    project_root: &Path,
    engine_names: &[&str],
    items: &[BenchItem],
    extra_hints: &[String],
    opt: &BenchOptions,
) -> Vec<EngineScore> {
    let engines = engine_names
        .iter()
        .map(|n| (n.to_string(), bench_engine_named(cfg, project_root, n, extra_hints)))
        .collect();
    bench_engines(engines, items, &bench_names(cfg, extra_hints), opt).await
}

/// Набор из папки: `refs.tsv` (`имя<TAB>текст`, `#` — комментарий) + `<имя>.wav` (любая частота, моно/стерео).
pub fn load_dir(dir: &Path) -> Result<Vec<BenchItem>> {
    let refs = dir.join("refs.tsv");
    let text = std::fs::read_to_string(&refs).map_err(|e| VoiceError::InvalidInput(format!("нет {}: {e}", refs.display())))?;
    let mut items = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, reference)) = line.split_once('\t') else {
            return Err(VoiceError::InvalidInput(format!("{}:{}: нужна строка «имя<TAB>текст»", refs.display(), n + 1)));
        };
        let name = name.trim();
        let wav_path = dir.join(format!("{name}.wav"));
        let bytes = std::fs::read(&wav_path).map_err(|e| VoiceError::InvalidInput(format!("нет {}: {e}", wav_path.display())))?;
        let pcm16k = crate::pcm::wav_decode(&bytes)?.to_stt_pcm()?;
        items.push(BenchItem { name: name.into(), pcm16k, reference: reference.trim().into() });
    }
    if items.is_empty() {
        return Err(VoiceError::InvalidInput(format!("{}: нет ни одной фразы", refs.display())));
    }
    Ok(items)
}

/// Содержимое `refs.tsv` для набора `CHECK_PHRASES` (имена `01`…`20`).
pub fn check_refs_tsv() -> String {
    CHECK_PHRASES.iter().enumerate().map(|(i, p)| format!("{:02}\t{p}\n", i + 1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use jarvis_core::stt_protocol::SttResult;

    #[test]
    fn normalization() {
        assert_eq!(normalize("Джарвис, ЁЛКА!  Открой  «Блокнот»…"), "джарвис елка открой блокнот");
        assert_eq!(normalize("в 18:30, RTX-4070"), "в 18 30 rtx 4070");
        assert_eq!(normalize(" ... "), "");
    }

    #[test]
    fn terms_applied_before_scoring() {
        assert_eq!(score_words("Джарвис, запусти вс код"), ["джарвис", "запусти", "vs", "code"]);
        assert_eq!(score_words("Джарвис, выключи вай фай"), ["джарвис", "выключи", "wi", "fi"]);
        assert_eq!(score_words("открой диск д"), ["открой", "диск", "d"]);
    }

    #[test]
    fn wer_cer() {
        assert_eq!(edit_distance(&[1, 2, 3], &[1, 3]), 1);
        assert_eq!(edit_distance::<u8>(&[], &[1, 2]), 2);
        assert_eq!(edit_distance(&['к', 'о', 'т'], &['к', 'и', 'т', 'ы']), 2);
        assert_eq!(wer("Джарвис, открой браузер", "джарвис открой браузер."), 0.0);
        assert_eq!(word_errors("Джарвис, запусти VS Code", "Джарвис запусти вэс код"), (0, 4));
        assert_eq!(word_errors("открой config.json", "открой конфиг джейсон"), (0, 3));
        // Числа как есть: «пятнадцать» ≠ «15».
        assert_eq!(word_errors("таймер на 15 минут", "таймер на пятнадцать минут"), (1, 4));
        assert_eq!(word_errors("открой браузер", ""), (2, 2));
        assert!((wer("а б в г", "а б в") - 25.0).abs() < 1e-4);
        assert_eq!(char_errors("кот", "кит"), (1, 3));
        assert!((cer("кот", "кот") - 0.0).abs() < 1e-6);
    }

    #[test]
    fn name_error_counting() {
        let names = stt_hints(&[], &[]);
        let (wrong, total) = name_errors("Джарвис, открой GitHub и Discord", "джарвис открой гитхаб и диксон", &names);
        assert_eq!(total, 3, "Джарвис, GitHub, Discord");
        assert_eq!(wrong, ["Discord"]);
    }

    #[test]
    fn check_phrases_and_refs() {
        assert_eq!(CHECK_PHRASES.len(), 20);
        let tsv = check_refs_tsv();
        assert!(tsv.starts_with("01\tДжарвис, открой браузер\n") && tsv.lines().count() == 20);
        assert!(CHECK_PHRASES.iter().all(|p| p.starts_with("Джарвис, ")));
    }

    #[test]
    fn load_dir_reads_refs_and_wavs() {
        let dir = std::env::temp_dir().join(format!("jarvis-bench-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("refs.tsv"), "# комментарий\n01\tДжарвис, открой браузер\r\n").unwrap();
        std::fs::write(dir.join("01.wav"), crate::pcm::wav_encode(&vec![0i16; 48_000], 48_000, 1)).unwrap();
        let items = load_dir(&dir).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reference, "Джарвис, открой браузер");
        assert_eq!(items[0].pcm16k.len(), 16_000, "48 кГц → 16 кГц");
        std::fs::write(dir.join("refs.tsv"), "02\tнет файла\n").unwrap();
        assert!(load_dir(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    struct Echo;

    #[async_trait]
    impl SttEngine for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        async fn transcribe(&self, pcm: &[i16], _c: &CancellationToken) -> Result<SttResult> {
            if pcm.len() == 1 {
                return Err(VoiceError::Other("сбой".into()));
            }
            Ok(SttResult { text: "Джарвис, открой браузер".into(), duration_ms: 0 })
        }
    }

    #[tokio::test]
    async fn bench_scores_engines() {
        let items = vec![
            BenchItem { name: "01".into(), pcm16k: vec![0; 10], reference: "Джарвис, открой браузер".into() },
            BenchItem { name: "02".into(), pcm16k: vec![0; 1], reference: "Джарвис, открой GitHub".into() },
        ];
        let names = stt_hints(&[], &[]);
        let engines: Vec<(String, Result<Arc<dyn SttEngine>>)> =
            vec![("echo".into(), Ok(Arc::new(Echo))), ("нет".into(), Err(VoiceError::Config("нет модели".into())))];
        let s = bench_engines(engines, &items, &names, &BenchOptions { warmup: false, ..Default::default() }).await;
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].failed, 1);
        assert!((s[0].wer - 50.0).abs() < 1e-4, "{}", s[0].wer); // 3 из 6 слов
        assert_eq!((s[0].name_errors, s[0].name_total), (2, 3)); // Джарвис ×2 + GitHub, вторая фраза — сбой
        assert!(s[1].error.as_deref().unwrap().contains("нет модели"));
        assert!(serde_json::to_string(&s).unwrap().contains("\"wrong_names\""));
    }
}
