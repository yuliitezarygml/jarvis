//! Числовые выражения в тексте → слова: телефоны, даты, время, IP/версии, диапазоны, знаки, валюты,
//! проценты и единицы, порядковые «1-й», сложные «5-летний», годы, «22 ноября». Падеж берётся из предлога
//! перед числом («до 5 минут» → «до пяти минут»), род — из единицы или следующего слова.

use regex::{Captures, Regex};
use std::sync::LazyLock;

use super::numbers::{
    Case, CountClass, Gender, MAX_WORDS, Noun, OrdForm, cardinal, count_class, decimal, digits, ordinal,
};

const UNITS: &str = r"%|‰|°C|°С|℃|°F|°|₽|руб\b\.?|\$|€|£|USD\b|EUR\b|RUB\b|Мбит/с|Гбит/с|МБ/с|ГБ\b|Гб\b|GB\b|МБ\b|Мб\b|MB\b|КБ\b|Кб\b|KB\b|ТБ\b|Тб\b|TB\b|ГГц\b|GHz\b|МГц\b|MHz\b|кГц\b|Гц\b|Hz\b|км/ч|м/с|км\b|см\b|мм\b|мс\b|ms\b|м\b|кг\b|сек\b\.?|мин\b\.?|ч\b\.?|кВт\b|Вт\b|W\b|fps\b|FPS\b|px\b|шт\b\.?";

static NUM_RE: LazyLock<Regex> = LazyLock::new(|| {
    let sp = r"[ \x{00A0}\x{202F}]";
    let pattern = format!(
        r"(?P<phone>\+7[\s(]*\d{{3}}[\s)]*\d{{3}}[-\s]?\d{{2}}[-\s]?\d{{2}}\b)|(?P<date>\b(?P<d>\d{{1,2}})\.(?P<mo>\d{{1,2}})\.(?P<y>\d{{4}}|\d{{2}})\b(?:{sp}?(?P<yg>г\.|года\b|год\b))?)|(?P<iso>\b(?P<iy>\d{{4}})-(?P<im>\d{{2}})-(?P<id>\d{{2}})\b)|(?P<time>\b(?P<h>[01]?\d|2[0-3]):(?P<mi>[0-5]\d)(?::(?P<s>[0-5]\d))?\b)|(?P<dotted>\b\d+(?:\.\d+){{2,}}\b)|(?P<range>\b(?P<ra>\d+){sp}?[-–—]{sp}?(?P<rb>\d+)\b(?:{sp}?(?P<runit>{UNITS}))?)|(?P<num>(?:(?P<sign>[-−+]){sp}?)?(?:(?P<cur>[$€£₽]){sp}?)?\b(?P<int>\d{{1,3}}(?:{sp}\d{{3}})+|\d+)(?:[.,](?P<frac>\d+))?(?:-(?P<suf>[а-яё]{{1,3}}))?(?:{sp}?(?P<yabbr>гг?\.))?(?:{sp}?(?P<mult>тыс\b\.?|млн\b\.?|млрд\b\.?|трлн\b\.?))?(?:{sp}?(?P<unit>{UNITS}))?)"
    );
    Regex::new(&pattern).expect("NUM_RE")
});

const MONTHS_GEN: [&str; 12] =
    ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"];

/// Единица измерения после числа.
struct Unit {
    noun: Noun,
    /// Неизменяемый хвост: «градусов *Цельсия*», «километров *в час*».
    tail: &'static str,
    /// Разменная монета для валют (для «12.50 ₽» → «двенадцать рублей пятьдесят копеек»).
    cents: Option<Noun>,
}

fn unit(u: &str) -> Option<Unit> {
    let u = u.trim_end_matches('.');
    let mk = |noun: Noun, tail: &'static str| Some(Unit { noun, tail, cents: None });
    let cur = |noun: Noun, cents: Noun| Some(Unit { noun, tail: "", cents: Some(cents) });
    match u {
        "%" => mk(Noun::masc_hard("процент", false), ""),
        "‰" => mk(Noun::indecl("промилле", Gender::Neut), ""),
        "°C" | "°С" | "℃" => mk(Noun::masc_hard("градус", false), " Цельсия"),
        "°F" => mk(Noun::masc_hard("градус", false), " по Фаренгейту"),
        "°" => mk(Noun::masc_hard("градус", false), ""),
        "₽" | "руб" | "RUB" => cur(Noun::masc_soft("рубль", "ём"), Noun::fem_a("копейка", true, "копеек")),
        "$" | "USD" => cur(Noun::masc_hard("доллар", false), Noun::masc_hard("цент", false)),
        "€" | "EUR" => cur(Noun::indecl("евро", Gender::Masc), Noun::masc_hard("евроцент", false)),
        "£" => cur(Noun::masc_hard("фунт", false), Noun::masc_hard("пенс", false)),
        "ГБ" | "Гб" | "GB" => mk(Noun::masc_hard("гигабайт", true), ""),
        "МБ" | "Мб" | "MB" => mk(Noun::masc_hard("мегабайт", true), ""),
        "КБ" | "Кб" | "KB" => mk(Noun::masc_hard("килобайт", true), ""),
        "ТБ" | "Тб" | "TB" => mk(Noun::masc_hard("терабайт", true), ""),
        "Мбит/с" => mk(Noun::masc_hard("мегабит", true), " в секунду"),
        "Гбит/с" => mk(Noun::masc_hard("гигабит", true), " в секунду"),
        "МБ/с" => mk(Noun::masc_hard("мегабайт", true), " в секунду"),
        "ГГц" | "GHz" => mk(Noun::masc_hard("гигагерц", true), ""),
        "МГц" | "MHz" => mk(Noun::masc_hard("мегагерц", true), ""),
        "кГц" => mk(Noun::masc_hard("килогерц", true), ""),
        "Гц" | "Hz" => mk(Noun::masc_hard("герц", true), ""),
        "км/ч" => mk(Noun::masc_hard("километр", false), " в час"),
        "м/с" => mk(Noun::masc_hard("метр", false), " в секунду"),
        "км" => mk(Noun::masc_hard("километр", false), ""),
        "см" => mk(Noun::masc_hard("сантиметр", false), ""),
        "мм" => mk(Noun::masc_hard("миллиметр", false), ""),
        "м" => mk(Noun::masc_hard("метр", false), ""),
        "кг" => mk(Noun::masc_hard("килограмм", false), ""),
        "мс" | "ms" => mk(Noun::fem_a("миллисекунда", false, "миллисекунд"), ""),
        "сек" => mk(Noun::fem_a("секунда", false, "секунд"), ""),
        "мин" => mk(Noun::fem_a("минута", false, "минут"), ""),
        "ч" => mk(Noun::masc_hard("час", false), ""),
        "кВт" => mk(Noun::masc_hard("киловатт", true), ""),
        "Вт" | "W" => mk(Noun::masc_hard("ватт", true), ""),
        "fps" | "FPS" => mk(Noun::masc_hard("кадр", false), " в секунду"),
        "px" => mk(Noun::masc_soft("пиксель", "ем"), ""),
        "шт" => mk(Noun::fem_a("штука", true, "штук"), ""),
        _ => None,
    }
}

fn multiplier(m: &str) -> Option<Noun> {
    match m.trim_end_matches('.') {
        "тыс" => Some(Noun {
            sg: ["тысяча", "тысячи", "тысяче", "тысячу", "тысячей", "тысяче"].map(String::from),
            pl: ["тысячи", "тысяч", "тысячам", "тысячи", "тысячами", "тысячах"].map(String::from),
            gender: Gender::Fem,
        }),
        "млн" => Some(Noun::masc_hard("миллион", false)),
        "млрд" => Some(Noun::masc_hard("миллиард", false)),
        "трлн" => Some(Noun::masc_hard("триллион", false)),
        _ => None,
    }
}

/// Последнее слово перед числом (только если отделено пробелом), в нижнем регистре, ё→е.
fn prev_word(before: &str) -> String {
    let t = before.trim_end_matches([' ', '\u{00A0}', '\u{202F}', '\t']);
    if t.len() == before.len() {
        return String::new();
    }
    let word: String = t.chars().rev().take_while(|c| c.is_alphabetic()).collect::<Vec<_>>().into_iter().rev().collect();
    word.to_lowercase().replace('ё', "е")
}

/// Первое слово после числа (через пробелы), в нижнем регистре.
fn next_word(after: &str) -> String {
    let t = after.trim_start_matches([' ', '\u{00A0}', '\u{202F}']);
    t.chars().take_while(|c| c.is_alphabetic()).collect::<String>().to_lowercase().replace('ё', "е")
}

fn is_cyr(c: char) -> bool {
    matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё')
}

/// Падеж числа по предлогу перед ним (и окончанию следующего слова там, где предлог двузначен).
pub(crate) fn case_from_prep(prev: &str, next: &str) -> Case {
    let ends = |e: &[&str]| e.iter().any(|x| next.ends_with(x));
    match prev {
        "до" | "от" | "из" | "около" | "более" | "менее" | "больше" | "меньше" | "свыше" | "кроме" | "для" | "без"
        | "у" | "после" | "порядка" | "среди" | "вокруг" | "против" | "возле" | "ради" | "вместо" | "мимо" | "сверх"
        | "выше" | "ниже" | "дольше" | "раньше" | "позже" | "старше" | "моложе" | "дороже" | "дешевле" => Case::Gen,
        "к" | "ко" | "согласно" | "благодаря" | "вопреки" | "навстречу" => Case::Dat,
        "о" | "об" | "обо" | "при" => Case::Prep,
        "над" | "под" | "перед" | "между" => Case::Ins,
        "в" | "во" | "на" => {
            if ends(&["ах", "ях"]) {
                Case::Prep
            } else {
                Case::Acc
            }
        }
        "за" | "через" | "про" | "сквозь" => Case::Acc,
        "с" | "со" => {
            if ends(&["ами", "ями", "ом", "ем", "ой", "ей", "ью"]) {
                Case::Ins
            } else {
                Case::Gen
            }
        }
        _ => Case::Nom,
    }
}

const FEM_SOFT: [&str; 20] = [
    "ночь", "дверь", "мышь", "вещь", "память", "жизнь", "роль", "цель", "связь", "модель", "тетрадь", "смерть",
    "печь", "речь", "мощь", "помощь", "ложь", "рожь", "соль", "боль",
];

/// Род по следующему слову: важен только для 1 (все падежи) и 2 (им./вин.).
pub(crate) fn guess_gender(next: &str, case: Case, n: u64) -> Gender {
    if !next.chars().next().is_some_and(is_cyr) {
        return Gender::Masc;
    }
    let ends = |e: &[&str]| e.iter().any(|x| next.ends_with(x));
    let soft_fem = next.ends_with("сть") || FEM_SOFT.contains(&next);
    match (count_class(n), case) {
        (CountClass::One, Case::Nom) => {
            if ends(&["а", "я"]) && !matches!(next, "папа" | "мужчина" | "дядя" | "юноша") {
                Gender::Fem
            } else if ends(&["о", "е", "ё"]) {
                Gender::Neut
            } else if soft_fem {
                Gender::Fem
            } else {
                Gender::Masc
            }
        }
        (CountClass::One, Case::Acc) => {
            if ends(&["у", "ю"]) {
                Gender::Fem
            } else if ends(&["о", "е"]) {
                Gender::Neut
            } else if soft_fem {
                Gender::Fem
            } else {
                Gender::Masc
            }
        }
        (CountClass::One, Case::Gen) => {
            if ends(&["ы", "и"]) {
                Gender::Fem
            } else {
                Gender::Masc
            }
        }
        (CountClass::One, Case::Dat) => {
            if ends(&["у", "ю"]) {
                Gender::Masc
            } else {
                Gender::Fem
            }
        }
        (CountClass::One, Case::Ins) => {
            if ends(&["ой", "ей", "ою", "ью"]) {
                Gender::Fem
            } else {
                Gender::Masc
            }
        }
        (CountClass::Few, Case::Nom | Case::Acc) => {
            if ends(&["ы", "и"]) {
                Gender::Fem
            } else {
                Gender::Masc
            }
        }
        _ => Gender::Masc,
    }
}

fn parse_int(raw: &str) -> Option<u64> {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    d.parse().ok().filter(|v| *v < MAX_WORDS)
}

/// Целое как слова; ведущие нули и огромные числа — по цифрам.
fn int_words(raw: &str, gender: Gender, case: Case) -> String {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() > 1 && d.starts_with('0') {
        return digits(&d);
    }
    match parse_int(&d) {
        Some(v) => cardinal(v, gender, case),
        None => digits(&d),
    }
}

fn date_words(d: u64, mo: u64, y: Option<u64>, case: Case) -> Option<String> {
    if !(1..=31).contains(&d) || !(1..=12).contains(&mo) {
        return None;
    }
    let day_case = if case == Case::Acc { Case::Nom } else { case };
    let mut s = format!("{} {}", ordinal(d, OrdForm::Sg(Gender::Neut), day_case), MONTHS_GEN[(mo - 1) as usize]);
    if let Some(y) = y {
        s.push(' ');
        s.push_str(&ordinal(y, OrdForm::Sg(Gender::Masc), Case::Gen));
        s.push_str(" года");
    }
    Some(s)
}

/// Порядковое по суффиксу «-й», «-го», «-х»… Возвращает None, если суффикс не порядковый.
fn suffixed(n: u64, suf: &str, prev: &str) -> Option<String> {
    use Case::*;
    let last = n % 10;
    let teen = (11..=19).contains(&(n % 100));
    let stressed = !teen && matches!(last, 2 | 6 | 7 | 8) || (n % 100 == 40);
    let card = |case| cardinal(n, Gender::Masc, case);
    let ord = |form, case| ordinal(n, form, case);
    let masc = OrdForm::Sg(Gender::Masc);
    Some(match suf {
        "й" | "ый" | "ий" => ord(masc, Nom),
        "ой" if stressed => ord(masc, Nom),
        "ой" => ord(OrdForm::Sg(Gender::Fem), Gen),
        "я" | "ая" | "ья" => ord(OrdForm::Sg(Gender::Fem), Nom),
        "ью" if last == 3 && !teen => ord(OrdForm::Sg(Gender::Fem), Acc),
        "ью" | "мя" => card(Ins),
        "ю" | "ую" => ord(OrdForm::Sg(Gender::Fem), Acc),
        "е" | "ое" | "ье" | "ее" => ord(OrdForm::Sg(Gender::Neut), Nom),
        "ые" | "ьи" => ord(OrdForm::Pl, Nom),
        "го" | "ого" | "его" | "ьего" => ord(masc, Gen),
        "му" | "ому" | "ему" => ord(masc, Dat),
        "м" | "ом" | "ем" => {
            if matches!(prev, "в" | "во" | "на" | "о" | "об" | "при") {
                ord(masc, Prep)
            } else {
                ord(masc, Ins)
            }
        }
        "ым" | "им" => ord(masc, Ins),
        "х" if !teen && (2..=4).contains(&last) && n < 10 => card(Gen),
        "х" | "ых" | "их" => ord(OrdForm::Pl, Gen),
        "ти" | "ми" | "и" => card(Gen),
        "ыми" | "ими" => ord(OrdForm::Pl, Ins),
        _ => return None,
    })
}

/// Первая часть сложного слова: «5-летний» → «пяти» + «летний».
fn compound_prefix(n: u64) -> Option<String> {
    if n == 0 || n >= 1000 {
        return None;
    }
    let (h, r) = (n / 100, n % 100);
    let mut s = String::new();
    if h > 0 {
        s.push_str(if h == 1 { "сто" } else { ["двух", "трёх", "четырёх", "пяти", "шести", "семи", "восьми", "девяти"][(h - 2) as usize] });
        if h > 1 {
            s.push_str("сот");
        }
    }
    let unit = |d: u64| -> String {
        match d {
            1 => "одно".into(),
            2 => "двух".into(),
            3 => "трёх".into(),
            4 => "четырёх".into(),
            40 => "сорока".into(),
            90 => "девяносто".into(),
            100 => "сто".into(),
            _ => cardinal(d, Gender::Masc, Case::Gen),
        }
    };
    if r > 0 {
        if r <= 20 || r % 10 == 0 {
            s.push_str(&unit(r));
        } else {
            s.push_str(&unit(r / 10 * 10));
            s.push_str(&unit(r % 10));
        }
    }
    Some(s)
}

/// Год по слову «год/года/году…» после числа: (форма порядкового, падеж).
fn year_form(word: &str, prev: &str) -> Option<(OrdForm, Case)> {
    let masc = OrdForm::Sg(Gender::Masc);
    Some(match word {
        "год" => (masc, Case::Nom),
        "года" => (masc, Case::Gen),
        "году" => (masc, if matches!(prev, "к" | "по") { Case::Dat } else { Case::Prep }),
        "годом" => (masc, Case::Ins),
        "годах" => (OrdForm::Pl, Case::Prep),
        "годов" => (OrdForm::Pl, Case::Gen),
        "годы" => (OrdForm::Pl, Case::Nom),
        _ => return None,
    })
}

pub fn replace_numbers(text: &str) -> String {
    NUM_RE.replace_all(text, |c: &Captures| expand(text, c)).into_owned()
}

fn expand(text: &str, c: &Captures) -> String {
    let m = c.get(0).unwrap();
    let before = &text[..m.start()];
    let after = &text[m.end()..];
    let prev = prev_word(before);
    let next = next_word(after);
    let prev_char = before.chars().next_back();
    let next_char = after.chars().next();
    let case = case_from_prep(&prev, &next);
    let mut out = if c.name("phone").is_some() {
        phone(m.as_str())
    } else if c.name("date").is_some() {
        let n = |k: &str| c.name(k).and_then(|x| x.as_str().parse::<u64>().ok());
        let y = n("y").map(|y| if y < 100 { 2000 + y } else { y });
        match date_words(n("d").unwrap(), n("mo").unwrap(), y, case) {
            Some(s) => s,
            None => dotted(m.as_str()),
        }
    } else if c.name("iso").is_some() {
        let n = |k: &str| c[k].parse::<u64>().unwrap_or(0);
        date_words(n("id"), n("im"), Some(n("iy")), case).unwrap_or_else(|| m.as_str().replace('-', " "))
    } else if c.name("time").is_some() {
        time(c, case)
    } else if c.name("dotted").is_some() {
        dotted(m.as_str())
    } else if c.name("range").is_some() {
        range(c, case, &next)
    } else {
        number(c, case, &prev, &next, before, after)
    };
    // Точку сокращения («г.», «мин.», «тыс.») съели вместе с единицей — если это был конец предложения, вернуть.
    if m.as_str().ends_with('.')
        && (after.trim().is_empty() || after.starts_with(char::is_whitespace) && after.trim_start().starts_with(char::is_uppercase))
    {
        out.push('.');
    }
    // Не склеивать слова с соседними буквами: «5G» → «пять G». Сложные слова («пяти|летний») — склеены намеренно.
    if next_char.is_some_and(char::is_alphabetic) && c.name("suf").is_none() {
        out.push(' ');
    }
    // Дефис после буквы («COVID-19») остаётся слитным, остальное отделяем пробелом.
    if prev_char.is_some_and(char::is_alphanumeric) && !out.starts_with(['-', '−', '+']) {
        out.insert(0, ' ');
    }
    out
}

fn phone(raw: &str) -> String {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut w = vec!["плюс семь".to_string()];
    for part in [&d[1..4], &d[4..7], &d[7..9], &d[9..11]] {
        w.push(int_words(part, Gender::Masc, Case::Nom));
    }
    w.join(" ")
}

fn dotted(raw: &str) -> String {
    raw.split('.').map(|p| int_words(p, Gender::Masc, Case::Nom)).collect::<Vec<_>>().join(" точка ")
}

fn time(c: &Captures, case: Case) -> String {
    let h: u64 = c["h"].parse().unwrap_or(0);
    let mi: u64 = c["mi"].parse().unwrap_or(0);
    if let Some(s) = c.name("s") {
        let s: u64 = s.as_str().parse().unwrap_or(0);
        let hn = Noun::masc_hard("час", false);
        let mn = Noun::fem_a("минута", false, "минут");
        let sn = Noun::fem_a("секунда", false, "секунд");
        return format!(
            "{} {} {} {} {} {}",
            cardinal(h, Gender::Masc, case),
            hn.after(h, case),
            cardinal(mi, Gender::Fem, case),
            mn.after(mi, case),
            cardinal(s, Gender::Fem, case),
            sn.after(s, case)
        );
    }
    let hw = cardinal(h, Gender::Masc, case);
    match mi {
        0 => format!("{hw} ноль-ноль"),
        1..=9 => format!("{hw} ноль {}", cardinal(mi, Gender::Fem, Case::Nom)),
        _ => format!("{hw} {}", cardinal(mi, Gender::Fem, case)),
    }
}

fn range(c: &Captures, case: Case, next: &str) -> String {
    let (ra, rb) = (&c["ra"], &c["rb"]);
    let b = parse_int(rb).unwrap_or(0);
    let u = c.name("runit").and_then(|u| unit(u.as_str()));
    let gender = u.as_ref().map(|u| u.noun.gender).unwrap_or_else(|| guess_gender(next, case, b));
    let mut s = format!("{}-{}", int_words(ra, gender, case), int_words(rb, gender, case));
    if let Some(u) = u {
        s.push(' ');
        s.push_str(u.noun.after(b, case));
        s.push_str(u.tail);
    }
    s
}

fn number(c: &Captures, case: Case, prev: &str, next: &str, before: &str, after: &str) -> String {
    let prev_char = before.chars().next_back();
    let prev_nonspace = before.trim_end().chars().next_back();
    let raw_int = &c["int"];
    let frac = c.name("frac").map(|f| f.as_str());
    let suf = c.name("suf").map(|s| s.as_str());
    let mult = c.name("mult").and_then(|m| multiplier(m.as_str()));
    let unit_m = c.name("unit").map(|u| u.as_str());
    let prefix_cur = c.name("cur").and_then(|u| unit(u.as_str()));
    let mut un = unit_m.and_then(unit);
    if un.is_none() {
        un = prefix_cur;
    }
    let body = number_body(c, parse_int(raw_int), raw_int, frac, suf, mult, un, case, prev, next, after);
    // Знак: в начале/после пробела или скобки — «минус/плюс», после цифры — операция, после буквы — дефис.
    let Some(sign) = c.name("sign") else {
        return body;
    };
    let unary = prev_char.is_none_or(|ch| ch.is_whitespace() || "([{=:;,".contains(ch));
    let after_digit = prev_char.is_some_and(|ch| ch.is_ascii_digit()) || prev_nonspace.is_some_and(|ch| ch.is_ascii_digit());
    // «Ответ - 42»: дефис с пробелами после слова — это тире, а не минус.
    let spaced = c.name("int").is_some_and(|i| i.start() > sign.end()) && c.name("cur").is_none();
    if spaced && !after_digit && sign.as_str() != "+" {
        return format!("— {body}");
    }
    match sign.as_str() {
        "+" if unary || after_digit => format!("плюс {body}"),
        "-" | "−" if unary || after_digit => format!("минус {body}"),
        s => format!("{s}{body}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn number_body(
    c: &Captures,
    n: Option<u64>,
    raw_int: &str,
    frac: Option<&str>,
    suf: Option<&str>,
    mult: Option<Noun>,
    un: Option<Unit>,
    case: Case,
    prev: &str,
    next: &str,
    after: &str,
) -> String {
    let Some(n) = n else {
        return digits(raw_int);
    };
    // Порядковые и сложные слова с суффиксом через дефис.
    if let (Some(suf), None) = (suf, frac) {
        let suffix_is_word_end = !after.starts_with(char::is_alphabetic);
        if suffix_is_word_end && let Some(s) = suffixed(n, suf, prev) {
            return s;
        }
        return match compound_prefix(n) {
            Some(p) => format!("{p}{suf}"),
            None => format!("{}-{suf}", cardinal(n, Gender::Masc, Case::Nom)),
        };
    }
    // Год: «2026 г.», «в 2026 году», «2026 год».
    if let Some(y) = c.name("yabbr") {
        let plural = y.as_str() == "гг.";
        if (1000..=2999).contains(&n) && frac.is_none() {
            let yc = match case {
                Case::Acc if matches!(prev, "в" | "во") => Case::Prep,
                Case::Acc => Case::Nom,
                other => other,
            };
            let form = if plural { OrdForm::Pl } else { OrdForm::Sg(Gender::Masc) };
            let noun = if plural {
                ["годы", "годов", "годам", "годы", "годами", "годах"]
            } else {
                ["год", "года", "году", "год", "годом", "году"]
            };
            let idx = [Case::Nom, Case::Gen, Case::Dat, Case::Acc, Case::Ins, Case::Prep].iter().position(|x| *x == yc).unwrap();
            return format!("{} {}", ordinal(n, form, yc), noun[idx]);
        }
        // Иначе это граммы.
        let g = Noun::masc_hard("грамм", true);
        return format!("{} {}", cardinal(n, Gender::Masc, case), g.after(n, case));
    }
    if let Some(frac) = frac {
        let frac_val = frac.trim_end_matches('0');
        // Валюта с копейками: «12.50 ₽».
        if let (Some(u), None, 2) = (&un, &mult, frac.len())
            && let Some(cents) = &u.cents
        {
            let cv: u64 = frac.parse().unwrap_or(0);
            let mut s = format!("{} {}", cardinal(n, u.noun.gender, case), u.noun.after(n, case));
            if cv > 0 {
                s.push_str(&format!(" {} {}", cardinal(cv, cents.gender, case), cents.after(cv, case)));
            }
            return s;
        }
        if frac_val.is_empty() {
            // «7.0» — читаем как целое.
            return number_body(c, Some(n), raw_int, None, None, mult, un, case, prev, next, after);
        }
        let mut s = decimal(n, frac, case);
        // После дроби — род. ед.: «1,5 гигабайта», «2,5 миллиона рублей».
        let gen_sg = |noun: &Noun| noun.form(false, if case.is_direct() { Case::Gen } else { case }).to_string();
        if let Some(m) = &mult {
            s.push(' ');
            s.push_str(&gen_sg(m));
            if let Some(u) = &un {
                s.push(' ');
                s.push_str(u.noun.form(true, Case::Gen));
                s.push_str(u.tail);
            }
        } else if let Some(u) = &un {
            s.push(' ');
            s.push_str(&gen_sg(&u.noun));
            s.push_str(u.tail);
        }
        return s;
    }
    if let Some(m) = &mult {
        let mut s = format!("{} {}", cardinal(n, m.gender, case), m.after(n, case));
        if let Some(u) = &un {
            s.push(' ');
            s.push_str(u.noun.form(true, if case.is_direct() { Case::Gen } else { case }));
            s.push_str(u.tail);
        }
        return s;
    }
    if let Some(u) = &un {
        return format!("{} {}{}", cardinal(n, u.noun.gender, case), u.noun.after(n, case), u.tail);
    }
    // «22 ноября» → «двадцать второе ноября».
    if (1..=31).contains(&n) && MONTHS_GEN.contains(&next) && !raw_int.starts_with('0') {
        let dc = if case == Case::Acc { Case::Nom } else { case };
        return ordinal(n, OrdForm::Sg(Gender::Neut), dc);
    }
    if let Some((form, yc)) = year_form(next, prev) {
        return ordinal(n, form, yc);
    }
    if raw_int.len() > 1 && raw_int.starts_with('0') {
        return digits(raw_int);
    }
    // «Подожди 1 минуту»: без предлога, но существительное в вин. п. — числительное тоже.
    let case = if case == Case::Nom && count_class(n) == CountClass::One && next.ends_with(['у', 'ю']) {
        Case::Acc
    } else {
        case
    };
    cardinal(n, guess_gender(next, case, n), case)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_helpers() {
        assert_eq!(prev_word("ждите до "), "до");
        assert_eq!(prev_word("до"), "");
        assert_eq!(next_word("  Минут осталось"), "минут");
        assert_eq!(case_from_prep("с", "файлами"), Case::Ins);
        assert_eq!(case_from_prep("с", "утра"), Case::Gen);
        assert_eq!(case_from_prep("в", "файлах"), Case::Prep);
        assert_eq!(guess_gender("минута", Case::Nom, 21), Gender::Fem);
        assert_eq!(guess_gender("окно", Case::Nom, 1), Gender::Neut);
        assert_eq!(guess_gender("папки", Case::Nom, 2), Gender::Fem);
        assert_eq!(guess_gender("файла", Case::Nom, 2), Gender::Masc);
        assert_eq!(compound_prefix(5).unwrap(), "пяти");
        assert_eq!(compound_prefix(1).unwrap(), "одно");
        assert_eq!(compound_prefix(100).unwrap(), "сто");
        assert_eq!(compound_prefix(25).unwrap(), "двадцатипяти");
    }
}
