//! Английские слова → как их произносят по-русски (голос Джарвиса русский и латиницу читает плохо).
//!
//! 1. Словарь частых названий (программы, сервисы, бренды, IT-слова): «Discord» → «Дискорд».
//! 2. Остальное — правила транскрипции по буквосочетаниям: «Telegram» → «Телеграм», «update» → «апдейт».
//! 3. Короткие слова ЗАГЛАВНЫМИ (аббревиатуры не из словаря `abbrev`) — по буквам: «OBS» → «о-би-эс».
//!
//! Слова с цифрами (`i7`, `x64`) и кириллица не трогаются.

use regex::{Captures, Regex};
use std::sync::LazyLock;

/// Фразы из нескольких слов (сначала они, потом одиночные слова). Ключ — нижний регистр.
const PHRASES: &[(&str, &str)] = &[
    ("visual studio code", "вижуал студио код"),
    ("vs code", "ви-эс код"),
    ("google chrome", "гугл хром"),
    ("microsoft edge", "майкрософт эдж"),
    ("task manager", "диспетчер задач"),
    ("open ai", "опен эй-ай"),
    ("iron man", "айрон мэн"),
    ("tony stark", "тони старк"),
    ("chat gpt", "чат джи-пи-ти"),
];

/// Словарь: нижний регистр → чтение.
const WORDS: &[(&str, &str)] = &[
    // программы и сервисы
    ("discord", "дискорд"), ("chrome", "хром"), ("google", "гугл"), ("youtube", "ютуб"), ("windows", "виндоус"),
    ("microsoft", "майкрософт"), ("github", "гитхаб"), ("gitlab", "гитлаб"), ("git", "гит"), ("telegram", "телеграм"),
    ("steam", "стим"), ("spotify", "спотифай"), ("firefox", "файрфокс"), ("edge", "эдж"), ("opera", "опера"),
    ("word", "ворд"), ("excel", "эксель"), ("powerpoint", "пауэрпоинт"), ("outlook", "аутлук"), ("teams", "тимс"),
    ("zoom", "зум"), ("skype", "скайп"), ("whatsapp", "ватсап"), ("vscode", "ви-эс код"), ("visual", "вижуал"),
    ("studio", "студио"), ("code", "код"), ("notepad", "ноутпад"), ("python", "пайтон"), ("rust", "раст"),
    ("javascript", "джаваскрипт"), ("typescript", "тайпскрипт"), ("java", "джава"), ("linux", "линукс"),
    ("ubuntu", "убунту"), ("apple", "эпл"), ("iphone", "айфон"), ("android", "андроид"), ("nvidia", "энвидиа"),
    ("intel", "интел"), ("amazon", "амазон"), ("netflix", "нетфликс"), ("twitch", "твич"), ("twitter", "твиттер"),
    ("instagram", "инстаграм"), ("facebook", "фейсбук"), ("tiktok", "тикток"), ("chatgpt", "чат джи-пи-ти"),
    ("claude", "клод"), ("openai", "опен эй-ай"), ("gemini", "джемини"), ("groq", "грок"), ("openrouter", "опенроутер"),
    ("jarvis", "джарвис"), ("docker", "докер"), ("node", "нод"), ("react", "реакт"), ("next", "некст"),
    ("tauri", "таури"), ("powershell", "пауэршелл"), ("bluetooth", "блютус"), ("office", "офис"),
    ("onedrive", "уандрайв"), ("paint", "пейнт"), ("explorer", "эксплорер"), ("terminal", "терминал"),
    ("roblox", "роблокс"), ("minecraft", "майнкрафт"), ("counter", "каунтер"), ("strike", "страйк"), ("dota", "дота"),
    ("valorant", "валорант"), ("fortnite", "фортнайт"), ("cyberpunk", "киберпанк"), ("yandex", "яндекс"),
    ("ozon", "озон"), ("avito", "авито"), ("geforce", "джифорс"), ("radeon", "радеон"), ("ryzen", "райзен"),
    ("llama", "лама"), ("qwen", "квен"), ("deepseek", "дипсик"), ("mistral", "мистраль"), ("nemotron", "немотрон"),
    ("cerebras", "церебрас"), ("wikipedia", "википедия"), ("reddit", "реддит"), ("stack", "стек"),
    ("overflow", "оверфлоу"), ("obsidian", "обсидиан"), ("notion", "ноушн"), ("figma", "фигма"), ("slack", "слак"),
    ("dropbox", "дропбокс"), ("winrar", "винрар"), ("anydesk", "энидеск"), ("teamviewer", "тимвьюер"),
    // частые слова
    ("commit", "коммит"), ("push", "пуш"), ("pull", "пулл"), ("branch", "бранч"), ("merge", "мёрдж"),
    ("file", "файл"), ("files", "файлы"), ("folder", "фолдер"), ("settings", "сеттингс"), ("update", "апдейт"),
    ("download", "даунлоуд"), ("downloads", "даунлоудс"), ("store", "стор"), ("game", "гейм"), ("games", "геймс"),
    ("server", "сервер"), ("home", "хоум"), ("page", "пейдж"), ("online", "онлайн"), ("smart", "смарт"),
    ("pro", "про"), ("max", "макс"), ("plus", "плюс"), ("mini", "мини"), ("ultra", "ультра"), ("sir", "сэр"),
    ("hello", "хэллоу"), ("ok", "окей"), ("okay", "окей"), ("yes", "йес"), ("no", "ноу"), ("the", "зе"),
    ("and", "энд"), ("of", "оф"), ("to", "ту"), ("for", "фор"), ("with", "уиз"), ("is", "из"), ("in", "ин"),
    ("on", "он"), ("new", "нью"), ("news", "ньюс"), ("free", "фри"), ("start", "старт"), ("stop", "стоп"),
    ("play", "плей"), ("player", "плеер"), ("music", "мьюзик"), ("video", "видео"), ("browser", "браузер"),
    ("cloud", "клауд"), ("login", "логин"), ("password", "пассворд"), ("email", "имейл"), ("chat", "чат"),
    ("bot", "бот"), ("app", "эпп"), ("apps", "эппс"), ("web", "веб"), ("site", "сайт"), ("blog", "блог"),
    ("release", "релиз"), ("notes", "ноутс"), ("week", "уик"), ("blue", "блю"), ("screen", "скрин"),
    ("task", "таск"), ("manager", "менеджер"), ("control", "контрол"), ("panel", "панель"), ("mark", "марк"),
    ("stark", "старк"), ("tony", "тони"), ("iron", "айрон"), ("man", "мэн"), ("service", "сервис"),
    ("spooler", "спулер"), ("ipconfig", "ай-пи-конфиг"), ("all", "олл"), ("com", "ком"), ("process", "процесс"), ("error", "эррор"), ("warning", "ворнинг"), ("debug", "дебаг"),
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
    capitalize_like(word, &transcribe(&clean))
}

static RE_PHRASE: LazyLock<Regex> = LazyLock::new(|| {
    let alt: Vec<String> = PHRASES.iter().map(|(k, _)| regex::escape(k).replace(' ', r"\s+")).collect();
    Regex::new(&format!(r"(?i)\b({})\b", alt.join("|"))).unwrap()
});

/// Латинское слово целиком (с апострофом внутри), не часть слова с цифрами/кириллицей.
static RE_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^\p{L}\p{N}_])([A-Za-z][A-Za-z']*)($|[^\p{L}\p{N}_])").unwrap());

pub fn expand(text: &str) -> String {
    let s = RE_PHRASE
        .replace_all(text, |c: &Captures| {
            let key = c[1].to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
            let r = PHRASES.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or("");
            capitalize_like(&c[1], r)
        })
        .into_owned();
    // Соседние слова делят разделитель — прогоняем дважды («Google Drive»).
    let mut s = s;
    for _ in 0..2 {
        s = RE_WORD
            .replace_all(&s, |c: &Captures| format!("{}{}{}", &c[1], read_word(&c[2]), &c[3]))
            .into_owned();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_and_phrases() {
        assert_eq!(expand("Discord свёрнут, сэр."), "Дискорд свёрнут, сэр.");
        assert_eq!(expand("Открываю Google Chrome"), "Открываю Гугл хром");
        assert_eq!(expand("коммит в GitHub из VS Code"), "коммит в Гитхаб из Ви-эс код");
        assert_eq!(expand("Процесс Code и chrome"), "Процесс Код и хром");
    }

    #[test]
    fn rules_for_unknown_words() {
        assert_eq!(transcribe("telegraph"), "телеграф");
        assert_eq!(transcribe("shadow"), "шадоу");
        assert_eq!(transcribe("night"), "найт");
        assert_eq!(read_word("Speed"), "Спид");
        assert_eq!(read_word("OBS"), "о-би-эс");
    }

    #[test]
    fn leaves_cyrillic_and_alnum_alone() {
        assert_eq!(expand("процессор i7 и x64, Привет"), "процессор i7 и x64, Привет");
        assert!(!expand("Spooler остановлен").chars().any(|c| c.is_ascii_alphabetic()));
    }
}
