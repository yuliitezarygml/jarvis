//! Выбор памяти перед ходом агента и блок «Память:» для системного промпта.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::db::Memory;
use crate::error::Result;
use crate::facts::{important_facts_conn, search_facts_scored, style_facts_conn};
use crate::lessons::{lessons_by_tool_or_app, search_lessons_scored};
use crate::model::{Fact, Lesson, MemoryKind, MemoryRef, Playbook, category};
use crate::playbooks::search_playbooks_scored;
use crate::text::{fold, truncate_chars};

/// Бюджет блока «Память» по умолчанию (план, раздел 6).
pub const DEFAULT_BUDGET_TOKENS: usize = 1500;
pub const FACTS_K: usize = 6;
pub const IMPORTANT_FACTS_K: usize = 5;
pub const LESSONS_K: usize = 5;
pub const PLAYBOOKS_K: usize = 3;
/// Сколько фактов о манере общения (`style`) всегда идёт в контекст.
pub const STYLE_FACTS_K: usize = 5;
/// Записи с доверием ниже не попадают в контекст.
const MIN_CONFIDENCE: f64 = 0.1;

/// Что память предлагает агенту перед ходом.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryContext {
    /// Сначала всегда важные (предпочтения), затем релевантные запросу. Без категории `style`.
    pub facts: Vec<Fact>,
    /// Как пользователь общается (категория `style`, ≤ [`STYLE_FACTS_K`], свежие первыми) — всегда в
    /// контексте, отдельным блоком «Как с ним общаться».
    #[serde(default)]
    pub style: Vec<Fact>,
    pub lessons: Vec<Lesson>,
    pub playbooks: Vec<Playbook>,
}

/// Грубая оценка токенов: ~3 символа на токен (для кириллицы это с запасом).
pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(3)
}

fn is_style(f: &Fact) -> bool {
    f.category == category::STYLE
}

/// Как в SQL `important_facts_conn`: `important` или `preference`, но не `style` (у тех свой блок).
fn is_important(f: &Fact) -> bool {
    !is_style(f) && (f.important || f.category == category::PREFERENCE)
}

fn normalize_scores<T>(items: &[(T, f64)]) -> f64 {
    items.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max).max(1e-9)
}

impl Memory {
    /// Топ фактов (+ всегда важные предпочтения), уроки k=5 по инструменту/приложению/словам,
    /// плейбуки k=3.
    pub fn retrieve_context(&self, query: &str, tool: Option<&str>, app: Option<&str>) -> Result<MemoryContext> {
        self.read(|c| retrieve_conn(c, query, tool, app))
    }

    /// Асинхронная обёртка (spawn_blocking).
    pub async fn retrieve_context_async(
        &self,
        query: String,
        tool: Option<String>,
        app: Option<String>,
    ) -> Result<MemoryContext> {
        self.run(move |m| m.retrieve_context(&query, tool.as_deref(), app.as_deref())).await
    }
}

fn retrieve_conn(c: &rusqlite::Connection, query: &str, tool: Option<&str>, app: Option<&str>) -> Result<MemoryContext> {
    {
        // --- факты
        // Факты `style` живут только в своём блоке: важные/релевантные их не дублируют (SQL важных их
        // исключает, здесь — релевантные по запросу).
        let style = style_facts_conn(c, STYLE_FACTS_K, MIN_CONFIDENCE)?;
        let mut facts = important_facts_conn(c, IMPORTANT_FACTS_K)?;
        let mut relevant = 0;
        for (f, _) in search_facts_scored(c, query, FACTS_K + STYLE_FACTS_K)? {
            if relevant >= FACTS_K {
                break;
            }
            if f.confidence >= MIN_CONFIDENCE && !is_style(&f) && !facts.iter().any(|x| x.id == f.id) {
                facts.push(f);
                relevant += 1;
            }
        }

        // --- уроки: инструмент +3, приложение +2, слова до +2, частота и доверие
        let mut lessons: HashMap<i64, (Lesson, f64)> = HashMap::new();
        let tool_key = tool.map(str::trim).filter(|t| !t.is_empty());
        let app_key = app.map(|a| fold(a.trim())).filter(|a| !a.is_empty());
        for l in lessons_by_tool_or_app(c, tool_key, app_key.as_deref(), 20)? {
            let mut s = 0.0;
            if tool_key.is_some() && l.tool.as_deref() == tool_key {
                s += 3.0;
            }
            if app_key.is_some() && l.app.as_deref().map(|a| fold(a.trim())) == app_key {
                s += 2.0;
            }
            lessons.insert(l.id, (l, s));
        }
        let hits = search_lessons_scored(c, query, 15)?;
        let max = normalize_scores(&hits);
        for (l, score) in hits {
            let add = 2.0 * score / max;
            lessons.entry(l.id).and_modify(|e| e.1 += add).or_insert((l, add));
        }
        let mut lessons: Vec<(Lesson, f64)> = lessons
            .into_values()
            .filter(|(l, _)| l.confidence >= MIN_CONFIDENCE)
            .map(|(l, s)| {
                let s = s + 0.3 * ((1 + l.seen) as f64).ln() + l.confidence + if l.has_fix() { 0.3 } else { 0.0 };
                (l, s)
            })
            .collect();
        lessons.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.last_seen.cmp(&a.0.last_seen)));
        let lessons = lessons.into_iter().take(LESSONS_K).map(|(l, _)| l).collect();

        // --- плейбуки
        let hits = search_playbooks_scored(c, query, 10)?;
        let max = normalize_scores(&hits);
        let mut playbooks: Vec<(Playbook, f64)> = hits
            .into_iter()
            .filter(|(p, _)| !(p.fail_count >= 3 && p.fail_count > 2 * p.success_count))
            .map(|(p, s)| {
                let score = 2.0 * s / max + if p.promoted { 1.0 } else { 0.0 } + p.success_rate();
                (p, score)
            })
            .collect();
        playbooks.sort_by(|a, b| b.1.total_cmp(&a.1));
        let playbooks = playbooks.into_iter().take(PLAYBOOKS_K).map(|(p, _)| p).collect();

        Ok(MemoryContext { facts, style, lessons, playbooks })
    }
}

const HEADER: &str = "Память:";
/// Заголовок блока фактов `style`.
pub const STYLE_HEADING: &str = "Как с ним общаться (так пользователь говорит и вот что его раздражает — подстраивай тон и манеру под это):";
const SECTIONS: [&str; 4] = [
    STYLE_HEADING,
    "Факты о пользователе:",
    "Уроки из прошлых ошибок (не повторяй их):",
    "Проверенные сценарии (плейбуки):",
];
const MAX_LINE_CHARS: usize = 420;
/// Меньше этого остатка строку не обрезаем — только пропускаем.
const MIN_TRUNCATED_TOKENS: usize = 12;

fn fact_line(f: &Fact) -> String {
    format!("- {}", f.text)
}

fn lesson_line(l: &Lesson) -> String {
    let mut s = String::from("- ");
    let tags: Vec<&str> = [l.tool.as_deref(), l.app.as_deref()].into_iter().flatten().collect();
    if !tags.is_empty() {
        s.push_str(&format!("[{}] ", tags.join(" · ")));
    }
    let mut parts = Vec::new();
    if !l.trigger.is_empty() {
        parts.push(format!("Ситуация: {}", l.trigger.trim_end_matches('.')));
    }
    if !l.mistake.is_empty() {
        parts.push(format!("Ошибка: {}", l.mistake.trim_end_matches('.')));
    } else if let Some(e) = &l.error_text {
        parts.push(format!("Ошибка: {}", truncate_chars(e, 160)));
    }
    if !l.cause.is_empty() {
        parts.push(format!("Причина: {}", l.cause.trim_end_matches('.')));
    }
    if l.has_fix() {
        parts.push(format!("Как правильно: {}", l.fix.trim_end_matches('.')));
    } else {
        parts.push("Как правильно: пока неизвестно — выбери другой путь".into());
    }
    s.push_str(&parts.join(". "));
    if l.seen > 1 {
        s.push_str(&format!(" (уже встречалось {} раз)", l.seen));
    } else {
        s.push('.');
    }
    s
}

fn playbook_line(p: &Playbook) -> String {
    let mut s = format!("- «{}»", p.name);
    if !p.when.is_empty() {
        s.push_str(&format!(" — когда: {}", p.when.trim_end_matches('.')));
    }
    let steps: Vec<String> = p
        .steps
        .iter()
        .enumerate()
        .map(|(i, st)| {
            let args = match &st.args {
                serde_json::Value::Object(m) if m.is_empty() => String::new(),
                serde_json::Value::Null => String::new(),
                a => format!(" {}", truncate_chars(&a.to_string(), 100)),
            };
            format!("{}) {}{}", i + 1, st.tool, args)
        })
        .collect();
    s.push_str(&format!(". Шаги: {}", steps.join("; ")));
    s.push_str(&format!(" (успехов {}, неудач {})", p.success_count, p.fail_count));
    s
}

impl MemoryContext {
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.style.is_empty() && self.lessons.is_empty() && self.playbooks.is_empty()
    }

    /// Только блок «Как с ним общаться» (без «Память:»), не длиннее `budget_tokens`; пусто, если фактов нет.
    /// Для промпта, который ставит его отдельно (сразу после характера) — тогда перед [`Self::render`]
    /// очистите `style`, чтобы не повторять.
    pub fn render_style(&self, budget_tokens: usize) -> String {
        let only = MemoryContext { style: self.style.clone(), ..Default::default() };
        let full = only.render(budget_tokens + estimate_tokens(HEADER) + 1);
        full.strip_prefix(HEADER).map(|s| s.trim_start_matches('\n').to_string()).unwrap_or_default()
    }

    /// Ссылки на все записи — передать в [`Memory::mark_used`] после хода.
    pub fn refs(&self) -> Vec<MemoryRef> {
        let f = self.style.iter().chain(&self.facts).map(|x| MemoryRef { kind: MemoryKind::Fact, id: x.id });
        let l = self.lessons.iter().map(|x| MemoryRef { kind: MemoryKind::Lesson, id: x.id });
        let p = self.playbooks.iter().map(|x| MemoryRef { kind: MemoryKind::Playbook, id: x.id });
        f.chain(l).chain(p).collect()
    }

    /// Русский блок «Память:» не длиннее `budget_tokens` (по [`estimate_tokens`]).
    ///
    /// Группы по приоритету: манера общения (`style`, своя секция первой) → важные факты → уроки →
    /// остальные факты → плейбуки. Сначала каждая группа получает свою долю бюджета (15 % на `style`,
    /// если он есть; остальное — 35/40/10/15 %) — так длинные факты не вытесняют уроки; затем
    /// остаток заполняется по приоритету. Не влезающая строка обрезается по символам с «…»
    /// (если остаётся ≥12 токенов), иначе пропускается. Пустая память → пустая строка.
    pub fn render(&self, budget_tokens: usize) -> String {
        // (секция, группа, строка)
        let mut cands: Vec<(usize, usize, String)> = Vec::new();
        // факты `style` в `facts` (если контекст собран вручную) тоже уходят в свою секцию, без дублей
        let mut style: Vec<&Fact> = self.style.iter().take(STYLE_FACTS_K).collect();
        for f in self.facts.iter().filter(|f| is_style(f)) {
            if style.len() < STYLE_FACTS_K && !style.iter().any(|x| x.id == f.id) {
                style.push(f);
            }
        }
        cands.extend(style.iter().map(|f| (0, 0, fact_line(f))));
        cands.extend(self.facts.iter().filter(|f| is_important(f)).map(|f| (1, 1, fact_line(f))));
        cands.extend(self.lessons.iter().map(|l| (2, 2, lesson_line(l))));
        cands.extend(self.facts.iter().filter(|f| !is_important(f) && !is_style(f)).map(|f| (1, 3, fact_line(f))));
        cands.extend(self.playbooks.iter().map(|p| (3, 4, playbook_line(p))));
        for c in &mut cands {
            c.2 = truncate_chars(&c.2, MAX_LINE_CHARS);
        }

        let mut st = RenderState { used: estimate_tokens(HEADER), open: [false; 4], picked: vec![None; cands.len()] };
        let style_share = if style.is_empty() { 0.0 } else { STYLE_SHARE };
        let shares = std::iter::once(style_share).chain(GROUP_SHARES.iter().map(|s| s * (1.0 - style_share)));
        // проход 1: доли групп; обрезать строку можно только чтобы группа не осталась пустой
        for (group, share) in shares.enumerate() {
            let limit = (st.used + (budget_tokens as f64 * share) as usize).min(budget_tokens);
            let mut group_has = false;
            for (i, (sec, g, line)) in cands.iter().enumerate() {
                if *g == group && st.try_add(i, *sec, line, limit, !group_has) {
                    group_has = true;
                }
            }
        }
        // проход 2: остаток бюджета по приоритету
        for (i, (sec, _, line)) in cands.iter().enumerate() {
            if st.picked[i].is_none() {
                st.try_add(i, *sec, line, budget_tokens, true);
            }
        }

        if st.picked.iter().all(Option::is_none) {
            return String::new();
        }
        let mut out = String::from(HEADER);
        for (sec, title) in SECTIONS.iter().enumerate() {
            let lines: Vec<&String> =
                cands.iter().zip(&st.picked).filter(|((s, _, _), p)| *s == sec && p.is_some()).filter_map(|(_, p)| p.as_ref()).collect();
            if lines.is_empty() {
                continue;
            }
            out.push('\n');
            out.push_str(title);
            for l in lines {
                out.push('\n');
                out.push_str(l);
            }
        }
        out
    }
}

/// Доли бюджета групп в первом проходе: важные факты, уроки, остальные факты, плейбуки.
const GROUP_SHARES: [f64; 4] = [0.35, 0.40, 0.10, 0.15];
/// Доля блока `style` (остальные доли сжимаются пропорционально, если он не пуст).
const STYLE_SHARE: f64 = 0.15;

struct RenderState {
    used: usize,
    open: [bool; 4],
    picked: Vec<Option<String>>,
}

impl RenderState {
    /// Добавить строку `i`, не выходя за `limit`. Стоимость: строка + перевод строки
    /// (+ заголовок секции, если она ещё не открыта). Сумма оценок по частям ≥ оценки целого,
    /// поэтому итоговый текст гарантированно укладывается в бюджет.
    fn try_add(&mut self, i: usize, sec: usize, line: &str, limit: usize, allow_cut: bool) -> bool {
        if self.picked[i].is_some() {
            return false;
        }
        let sec_cost = if self.open[sec] { 0 } else { estimate_tokens(SECTIONS[sec]) + 1 };
        let cost = sec_cost + estimate_tokens(line) + 1;
        let chosen = if self.used + cost <= limit {
            line.to_string()
        } else {
            let avail = limit.saturating_sub(self.used + sec_cost + 1);
            if !allow_cut || avail < MIN_TRUNCATED_TOKENS {
                return false;
            }
            truncate_chars(line, avail * 3)
        };
        self.used += sec_cost + estimate_tokens(&chosen) + 1;
        self.open[sec] = true;
        self.picked[i] = Some(chosen);
        true
    }
}
