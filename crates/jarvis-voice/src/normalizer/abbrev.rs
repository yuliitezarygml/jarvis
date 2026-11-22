//! Словарь сокращений: латинские аббревиатуры → как их произносят по-русски, русские сокращения с точками.
//! Регистр важен («ПО» ≠ «по»), поэтому совпадение строгое; английские слова не трогаем.

use regex::{Captures, Regex};
use std::sync::LazyLock;

/// (как пишут, как читать). Совпадение по границам слов, с учётом регистра.
pub const ACRONYMS: &[(&str, &str)] = &[
    ("Wi-Fi", "вай-фай"),
    ("WiFi", "вай-фай"),
    ("Wi‑Fi", "вай-фай"),
    ("wifi", "вай-фай"),
    ("WI-FI", "вай-фай"),
    ("VPN", "ви-пи-эн"),
    ("CPU", "си-пи-ю"),
    ("GPU", "джи-пи-ю"),
    ("NPU", "эн-пи-ю"),
    ("RAM", "рам"),
    ("ROM", "ром"),
    ("USB", "ю-эс-би"),
    ("URL", "ю-ар-эл"),
    ("IP", "ай-пи"),
    ("PDF", "пи-ди-эф"),
    ("HTTPS", "эйч-ти-ти-пи-эс"),
    ("HTTP", "эйч-ти-ти-пи"),
    ("API", "эй-пи-ай"),
    ("SSD", "эс-эс-ди"),
    ("HDD", "эйч-ди-ди"),
    ("NVMe", "эн-ви-эм-и"),
    ("DNS", "ди-эн-эс"),
    ("LAN", "лан"),
    ("AI", "эй-ай"),
    ("LLM", "эл-эл-эм"),
    ("GPT", "джи-пи-ти"),
    ("SQL", "эс-кью-эл"),
    ("JSON", "джейсон"),
    ("HTML", "эйч-ти-эм-эл"),
    ("CSS", "си-эс-эс"),
    ("XML", "икс-эм-эл"),
    ("HDMI", "эйч-ди-эм-ай"),
    ("BIOS", "биос"),
    ("UEFI", "ю-е-эф-ай"),
    ("ID", "ай-ди"),
    ("PIN", "пин"),
    ("SMS", "эс-эм-эс"),
    ("TTS", "ти-ти-эс"),
    ("STT", "эс-ти-ти"),
    ("OS", "о-эс"),
    ("PC", "пи-си"),
    ("UI", "ю-ай"),
    ("IT", "ай-ти"),
    ("OK", "окей"),
    ("FPS", "эф-пи-эс"),
    ("FTP", "эф-ти-пи"),
    ("SSH", "эс-эс-эйч"),
    ("SSL", "эс-эс-эль"),
    ("TLS", "ти-эл-эс"),
    ("RGB", "ар-джи-би"),
    ("GPS", "джи-пи-эс"),
    ("NFC", "эн-эф-си"),
    ("LTE", "эл-ти-и"),
    ("DVD", "ди-ви-ди"),
    ("CD", "си-ди"),
    ("TV", "ти-ви"),
    ("VR", "ви-ар"),
    ("PNG", "пи-эн-джи"),
    ("JPEG", "джейпег"),
    ("JPG", "джейпег"),
    ("ZIP", "зип"),
    ("PCIe", "пи-си-ай-е"),
    ("DDR", "ди-ди-ар"),
    ("AMD", "эй-эм-ди"),
    ("FAQ", "эф-эй-кью"),
    ("IoT", "ай-о-ти"),
    ("e-mail", "имейл"),
    ("email", "имейл"),
    ("ИИ", "и-и"),
    ("ПК", "пэ-ка"),
    ("ПО", "пэ-о"),
    ("ОС", "о-эс"),
    ("ОЗУ", "о-зэ-у"),
    ("СМС", "эс-эм-эс"),
    ("США", "сэ-шэ-а"),
    ("РФ", "эр-эф"),
];

/// Русские сокращения с точками: (регулярка, расшифровка).
const DOTTED: &[(&str, &str)] = &[
    (r"т\.\s?е\.", "то есть"),
    (r"т\.\s?к\.", "так как"),
    (r"т\.\s?д\.", "так далее"),
    (r"т\.\s?п\.", "тому подобное"),
    (r"т\.\s?н\.", "так называемый"),
    (r"т\.\s?ч\.", "том числе"),
    (r"и\s?др\.", "и другие"),
    (r"и\s?пр\.", "и прочее"),
    (r"напр\.", "например"),
    (r"см\.", "смотри"),
    (r"стр\.", "страница"),
    (r"рис\.", "рисунок"),
    (r"прим\.", "примечание"),
    (r"ул\.", "улица"),
    (r"г-н", "господин"),
    (r"г-жа", "госпожа"),
];

static RE_ACRONYM: LazyLock<Regex> = LazyLock::new(|| {
    let mut keys: Vec<&str> = ACRONYMS.iter().map(|(k, _)| *k).collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.chars().count()));
    let alt: Vec<String> = keys.iter().map(|k| regex::escape(k)).collect();
    // Границы вручную: дефис внутри «Wi-Fi» допустим, а снаружи слово не должно продолжаться буквой/цифрой.
    Regex::new(&format!(r"(^|[^\p{{L}}\p{{N}}\-])({})($|[^\p{{L}}\p{{N}}])", alt.join("|"))).unwrap()
});

static RE_DOTTED: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    DOTTED
        .iter()
        .map(|(p, r)| (Regex::new(&format!(r"(?i:^|\b){p}")).unwrap(), *r))
        .collect()
});

fn expand_acronyms(text: &str) -> String {
    // Соседние совпадения делят разделитель — прогоняем дважды («CPU GPU»).
    let mut s = text.to_string();
    for _ in 0..2 {
        s = RE_ACRONYM
            .replace_all(&s, |c: &Captures| {
                let word = ACRONYMS.iter().find(|(k, _)| *k == &c[2]).map(|(_, v)| *v).unwrap_or("");
                format!("{}{}{}", &c[1], word, &c[3])
            })
            .into_owned();
    }
    s
}

fn expand_dotted(text: &str) -> String {
    let mut s = text.to_string();
    for (re, word) in RE_DOTTED.iter() {
        let hay = std::mem::take(&mut s);
        s = re
            .replace_all(&hay, |c: &Captures| {
                let m = c.get(0).unwrap();
                let after = &hay[m.end()..];
                // Сокращение в конце предложения «… и т. д. Потом» — точку сохраняем.
                let ends_sentence = m.as_str().ends_with('.')
                    && (after.trim().is_empty()
                        || after.starts_with(char::is_whitespace)
                            && after.trim_start().starts_with(|ch: char| ch.is_uppercase()));
                if ends_sentence { format!("{word}.") } else { word.to_string() }
            })
            .into_owned();
    }
    s
}

pub fn expand(text: &str) -> String {
    expand_acronyms(&expand_dotted(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acronyms_with_boundaries() {
        assert_eq!(expand("Включи Wi-Fi и VPN"), "Включи вай-фай и ви-пи-эн");
        assert_eq!(expand("CPU GPU RAM"), "си-пи-ю джи-пи-ю рам");
        assert_eq!(expand("по ПО"), "по пэ-о");
        assert_eq!(expand("APIs и VPNка"), "APIs и VPNка");
        assert_eq!(expand("(USB)"), "(ю-эс-би)");
    }

    #[test]
    fn dotted() {
        assert_eq!(expand("то, т. е. это"), "то, то есть это");
        assert_eq!(expand("хлеб, молоко и т. д. Потом"), "хлеб, молоко и так далее. Потом");
        assert_eq!(expand("и т.п. и пр."), "и тому подобное и прочее.");
        assert_eq!(expand("см. выше"), "смотри выше");
    }
}
