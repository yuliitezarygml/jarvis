//! Нормализация текста и метрики WER/CER для выбора модели и интеграционных тестов.
//!
//! Нормализация (одинаково для эталона и гипотезы): нижний регистр, «ё»→«е», числа цифрами → словами
//! (именительный падеж), «%»→«процентов», «+»→«плюс», дефис склеивает слова («вай-фай»→«вайфай»),
//! прочая пунктуация удаляется, латинские написания частых слов и названия букв приводятся к одному
//! написанию («Wi-Fi»/«вай фай»→«вайфай», «D»/«де»→«дэ»). Ошибки распознавания («джарвиз») не прощаются.

const ALIASES: &[(&str, &str)] = &[
    ("wifi", "вайфай"),
    ("bluetooth", "блютуз"),
    ("youtube", "ютуб"),
    ("chrome", "хром"),
    ("google", "гугл"),
    ("rust", "раст"),
    ("d", "дэ"),
    ("де", "дэ"),
];

/// Слитные написания, которые модели иногда разбивают на два слова.
const COMPOUNDS: &[(&str, &str, &str)] = &[
    ("вай", "фай", "вайфай"),
    ("wi", "fi", "вайфай"),
    ("блю", "туз", "блютуз"),
];

const UNITS: [&str; 10] = [
    "ноль",
    "один",
    "два",
    "три",
    "четыре",
    "пять",
    "шесть",
    "семь",
    "восемь",
    "девять",
];
const UNITS_F: [&str; 3] = ["", "одна", "две"];
const TEENS: [&str; 10] = [
    "десять",
    "одиннадцать",
    "двенадцать",
    "тринадцать",
    "четырнадцать",
    "пятнадцать",
    "шестнадцать",
    "семнадцать",
    "восемнадцать",
    "девятнадцать",
];
const TENS: [&str; 10] = [
    "",
    "",
    "двадцать",
    "тридцать",
    "сорок",
    "пятьдесят",
    "шестьдесят",
    "семьдесят",
    "восемьдесят",
    "девяносто",
];
const HUNDREDS: [&str; 10] = [
    "",
    "сто",
    "двести",
    "триста",
    "четыреста",
    "пятьсот",
    "шестьсот",
    "семьсот",
    "восемьсот",
    "девятьсот",
];

fn plural<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let (d, h) = (n % 10, n % 100);
    if (11..=14).contains(&h) {
        many
    } else if d == 1 {
        one
    } else if (2..=4).contains(&d) {
        few
    } else {
        many
    }
}

fn triad(n: u64, feminine: bool, out: &mut Vec<String>) {
    let (h, t, u) = (n / 100, n / 10 % 10, n % 10);
    if h > 0 {
        out.push(HUNDREDS[h as usize].into());
    }
    if t == 1 {
        out.push(TEENS[u as usize].into());
        return;
    }
    if t > 1 {
        out.push(TENS[t as usize].into());
    }
    if u > 0 {
        if feminine && u <= 2 {
            out.push(UNITS_F[u as usize].into());
        } else {
            out.push(UNITS[u as usize].into());
        }
    }
}

/// Целое число → слова (именительный падеж, мужской род), до 999 999 999 999.
pub fn number_to_words(n: u64) -> Vec<String> {
    if n == 0 {
        return vec![UNITS[0].into()];
    }
    if n > 999_999_999_999 {
        return n
            .to_string()
            .chars()
            .map(|c| UNITS[c as usize - '0' as usize].to_string())
            .collect();
    }
    let mut out = Vec::new();
    let groups = [
        (
            1_000_000_000,
            false,
            ("миллиард", "миллиарда", "миллиардов"),
        ),
        (1_000_000, false, ("миллион", "миллиона", "миллионов")),
        (1_000, true, ("тысяча", "тысячи", "тысяч")),
    ];
    let mut rest = n;
    for (div, fem, (one, few, many)) in groups {
        let g = rest / div;
        if g > 0 {
            triad(g, fem, &mut out);
            out.push(plural(g, one, few, many).into());
        }
        rest %= div;
    }
    triad(rest, false, &mut out);
    out
}

/// Нормализованные слова текста.
pub fn normalize_words(text: &str) -> Vec<String> {
    let mut prepared = String::with_capacity(text.len() + 8);
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '%' => prepared.push_str(" процентов "),
            '+' => prepared.push_str(" плюс "),
            // дефис внутри слова склеивает части: «вай-фай», «wi-fi»
            '-' | '‐' | '‑'
                if i > 0
                    && i + 1 < chars.len()
                    && chars[i - 1].is_alphabetic()
                    && chars[i + 1].is_alphabetic() => {}
            'ё' => prepared.push('е'),
            'Ё' => prepared.push('Е'),
            c if c.is_alphanumeric() => prepared.extend(c.to_lowercase()),
            _ => prepared.push(' '),
        }
    }
    let mut words = Vec::new();
    for w in prepared.split_whitespace() {
        // «7» / «243» / «30»; смешанное «2к» разделяем на число и буквы
        let mut buf = String::new();
        let mut digits = String::new();
        for c in w.chars() {
            if c.is_ascii_digit() {
                if !buf.is_empty() {
                    words.push(std::mem::take(&mut buf));
                }
                digits.push(c);
            } else {
                if !digits.is_empty() {
                    words.extend(number_to_words(digits.parse().unwrap_or(0)));
                    digits.clear();
                }
                buf.push(c);
            }
        }
        if !digits.is_empty() {
            words.extend(number_to_words(digits.parse().unwrap_or(0)));
        }
        if !buf.is_empty() {
            words.push(buf);
        }
    }
    let mut merged: Vec<String> = Vec::with_capacity(words.len());
    for w in words {
        if let Some(prev) = merged.last_mut()
            && let Some((_, _, joined)) = COMPOUNDS
                .iter()
                .find(|(a, b, _)| *a == prev.as_str() && *b == w.as_str())
        {
            *prev = (*joined).to_string();
            continue;
        }
        merged.push(w);
    }
    for w in merged.iter_mut() {
        if let Some((_, to)) = ALIASES.iter().find(|(from, _)| *from == w.as_str()) {
            *w = (*to).to_string();
        }
    }
    merged
}

/// Расстояние Левенштейна между последовательностями.
pub fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(x != y))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Ошибки по словам и символам (символы — без пробелов).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Score {
    pub word_errors: usize,
    pub words: usize,
    pub char_errors: usize,
    pub chars: usize,
}

impl Score {
    pub fn add(&mut self, o: Score) {
        self.word_errors += o.word_errors;
        self.words += o.words;
        self.char_errors += o.char_errors;
        self.chars += o.chars;
    }
    pub fn wer(&self) -> f64 {
        self.word_errors as f64 / self.words.max(1) as f64
    }
    pub fn cer(&self) -> f64 {
        self.char_errors as f64 / self.chars.max(1) as f64
    }
}

pub fn score(reference: &str, hypothesis: &str) -> Score {
    let r = normalize_words(reference);
    let h = normalize_words(hypothesis);
    let rc: Vec<char> = r.concat().chars().collect();
    let hc: Vec<char> = h.concat().chars().collect();
    Score {
        word_errors: edit_distance(&r, &h),
        words: r.len(),
        char_errors: edit_distance(&rc, &hc),
        chars: rc.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number_to_words(7).join(" "), "семь");
        assert_eq!(number_to_words(12).join(" "), "двенадцать");
        assert_eq!(number_to_words(243).join(" "), "двести сорок три");
        assert_eq!(number_to_words(2001).join(" "), "две тысячи один");
        assert_eq!(number_to_words(21_000).join(" "), "двадцать одна тысяча");
        assert_eq!(number_to_words(5_000_000).join(" "), "пять миллионов");
        assert_eq!(number_to_words(0).join(" "), "ноль");
    }

    #[test]
    fn normalization() {
        assert_eq!(
            normalize_words("Джарвис, открой Блокнот!"),
            ["джарвис", "открой", "блокнот"]
        );
        assert_eq!(
            normalize_words("Сколько будет 7 + 5?"),
            ["сколько", "будет", "семь", "плюс", "пять"]
        );
        assert_eq!(normalize_words("Включи вай-фай"), ["включи", "вайфай"]);
        assert_eq!(normalize_words("включи Wi-Fi."), ["включи", "вайфай"]);
        assert_eq!(normalize_words("включи вай фай"), ["включи", "вайфай"]);
        assert_eq!(normalize_words("на диске D"), ["на", "диске", "дэ"]);
        assert_eq!(normalize_words("джарвиз"), ["джарвиз"]); // ошибки не прощаем
        assert_eq!(normalize_words("на 30%"), ["на", "тридцать", "процентов"]);
        assert_eq!(
            normalize_words("Ещё раз — и всё"),
            ["еще", "раз", "и", "все"]
        );
    }

    #[test]
    fn wer_and_cer() {
        assert_eq!(edit_distance(&[1, 2, 3], &[1, 3]), 1);
        assert_eq!(edit_distance::<u8>(&[], &[1, 2]), 2);
        let s = score("Джарвис, открой блокнот.", "джарвис открой блок нот");
        assert_eq!((s.word_errors, s.words), (2, 3)); // замена + вставка
        assert_eq!(s.char_errors, 0);
        let s = score("да сэр", "да сэр");
        assert_eq!(s.wer(), 0.0);
        let mut total = Score::default();
        total.add(score("раз два", "раз"));
        total.add(score("три", "три"));
        assert!((total.wer() - 1.0 / 3.0).abs() < 1e-9);
    }
}
