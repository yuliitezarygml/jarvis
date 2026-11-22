//! Русские числительные с нуля: количественные (род × падеж), порядковые (род/число × падеж),
//! десятичные дроби, согласование «1 файл / 2 файла / 5 файлов».

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gender {
    Masc,
    Fem,
    Neut,
}

/// Падеж (винительный — для неодушевлённых).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Case {
    Nom,
    Gen,
    Dat,
    Acc,
    Ins,
    Prep,
}

impl Case {
    #[inline]
    fn i(self) -> usize {
        match self {
            Case::Nom => 0,
            Case::Gen => 1,
            Case::Dat => 2,
            Case::Acc => 3,
            Case::Ins => 4,
            Case::Prep => 5,
        }
    }
    pub fn is_direct(self) -> bool {
        matches!(self, Case::Nom | Case::Acc)
    }
}

/// Класс числа для согласования: 1/21/101 — One, 2–4/22–24 — Few, остальное — Many.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountClass {
    One,
    Few,
    Many,
}

pub fn count_class(n: u64) -> CountClass {
    let (d, dd) = (n % 10, n % 100);
    if (11..=14).contains(&dd) {
        CountClass::Many
    } else if d == 1 {
        CountClass::One
    } else if (2..=4).contains(&d) {
        CountClass::Few
    } else {
        CountClass::Many
    }
}

/// Выбор из трёх форм: `plural(5, ["файл", "файла", "файлов"])` → «файлов».
pub fn plural<'a>(n: u64, forms: [&'a str; 3]) -> &'a str {
    match count_class(n) {
        CountClass::One => forms[0],
        CountClass::Few => forms[1],
        CountClass::Many => forms[2],
    }
}

/// «пять файлов», «две папки», «двадцать одна минута».
pub fn with_noun(n: u64, gender: Gender, forms: [&str; 3]) -> String {
    format!("{} {}", cardinal(n, gender, Case::Nom), plural(n, forms))
}

/// Какая форма существительного стоит после числа: (множественное?, падеж формы).
/// Им./вин.: 1 → ед. того же падежа, 2–4 → ед. род., 5+ → мн. род.; косвенные: 1 → ед., иначе мн. того же падежа.
pub fn noun_slot(n: u64, case: Case) -> (bool, Case) {
    let cc = count_class(n);
    if case.is_direct() {
        match cc {
            CountClass::One => (false, case),
            CountClass::Few => (false, Case::Gen),
            CountClass::Many => (true, Case::Gen),
        }
    } else {
        (cc != CountClass::One, case)
    }
}

type Forms = [&'static str; 6];

/// С этого значения (10^15) числа читаются по цифрам.
pub const MAX_WORDS: u64 = 1_000_000_000_000_000;

const ZERO: Forms = ["ноль", "ноля", "нолю", "ноль", "нолём", "ноле"];
const ONE: [Forms; 3] = [
    ["один", "одного", "одному", "один", "одним", "одном"],
    ["одна", "одной", "одной", "одну", "одной", "одной"],
    ["одно", "одного", "одному", "одно", "одним", "одном"],
];
const TWO: Forms = ["два", "двух", "двум", "два", "двумя", "двух"];
const THREE: Forms = ["три", "трёх", "трём", "три", "тремя", "трёх"];
const FOUR: Forms = ["четыре", "четырёх", "четырём", "четыре", "четырьмя", "четырёх"];
const EIGHT: Forms = ["восемь", "восьми", "восьми", "восемь", "восемью", "восьми"];
const FORTY: Forms = ["сорок", "сорока", "сорока", "сорок", "сорока", "сорока"];
const NINETY: Forms = ["девяносто", "девяноста", "девяноста", "девяносто", "девяноста", "девяноста"];
const HUNDREDS: [Forms; 9] = [
    ["сто", "ста", "ста", "сто", "ста", "ста"],
    ["двести", "двухсот", "двумстам", "двести", "двумястами", "двухстах"],
    ["триста", "трёхсот", "трёмстам", "триста", "тремястами", "трёхстах"],
    ["четыреста", "четырёхсот", "четырёмстам", "четыреста", "четырьмястами", "четырёхстах"],
    ["пятьсот", "пятисот", "пятистам", "пятьсот", "пятьюстами", "пятистах"],
    ["шестьсот", "шестисот", "шестистам", "шестьсот", "шестьюстами", "шестистах"],
    ["семьсот", "семисот", "семистам", "семьсот", "семьюстами", "семистах"],
    ["восемьсот", "восьмисот", "восьмистам", "восемьсот", "восемьюстами", "восьмистах"],
    ["девятьсот", "девятисот", "девятистам", "девятьсот", "девятьюстами", "девятистах"],
];
/// Сложные десятки 50–80: «пятьдесят» → «пятидесяти» / «пятьюдесятью».
const TENS_COMPOUND: [Forms; 4] = [
    ["пятьдесят", "пятидесяти", "пятидесяти", "пятьдесят", "пятьюдесятью", "пятидесяти"],
    ["шестьдесят", "шестидесяти", "шестидесяти", "шестьдесят", "шестьюдесятью", "шестидесяти"],
    ["семьдесят", "семидесяти", "семидесяти", "семьдесят", "семьюдесятью", "семидесяти"],
    ["восемьдесят", "восьмидесяти", "восьмидесяти", "восемьдесят", "восемьюдесятью", "восьмидесяти"],
];
/// Числительные на -ь (5–7, 9–20, 30) склоняются как «тетрадь».
const SOFT: [&str; 15] = [
    "пять", "шесть", "семь", "девять", "десять", "одиннадцать", "двенадцать", "тринадцать", "четырнадцать",
    "пятнадцать", "шестнадцать", "семнадцать", "восемнадцать", "девятнадцать", "двадцать",
];

fn soft(nom: &str, case: Case) -> String {
    let stem = nom.strip_suffix('ь').unwrap_or(nom);
    match case {
        Case::Nom | Case::Acc => nom.to_string(),
        Case::Ins => format!("{stem}ью"),
        _ => format!("{stem}и"),
    }
}

fn soft_nom(n: u64) -> &'static str {
    match n {
        5 => SOFT[0],
        6 => SOFT[1],
        7 => SOFT[2],
        9 => SOFT[3],
        10..=20 => SOFT[(n - 6) as usize],
        _ => unreachable!(),
    }
}

/// 0..=19 и десятки — одно слово.
fn small(n: u64, gender: Gender, case: Case) -> String {
    let c = case.i();
    match n {
        0 => ZERO[c].into(),
        1 => ONE[gender as usize][c].into(),
        2 if gender == Gender::Fem && case.is_direct() => "две".into(),
        2 => TWO[c].into(),
        3 => THREE[c].into(),
        4 => FOUR[c].into(),
        8 => EIGHT[c].into(),
        5..=7 | 9..=20 => soft(soft_nom(n), case),
        30 => soft("тридцать", case),
        40 => FORTY[c].into(),
        50 | 60 | 70 | 80 => TENS_COMPOUND[(n / 10 - 5) as usize][c].into(),
        90 => NINETY[c].into(),
        _ => unreachable!("small({n})"),
    }
}

fn triad(n: u64, gender: Gender, case: Case, out: &mut Vec<String>) {
    debug_assert!(n < 1000);
    let (h, r) = (n / 100, n % 100);
    if h > 0 {
        out.push(HUNDREDS[(h - 1) as usize][case.i()].into());
    }
    if r == 0 {
        return;
    }
    if r <= 20 {
        out.push(small(r, gender, case));
    } else {
        out.push(small(r / 10 * 10, gender, case));
        if r % 10 > 0 {
            out.push(small(r % 10, gender, case));
        }
    }
}

/// Склонение существительного по образцу.
#[derive(Debug, Clone)]
pub struct Noun {
    pub sg: [String; 6],
    pub pl: [String; 6],
    pub gender: Gender,
}

impl Noun {
    /// Твёрдый мужской: «процент», «гигабайт» (`zero_gen_pl`: «пять гигабайт»).
    pub fn masc_hard(stem: &str, zero_gen_pl: bool) -> Self {
        let s = stem;
        Self {
            sg: [s.into(), format!("{s}а"), format!("{s}у"), s.into(), format!("{s}ом"), format!("{s}е")],
            pl: [
                format!("{s}ы"),
                if zero_gen_pl { s.into() } else { format!("{s}ов") },
                format!("{s}ам"),
                format!("{s}ы"),
                format!("{s}ами"),
                format!("{s}ах"),
            ],
            gender: Gender::Masc,
        }
    }
    /// Мягкий мужской на -ь: «рубль», «пиксель». `ins` — окончание тв. п. («ём»/«ем»).
    pub fn masc_soft(nom: &str, ins: &str) -> Self {
        let s = nom.strip_suffix('ь').unwrap_or(nom);
        Self {
            sg: [nom.into(), format!("{s}я"), format!("{s}ю"), nom.into(), format!("{s}{ins}"), format!("{s}е")],
            pl: [format!("{s}и"), format!("{s}ей"), format!("{s}ям"), format!("{s}и"), format!("{s}ями"), format!("{s}ях")],
            gender: Gender::Masc,
        }
    }
    /// Женский на -а: «минута», «копейка» (`gen_sg_i`: «копейки», `gen_pl` явно: «копеек»).
    pub fn fem_a(nom: &str, gen_sg_i: bool, gen_pl: &str) -> Self {
        let s = nom.strip_suffix('а').unwrap_or(nom);
        let y = if gen_sg_i { "и" } else { "ы" };
        Self {
            sg: [nom.into(), format!("{s}{y}"), format!("{s}е"), format!("{s}у"), format!("{s}ой"), format!("{s}е")],
            pl: [format!("{s}{y}"), gen_pl.into(), format!("{s}ам"), format!("{s}{y}"), format!("{s}ами"), format!("{s}ах")],
            gender: Gender::Fem,
        }
    }
    /// Несклоняемое: «евро», «промилле».
    pub fn indecl(word: &str, gender: Gender) -> Self {
        Self { sg: std::array::from_fn(|_| word.into()), pl: std::array::from_fn(|_| word.into()), gender }
    }
    pub fn form(&self, plural: bool, case: Case) -> &str {
        if plural { &self.pl[case.i()] } else { &self.sg[case.i()] }
    }
    /// Форма после целого числа `n` в падеже `case`.
    pub fn after(&self, n: u64, case: Case) -> &str {
        let (pl, c) = noun_slot(n, case);
        self.form(pl, c)
    }
}

fn thousand() -> Noun {
    Noun {
        sg: ["тысяча", "тысячи", "тысяче", "тысячу", "тысячей", "тысяче"].map(String::from),
        pl: ["тысячи", "тысяч", "тысячам", "тысячи", "тысячами", "тысячах"].map(String::from),
        gender: Gender::Fem,
    }
}

/// Количественное числительное: `cardinal(2026, Masc, Nom)` → «две тысячи двадцать шесть».
pub fn cardinal(n: u64, gender: Gender, case: Case) -> String {
    if n == 0 {
        return ZERO[case.i()].into();
    }
    if n >= MAX_WORDS {
        return digits(&n.to_string());
    }
    let scales: [(u64, Option<Noun>); 5] = [
        (1_000_000_000_000, Some(Noun::masc_hard("триллион", false))),
        (1_000_000_000, Some(Noun::masc_hard("миллиард", false))),
        (1_000_000, Some(Noun::masc_hard("миллион", false))),
        (1_000, Some(thousand())),
        (1, None),
    ];
    let mut out = Vec::new();
    let mut rest = n;
    for (scale, noun) in scales {
        let t = rest / scale;
        rest %= scale;
        if t == 0 {
            continue;
        }
        match noun {
            Some(noun) => {
                // Ровно «тысяча», «миллион» — без «одна/один».
                if t != 1 {
                    triad(t, noun.gender, case, &mut out);
                }
                out.push(noun.after(t, case).to_string());
            }
            None => triad(t, gender, case, &mut out),
        }
    }
    out.join(" ")
}

/// Цифры по одной: «ноль ноль семь».
pub fn digits(s: &str) -> String {
    s.chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| small(d as u64, Gender::Masc, Case::Nom))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, PartialEq)]
enum Stem {
    /// «первый»: ый/ого/ому…
    Hard,
    /// «второй»: ой в им. п. м. р.
    Stressed,
    /// «третий»
    Third,
}

/// Род/число порядкового: мн. ч. не различает род.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdForm {
    Sg(Gender),
    Pl,
}

fn ord_unit(n: u64) -> (&'static str, Stem) {
    match n {
        1 => ("перв", Stem::Hard),
        2 => ("втор", Stem::Stressed),
        3 => ("трет", Stem::Third),
        4 => ("четвёрт", Stem::Hard),
        5 => ("пят", Stem::Hard),
        6 => ("шест", Stem::Stressed),
        7 => ("седьм", Stem::Stressed),
        8 => ("восьм", Stem::Stressed),
        9 => ("девят", Stem::Hard),
        10 => ("десят", Stem::Hard),
        11 => ("одиннадцат", Stem::Hard),
        12 => ("двенадцат", Stem::Hard),
        13 => ("тринадцат", Stem::Hard),
        14 => ("четырнадцат", Stem::Hard),
        15 => ("пятнадцат", Stem::Hard),
        16 => ("шестнадцат", Stem::Hard),
        17 => ("семнадцат", Stem::Hard),
        18 => ("восемнадцат", Stem::Hard),
        19 => ("девятнадцат", Stem::Hard),
        20 => ("двадцат", Stem::Hard),
        30 => ("тридцат", Stem::Hard),
        40 => ("сороков", Stem::Stressed),
        50 => ("пятидесят", Stem::Hard),
        60 => ("шестидесят", Stem::Hard),
        70 => ("семидесят", Stem::Hard),
        80 => ("восьмидесят", Stem::Hard),
        90 => ("девяност", Stem::Hard),
        100 => ("сот", Stem::Hard),
        200 => ("двухсот", Stem::Hard),
        300 => ("трёхсот", Stem::Hard),
        400 => ("четырёхсот", Stem::Hard),
        500 => ("пятисот", Stem::Hard),
        600 => ("шестисот", Stem::Hard),
        700 => ("семисот", Stem::Hard),
        800 => ("восьмисот", Stem::Hard),
        900 => ("девятисот", Stem::Hard),
        _ => unreachable!("ord_unit({n})"),
    }
}

fn ord_ending(stem: Stem, form: OrdForm, case: Case) -> &'static str {
    const HARD_M: Forms = ["ый", "ого", "ому", "ый", "ым", "ом"];
    const HARD_F: Forms = ["ая", "ой", "ой", "ую", "ой", "ой"];
    const HARD_N: Forms = ["ое", "ого", "ому", "ое", "ым", "ом"];
    const HARD_P: Forms = ["ые", "ых", "ым", "ые", "ыми", "ых"];
    const THIRD_M: Forms = ["ий", "ьего", "ьему", "ий", "ьим", "ьем"];
    const THIRD_F: Forms = ["ья", "ьей", "ьей", "ью", "ьей", "ьей"];
    const THIRD_N: Forms = ["ье", "ьего", "ьему", "ье", "ьим", "ьем"];
    const THIRD_P: Forms = ["ьи", "ьих", "ьим", "ьи", "ьими", "ьих"];
    let c = case.i();
    match (stem, form) {
        (Stem::Third, OrdForm::Sg(Gender::Masc)) => THIRD_M[c],
        (Stem::Third, OrdForm::Sg(Gender::Fem)) => THIRD_F[c],
        (Stem::Third, OrdForm::Sg(Gender::Neut)) => THIRD_N[c],
        (Stem::Third, OrdForm::Pl) => THIRD_P[c],
        (Stem::Stressed, OrdForm::Sg(Gender::Masc)) if case.is_direct() => "ой",
        (_, OrdForm::Sg(Gender::Masc)) => HARD_M[c],
        (_, OrdForm::Sg(Gender::Fem)) => HARD_F[c],
        (_, OrdForm::Sg(Gender::Neut)) => HARD_N[c],
        (_, OrdForm::Pl) => HARD_P[c],
    }
}

/// Первая часть сложных слов: 2 → «двух», 25 → «двадцатипяти», 100 → «сто» («двухтысячный», «стотысячный»).
fn compound_prefix(n: u64) -> String {
    let (h, r) = (n / 100, n % 100);
    let mut s = String::new();
    if h > 0 {
        s.push_str(if h == 1 { "сто" } else { HUNDREDS[(h - 1) as usize][1] });
    }
    let unit = |d: u64| -> String {
        match d {
            1 => "одно".into(),
            90 => "девяносто".into(),
            _ => small(d, Gender::Masc, Case::Gen),
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
    s
}

/// Порядковое: `ordinal(2026, Sg(Masc), Prep)` → «две тысячи двадцать шестом».
pub fn ordinal(n: u64, form: OrdForm, case: Case) -> String {
    if n == 0 {
        return format!("нулев{}", ord_ending(Stem::Stressed, form, case));
    }
    let r = n % 100;
    let h = (n % 1000) / 100;
    let mut words: Vec<String> = Vec::new();
    let push_prefix = |words: &mut Vec<String>, v: u64| {
        if v > 0 {
            words.push(cardinal(v, Gender::Masc, Case::Nom));
        }
    };
    if r != 0 {
        if r <= 20 || r % 10 == 0 {
            push_prefix(&mut words, n - r);
            let (stem, kind) = ord_unit(r);
            words.push(format!("{stem}{}", ord_ending(kind, form, case)));
        } else {
            push_prefix(&mut words, n - r % 10);
            let (stem, kind) = ord_unit(r % 10);
            words.push(format!("{stem}{}", ord_ending(kind, form, case)));
        }
    } else if h != 0 {
        push_prefix(&mut words, n - h * 100);
        let (stem, kind) = ord_unit(h * 100);
        words.push(format!("{stem}{}", ord_ending(kind, form, case)));
    } else {
        // Круглые тысячи/миллионы/миллиарды: «двухтысячный», «миллионный».
        let (scale, word) = if n % 1_000_000_000 == 0 {
            (1_000_000_000, "миллиардн")
        } else if n % 1_000_000 == 0 {
            (1_000_000, "миллионн")
        } else {
            (1_000, "тысячн")
        };
        let k = (n / scale) % 1000;
        push_prefix(&mut words, n - k * scale);
        let prefix = if k == 1 { String::new() } else { compound_prefix(k) };
        words.push(format!("{prefix}{word}{}", ord_ending(Stem::Hard, form, case)));
    }
    words.join(" ")
}

/// Десятичная дробь: «три целых четырнадцать сотых». `frac` — цифры после запятой (≤6, иначе по цифрам).
pub fn decimal(int: u64, frac: &str, case: Case) -> String {
    let frac = frac.trim_end_matches(|c: char| !c.is_ascii_digit());
    if frac.is_empty() || frac.chars().all(|c| c == '0') {
        return cardinal(int, Gender::Masc, case);
    }
    if frac.len() > 6 {
        return format!("{} запятая {}", cardinal(int, Gender::Fem, case), digits(frac));
    }
    let f: u64 = frac.parse().unwrap_or(0);
    let whole = Noun {
        sg: ["целая", "целой", "целой", "целую", "целой", "целой"].map(String::from),
        pl: ["целых", "целых", "целым", "целых", "целыми", "целых"].map(String::from),
        gender: Gender::Fem,
    };
    let denom_stem = ["десят", "сот", "тысячн", "десятитысячн", "стотысячн", "миллионн"][frac.len() - 1];
    let denom = Noun {
        sg: ["ая", "ой", "ой", "ую", "ой", "ой"].map(|e| format!("{denom_stem}{e}")),
        pl: ["ых", "ых", "ым", "ых", "ыми", "ых"].map(|e| format!("{denom_stem}{e}")),
        gender: Gender::Fem,
    };
    // У дробей после 2–4 в им. п. — род. мн. («две целых», «три десятых»), а не род. ед.
    let pick = |noun: &Noun, v: u64| -> String {
        let (pl, c) = noun_slot(v, case);
        let pl = pl || (case.is_direct() && count_class(v) == CountClass::Few);
        noun.form(pl, if pl && case.is_direct() { Case::Gen } else { c }).to_string()
    };
    format!(
        "{} {} {} {}",
        cardinal(int, Gender::Fem, case),
        pick(&whole, int),
        cardinal(f, Gender::Fem, case),
        pick(&denom, f)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use Case::*;
    use Gender::*;

    #[test]
    fn cardinals_nominative() {
        let t = |n| cardinal(n, Masc, Nom);
        assert_eq!(t(0), "ноль");
        assert_eq!(t(7), "семь");
        assert_eq!(t(11), "одиннадцать");
        assert_eq!(t(21), "двадцать один");
        assert_eq!(t(40), "сорок");
        assert_eq!(t(99), "девяносто девять");
        assert_eq!(t(100), "сто");
        assert_eq!(t(115), "сто пятнадцать");
        assert_eq!(t(1000), "тысяча");
        assert_eq!(t(2026), "две тысячи двадцать шесть");
        assert_eq!(t(5000), "пять тысяч");
        assert_eq!(t(21_000), "двадцать одна тысяча");
        assert_eq!(t(1_000_000), "миллион");
        assert_eq!(t(2_500_000), "два миллиона пятьсот тысяч");
        assert_eq!(t(1_234_567_890), "миллиард двести тридцать четыре миллиона пятьсот шестьдесят семь тысяч восемьсот девяносто");
        assert_eq!(cardinal(2, Fem, Nom), "две");
        assert_eq!(cardinal(1, Neut, Nom), "одно");
    }

    #[test]
    fn cardinals_cases() {
        assert_eq!(cardinal(5, Masc, Gen), "пяти");
        assert_eq!(cardinal(8, Masc, Ins), "восемью");
        assert_eq!(cardinal(1, Fem, Acc), "одну");
        assert_eq!(cardinal(25, Masc, Gen), "двадцати пяти");
        assert_eq!(cardinal(50, Masc, Ins), "пятьюдесятью");
        assert_eq!(cardinal(200, Masc, Dat), "двумстам");
        assert_eq!(cardinal(1000, Masc, Gen), "тысячи");
        assert_eq!(cardinal(1000, Masc, Acc), "тысячу");
        assert_eq!(cardinal(3000, Masc, Prep), "трёх тысячах");
        assert_eq!(cardinal(21_000, Masc, Gen), "двадцати одной тысячи");
        assert_eq!(cardinal(90, Masc, Gen), "девяноста");
        assert_eq!(cardinal(144, Masc, Ins), "ста сорока четырьмя");
    }

    #[test]
    fn ordinals() {
        use OrdForm::*;
        assert_eq!(ordinal(1, Sg(Masc), Nom), "первый");
        assert_eq!(ordinal(2, Sg(Masc), Nom), "второй");
        assert_eq!(ordinal(3, Sg(Fem), Nom), "третья");
        assert_eq!(ordinal(3, Sg(Masc), Gen), "третьего");
        assert_eq!(ordinal(22, Sg(Neut), Nom), "двадцать второе");
        assert_eq!(ordinal(22, Sg(Neut), Gen), "двадцать второго");
        assert_eq!(ordinal(40, Sg(Masc), Nom), "сороковой");
        assert_eq!(ordinal(90, Pl, Gen), "девяностых");
        assert_eq!(ordinal(100, Sg(Masc), Nom), "сотый");
        assert_eq!(ordinal(300, Sg(Masc), Nom), "трёхсотый");
        assert_eq!(ordinal(1000, Sg(Masc), Nom), "тысячный");
        assert_eq!(ordinal(2000, Sg(Masc), Gen), "двухтысячного");
        assert_eq!(ordinal(2026, Sg(Masc), Prep), "две тысячи двадцать шестом");
        assert_eq!(ordinal(1999, Sg(Masc), Gen), "тысяча девятьсот девяносто девятого");
        assert_eq!(ordinal(2100, Sg(Masc), Nom), "две тысячи сотый");
        assert_eq!(ordinal(1_000_000, Sg(Masc), Nom), "миллионный");
        assert_eq!(ordinal(0, Sg(Masc), Nom), "нулевой");
    }

    #[test]
    fn decimals() {
        assert_eq!(decimal(3, "14", Nom), "три целых четырнадцать сотых");
        assert_eq!(decimal(0, "5", Nom), "ноль целых пять десятых");
        assert_eq!(decimal(1, "5", Nom), "одна целая пять десятых");
        assert_eq!(decimal(2, "25", Nom), "две целых двадцать пять сотых");
        assert_eq!(decimal(1, "1", Nom), "одна целая одна десятая");
        assert_eq!(decimal(2, "5", Gen), "двух целых пяти десятых");
        assert_eq!(decimal(7, "0", Nom), "семь");
        assert_eq!(decimal(1, "001", Nom), "одна целая одна тысячная");
    }

    #[test]
    fn agreement() {
        assert_eq!(with_noun(1, Masc, ["файл", "файла", "файлов"]), "один файл");
        assert_eq!(with_noun(2, Masc, ["файл", "файла", "файлов"]), "два файла");
        assert_eq!(with_noun(5, Masc, ["файл", "файла", "файлов"]), "пять файлов");
        assert_eq!(with_noun(11, Masc, ["файл", "файла", "файлов"]), "одиннадцать файлов");
        assert_eq!(with_noun(22, Fem, ["папка", "папки", "папок"]), "двадцать две папки");
        assert_eq!(with_noun(101, Fem, ["минута", "минуты", "минут"]), "сто одна минута");
        assert_eq!(plural(112, ["a", "b", "c"]), "c");
        let gb = Noun::masc_hard("гигабайт", true);
        assert_eq!(gb.after(16, Nom), "гигабайт");
        assert_eq!(gb.after(3, Nom), "гигабайта");
        assert_eq!(gb.after(21, Gen), "гигабайта");
        assert_eq!(gb.after(5, Dat), "гигабайтам");
        let rub = Noun::masc_soft("рубль", "ём");
        assert_eq!(rub.after(5, Nom), "рублей");
        assert_eq!(rub.after(2, Nom), "рубля");
        let kop = Noun::fem_a("копейка", true, "копеек");
        assert_eq!(kop.after(3, Nom), "копейки");
        assert_eq!(kop.after(1, Acc), "копейку");
    }
}
