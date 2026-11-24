//! Английские слова → как их произносят по-русски (голос Джарвиса русский и латиницу читает плохо).
//!
//! 1. Словарь частых названий (программы, сервисы, бренды, IT-слова): «Discord» → «Дискорд».
//! 2. Остальное — правила транскрипции по буквосочетаниям: «Telegram» → «Телеграм», «update» → «апдейт».
//! 3. Короткие слова ЗАГЛАВНЫМИ (аббревиатуры не из словаря `abbrev`) — по буквам: «OBS» → «о-би-эс».
//!
//! 4. Ударение: в словаре `+` после ударной гласной («па+уэршэл» — ударная «а»), у слов по правилам — первый слог (как чаще
//!    всего в английском). На выходе `+` превращается в знак ударения U+0301 — сервер голоса берёт его как есть,
//!    поэтому английские слова звучат ближе к оригиналу, а не с русским ударением.
//! 5. Имена файлов: «config.json» → «ко́нфиг точка джэ́йсон».
//!
//! Слова с цифрами (`i7`, `x64`) и кириллица не трогаются.

use regex::{Captures, Regex};
use std::sync::LazyLock;

/// Фразы из нескольких слов (сначала они, потом одиночные слова). Ключ — нижний регистр.
const PHRASES: &[(&str, &str)] = &[
    ("visual studio code", "ви+жуал сту+дио код"),
    ("vs code", "ви-эс код"),
    ("google chrome", "гугл хром"),
    ("microsoft edge", "майкрософт эдж"),
    ("task manager", "диспетчер задач"),
    ("open ai", "опен эй-ай"),
    ("iron man", "айрон мэн"),
    ("tony stark", "тони старк"),
    ("chat gpt", "чат джи-пи-ти"),
    ("pull request", "пул риквэ+ст"),
];

/// Словарь: нижний регистр → чтение.
const WORDS: &[(&str, &str)] = &[
    // программы и сервисы
    ("discord", "ди+скорд"), ("chrome", "хром"), ("google", "гу+гл"), ("youtube", "ю+туб"), ("windows", "ви+ндоус"),
    ("microsoft", "ма+йкрософт"), ("github", "ги+тхаб"), ("gitlab", "ги+тлаб"), ("git", "гит"), ("telegram", "те+леграм"),
    ("steam", "стим"), ("spotify", "спо+тифай"), ("firefox", "фа+йрфокс"), ("edge", "эдж"), ("opera", "опера"),
    ("word", "ворд"), ("excel", "э+ксель"), ("powerpoint", "па+уэрпоинт"), ("outlook", "а+утлук"), ("teams", "тимс"),
    ("zoom", "зум"), ("skype", "скайп"), ("whatsapp", "ва+тсап"), ("vscode", "ви-эс код"), ("visual", "ви+жуал"),
    ("studio", "сту+дио"), ("code", "код"), ("notepad", "но+утпад"), ("python", "па+йтон"), ("rust", "раст"),
    ("javascript", "джа+васкрипт"), ("typescript", "та+йпскрипт"), ("java", "джа+ва"), ("linux", "ли+нукс"),
    ("ubuntu", "убу+нту"), ("apple", "э+пл"), ("iphone", "а+йфон"), ("android", "а+ндроид"), ("nvidia", "энви+диа"),
    ("intel", "и+нтел"), ("amazon", "а+мазон"), ("netflix", "не+тфликс"), ("twitch", "твич"), ("twitter", "тви+ттер"),
    ("instagram", "и+нстаграм"), ("facebook", "фе+йсбук"), ("tiktok", "ти+кток"), ("chatgpt", "чат джи-пи-ти"),
    ("claude", "кло+д"), ("openai", "опен эй-ай"), ("gemini", "дже+мини"), ("groq", "грок"), ("openrouter", "о+пенроутер"),
    ("jarvis", "джа+рвис"), ("docker", "до+кер"), ("react", "риэ+кт"), ("next", "некст"),
    ("tauri", "та+ури"), ("powershell", "па+уэршэл"), ("bluetooth", "блю+тус"), ("office", "о+фис"),
    ("onedrive", "уа+ндрайв"), ("paint", "пейнт"), ("explorer", "экспло+рер"), ("terminal", "тё+рминал"),
    ("roblox", "ро+блокс"), ("minecraft", "ма+йнкрафт"), ("counter", "ка+унтер"), ("strike", "страйк"), ("dota", "дота"),
    ("valorant", "вэ+лорант"), ("fortnite", "фо+ртнайт"), ("cyberpunk", "са+йберпанк"), ("yandex", "яндекс"),
    ("ozon", "озон"), ("avito", "авито"), ("geforce", "джифо+рс"), ("radeon", "рэ+йдеон"), ("ryzen", "ра+йзен"),
    ("llama", "лама"), ("qwen", "квен"), ("deepseek", "ди+псик"), ("mistral", "мистра+ль"), ("nemotron", "немотрон"),
    ("cerebras", "це+ребрас"), ("wikipedia", "википедия"), ("reddit", "ре+ддит"), ("stack", "стек"),
    ("overflow", "о+уверфлоу"), ("obsidian", "обси+диан"), ("notion", "но+ушн"), ("figma", "фи+гма"), ("slack", "слак"),
    ("dropbox", "дро+пбокс"), ("winrar", "ви+нрар"), ("anydesk", "э+нидеск"), ("teamviewer", "ти+мвьюер"),
    // частые слова
    ("commit", "ко+ммит"), ("push", "пуш"), ("pull", "пулл"), ("branch", "бранч"), ("merge", "мё+рдж"),
    ("file", "файл"), ("files", "файлы"), ("folder", "фо+лдер"), ("settings", "се+ттингс"), ("update", "апде+йт"),
    ("download", "да+унлоуд"), ("downloads", "да+унлоудс"), ("store", "стор"), ("game", "гейм"), ("games", "геймс"),
    ("server", "сё+рвер"), ("home", "хоум"), ("page", "пейдж"), ("online", "онла+йн"), ("smart", "смарт"),
    ("pro", "про"), ("max", "макс"), ("plus", "плюс"), ("mini", "мини"), ("ultra", "ультра"), ("sir", "сэр"),
    ("hello", "хэло+у"), ("ok", "оке+й"), ("okay", "оке+й"), ("yes", "йес"), ("no", "ноу"), ("the", "зе"),
    ("and", "энд"), ("of", "оф"), ("to", "ту"), ("for", "фор"), ("with", "уиз"), ("is", "из"), ("in", "ин"),
    ("on", "он"), ("new", "нью"), ("news", "ньюс"), ("free", "фри"), ("start", "старт"), ("stop", "стоп"),
    ("play", "плей"), ("player", "пле+йер"), ("music", "мью+зик"), ("video", "ви+део"), ("browser", "бра+узер"),
    ("cloud", "клауд"), ("login", "ло+гин"), ("password", "па+ссворд"), ("email", "и+мейл"), ("chat", "чат"),
    ("bot", "бот"), ("app", "эпп"), ("apps", "эппс"), ("web", "веб"), ("site", "сайт"), ("blog", "блог"),
    ("release", "рили+з"), ("notes", "ноутс"), ("week", "уик"), ("blue", "блю"), ("screen", "скрин"),
    ("task", "таск"), ("manager", "мэ+неджер"), ("control", "контро+л"), ("panel", "пэ+нел"), ("mark", "марк"),
    ("stark", "старк"), ("tony", "тони"), ("iron", "айрон"), ("man", "мэн"), ("service", "сё+рвис"),
    ("json", "джэ+йсон"), ("config", "ко+нфиг"), ("yaml", "я+мл"), ("toml", "то+мл"), ("readme", "ри+дми"),
    ("localhost", "ло+калхост"), ("npm", "эн-пи-эм"), ("cargo", "ка+рго"), ("node", "но+уд"), ("api", "эй-пи-а+й"),
    ("frontend", "фро+нтенд"), ("backend", "бэ+кенд"), ("deploy", "дипло+й"), ("build", "билд"), ("bug", "баг"),
    ("spooler", "спу+лер"), ("ipconfig", "ай-пи-конфиг"), ("all", "олл"), ("com", "ком"), ("process", "про+цесс"), ("error", "э+ррор"), ("warning", "во+рнинг"), ("debug", "ди+баг"),
];

/// Буквы для аббревиатур по буквам.
fn letter_name(c: char) -> &'static str {
    match c.to_ascii_lowercase() {
        'a' => "эй", 'b' => "би", 'c' => "си", 'd' => "ди", 'e' => "и", 'f' => "эф", 'g' => "джи", 'h' => "эйч",
        'i' => "ай", 'j' => "джей", 'k' => "кей", 'l' => "эл", 'm' => "эм", 'n' => "эн", 'o' => "о", 'p' => "пи",
        'q' => "кью", 'r' => "ар", 's' => "эс", 't' => "ти", 'u' => "ю", 'v' => "ви", 'w' => "дабл-ю", 'x' => "икс",
        'y' => "уай", 'z' => "зет",
        _ => "",
    }
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

/// Транскрипция английского слова (нижний регистр, только a–z) по буквосочетаниям — приблизительно,
/// но заметно лучше побуквенного чтения русским голосом.
pub fn transcribe(word: &str) -> String {
    let w: Vec<char> = word.chars().collect();
    let n = w.len();
    let at = |i: usize| w.get(i).copied().unwrap_or(' ');
    let starts = |i: usize, s: &str| s.chars().enumerate().all(|(k, c)| at(i + k) == c);
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let c = w[i];
        let next = at(i + 1);
        // Немая «e» в конце слова длиннее трёх букв: «code», «game».
        if c == 'e' && i == n - 1 && n > 3 && !is_vowel(at(i.wrapping_sub(1))) {
            break;
        }
        // Буквосочетания (длинные первыми).
        let rules: &[(&str, &str)] = &[
            ("tion", "шн"), ("sion", "жн"), ("ight", "айт"), ("igh", "ай"), ("ough", "оу"), ("sch", "ск"),
            ("tch", "ч"), ("sh", "ш"), ("ch", "ч"), ("th", "т"), ("ph", "ф"), ("ck", "к"), ("qu", "кв"),
            ("oo", "у"), ("ee", "и"), ("ea", "и"), ("ai", "эй"), ("ay", "эй"), ("ey", "эй"), ("ou", "ау"), ("ow", "оу"),
            ("oa", "оу"), ("ew", "ью"), ("wh", "у"), ("gh", ""), ("ng", "нг"), ("oi", "ой"), ("oy", "ой"),
            ("au", "о"), ("aw", "о"), ("ie", "и"), ("ue", "ю"),
        ];
        if i == 0 && (starts(0, "kn") || starts(0, "wr")) {
            out.push(if w[1] == 'n' { 'н' } else { 'р' });
            i += 2;
            continue;
        }
        if let Some((pat, rep)) = rules.iter().find(|(p, _)| starts(i, p)) {
            out.push_str(rep);
            i += pat.chars().count();
            continue;
        }
        let rep: &str = match c {
            'a' => "а",
            'b' => "б",
            'c' => if matches!(next, 'e' | 'i' | 'y') { "с" } else { "к" },
            'd' => "д",
            'e' => if i == 0 { "э" } else { "е" },
            'f' => "ф",
            'g' => if matches!(next, 'e' | 'i' | 'y') && i > 0 { "дж" } else { "г" },
            'h' => "х",
            'i' => "и",
            'j' => "дж",
            'k' => "к",
            'l' => "л",
            'm' => "м",
            'n' => "н",
            'o' => "о",
            'p' => "п",
            'q' => "к",
            'r' => "р",
            's' => "с",
            't' => "т",
            'u' => if i == 0 || is_vowel(next) { "ю" } else { "а" },
            'v' => "в",
            'w' => "у",
            'x' => if i == 0 { "з" } else { "кс" },
            'y' => if i == 0 { "й" } else if i == n - 1 { "и" } else { "ай" },
            'z' => "з",
            _ => "",
        };
        out.push_str(rep);
        i += 1;
    }
    out
}

const RU_VOWELS: &str = "аеёиоуыэюяАЕЁИОУЫЭЮЯ";

/// `+` сразу после гласной → знак ударения U+0301 на ней (так его понимает сервер голоса).
/// Другие `+` не трогаются.
pub fn apply_stress(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev = ' ';
    for c in s.chars() {
        if c == '+' && RU_VOWELS.contains(prev) {
            out.push('́');
        } else {
            out.push(c);
        }
        prev = c;
    }
    out
}

/// Ударение на первый слог (если слогов больше одного и ударения ещё нет).
fn stress_first_syllable(ru: &str) -> String {
    if ru.contains('+') || ru.chars().filter(|c| RU_VOWELS.contains(*c)).count() < 2 {
        return ru.to_string();
    }
    let (i, c) = ru.char_indices().find(|(_, c)| RU_VOWELS.contains(*c)).unwrap_or((0, ' '));
    let end = i + c.len_utf8();
    format!("{}+{}", &ru[..end], &ru[end..])
}

/// Словарь для обратной замены в распознавании речи: английское написание → кириллическое чтение
/// (без знаков ударения).
pub fn dictionary() -> impl Iterator<Item = (&'static str, String)> {
    PHRASES.iter().chain(WORDS.iter()).map(|(en, ru)| (*en, ru.replace('+', "")))
}

fn capitalize_like(src: &str, ru: &str) -> String {
    if src.chars().next().is_some_and(|c| c.is_uppercase()) {
        let mut cs = ru.chars();
        match cs.next() {
            Some(f) => f.to_uppercase().chain(cs).collect(),
            None => String::new(),
        }
    } else {
        ru.to_string()
    }
}

fn read_word(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    if let Some((_, r)) = WORDS.iter().find(|(k, _)| *k == lower) {
        return capitalize_like(word, r);
    }
    let letters = word.chars().filter(|c| c.is_ascii_alphabetic()).count();
    // «OBS», «RTX»: короткое слово заглавными — по буквам.
    if letters <= 4 && word.chars().all(|c| c.is_ascii_uppercase()) {
        return word.chars().map(letter_name).collect::<Vec<_>>().join("-");
    }
    let clean: String = lower.chars().filter(|c| c.is_ascii_lowercase()).collect();
    capitalize_like(word, &stress_first_syllable(&transcribe(&clean)))
}

static RE_PHRASE: LazyLock<Regex> = LazyLock::new(|| {
    let alt: Vec<String> = PHRASES.iter().map(|(k, _)| regex::escape(k).replace(' ', r"\s+")).collect();
    Regex::new(&format!(r"(?i)\b({})\b", alt.join("|"))).unwrap()
});

/// Латинское слово целиком (с апострофом внутри), не часть слова с цифрами/кириллицей.
static RE_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^\p{L}\p{N}_])([A-Za-z][A-Za-z']*)($|[^\p{L}\p{N}_])").unwrap());

/// Имя файла с расширением: «config.json» → «config точка json» (дальше слова читаются как обычно).
static RE_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([a-z][a-z0-9_-]*)\.(json|toml|ya?ml|txt|md|log|exe|dll|rs|ts|tsx|js|py|bat|cmd|ps1|zip|csv|xml|html?|css|ini|cfg)\b").unwrap()
});

pub fn expand(text: &str) -> String {
    let text = RE_FILE.replace_all(text, "$1 точка $2");
    let s = RE_PHRASE
        .replace_all(&text, |c: &Captures| {
            let key = c[1].to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
            let r = PHRASES.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or("");
            apply_stress(&capitalize_like(&c[1], r))
        })
        .into_owned();
    // Соседние слова делят разделитель — прогоняем дважды («Google Drive»).
    let mut s = s;
    for _ in 0..2 {
        s = RE_WORD
            .replace_all(&s, |c: &Captures| format!("{}{}{}", &c[1], apply_stress(&read_word(&c[2])), &c[3]))
            .into_owned();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Без знаков ударения — для проверки самих слов.
    fn plain(s: &str) -> String {
        s.replace('́', "")
    }

    #[test]
    fn stress_marks_and_file_names() {
        assert_eq!(expand("PowerShell"), "Па́уэршэл");
        assert_eq!(expand("открой config.json"), "открой ко́нфиг точка джэ́йсон");
        // Слово не из словаря — ударение на первый слог; односложное — без знака.
        assert_eq!(expand("telegraph"), "те́леграф");
        assert_eq!(expand("night"), "найт");
        // Кириллица не трогается, плюс вне английских слов не появляется.
        assert_eq!(expand("пять + два"), "пять + два");
        assert_eq!(expand("1+1 в Excel"), "1+1 в Э́ксель");
        assert!(dictionary().any(|(en, ru)| en == "powershell" && ru == "пауэршэл"));
    }

    #[test]
    fn dictionary_and_phrases() {
        assert_eq!(plain(&expand("Discord свёрнут, сэр.")), "Дискорд свёрнут, сэр.");
        assert_eq!(plain(&expand("Открываю Google Chrome")), "Открываю Гугл хром");
        assert_eq!(plain(&expand("коммит в GitHub из VS Code")), "коммит в Гитхаб из Ви-эс код");
        assert_eq!(plain(&expand("Процесс Code и chrome")), "Процесс Код и хром");
    }

    #[test]
    fn rules_for_unknown_words() {
        assert_eq!(transcribe("telegraph"), "телеграф");
        assert_eq!(transcribe("shadow"), "шадоу");
        assert_eq!(transcribe("night"), "найт");
        assert_eq!(plain(&read_word("Speed")), "Спид");
        assert_eq!(read_word("OBS"), "о-би-эс");
    }

    #[test]
    fn leaves_cyrillic_and_alnum_alone() {
        assert_eq!(plain(&expand("процессор i7 и x64, Привет")), "процессор i7 и x64, Привет");
        assert!(!expand("Spooler остановлен").chars().any(|c| c.is_ascii_alphabetic()));
    }
}
