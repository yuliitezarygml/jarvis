//! Вырезание `<think>…</think>` в начале ответа (часть reasoning-моделей кладёт рассуждения прямо в content).
//! Рассуждения не должны попасть в субтитры и TTS.

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Start,
    InThink,
    Pass,
}

/// Потоковый фильтр: дельты на входе, видимый текст на выходе.
#[derive(Debug, Default)]
pub(crate) struct ThinkFilter {
    state: State,
    pending: String,
}

impl ThinkFilter {
    pub(crate) fn push(&mut self, s: &str) -> String {
        match self.state {
            State::Pass => s.to_string(),
            State::Start => {
                self.pending.push_str(s);
                let t = self.pending.trim_start();
                if let Some(rest) = t.strip_prefix(OPEN) {
                    let rest = rest.to_string();
                    self.pending.clear();
                    self.state = State::InThink;
                    self.push(&rest)
                } else if OPEN.starts_with(t) {
                    String::new() // пока неясно: пусто или начало «<think>»
                } else {
                    self.state = State::Pass;
                    std::mem::take(&mut self.pending)
                }
            }
            State::InThink => {
                self.pending.push_str(s);
                if let Some(i) = self.pending.find(CLOSE) {
                    let rest = self.pending[i + CLOSE.len()..].trim_start().to_string();
                    self.pending.clear();
                    self.state = State::Pass;
                    rest
                } else {
                    // Держим только хвост, в котором может начинаться «</think>».
                    let keep = CLOSE.len() - 1;
                    if self.pending.len() > keep {
                        let mut cut = self.pending.len() - keep;
                        while !self.pending.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        self.pending.drain(..cut);
                    }
                    String::new()
                }
            }
        }
    }

    /// Конец потока: отдать то, что ещё держали (незакрытый think отбрасывается).
    pub(crate) fn finish(&mut self) -> String {
        let out = match self.state {
            State::Start => std::mem::take(&mut self.pending),
            _ => String::new(),
        };
        self.pending.clear();
        out
    }
}

/// Для обычного (не потокового) ответа.
pub(crate) fn strip_think(s: &str) -> String {
    let mut f = ThinkFilter::default();
    let mut out = f.push(s);
    out.push_str(&f.finish());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(parts: &[&str]) -> String {
        let mut f = ThinkFilter::default();
        let mut out = String::new();
        for p in parts {
            out.push_str(&f.push(p));
        }
        out.push_str(&f.finish());
        out
    }

    #[test]
    fn strips_leading_think_across_chunks() {
        assert_eq!(run(&["<th", "ink>рассуждаю…</thi", "nk>\n\nГолубое"]), "Голубое");
        assert_eq!(run(&["  <think>x</think>Да"]), "Да");
        assert_eq!(strip_think("<think>a</think> b"), "b");
    }

    #[test]
    fn passes_normal_text() {
        assert_eq!(run(&["Голу", "бое"]), "Голубое");
        assert_eq!(run(&["<", "b>жирный</b>"]), "<b>жирный</b>");
        assert_eq!(run(&["  "]), "  ");
        assert_eq!(run(&["текст <think>не в начале</think>"]), "текст <think>не в начале</think>");
    }

    #[test]
    fn unterminated_think_is_dropped() {
        assert_eq!(run(&["<think>бесконечные рассуждения про небо"]), "");
    }
}
