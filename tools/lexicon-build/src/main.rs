//! `lexicon-build` — сборщик словаря характера J.A.R.V.I.S.
//!
//! 1. Викисловарь (MediaWiki API): все страницы категорий «Матерные выражения/ru» и соседних (бранные, грубые,
//!    вульгарные, презрительные — какие существуют), для каждой статьи — первое значение, основы и явные формы
//!    из шаблонов морфологии. Итог: `config/persona/lexicon/wiktionary-ru.json` + `config/persona/SOURCES.md`.
//! 2. Пост ЖЖ «Подъёбки» → `data/persona/replies-lj.json` (только локально: лицензии у поста нет).
//!
//! Сырые ответы кэшируются в `data/persona/cache/`: повторный запуск ничего не качает заново.
//! `lexicon/user.json` (свои слова владельца) не перезаписывается никогда.
//!
//! Запуск: `cargo run -p lexicon-build --release -- [--root <корень проекта>] [--offline] [--skip-lj]`.

mod http;
mod lj;
mod wikitext;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use http::Client;
use wikitext::norm_word;

const API: &str = "https://ru.wiktionary.org/w/api.php";

/// Категории-кандидаты: (имя без префикса «Категория:», уровень, never_say для всех записей).
/// «Пренебрежительные выражения/ru» (≈1100 страниц) сознательно не берём: там в основном обычные слова
/// с уменьшительно-пренебрежительным оттенком («книжонка», «домишко»), это не ругань.
const CANDIDATES: [(&str, &str, bool); 12] = [
    ("Матерные выражения/ru", "mat", false),
    ("Обсценная лексика/ru", "mat", false),
    ("Обсценные выражения/ru", "mat", false),
    ("Обсценные прилагательные/ru", "mat", false),
    ("Нецензурные выражения/ru", "mat", false),
    ("Бранные выражения/ru", "rude", false),
    ("Грубые выражения/ru", "rude", false),
    ("Грубость/ru", "soft", false),
    ("Вульгарные выражения/ru", "rude", false),
    ("Вульгарность/ru", "soft", false),
    ("Презрительные выражения/ru", "soft", true),
    ("Оскорбительные выражения/ru", "rude", true),
];

/// Пометки грубости в первом значении статьи. Если их нет, грубый смысл у слова вторичный
/// («ядро», «мышка», «рак» в бранной категории) — запись остаётся в словаре справочно, но не распознаётся.
const RUDE_LABELS: [&str; 16] = [
    "обсц.", "мат", "мат.", "неценз.", "бран.", "бранн.", "груб.", "вульг.", "презр.", "пренебр.", "уничиж.", "оскорб.",
    "ругат.", "груб.-прост.", "эвф.", "неодобр.",
];
/// Пометки мата: запись становится `mat` в любой категории.
const MAT_LABELS: [&str; 4] = ["обсц.", "мат", "мат.", "неценз."];

/// Префиксы для разведки `list=allcategories` (в отчёт: какие категории есть на самом деле).
const DISCOVER: [&str; 9] = ["Матерн", "Бранн", "Груб", "Вульгарн", "Презрит", "Обсцен", "Оскорбит", "Нецензурн", "Пренебрежит"];

/// Корни оскорблений по национальности, религии, ориентации, здоровью, внешности: такие записи
/// распознаются, но Джарвис их не произносит никогда (`never_say`).
const SLUR_ROOTS: [&str; 46] = [
    "пидор", "пидар", "пидр", "педик", "педераст", "педрил", "гомик", "гомос", "гомосек", "жид", "хохол", "хохл",
    "чурк", "чучмек", "хач", "москал", "кацап", "нигер", "нигга", "негр", "черножоп", "узкоглаз", "косоглаз",
    "чукч", "даун", "аутист", "инвалид", "калек", "олигофрен", "дцп", "жирдя", "жиртрес", "толстожоп", "лесб",
    "пархат", "бульбаш", "армяшк", "цыган", "татарв", "жидов", "чернома", "чернозад", "азиат", "петушар", "опущенн",
    "шлюх",
];

/// Ловушки: обычные слова, с которых не может начинаться основа, используемая как префикс.
const TRAPS: &[&str] = &[
    "хлеб", "хлебать", "ребенок", "ребята", "ребус", "гребля", "учеба", "учебник", "небо", "небольшой", "себе", "тебе",
    "тебя", "себя", "колебания", "требовать", "употреблять", "оскорблять", "погреб", "щебень", "жеребец", "глеб",
    "амеба", "мебель", "лебедь", "зебра", "дебют", "дебет", "дебаты", "серебро", "ребро", "стебель", "хребет", "рубля",
    "корабля", "сабля", "ослабляет", "сукно", "суккуб", "мудрый", "мудрость", "касса", "психология", "психует", "страх",
    "страхует", "скипидар", "мандарин", "мандат", "команда", "командир", "херес", "херувим", "херсон", "фигура",
    "блюдо", "блуза", "блуждать", "пищать", "писать", "пистолет", "жизнь", "жидкий", "жираф", "сучок", "сучковатый",
    "говорить", "говядина", "срам", "сравнить", "сражение", "сразу", "посрамить", "ссылка", "ссора", "дрова",
    "дрожать", "трахея", "трактор", "тупик", "пульт", "пудра", "залив", "заливать", "манекен", "ёжик",
    "хуторок", "хулиган", "хулить", "нахлебник", "охота", "охрана", "похвала", "похороны", "поход",
    "муравей", "мудьюг", "гандбол", "гондола", "педаль", "педагог", "педант", "педиатр",
    "пидж", "пиджак", "засада", "засаленный", "засуха", "насморк", "насос", "уезд",
    "выезд", "выемка", "доезжать", "проезд", "поезд", "поездка", "переезд", "приезд", "наезд", "заезд",
    "съезд", "подъезд", "разъезд", "объезд", "въезд", "отъезд", "сыр", "сырой", "ебонит", "дура", "дурак", "дурман",
    "курица", "курить", "бляха", "бляшка", "долбить", "долото", "сучить",
];

#[derive(Serialize)]
struct Entry {
    text: String,
    kind: &'static str,
    level: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    forms: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stems: Vec<String>,
    /// Основы, по которым распознаватель ловит начало слова (проверены по ловушкам).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    prefix: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    meaning: String,
    category: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    never_say: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    insult: bool,
}

#[derive(Serialize)]
struct LexFile {
    name: &'static str,
    lang: &'static str,
    note: String,
    entries: Vec<Entry>,
}

struct Args {
    root: PathBuf,
    offline: bool,
    skip_lj: bool,
}

fn args() -> Args {
    let mut a = Args {
        root: Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."),
        offline: false,
        skip_lj: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--root" => a.root = PathBuf::from(it.next().expect("--root <dir>")),
            "--offline" => a.offline = true,
            "--skip-lj" => a.skip_lj = true,
            "-h" | "--help" => {
                println!("lexicon-build [--root <dir>] [--offline] [--skip-lj]");
                std::process::exit(0);
            }
            other => panic!("неизвестный аргумент: {other}"),
        }
    }
    a
}

fn api(c: &mut Client, params: &[(&str, &str)]) -> Result<Value, String> {
    let mut p: Vec<(&str, &str)> = vec![("format", "json"), ("formatversion", "2"), ("maxlag", "5")];
    p.extend_from_slice(params);
    let body = c.get(API, &p)?;
    let v: Value = serde_json::from_str(&body).map_err(|e| format!("JSON: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("API error: {err}"));
    }
    Ok(v)
}

/// Существующие категории с данным префиксом (имя, число страниц).
fn discover(c: &mut Client, prefix: &str) -> Result<Vec<(String, u64)>, String> {
    let v = api(c, &[("action", "query"), ("list", "allcategories"), ("acprefix", prefix), ("aclimit", "100"), ("acprop", "size")])?;
    Ok(v["query"]["allcategories"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x["category"].as_str()?.to_string(), x["pages"].as_u64().unwrap_or(0))))
                .collect()
        })
        .unwrap_or_default())
}

fn category_members(c: &mut Client, cat: &str) -> Result<Vec<String>, String> {
    let title = format!("Категория:{cat}");
    let mut out = Vec::new();
    let mut cont: Option<String> = None;
    loop {
        let mut p = vec![
            ("action", "query"),
            ("list", "categorymembers"),
            ("cmtitle", title.as_str()),
            ("cmlimit", "500"),
            ("cmtype", "page"),
            ("cmnamespace", "0"),
            ("cmprop", "title"),
        ];
        if let Some(cc) = &cont {
            p.push(("cmcontinue", cc.as_str()));
        }
        let v = api(c, &p)?;
        for m in v["query"]["categorymembers"].as_array().into_iter().flatten() {
            if let Some(t) = m["title"].as_str() {
                out.push(t.to_string());
            }
        }
        match v["continue"]["cmcontinue"].as_str() {
            Some(cc) => cont = Some(cc.to_string()),
            None => break,
        }
    }
    Ok(out)
}

/// Викитекст страниц пачками (≤ 50 заголовков и разумная длина URL).
fn fetch_wikitext(c: &mut Client, titles: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut batches: Vec<Vec<&String>> = Vec::new();
    let mut cur: Vec<&String> = Vec::new();
    let mut len = 0;
    for t in titles {
        let l = t.len() * 3 + 3;
        if cur.len() >= 50 || (len + l > 5000 && !cur.is_empty()) {
            batches.push(std::mem::take(&mut cur));
            len = 0;
        }
        cur.push(t);
        len += l;
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    for (i, b) in batches.iter().enumerate() {
        let joined = b.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("|");
        let mut cont: Option<String> = None;
        loop {
            let mut p = vec![
                ("action", "query"),
                ("prop", "revisions"),
                ("rvprop", "content"),
                ("rvslots", "main"),
                ("titles", joined.as_str()),
            ];
            if let Some(cc) = &cont {
                p.push(("rvcontinue", cc.as_str()));
            }
            let v = api(c, &p)?;
            for page in v["query"]["pages"].as_array().into_iter().flatten() {
                let (Some(title), Some(text)) =
                    (page["title"].as_str(), page["revisions"][0]["slots"]["main"]["content"].as_str())
                else {
                    continue;
                };
                out.insert(title.to_string(), text.to_string());
            }
            match v["continue"]["rvcontinue"].as_str() {
                Some(cc) => cont = Some(cc.to_string()),
                None => break,
            }
        }
        if (i + 1) % 10 == 0 {
            eprintln!("  статьи: {}/{} пачек", i + 1, batches.len());
        }
    }
    Ok(out)
}

fn level_rank(l: &str) -> u8 {
    match l {
        "mat" => 3,
        "rude" => 2,
        "soft" => 1,
        _ => 0,
    }
}

/// Основа годится как префикс: достаточно длинная и ни одна ловушка с неё не начинается.
fn safe_prefix(stem: &str, level: &str, lemma: &str) -> bool {
    let n = stem.chars().count();
    let min = if level == "mat" { 4 } else { 6 };
    n >= min
        && stem.chars().all(|c| c.is_alphabetic())
        && !TRAPS.iter().any(|t| {
            norm_word(t).starts_with(stem) && !lemma.starts_with(&norm_word(t))
        })
}

fn is_insult(pos: Option<&str>, meaning: &str) -> bool {
    let m = meaning.to_lowercase();
    pos == Some("сущ")
        && ["человек", "о мужчине", "о женщине", "о ком-либо", "о ком-л", "бранное", "ругательство", "негодя", "дурак", "глуп"]
            .iter()
            .any(|k| m.contains(k))
}

fn main() {
    let a = args();
    let root = a.root.canonicalize().unwrap_or(a.root.clone());
    let cache = root.join("data").join("persona").join("cache");
    let mut c = Client::new(cache, a.offline);
    let today = today();

    // ── разведка категорий ──
    println!("== Разведка категорий (list=allcategories) ==");
    let mut existing: BTreeMap<String, u64> = BTreeMap::new();
    for p in DISCOVER {
        match discover(&mut c, p) {
            Ok(list) => {
                for (name, n) in list.into_iter().filter(|(n, _)| n.ends_with("/ru")) {
                    println!("  {name}: {n}");
                    existing.insert(name, n);
                }
            }
            Err(e) => eprintln!("  {p}: {e}"),
        }
    }

    // ── участники категорий ──
    let mut title_cats: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    let mut used: Vec<(&str, usize)> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for (cat, _, _) in CANDIDATES {
        if !existing.contains_key(cat) {
            missing.push(cat);
            continue;
        }
        match category_members(&mut c, cat) {
            Ok(m) => {
                println!("Категория:{cat}: {} страниц", m.len());
                used.push((cat, m.len()));
                for t in m {
                    title_cats.entry(t).or_default().insert(cat);
                }
            }
            Err(e) => {
                eprintln!("Категория:{cat}: {e}");
                missing.push(cat);
            }
        }
    }
    if used.is_empty() {
        eprintln!("ни одной категории не загружено");
        std::process::exit(1);
    }

    // ── статьи ──
    let titles: Vec<String> = title_cats.keys().cloned().collect();
    println!("Статей к разбору: {}", titles.len());
    let texts = match fetch_wikitext(&mut c, &titles) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("статьи: {e}");
            std::process::exit(1);
        }
    };

    let mut entries = Vec::new();
    let mut per_cat: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let (mut n_words, mut n_phrases, mut n_forms, mut n_stems, mut n_prefix, mut n_meaning) = (0, 0, 0, 0, 0, 0);
    for (title, cats) in &title_cats {
        let lemma = norm_word(title);
        let is_phrase = title.trim().contains(' ');
        let level = cats
            .iter()
            .map(|c| CANDIDATES.iter().find(|x| x.0 == *c).map(|x| x.1).unwrap_or("soft"))
            .max_by_key(|l| level_rank(l))
            .unwrap_or("soft")
            .to_string();
        let section = texts.get(title).and_then(|t| wikitext::russian_section(t));
        let meaning = section.map(|s| wikitext::first_meaning(s, 120)).unwrap_or_default();
        // Уровень по пометкам первого значения: мат → mat; иная грубость → по категории; нет пометок → none.
        let labels = section.map(wikitext::first_meaning_labels).unwrap_or_default();
        let has = |set: &[&str]| labels.iter().any(|l| set.contains(&l.as_str()));
        let slur = SLUR_ROOTS.iter().any(|r| lemma.contains(r));
        let level = if has(&MAT_LABELS) {
            "mat".to_string()
        } else if has(&RUDE_LABELS) || slur {
            level
        } else {
            "none".to_string()
        };
        let active = level != "none";
        let never_say = slur || (active && cats.iter().any(|c| CANDIDATES.iter().any(|x| x.0 == *c && x.2)));
        for c in cats {
            let s = per_cat.entry(c).or_insert((0, 0));
            s.0 += 1;
            s.1 += usize::from(active);
        }
        let mut forms = vec![lemma.clone()];
        let mut stems = Vec::new();
        let mut prefix = Vec::new();
        let mut insult = false;
        if !is_phrase {
            if let Some(s) = section {
                let m = wikitext::morph(s);
                for f in m.forms {
                    if !forms.contains(&f) && !f.contains(' ') {
                        forms.push(f);
                    }
                }
                for st in m.stems {
                    if level_rank(&level) >= 2 && safe_prefix(&st, &level, &lemma) && !prefix.contains(&st) {
                        prefix.push(st.clone());
                    }
                    stems.push(st);
                }
                insult = active && is_insult(m.pos.as_deref(), &meaning);
            }
            n_words += 1;
        } else {
            n_phrases += 1;
        }
        n_forms += forms.len();
        n_stems += stems.len();
        n_prefix += prefix.len();
        n_meaning += usize::from(!meaning.is_empty());
        entries.push(Entry {
            text: title.clone(),
            kind: if is_phrase { "phrase" } else { "word" },
            level,
            forms,
            stems,
            prefix,
            meaning,
            category: cats.iter().map(|c| c.to_string()).collect(),
            never_say,
            insult,
        });
    }
    let n_never = entries.iter().filter(|e| e.never_say).count();
    let n_insult = entries.iter().filter(|e| e.insult).count();

    let lex_dir = root.join("config").join("persona").join("lexicon");
    fs::create_dir_all(&lex_dir).expect("config/persona/lexicon");
    let file = LexFile {
        name: "wiktionary-ru",
        lang: "ru",
        note: format!(
            "Сгенерировано lexicon-build {today} из ru.wiktionary.org (CC BY-SA 4.0), см. ../SOURCES.md. Не править руками: \
             перезаписывается при пересборке; свои слова — в user.json."
        ),
        entries,
    };
    let json = serde_json::to_string_pretty(&file).expect("json");
    fs::write(lex_dir.join("wiktionary-ru.json"), json).expect("write wiktionary-ru.json");

    // user.json — шаблон, только если его ещё нет.
    let user = lex_dir.join("user.json");
    if !user.exists() {
        fs::write(&user, "{\n  \"words\": [],\n  \"never_say\": []\n}\n").expect("user.json");
    }

    // ── ЖЖ ──
    let mut lj_stats = None;
    if !a.skip_lj {
        match c.get(lj::URL, &[]) {
            Ok(html) => {
                let lines = lj::parse(&html);
                let dir = root.join("data").join("persona");
                fs::create_dir_all(&dir).expect("data/persona");
                let mut per: BTreeMap<String, usize> = BTreeMap::new();
                for l in &lines {
                    *per.entry(l.section.clone()).or_default() += 1;
                }
                let usable = lines.iter().filter(|l| l.usable).count();
                let out = serde_json::json!({
                    "source": lj::URL,
                    "fetched": today,
                    "license": "не указана — только для локального использования, в git не класть",
                    "lines": lines,
                });
                fs::write(dir.join("replies-lj.json"), serde_json::to_string_pretty(&out).unwrap()).expect("replies-lj.json");
                println!("ЖЖ: {} строк ({usable} пригодных): {per:?}", lines.len());
                lj_stats = Some((lines.len(), usable, per));
            }
            Err(e) => eprintln!("ЖЖ: {e}"),
        }
    }

    // ── SOURCES.md ──
    let mut md = String::new();
    md.push_str("# Источники словаря характера\n\n");
    md.push_str(&format!("Собрано `tools/lexicon-build` {today}.\n\n"));
    md.push_str("## Викисловарь (ru.wiktionary.org)\n\n");
    md.push_str(
        "Файл `lexicon/wiktionary-ru.json` получен из Русского Викисловаря через MediaWiki API \
         (`https://ru.wiktionary.org/w/api.php`, `list=categorymembers` и `prop=revisions`).\n\
         Материалы Викисловаря доступны по лицензии [Creative Commons Attribution-ShareAlike 4.0]\
         (https://creativecommons.org/licenses/by-sa/4.0/deed.ru); авторы — участники Викисловаря \
         (история правок каждой статьи: `https://ru.wiktionary.org/w/index.php?title=<статья>&action=history`). \
         Изменения: взяты заголовок, первое значение (без разметки, сокращено до 120 символов), основы и явные \
         словоформы из шаблонов морфологии; уровни грубости проставлены по категории и пометкам первого значения \
         (без пометки грубости запись остаётся справочной, `level: none`, и не распознаётся). Производный файл \
         распространяется на тех же условиях (CC BY-SA 4.0).\n\n",
    );
    md.push_str("| Категория | URL | Страниц | Распознаётся |\n|---|---|---|---|\n");
    for (cat, n) in &used {
        let active = per_cat.get(cat).map(|s| s.1).unwrap_or(0);
        md.push_str(&format!(
            "| {cat} | https://ru.wiktionary.org/wiki/Категория:{} | {n} | {active} |\n",
            cat.replace(' ', "_")
        ));
    }
    if !missing.is_empty() {
        md.push_str(&format!("\nНе существуют (проверено `list=allcategories`): {}.\n", missing.join(", ")));
    }
    md.push_str(
        "\nСуществует, но не взята: «Пренебрежительные выражения/ru» — обычные слова с пренебрежительным оттенком, не ругань.\n",
    );
    md.push_str(&format!(
        "\nИтого: {} записей ({n_words} слов, {n_phrases} выражений), словоформ {n_forms}, основ {n_stems} \
         (из них {n_prefix} используются как префикс), значений {n_meaning}, `never_say` {n_never}, оскорблений {n_insult}.\n\n",
        n_words + n_phrases
    ));
    md.push_str("## Ручные списки\n\n");
    md.push_str(
        "`lexicon/roots-ru.json`, `lexicon/en.json`, `lexicon/ro.json`, `replies/*.json` — написаны вручную для \
         J.A.R.V.I.S. (корни, исключения-ловушки, варианты распознавателя речи, английский и румынский, похвала и \
         извинения, банк реплик). `lexicon/user.json` — слова владельца, сборщик его не трогает.\n\n",
    );
    md.push_str("## ЖЖ «Подъёбки» (только локально)\n\n");
    md.push_str(&format!(
        "{} (2013). Лицензия не указана, поэтому разобранные строки лежат только в `data/persona/replies-lj.json` \
         (папка `data/` не попадает в git) и в репозиторий не входят.",
        lj::URL
    ));
    if let Some((n, u, per)) = &lj_stats {
        md.push_str(&format!(" Последний разбор: {n} строк, пригодных {u}; по разделам: "));
        md.push_str(&per.iter().map(|(k, v)| format!("«{k}» {v}")).collect::<Vec<_>>().join(", "));
        md.push('.');
    }
    md.push('\n');
    fs::write(root.join("config").join("persona").join("SOURCES.md"), md).expect("SOURCES.md");

    for (cat, (n, act)) in &per_cat {
        println!("  {cat}: {n} записей, распознаётся {act}");
    }
    println!(
        "Готово: {} записей ({n_words} слов, {n_phrases} выражений), форм {n_forms}, основ {n_stems}, префиксов {n_prefix}, \
         значений {n_meaning}, never_say {n_never}, insult {n_insult}. Сеть: {} запросов, кэш: {}.",
        n_words + n_phrases,
        c.fetched,
        c.from_cache
    );
}

/// Сегодняшняя дата (UTC) без зависимостей: дни с эпохи → гражданская дата.
fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let z = (secs / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
