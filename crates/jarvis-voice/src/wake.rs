//! `WakeFilter`: текстовый wake word «Джарвис, …» по транскрипту, стоп-слова (барж-ин), подтверждение/отказ,
//! голосовое переключение характера («Джарвис, давай на ты»).

use jarvis_core::config::PersonaMode;

use crate::tone::VoiceTone;

/// Что означает реплика пользователя.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Команда без обращения, с исходным регистром и пунктуацией: «Джарвис, открой Блокнот» → «открой Блокнот».
    /// `voice` — тон голоса этой фразы (громкость/темп относительно обычного), если голосовой конвейер его
    /// оценил; `WakeFilter::classify` всегда ставит `None`.
    Command { text: String, voice: Option<VoiceTone> },
    /// Сменить характер голосом: «Джарвис, будь вежливым» / «давай на ты» / «можешь материться».
    /// Только с обращением или в окне продолжения и только если фраза — (почти) одна эта просьба.
    Persona(PersonaMode),
    /// Только обращение «Джарвис» — ждать команду следующей репликой.
    WakeOnly,
    /// «стоп / хватит / отмена» — прервать речь и действие.
    Stop,
    /// «да / да, сэр / подтверждаю / выполняй» (только в режиме ожидания подтверждения).
    Confirm,
    /// «нет / отмена / не надо» (только в режиме ожидания подтверждения).
    Deny,
    /// Не к Джарвису.
    Ignored,
}

impl Intent {
    /// Команда без оценки голоса.
    pub fn command(text: impl Into<String>) -> Self {
        Intent::Command { text: text.into(), voice: None }
    }
}

/// Контекст разбора.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeContext {
    /// Джарвис ждёт «да, сэр» для действия T3.
    pub awaiting_confirm: bool,
    /// Окно продолжения: после «Джарвис» или ответа команда принимается без обращения.
    pub follow_up: bool,
}

const WAKE_VARIANTS: &[&str] = &["джарвис", "джервис", "джарвиз", "жарвис", "jarvis", "джарвес", "чарвис", "джаврис"];
const INTERJECTIONS: &[&str] = &["эй", "окей", "ok", "ну", "слушай", "алло", "привет", "hey"];
const STOP_WORDS: &[&str] = &[
    "стоп", "хватит", "отмена", "отмени", "отменить", "замолчи", "замолчите", "молчи", "тихо", "тише", "остановись",
    "остановить", "прекрати", "перестань", "довольно", "стой", "stop",
];
const FILLERS: &[&str] = &["пожалуйста", "сэр", "уже", "все", "же", "ну", "ладно"];
const CONFIRM: &[&[&str]] = &[
    &["да"],
    &["да", "да"],
    &["подтверждаю"],
    &["выполняй"],
    &["выполняйте"],
    &["выполнить"],
    &["делай"],
    &["давай"],
    &["да", "давай"],
    &["да", "выполняй"],
    &["да", "подтверждаю"],
    &["да", "конечно"],
    &["конечно"],
    &["согласен"],
    &["разрешаю"],
    &["окей"],
];
const NEGATIONS: &[&str] = &["нет", "не", "ни", "отмена", "отмени", "отменить", "стоп", "отставить", "нельзя", "хватит", "стой"];
/// Просьбы сменить характер. Сравниваются по словам целиком (после выкидывания `PERSONA_FILLERS` с обеих
/// сторон), поэтому «напиши в блокноте будь вежливым» — обычная команда, а «не матерись» ≠ «матерись».
const PERSONA_PHRASES: &[(PersonaMode, &[&str])] = &[
    (
        PersonaMode::Butler,
        &[
            "будь культурным", "будь культурнее", "будь вежливым", "будь вежливее", "как дворецкий", "будь дворецким",
            "без мата", "без матов", "культурно", "говори культурно", "говори вежливо", "вежливо", "не матерись",
            "хватит материться", "режим дворецкого",
        ],
    ),
    (
        PersonaMode::Buddy,
        &[
            "давай на ты", "на ты", "будь проще", "будь попроще", "по простому", "попроще", "как друг",
            "говори по простому", "говори проще", "общайся на ты",
        ],
    ),
    (
        PersonaMode::Uncensored,
        &[
            "можешь материться", "без цензуры", "матерись", "разрешаю материться", "можно с матом", "можно материться",
            "с матом", "давай с матом", "говори с матом", "режим без цензуры",
        ],
    ),
];
/// Слова, которые не мешают просьбе сменить характер: «ну давай теперь со мной на ты, пожалуйста».
const PERSONA_FILLERS: &[&str] = &[
    "пожалуйста", "сэр", "уже", "все", "же", "ну", "ладно", "давай", "давайте", "теперь", "отныне", "впредь", "снова",
    "опять", "дальше", "со", "мной", "мне", "тогда", "короче", "хорошо", "окей", "ok", "а", "и", "чуть", "немного",
    "немножко", "более", "можно", "будь",
];

/// Слово распознано «почти так же» (ошибки ASR): средние слова — до 1 правки, длинные в просьбе из
/// нескольких слов — до 2 («будь культурней»), короткие — точно. Одно длинное слово — не больше 1 правки,
/// иначе «культура» сошла бы за «культурно».
fn word_close(heard: &str, want: &str, multi_word: bool) -> bool {
    let len = want.chars().count();
    heard == want
        || (len >= 5 && edit_distance(heard, want) <= 1)
        || (multi_word && len >= 8 && edit_distance(heard, want) <= 2)
}

/// Слова реплики: нижний регистр, ё→е, только буквы и цифры.
pub fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('ё', "е")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// Расстояние Левенштейна для коротких слов (по символам).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[derive(Debug, Clone)]
pub struct WakeFilter {
    names: Vec<String>,
}

impl Default for WakeFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl WakeFilter {
    pub fn new() -> Self {
        Self { names: WAKE_VARIANTS.iter().map(|s| s.to_string()).collect() }
    }

    /// Свои варианты имени (например, из настроек).
    pub fn with_names(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { names: names.into_iter().map(|n| n.into().to_lowercase().replace('ё', "е")).collect() }
    }

    pub fn is_wake_word(&self, word: &str) -> bool {
        let w = word.to_lowercase().replace('ё', "е");
        let len = w.chars().count();
        // Точное совпадение или одна ошибка распознавания для длинных вариантов («джарвас»).
        if self.names.iter().any(|n| *n == w || (len >= 6 && edit_distance(n, &w) <= 1)) {
            return true;
        }
        // «Дж…», «жар…», «чар…» — две ошибки распознавания («джаруз», «джарис», «жарыс») — тоже обращение.
        // Обычные слова («жарко», «чарка», «джинсы») отличаются сильнее и не проходят.
        (w.starts_with("дж") || w.starts_with("жар") || w.starts_with("чар"))
            && (5..=8).contains(&len)
            && self.names.iter().any(|n| edit_distance(n, &w) <= 2)
    }

    /// Если реплика начинается с обращения — вернуть остаток в исходном виде (без запятой/тире после имени).
    pub fn strip_wake<'a>(&self, text: &'a str) -> Option<&'a str> {
        let mut rest = text;
        loop {
            let trimmed = rest.trim_start_matches(|c: char| !c.is_alphanumeric());
            let end = trimmed.find(|c: char| !c.is_alphanumeric()).unwrap_or(trimmed.len());
            if end == 0 {
                return None;
            }
            let word = &trimmed[..end];
            let after = &trimmed[end..];
            let lw = word.to_lowercase();
            if INTERJECTIONS.contains(&lw.as_str()) {
                rest = after;
                continue;
            }
            if self.is_wake_word(word) {
                return Some(after.trim_start_matches(|c: char| c.is_whitespace() || ",.!?:;—–-".contains(c)).trim_end());
            }
            return None;
        }
    }

    pub fn has_wake_prefix(&self, text: &str) -> bool {
        self.strip_wake(text).is_some()
    }

    /// Короткая реплика только из стоп-слов (плюс «пожалуйста», «сэр», обращение).
    pub fn is_stop(&self, text: &str) -> bool {
        let w = words(text);
        let core: Vec<&String> = w.iter().filter(|x| !FILLERS.contains(&x.as_str()) && !self.is_wake_word(x)).collect();
        !core.is_empty() && core.len() <= 3 && core.iter().all(|x| STOP_WORDS.contains(&x.as_str()))
    }

    /// Подтверждение: `Some(true)` — да, `Some(false)` — нет, `None` — непонятно (переспросить).
    /// Отказ сильнее согласия: «да нет», «да, не надо» → нет (fail-closed).
    pub fn confirmation(&self, text: &str) -> Option<bool> {
        let w: Vec<String> = words(text).into_iter().filter(|x| !FILLERS.contains(&x.as_str()) && !self.is_wake_word(x)).collect();
        if w.is_empty() {
            return None;
        }
        if w.iter().any(|x| NEGATIONS.contains(&x.as_str())) {
            return Some(false);
        }
        let ws: Vec<&str> = w.iter().map(String::as_str).collect();
        CONFIRM.contains(&ws.as_slice()).then_some(true)
    }

    /// Просьба сменить характер: вся реплика (без обращения и слов-связок) — одна из `PERSONA_PHRASES`.
    pub fn persona_switch(&self, text: &str) -> Option<PersonaMode> {
        let core = |t: &str| -> Vec<String> {
            words(t).into_iter().filter(|x| !PERSONA_FILLERS.contains(&x.as_str()) && !self.is_wake_word(x)).collect()
        };
        let heard = core(text);
        if heard.is_empty() || heard.len() > 4 {
            return None;
        }
        PERSONA_PHRASES.iter().find_map(|(mode, phrases)| {
            phrases.iter().any(|p| {
                let want = core(p);
                want.len() == heard.len() && heard.iter().zip(&want).all(|(h, w)| word_close(h, w, want.len() > 1))
            })
            .then_some(*mode)
        })
    }

    pub fn classify(&self, text: &str, ctx: WakeContext) -> Intent {
        let stripped = self.strip_wake(text);
        let body = stripped.unwrap_or(text);
        if ctx.awaiting_confirm {
            match self.confirmation(body) {
                Some(true) => return Intent::Confirm,
                Some(false) => return Intent::Deny,
                None => {}
            }
        }
        if self.is_stop(body) {
            return Intent::Stop;
        }
        let addressed = match stripped {
            Some("") => return Intent::WakeOnly,
            Some(cmd) => cmd,
            // Без обращения в окне ожидания — только осмысленная фраза (≥4 букв): обрывки вроде «п»,
            // «ну» или кашель не тратят запрос к модели.
            None if ctx.follow_up && text.chars().filter(|c| c.is_alphabetic()).count() >= 4 => text.trim(),
            None => return Intent::Ignored,
        };
        match self.persona_switch(addressed) {
            Some(mode) => Intent::Persona(mode),
            None => Intent::command(addressed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> WakeContext {
        WakeContext::default()
    }

    #[test]
    fn wake_variants_and_command() {
        let f = WakeFilter::new();
        for t in ["Джарвис, открой браузер", "джервис открой браузер", "Джарвиз — открой браузер", "Жарвис: открой браузер", "Jarvis, открой браузер", "Эй, Джарвис, открой браузер", "ДЖАРВИС открой браузер"] {
            assert_eq!(f.classify(t, idle()), Intent::command("открой браузер"), "{t}");
        }
        assert_eq!(f.classify("Джарвис, сколько будет 7 + 5?", idle()), Intent::command("сколько будет 7 + 5?"));
        assert_eq!(f.classify("Джарвис.", idle()), Intent::WakeOnly);
        assert_eq!(f.classify("джарвас открой", idle()), Intent::command("открой"), "одна ошибка распознавания");
        assert_eq!(f.classify("открой браузер", idle()), Intent::Ignored);
        // Частые ошибки распознавания имени (из журнала) и защита от похожих обычных слов.
        for t in ["джаруз", "джарис открой", "Джервиз, открой", "жарыс", "джарлис", "джабиз"] {
            assert_ne!(f.classify(t, idle()), Intent::Ignored, "{t}");
        }
        for t in ["джинсы купи", "жарко сегодня", "джаз включи", "жареный картофель", "чарка вина"] {
            assert_eq!(f.classify(t, idle()), Intent::Ignored, "{t}");
        }
        assert_eq!(f.classify("я говорил с Джарвисом", idle()), Intent::Ignored, "обращение только в начале");
        assert_eq!(f.classify("открой браузер", WakeContext { follow_up: true, ..idle() }), Intent::command("открой браузер"));
        for junk in ["п", "ну", "а?", "м-м"] {
            assert_eq!(f.classify(junk, WakeContext { follow_up: true, ..idle() }), Intent::Ignored, "{junk}");
        }
    }

    #[test]
    fn stop_words() {
        let f = WakeFilter::new();
        for t in ["Стоп!", "хватит", "Джарвис, стоп", "отмена", "стоп, стоп", "замолчи, пожалуйста", "тише, сэр"] {
            assert_eq!(f.classify(t, idle()), Intent::Stop, "{t}");
        }
        assert_eq!(f.classify("Джарвис, хватит открывать окна", idle()), Intent::command("хватит открывать окна"));
        assert!(!f.is_stop("стоп-кран сорвали вчера вечером"));
    }

    #[test]
    fn confirmations() {
        let f = WakeFilter::new();
        let wait = WakeContext { awaiting_confirm: true, ..idle() };
        for t in ["да", "Да, сэр.", "подтверждаю", "выполняй", "Джарвис, да", "да, давай"] {
            assert_eq!(f.classify(t, wait), Intent::Confirm, "{t}");
        }
        for t in ["нет", "Нет, сэр", "отмена", "не надо", "да нет", "да, не надо", "стоп"] {
            assert_eq!(f.classify(t, wait), Intent::Deny, "{t}");
        }
        // Неясный ответ — не подтверждение.
        assert_eq!(f.confirmation("ну может быть"), None);
        assert_eq!(f.classify("ну может быть", wait), Intent::Ignored);
        // Вне режима подтверждения «да» ничего не подтверждает.
        assert_eq!(f.classify("да", idle()), Intent::Ignored);
    }

    #[test]
    fn helpers() {
        assert_eq!(words("Ёлка, Жарвис!"), vec!["елка", "жарвис"]);
        assert_eq!(edit_distance("джарвис", "джарвас"), 1);
        let f = WakeFilter::with_names(["Пятница"]);
        assert_eq!(f.strip_wake("пятница, свет"), Some("свет"));
        assert!(!f.has_wake_prefix("джарвис, свет"));
    }
    #[test]
    fn persona_switch_phrases() {
        let f = WakeFilter::new();
        let follow = WakeContext { follow_up: true, ..idle() };
        let cases: &[(&str, PersonaMode)] = &[
            ("Джарвис, будь культурным", PersonaMode::Butler),
            ("Джарвис, будь вежливым, пожалуйста", PersonaMode::Butler),
            ("Джарвис, будь вежливее", PersonaMode::Butler),
            ("Джарвис, будь культурней", PersonaMode::Butler),
            ("Джарвис, как дворецкий", PersonaMode::Butler),
            ("Джарвис, давай без мата", PersonaMode::Butler),
            ("Джарвис, культурно", PersonaMode::Butler),
            ("Джарвис, не матерись", PersonaMode::Butler),
            ("Джарвис, давай на ты", PersonaMode::Buddy),
            ("Джарвис, ну давай теперь со мной на ты", PersonaMode::Buddy),
            ("Джарвис, будь проще", PersonaMode::Buddy),
            ("Джарвис, по-простому", PersonaMode::Buddy),
            ("Джарвис, по простому", PersonaMode::Buddy),
            ("Джарвис, как друг", PersonaMode::Buddy),
            ("Джарвис, можешь материться", PersonaMode::Uncensored),
            ("Джарвис, без цензуры", PersonaMode::Uncensored),
            ("Джарвис, матерись", PersonaMode::Uncensored),
            ("Джарвис, разрешаю материться", PersonaMode::Uncensored),
            ("Джарвис, можно с матом", PersonaMode::Uncensored),
            ("Джарвис, можеш матерится", PersonaMode::Uncensored),
            ("Эй, Джарвис, без цензуры!", PersonaMode::Uncensored),
        ];
        for (t, mode) in cases {
            assert_eq!(f.classify(t, idle()), Intent::Persona(*mode), "{t}");
        }
        // В окне продолжения обращение не нужно.
        assert_eq!(f.classify("будь вежливым", follow), Intent::Persona(PersonaMode::Butler));
        assert_eq!(f.classify("давай на ты", follow), Intent::Persona(PersonaMode::Buddy));
        // Без обращения и вне окна — не к Джарвису.
        assert_eq!(f.classify("будь вежливым", idle()), Intent::Ignored);
        assert_eq!(f.classify("давай на ты", idle()), Intent::Ignored);
        // Длинные команды с теми же словами — обычные команды.
        for t in [
            "Джарвис, напиши в блокноте будь вежливым",
            "Джарвис, найди песню без цензуры",
            "Джарвис, как друг посоветуй фильм",
            "Джарвис, открой браузер без мата в поиске",
            "Джарвис, будь другом, открой почту",
            "Джарвис, культура",
            "Джарвис, давай",
        ] {
            assert!(matches!(f.classify(t, idle()), Intent::Command { voice: None, .. }), "{t}: {:?}", f.classify(t, idle()));
        }
        assert_eq!(f.persona_switch("не матерись"), Some(PersonaMode::Butler), "отрицание не путается с разрешением");
        assert_eq!(f.persona_switch("пожалуйста"), None);
        assert_eq!(f.persona_switch(""), None);
        // В режиме подтверждения «давай» — подтверждение, а «давай на ты» — смена характера.
        let wait = WakeContext { awaiting_confirm: true, ..idle() };
        assert_eq!(f.classify("давай", wait), Intent::Confirm);
        assert_eq!(f.classify("Джарвис, давай на ты", wait), Intent::Persona(PersonaMode::Buddy));
    }
}
