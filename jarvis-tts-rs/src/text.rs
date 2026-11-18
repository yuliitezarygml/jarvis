use crate::{linguistics::Normalizer, stress::Accent};
use std::{path::Path, sync::atomic::AtomicBool};
pub fn valid(text: &str) -> bool {
    text.chars().count() <= 2000
        && text
            .chars()
            .any(|c| c.is_ascii_alphanumeric() || matches!(c,'а'..='я'|'А'..='Я'|'ё'|'Ё'))
}
pub struct Frontend {
    normalizer: Normalizer,
    accent: Accent,
}
impl Frontend {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            normalizer: Normalizer::load(dir)?,
            accent: Accent::load(dir)?,
        })
    }
    pub fn prepare(&mut self, text: &str, stop: &AtomicBool) -> anyhow::Result<String> {
        anyhow::ensure!(valid(text), "invalid text");
        let mut sentences = Vec::new();
        let mut start = 0;
        let boundaries = text
            .char_indices()
            .filter_map(|(i, c)| {
                let end = i + c.len_utf8();
                let inside_number = c == '.'
                    && i > 0
                    && text.as_bytes()[i - 1].is_ascii_digit()
                    && text.as_bytes().get(end).is_some_and(u8::is_ascii_digit);
                (matches!(c, '.' | '!' | '?' | '\n') && !inside_number).then_some(end)
            })
            .chain([text.len()]);
        for end in boundaries {
            let s = &text[start..end];
            start = end;
            let s = s.trim();
            if s.is_empty() {
                continue;
            }
            let norm = self.normalizer.sentence(s, stop)?;
            sentences.push(self.accent.mark(&norm)?);
        }
        Ok(sentences.join(" "))
    }
}
