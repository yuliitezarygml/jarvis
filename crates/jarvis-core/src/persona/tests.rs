use super::*;
use crate::config::PersonaMode::{Buddy, Butler, Uncensored};

fn real_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("config").join("persona")
}

fn real() -> Persona {
    Persona::load(&real_dir(), None).expect("config/persona загружается")
}

/// Полный словарь загружается один раз на весь прогон тестов.
fn lex() -> &'static Lexicon {
    static L: std::sync::OnceLock<Lexicon> = std::sync::OnceLock::new();
    L.get_or_init(|| Lexicon::load(&real_dir()).expect("lexicon"))
}

fn level(text: &str) -> SwearLevel {
    analyze(lex(), text).swear_level
}

#[test]
fn normalize_and_tokenize() {
    assert_eq!(normalize("Ёжик, ПОШЁЛ-ка!  ну…"), "ежик пошел ка ну");
    assert_eq!(normalize("Futu-ți mama, pulă"), "futu ti mama pula");
    assert_eq!(tokenize("you're  ху́й"), vec!["youre", "хуй"]);
    assert!(tokenize(" ,.!").is_empty());
}

#[test]
fn every_root_in_many_forms() {
    let mat = [
        "нахуй", "на хуй", "нах", "похуй", "охуел", "хуёвый", "хуйня", "нихуя", "хули ты встал", "охуенно", "ахуеть",
        "заебал", "заебала", "заёбывать", "ебанутый", "наебал", "уёбок", "ёбаный", "ебать", "въебал", "отъебись", "проебал",
        "долбоёб", "выебывается", "пиздец", "пиздёж", "распиздяй", "спиздил", "пизда", "бля", "блять", "блядь", "бл",
        "выблядок", "сука", "суки", "сучка", "сукин сын", "мудак", "мудила", "мудозвон", "говно", "говнюк", "говённый",
        "жопа", "жопе", "жопу", "залупа", "манда", "мандавошка", "шлюха", "дрочит", "срать", "обосрался", "засранец",
        "ссать", "обоссал", "ссыкло", "гандон", "пидорас",
    ];
    for w in mat {
        assert_eq!(level(w), SwearLevel::Mat, "«{w}» — мат");
    }
    for w in ["дебил", "идиот", "тупой", "придурок", "лох", "кретин", "козёл", "херня", "нахер", "дерьмо", "заткнись"] {
        assert!(level(w) >= SwearLevel::Rude, "«{w}» — грубо, а вышло {:?}", level(w));
    }
    for w in ["блин", "капец", "жесть", "офигеть", "ёлки-палки", "чёрт", "фигня", "хрен", "пофиг"] {
        assert!(level(w) >= SwearLevel::Soft, "«{w}» — мягко, а вышло {:?}", level(w));
    }
}

#[test]
fn false_positive_traps_never_flagged() {
    let traps = [
        "употреблять", "оскорблять", "хлебать", "хлеб", "ребята", "ребёнок", "учебник", "учёба", "небо", "небольшой",
        "колебания", "требовать", "погреб", "рубля", "корабля", "сукно", "суккуб", "мудрый", "мудрость", "касса",
        "психология", "психуешь", "страх", "застрахуй", "скипидар", "тебе", "себе", "тебя", "себя", "Глеб", "сабля",
        "ребус", "гребля", "щебень", "жеребец", "амёба", "мебель", "лебедь", "дебют", "дебаты", "серебро", "команда",
        "командир", "мандарин", "мандат", "херес", "фигура", "тупик", "спидран", "педикюр", "обсерватория",
        "посрамить", "посередине", "бляшка", "бляха", "веб-сайт", "ссылка", "ссора", "сразу", "сравнить", "сражение",
        "страхует", "оскорбление", "истребитель", "потребитель", "дебри", "хребет", "ребро", "трактор", "трахея",
        "скипидаром", "хулиган", "хутор", "колеблется", "ослабляет", "расслабляет", "углубляет", "ебонит",
        "процессор загружен", "открой блокнот", "удали папку", "мышка", "ядро процессора", "сколько стоит хлебушек",
        "сегодня небо ясное", "учебная тревога", "катался на велосипеде", "оторвать кусок", "кассир", "наступил",
    ];
    for t in traps {
        let tone = analyze(lex(), t);
        assert!(tone.swear_level <= SwearLevel::Soft && !tone.sendoff, "ловушка «{t}» сработала: {tone:?}");
        if !["бляха"].contains(&t) {
            assert_eq!(tone.swear_level, SwearLevel::None, "ловушка «{t}»: {:?}", tone.matched);
        }
    }
}

#[test]
fn english_and_romanian() {
    for w in ["fuck", "fucking hell", "motherfucker", "фак", "вотафак", "мазафака"] {
        assert_eq!(level(w), SwearLevel::Mat, "{w}");
    }
    for w in ["shit", "bullshit", "щит", "bitch", "asshole", "асхол"] {
        assert!(level(w) >= SwearLevel::Rude, "{w}");
    }
    assert!(level("damn it") >= SwearLevel::Soft);
    assert_eq!(level("факт"), SwearLevel::None);
    assert_eq!(level("факультет"), SwearLevel::None);
    for w in ["pulă", "pizda mătii", "futu-ți mama", "muie", "пула", "футуць", "пиздэ"] {
        assert_eq!(level(w), SwearLevel::Mat, "{w}");
    }
    for w in ["la dracu", "naiba", "наиба", "prostule", "bou", "căcat"] {
        assert!(level(w) >= SwearLevel::Soft, "{w}");
    }
    assert_eq!(level("populară"), SwearLevel::None);
    assert!(analyze(lex(), "fuck off").sendoff);
    assert!(analyze(lex(), "du-te dracu").sendoff);
    assert!(analyze(lex(), "multumesc").praise);
    assert!(analyze(lex(), "sorry").apology);
}

#[test]
fn sendoff_phrases() {
    for p in [
        "пошёл нахуй", "пошел на хуй", "иди нахуй", "иди ты нахуй", "иди в жопу", "иди в пизду", "отъебись", "отвали",
        "заткнись", "да пошёл ты", "иди ты", "нахуй иди", "вали отсюда", "пошёл вон", "иди к чёрту", "катись нафиг",
        "завали ебало", "отстань от меня", "иди лесом", "проваливай", "пошла ты", "съебись",
    ] {
        let t = analyze(lex(), p);
        assert!(t.sendoff && t.insult_to_jarvis, "«{p}» — посылание: {t:?}");
    }
    for p in [
        "нахуй ты это сделал", "пошли в кино", "давай пошли отсюда", "иди сюда", "пошёл дождь", "блин опять не работает",
        "открой папку", "что за хуйня",
    ] {
        assert!(!analyze(lex(), p).sendoff, "«{p}» — не посылание");
    }
}

#[test]
fn insult_to_jarvis_best_effort() {
    for p in ["ты дебил", "какой же ты тупой", "Джарвис, ты идиот", "тупая железяка", "ты охуел?", "дебил", "ну ты и мудак"] {
        assert!(analyze(lex(), p).insult_to_jarvis, "«{p}»");
    }
    for p in ["он дебил", "мой сосед идиот", "блин", "сука, опять не работает", "этот сайт тупой какой-то"] {
        assert!(!analyze(lex(), p).insult_to_jarvis, "«{p}»");
    }
}

#[test]
fn praise_and_apology() {
    for p in ["молодец", "красава", "спасибо", "ты лучший", "умница, Джарвис", "круто"] {
        assert!(analyze(lex(), p).praise, "«{p}»");
    }
    assert!(!analyze(lex(), "не круто").praise);
    for p in ["извини", "прости меня", "сорри", "ладно, не обижайся", "я был не прав", "погорячился"] {
        assert!(analyze(lex(), p).apology, "«{p}»");
    }
    let t = analyze(lex(), "спасибо");
    assert_eq!(t.swear_level, SwearLevel::None);
    assert_eq!(t.note().as_deref(), Some("похвала"));
}

#[test]
fn tone_note() {
    let t = analyze(lex(), "заебал, пошёл нахуй");
    let note = t.note().unwrap();
    assert!(note.starts_with("мат (") && note.contains("посылание"), "{note}");
    assert!(analyze(lex(), "открой браузер").note().is_none());
    assert!(analyze(lex(), "открой браузер").is_neutral());
}

#[test]
fn builtin_minimal_lexicon_works() {
    let l = Lexicon::builtin_minimal();
    assert_eq!(analyze(&l, "заебал").swear_level, SwearLevel::Mat);
    assert!(analyze(&l, "пошёл нахуй").sendoff);
    assert_eq!(analyze(&l, "хлеб и ребята").swear_level, SwearLevel::None);
    assert!(analyze(&l, "спасибо").praise);
}

#[test]
fn user_words_and_never_say() {
    let mut l = Lexicon::builtin_minimal();
    l.add_json("user", r#"{"words": ["жиза", {"text": "ёпта", "level": "rude"}], "never_say": ["кожаный мешок"]}"#).unwrap();
    let t = analyze(&l, "ну жиза");
    assert_eq!(t.swear_level, SwearLevel::None);
    assert_eq!(analyze(&l, "ёпта").swear_level, SwearLevel::Rude);
    assert_eq!(l.never_say_hits("ты кожаный мешок"), vec!["кожаный мешок".to_string()]);
    assert!(!l.never_say_hits("пидор").is_empty(), "словарный never_say");
}

#[test]
fn render_address() {
    let none = Vars::new();
    let sir = Vars::new().address(Some("сэр"));
    assert_eq!(render("Слушаю, {address}.", &none), "Слушаю.");
    assert_eq!(render("Слушаю, {address}.", &sir), "Слушаю, сэр.");
    assert_eq!(render("{address}, удалить папку?", &none), "Удалить папку?");
    assert_eq!(render("{address}, удалить папку?", &sir), "Сэр, удалить папку?");
    assert_eq!(render("Да, {address}, слушаю.", &none), "Да, слушаю.");
    assert_eq!(render("Готово. {address}, что дальше?", &none), "Готово. Что дальше?");
    assert_eq!(render("Готово. {address}, что дальше?", &sir), "Готово. Сэр, что дальше?");
    assert_eq!(render("Как скажете {address}!", &none), "Как скажете!");
    let v = Vars::new().set("action", "удалить папку").address(Some("Юлиан"));
    assert_eq!(render("{address}, {action}? Да или нет?", &v), "Юлиан, удалить папку? Да или нет?");
    assert_eq!(render("{action}, точно?", &v), "Удалить папку, точно?");
    // Подстановка в начале предложения — с заглавной, в середине — как есть.
    assert_eq!(render("Сношу, уверен? {action}. Да или нет?", &v), "Сношу, уверен? Удалить папку. Да или нет?");
    assert_eq!(render("Собираюсь {action}. {action}?", &v), "Собираюсь удалить папку. Удалить папку?");
    assert_eq!(render("Ошибка: {err}.", &Vars::new()), "Ошибка: ….");
    assert_eq!(placeholders("{a} и {b_1}, {a} {не}"), vec!["a", "b_1"]);
}

fn test_replies() -> Replies {
    let lex = Lexicon::builtin_minimal();
    let mut r = Replies::builtin_minimal(&lex);
    for (i, t) in ["Раз, {address}.", "Два.", "Три.", "Четыре.", "Пять, бля.", "Шесть, хлеб."].iter().enumerate() {
        r.add_line(&lex, Uncensored, Situation::Wake, t, i == 5, "test");
    }
    r
}

#[test]
fn pick_without_immediate_repeats() {
    let r = test_replies();
    let mut st = PickState::with_seed(42);
    let v = Vars::new().profanity(true);
    let mut prev = String::new();
    let mut seen = HashSet::new();
    for _ in 0..200 {
        let p = r.pick(Uncensored, Situation::Wake, &mut st, &v).unwrap();
        assert_ne!(p, prev, "повтор подряд");
        seen.insert(p.clone());
        prev = p;
    }
    assert_eq!(seen.len(), 6, "все варианты выпадают: {seen:?}");
    // Память 4 из 6: среди любых 5 подряд нет одинаковых.
    let mut st = PickState::with_seed(7);
    let seq: Vec<String> = (0..60).map(|_| r.pick(Uncensored, Situation::Wake, &mut st, &v).unwrap()).collect();
    for w in seq.windows(5) {
        let s: HashSet<&String> = w.iter().collect();
        assert_eq!(s.len(), 5, "{w:?}");
    }
}

#[test]
fn pick_respects_profanity_banned_and_placeholders() {
    let mut r = test_replies();
    let mut st = PickState::with_seed(1);
    let clean = Vars::new();
    for _ in 0..50 {
        let p = r.pick(Uncensored, Situation::Wake, &mut st, &clean).unwrap();
        assert!(!p.contains("бля") && !p.contains("Шесть"), "мат/жёсткое без разрешения: {p}");
    }
    r.set_banned(&["Два".into(), "три".into()]);
    for _ in 0..50 {
        let p = r.pick(Uncensored, Situation::Wake, &mut st, &clean).unwrap();
        assert!(p != "Два." && p != "Три.", "{p}");
    }
    // Без {action} шаблон подтверждения не выбирается вовсе.
    assert!(r.pick(Butler, Situation::ConfirmDangerous, &mut st, &clean).is_none());
    let v = Vars::new().address(Some("сэр")).set("action", "удалить папку");
    assert_eq!(r.pick(Butler, Situation::ConfirmDangerous, &mut st, &v).unwrap(), "Сэр, удалить папку? Скажите «да» или «нет».");
    // {err} необязателен: без него берётся вариант без подстановки.
    assert_eq!(r.pick(Butler, Situation::ModelError, &mut st, &Vars::new()).unwrap(), "Прошу прощения, произошла ошибка.");
    let s = r.samples(Uncensored, Situation::Wake, 3, &mut st, &clean);
    assert_eq!(s.len(), 2, "после запрета остались «Раз» и «Четыре»: {s:?}");
    assert_eq!(s.iter().collect::<HashSet<_>>().len(), 2);
}

#[test]
fn never_say_lines_are_dropped() {
    let lex = Lexicon::builtin_minimal();
    let mut r = Replies::builtin_minimal(&lex);
    assert!(!r.add_line(&lex, Uncensored, Situation::InsultReply, "Сам ты пидор.", false, "test"));
    assert!(r.warnings().iter().any(|w| w.contains("never_say")));
    assert!(r.add_line(&lex, Uncensored, Situation::InsultReply, "Сам такой.", false, "test"));
}

#[test]
fn mood_rises_and_decays() {
    let t0 = Instant::now();
    let mut m = Mood::new();
    let sendoff = analyze(lex(), "пошёл нахуй");
    assert_eq!(m.label_at(t0), MoodLabel::Calm);
    m.on_tone_at(&sendoff, false, t0);
    assert_eq!(m.label_at(t0), MoodLabel::Irritated);
    assert_eq!(m.note_at(t0).as_deref(), Some("раздражён — его послали"));
    m.on_tone_at(&sendoff, false, t0 + Duration::from_secs(10));
    let t1 = t0 + Duration::from_secs(11);
    assert_eq!(m.label_at(t1), MoodLabel::Offended);
    assert_eq!(m.note_at(t1).as_deref(), Some("обижен — его дважды послали"));
    assert_eq!(m.sendoffs_at(t1), 2);
    // Полураспад 10 минут: через 10 минут вдвое меньше.
    let i1 = m.irritation_at(t1);
    let i2 = m.irritation_at(t1 + Duration::from_secs(600));
    assert!((i2 - i1 / 2.0).abs() < 0.01, "{i1} → {i2}");
    assert_eq!(m.label_at(t1 + Duration::from_secs(3600)), MoodLabel::Calm);
    assert!(m.note_at(t1 + Duration::from_secs(3600)).is_none());
}

#[test]
fn mood_praise_apology_shouting() {
    let t0 = Instant::now();
    let mut m = Mood::new();
    let sendoff = analyze(lex(), "отвали");
    m.on_tone_at(&sendoff, true, t0);
    m.on_tone_at(&sendoff, false, t0);
    assert_eq!(m.label_at(t0), MoodLabel::Offended);
    m.on_tone_at(&analyze(lex(), "ладно, извини"), false, t0);
    assert!(m.irritation_at(t0) < MOOD_OFFENDED);
    m.on_apology_at(t0);
    assert_eq!(m.label_at(t0), MoodLabel::Calm);
    assert_eq!(m.sendoffs_at(t0), 0);
    let mut m = Mood::new();
    let calm = analyze(lex(), "открой браузер");
    for k in 0..3 {
        m.on_tone_at(&calm, true, t0 + Duration::from_secs(k));
    }
    assert_eq!(m.label_at(t0 + Duration::from_secs(3)), MoodLabel::Irritated);
    assert_eq!(m.note_at(t0 + Duration::from_secs(3)).as_deref(), Some("раздражён — на него несколько раз кричали"));
    m.on_praise_at(t0 + Duration::from_secs(4));
    assert_eq!(m.label_at(t0 + Duration::from_secs(4)), MoodLabel::Calm);
    // Мат «в воздух» почти не злит.
    let mut m = Mood::new();
    m.on_tone_at(&analyze(lex(), "бля, опять не работает"), false, t0);
    assert_eq!(m.label_at(t0), MoodLabel::Calm);
}

#[test]
fn mirror_profanity() {
    let t0 = Instant::now();
    let mut mr = MirrorState::new();
    assert!(!mr.may_swear_at(Uncensored, t0));
    mr.on_user_at(&analyze(lex(), "блин"), t0);
    assert!(!mr.may_swear_at(Uncensored, t0), "«блин» — не мат");
    mr.on_user_at(&analyze(lex(), "заебало всё"), t0);
    assert!(mr.may_swear_at(Uncensored, t0));
    assert!(!mr.may_swear_at(Buddy, t0) && !mr.may_swear_at(Butler, t0));
    assert!(!mr.may_swear_at(Uncensored, t0 + Duration::from_secs(16 * 60)), "окно 15 минут");
    for _ in 0..6 {
        mr.on_user_at(&analyze(lex(), "открой браузер"), t0);
    }
    assert!(!mr.may_swear_at(Uncensored, t0), "6 спокойных реплик подряд");
    mr.on_user_at(&analyze(lex(), "отвали"), t0);
    assert!(mr.may_swear_at(Uncensored, t0), "послали — можно ответить");
}

// ───────────── проверка настоящих файлов config/persona ─────────────

#[test]
fn real_config_files_are_valid() {
    let p = real();
    assert!(p.lexicon.files().iter().any(|f| f == "wiktionary-ru"), "{:?}", p.lexicon.files());
    assert!(p.lexicon.len() > 1500, "записей {}", p.lexicon.len());
    assert!(p.replies.warnings().is_empty(), "предупреждения загрузки:\n{}", p.replies.warnings().join("\n"));
    let mut total = 0;
    let mut bad: Vec<String> = Vec::new();
    macro_rules! check { ($c:expr, $($m:tt)*) => { if !$c { bad.push(format!($($m)*)); } } }
    for mode in PersonaMode::ALL {
        for sit in Situation::ALL {
            let lines = p.replies.lines(mode, sit);
            let what = format!("{}/{}", mode.as_str(), sit.key());
            check!(lines.len() >= 10, "{what}: {} фраз (< 10)", lines.len());
            total += lines.len();
            for l in lines {
                let t = &l.text;
                check!(!t.contains('*'), "{what}: звёздочка в «{t}»");
                check!(t.chars().count() <= 220, "{what}: слишком длинно «{t}»");
                check!(p.lexicon.never_say_hits(t).is_empty(), "{what}: never_say в «{t}»");
                for ph in placeholders(t) {
                    check!(sit.allows(&ph), "{what}: недопустимая подстановка {{{ph}}} в «{t}»");
                }
                for req in sit.required() {
                    check!(t.contains(&format!("{{{req}}}")), "{what}: нет {{{req}}} в «{t}»");
                }
                match mode {
                    Butler => check!(l.level == SwearLevel::None, "{what}: дворецкий грубит: «{t}» {:?}", p.lexicon.hits(t)),
                    Buddy => check!(l.level <= SwearLevel::Soft, "{what}: мат у «своего парня»: «{t}» {:?}", p.lexicon.hits(t)),
                    Uncensored => {}
                }
                if matches!(sit, Situation::ConfirmDangerous | Situation::ProtectedPath) {
                    let toks = tokenize(t);
                    check!(toks.contains(&"да".into()) && toks.contains(&"нет".into()), "{what}: нет вопроса «да или нет»: «{t}»");
                }
            }
            // Без разрешённого мата в «Без цензуры» тоже есть из чего выбрать.
            let clean = lines.iter().filter(|l| !l.needs_profanity()).count();
            check!(clean >= 3, "{what}: чистых фраз {clean} (< 3)");
            // Необязательные подстановки: есть фразы и без них.
            if !sit.optional().is_empty() {
                let bare = lines.iter().filter(|l| sit.optional().iter().all(|o| !l.text.contains(&format!("{{{o}}}")))).count();
                check!(bare >= 3, "{what}: фраз без необязательных подстановок {bare} (< 3)");
            }
        }
    }
    assert!(bad.is_empty(), "проблемы в config/persona ({}):
{}", bad.len(), bad.join("
"));
    assert!(total >= 900, "всего фраз {total}");
    // В «Без цензуры» мат реально есть (иначе зеркалить нечем).
    let mat = Situation::ALL.iter().map(|s| p.replies.lines(Uncensored, *s).iter().filter(|l| l.level == SwearLevel::Mat).count()).sum::<usize>();
    assert!(mat >= 60, "фраз с матом в uncensored: {mat}");
}

#[test]
fn real_config_pick_everywhere() {
    let p = real();
    let mut st = PickState::with_seed(3);
    let full = Vars::new()
        .address(Some("сэр"))
        .set("action", "удалить папку")
        .set("err", "нет связи с сервером")
        .set("pct", "93")
        .set("proc", "Chrome")
        .set("disk", "C:")
        .set("free", "4 ГБ")
        .set("n", "25")
        .set("problem", "сервер не отвечает");
    for mode in PersonaMode::ALL {
        for sit in Situation::ALL {
            for vars in [full.clone(), full.clone().profanity(true), Vars::new().set("action", "удалить папку").set("pct", "93").set("disk", "C:")] {
                let line = p.replies.pick(mode, sit, &mut st, &vars).unwrap_or_else(|| panic!("{}/{}", mode.as_str(), sit.key()));
                assert!(!line.contains('{') && !line.contains('}'), "{line}");
                assert!(!line.contains(" ,") && !line.contains(",."), "{line}");
            }
            let s = p.replies.samples(mode, sit, 8, &mut st, &Vars::new());
            assert!(s.len() >= 3, "{}/{}: примеров {}", mode.as_str(), sit.key(), s.len());
        }
    }
}

#[test]
fn analyze_is_fast() {
    let l = lex();
    let text = "слушай, джарвис, ну ты и тупой, заебал уже, открой этот грёбаный браузер и иди нахуй, понял";
    let t0 = Instant::now();
    for _ in 0..200 {
        std::hint::black_box(analyze(l, text));
    }
    let per = t0.elapsed() / 200;
    assert!(per < Duration::from_millis(5), "analyze: {per:?} на реплику");
}

#[test]
fn everyday_commands_are_neutral() {
    let cmds = [
        "открой хром", "поставь музыку погромче", "выключи компьютер через час", "сделай скриншот экрана", "найди файл отчёт",
        "удали временные файлы", "какая сегодня погода", "запусти игру", "сверни все окна", "открой диск це",
        "сколько свободного места на диске", "покажи загрузку процессора", "закрой браузер", "перезагрузи компьютер",
        "напиши письмо начальнику", "включи ночной режим", "открой папку загрузки", "переименуй файл", "скачай обновления",
        "поставь будильник на семь утра", "что нового в новостях", "переведи текст на английский", "открой ютуб",
        "вруби что-нибудь бодрое", "отключи звук", "сделай потише", "мышка не работает", "ядро процессора греется",
        "почисти корзину", "кто такой Глеб Жеглов", "расскажи анекдот", "найди рецепт борща", "собери статистику",
        "запусти командную строку", "открой диспетчер задач", "убей процесс хром", "останови службу печати",
        "проверь обновления драйверов", "сколько стоит биткоин", "напомни купить хлеб", "в чём смысл жизни",
        "открой настройки", "давай поиграем", "мне скучно", "я устал", "работай быстрее", "ты где", "ну что там",
        "полный экран", "свернуть", "бабушка звонила", "начальник опять ругается", "кот наступил на клавиатуру",
    ];
    let mut bad = Vec::new();
    for c in cmds {
        let t = analyze(lex(), c);
        if t.swear_level > SwearLevel::None || t.sendoff || t.insult_to_jarvis {
            bad.push(format!("«{c}»: {:?} {:?}", t.swear_level, lex().hits(c).iter().map(|h| (&h.surface, &h.entry)).collect::<Vec<_>>()));
        }
    }
    assert!(bad.is_empty(), "обычные команды сработали:\n{}", bad.join("\n"));
}

#[test]
fn local_lj_file_if_present() {
    let local = real_dir().join("..").join("..").join("data").join("persona");
    if !local.join("replies-lj.json").exists() {
        return; // локальный файл есть только на машине владельца (не в git)
    }
    let p = Persona::load(&real_dir(), Some(&local)).unwrap();
    assert!(p.replies.warnings().iter().all(|w| !w.contains("replies-lj")), "{:?}", p.replies.warnings());
    let harsh = p.replies.lines(Uncensored, Situation::InsultReply).iter().filter(|l| l.harsh).count();
    assert!(harsh > 0);
    assert!(p.replies.lines(Buddy, Situation::InsultReply).iter().all(|l| !l.harsh), "жёсткие — только «Без цензуры»");
    let mut st = PickState::with_seed(5);
    for _ in 0..40 {
        let l = p.replies.pick(Uncensored, Situation::InsultReply, &mut st, &Vars::new()).unwrap();
        assert!(p.replies.lines(Uncensored, Situation::InsultReply).iter().any(|x| x.text == l && !x.harsh), "без мата жёсткие не выбираются: {l}");
    }
}
