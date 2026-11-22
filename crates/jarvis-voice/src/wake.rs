//! `WakeFilter`: текстовый wake word «Джарвис, …» по транскрипту, стоп-слова (барж-ин), подтверждение/отказ.

/// Что означает реплика пользователя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Команда без обращения, с исходным регистром и пунктуацией: «Джарвис, открой Блокнот» → «открой Блокнот».
    Command(String),
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
        // Точное совпадение или одна ошибка распознавания для длинных вариантов («джарвас»).
        self.names.iter().any(|n| *n == w || (w.chars().count() >= 6 && edit_distance(n, &w) <= 1))
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
        CONFIRM.iter().any(|c| *c == ws.as_slice()).then_some(true)
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
        match stripped {
            Some("") => Intent::WakeOnly,
            Some(cmd) => Intent::Command(cmd.to_string()),
            None if ctx.follow_up && words(text).iter().any(|w| w.chars().any(char::is_alphabetic)) => {
                Intent::Command(text.trim().to_string())
            }
            None => Intent::Ignored,
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
            assert_eq!(f.classify(t, idle()), Intent::Command("открой браузер".into()), "{t}");
        }
        assert_eq!(f.classify("Джарвис, сколько будет 7 + 5?", idle()), Intent::Command("сколько будет 7 + 5?".into()));
        assert_eq!(f.classify("Джарвис.", idle()), Intent::WakeOnly);
        assert_eq!(f.classify("джарвас открой", idle()), Intent::Command("открой".into()), "одна ошибка распознавания");
        assert_eq!(f.classify("открой браузер", idle()), Intent::Ignored);
        assert_eq!(f.classify("я говорил с Джарвисом", idle()), Intent::Ignored, "обращение только в начале");
        assert_eq!(f.classify("открой браузер", WakeContext { follow_up: true, ..idle() }), Intent::Command("открой браузер".into()));
    }

    #[test]
    fn stop_words() {
        let f = WakeFilter::new();
        for t in ["Стоп!", "хватит", "Джарвис, стоп", "отмена", "стоп, стоп", "замолчи, пожалуйста", "тише, сэр"] {
            assert_eq!(f.classify(t, idle()), Intent::Stop, "{t}");
        }
        assert_eq!(f.classify("Джарвис, хватит открывать окна", idle()), Intent::Command("хватит открывать окна".into()));
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
}
