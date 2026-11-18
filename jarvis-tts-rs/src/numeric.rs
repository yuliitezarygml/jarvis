//! Expand explicit phone and money formats before RUNorm predicts grammatical case.
use regex::{Captures, Regex};
use std::sync::LazyLock;

static PHONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+7\s*\(?[0-9]{3}\)?\s*[0-9]{3}[-\s]?[0-9]{2}[-\s]?[0-9]{2}|\b8[0-9]{10}\b)")
        .unwrap()
});
static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:([$£])([0-9]+(?:[.,][0-9]{2})?)|([0-9]+(?:[.,][0-9]{2})?)\s*(руб(?:лей|ля|ль)?|₽|RUB|доллар(?:ов|а|ы)?|\$|USD|евро|€|EUR|фунт(?:ов|а|ы)?|£|GBP|грив(?:ен|ны|на)|₴|UAH))").unwrap()
});
fn plural(value: u64) -> usize {
    if (11..=14).contains(&(value % 100)) {
        2
    } else {
        match value % 10 {
            1 => 0,
            2..=4 => 1,
            _ => 2,
        }
    }
}
fn words(value: u64) -> String {
    num2words2_core::to_words(value as i64, "ru").expect("Russian cardinals are supported")
}
pub fn expand(text: &str) -> String {
    let currency = AMOUNT.replace_all(text, |c: &Captures| {
        let (raw, unit) = if let Some(sign) = c.get(1) {
            (&c[2], sign.as_str())
        } else {
            (&c[3], &c[4])
        };
        let (whole, cents) = raw.split_once(['.', ',']).unwrap_or((raw, "0"));
        let Ok(whole) = whole.parse::<u64>() else {
            return c[0].into();
        };
        if whole > 999_999_999_999 {
            return c[0].into();
        }
        let cents = cents.parse::<u64>().unwrap_or(0);
        let unit = unit.to_lowercase();
        let (main, sub) = match unit.as_str() {
            "₽" | "rub" | "руб" | "рубль" | "рубля" | "рублей" => (
                ["рубль", "рубля", "рублей"],
                ["копейка", "копейки", "копеек"],
            ),
            "€" | "eur" | "евро" => (["евро"; 3], ["евроцент", "евроцента", "евроцентов"]),
            "£" | "gbp" | "фунт" | "фунта" | "фунтов" | "фунты" => {
                (["фунт", "фунта", "фунтов"], ["пенс", "пенса", "пенсов"])
            }
            "₴" | "uah" | "гривна" | "гривны" | "гривен" => (
                ["гривна", "гривны", "гривен"],
                ["копейка", "копейки", "копеек"],
            ),
            _ => (
                ["доллар", "доллара", "долларов"],
                ["цент", "цента", "центов"],
            ),
        };
        let mut value = format!("{} {}", words(whole), main[plural(whole)]);
        if cents > 0 {
            value.push_str(&format!(" {} {}", words(cents), sub[plural(cents)]));
        }
        value
    });
    PHONE
        .replace_all(&currency, |c: &Captures| {
            let digits = c[0]
                .bytes()
                .filter(u8::is_ascii_digit)
                .map(char::from)
                .collect::<String>();
            let mut words_out = vec![if digits.starts_with('7') {
                "плюс семь".into()
            } else {
                "восемь".into()
            }];
            for part in [&digits[1..4], &digits[4..7], &digits[7..9], &digits[9..11]] {
                words_out.push(words(part.parse().expect("matched ASCII digits")));
            }
            words_out.join(" ")
        })
        .into_owned()
}
#[cfg(test)]
mod tests {
    use super::expand;
    #[test]
    fn currency_decimals_and_plural() {
        assert_eq!(expand("12.05 ₽"), "двенадцать рублей пять копеек");
        assert_eq!(expand("$21"), "двадцать один доллар");
    }
    #[test]
    fn phone_preserves_grouping() {
        assert_eq!(
            expand("+7 (999) 123-45-67"),
            "плюс семь девятьсот девяносто девять сто двадцать три сорок пять шестьдесят семь"
        );
    }
}
