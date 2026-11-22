//! `SpeechNormalizer` — текст ответа LLM → текст, который русский TTS прочитает правильно.
//!
//! Порядок: Markdown → эмодзи → ссылки/почта → символы перед числами → числа → сокращения →
//! английские слова (словарь + транскрипция) → символы → чистка.

mod abbrev;
pub mod english;
mod markdown;
pub mod numbers;
mod numexpr;
mod text;

pub use abbrev::ACRONYMS;
pub use numbers::{Case, Gender, OrdForm, cardinal, decimal, ordinal, plural, with_noun};
pub use text::is_emoji;

#[derive(Debug, Clone)]
pub struct SpeechNormalizer {
    /// Чем заменяется блок кода.
    pub code_placeholder: String,
}

impl Default for SpeechNormalizer {
    fn default() -> Self {
        Self { code_placeholder: "Пример кода".into() }
    }
}

impl SpeechNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn normalize(&self, input: &str) -> String {
        let s = markdown::to_plain(input, &self.code_placeholder);
        let s = text::strip_emoji(&s);
        let s = text::links_to_words(&s);
        let s = text::symbols_pre(&s);
        let s = numexpr::replace_numbers(&s);
        let s = abbrev::expand(&s);
        let s = english::expand(&s);
        let s = text::symbols_post(&s);
        text::cleanup(&s)
    }

    /// Есть ли в тексте что произносить (TTS-сервер отвергает текст без букв и цифр).
    pub fn is_speakable(text: &str) -> bool {
        text.chars().any(|c| c.is_alphanumeric())
    }
}

/// Только числа (без Markdown и прочего) — удобно для отладки и тестов.
pub fn numbers_to_words(s: &str) -> String {
    numexpr::replace_numbers(s)
}
