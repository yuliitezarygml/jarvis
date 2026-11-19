//! Russian agreement for mathematical operands. Parse the written digits directly:
//! neither a large integer nor a decimal tail ever passes through floating point.
use num2words2_core::{get_lang_by_key, KwVal, Kwargs};

pub(super) fn integer(raw: &str, case: &str, gender: &str, ordinal: bool) -> Option<String> {
    if raw.len() > 22 {
        return None;
    }
    let n = raw.parse().ok()?;
    let options = Kwargs(vec![
        ("case".into(), KwVal::Str(case.into())),
        ("gender".into(), KwVal::Str(gender.into())),
    ]);
    let ru = get_lang_by_key("ru")?;
    let words = if ordinal {
        ru.to_ordinal_kw(&n, &options)
    } else {
        ru.to_cardinal_kw(&n, &options)
    }.ok()?;
    Some(words.strip_prefix("одной тысяче").map(|s| format!("тысяче{s}")).unwrap_or(words))
}

pub(super) fn digits(raw: &str) -> String {
    const NAMES: [&str; 10] = ["ноль","один","два","три","четыре","пять","шесть","семь","восемь","девять"];
    raw.bytes().map(|b| NAMES[(b - b'0') as usize]).collect::<Vec<_>>().join(" ")
}

fn one(raw: &str) -> bool {
    raw.ends_with('1') && !raw.ends_with("11")
}

pub(super) fn fraction(top: &str, bottom: &str, case: &str) -> Option<String> {
    if bottom.trim_start_matches('0').is_empty() { return None }
    let numerator = integer(top, case, "f", false)?;
    let singular = one(top);
    let gender = if singular { "f" } else { "p" };
    let denominator_case = if singular || case == "d" {case} else {"g"};
    // num2words uses второй for ordinal 2. Russian fractions use вторая/вторых.
    let denominator = integer(bottom, denominator_case, gender, true)?;
    Some(format!("{numerator} {denominator}"))
}

pub(super) fn number(raw: &str, case: &str) -> String {
    let Some((whole, tail)) = raw.split_once(['.', ',']) else {
        return if raw.len() > 1 && raw.starts_with('0') { digits(raw) } else {
            integer(raw, case, "m", false).unwrap_or_else(|| digits(raw))
        };
    };
    let Some(whole_words) = integer(whole, case, "f", false) else {return format!("{} запятая {}",digits(whole),digits(tail))};
    if tail.len() > 9 {return format!("{whole_words} запятая {}", digits(tail))}
    let unit = match (case, one(whole)) {
        ("d", true) | ("g", true) => "целой",
        ("d", false) => "целым",
        (_, true) => "целая",
        _ => "целых",
    };
    let denominator = format!("1{}", "0".repeat(tail.len()));
    match fraction(tail,&denominator,case) {
        Some(frac) => format!("{whole_words} {unit} {frac}"),
        None => format!("{whole_words} запятая {}", digits(tail))
    }
}

pub(super) fn percent(raw: &str, case: &str) -> &'static str {
    if raw.contains(['.', ',']) {return "процента"}
    match (case, one(raw)) {
        ("d", true) => "проценту",
        ("d", false) => "процентам",
        ("g", true) => "процента",
        ("g", false) => "процентов",
        (_, true) => "процент",
        _ if !raw.ends_with("12") && !raw.ends_with("13") && !raw.ends_with("14") && raw.ends_with(['2','3','4']) => "процента",
        _ => "процентов",
    }
}
