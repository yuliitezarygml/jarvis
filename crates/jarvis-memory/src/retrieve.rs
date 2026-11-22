//! Выбор памяти перед ходом агента и блок «Память:» для системного промпта.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::db::Memory;
use crate::error::Result;
use crate::facts::{important_facts_conn, search_facts_scored};
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
/// Записи с доверием ниже не попадают в контекст.
const MIN_CONFIDENCE: f64 = 0.1;

/// Что память предлагает агенту перед ходом.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryContext {
    /// Сначала всегда важные (предпочтения), затем релевантные запросу.
    pub facts: Vec<Fact>,
    pub lessons: Vec<Lesson>,
    pub playbooks: Vec<Playbook>,
}

/// Грубая оценка токенов: ~3 символа на токен (для кириллицы это с запасом).
pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(3)
}

fn is_important(f: &Fact) -> bool {
    f.important || f.category == category::PREFERENCE
}

fn normalize_scores<T>(items: &[(T, f64)]) -> f64 {
    items.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max).max(1e-9)
}

impl Memory {
    /// Топ фактов (+ всегда важные предпочтения), уроки k=5 по инструменту/приложению/словам,
    /// плейбуки k=3.
    pub fn retrieve_context(&self, query: &str, tool: Option<&str>, app: Option<&str>) -> Result<MemoryContext> {
        let c = self.conn();

        // --- факты
        let mut facts = important_facts_conn(&c, IMPORTANT_FACTS_K)?;
        for (f, _) in search_facts_scored(&c, query, FACTS_K)? {
            if f.confidence >= MIN_CONFIDENCE && !facts.iter().any(|x| x.id == f.id) {
                facts.push(f);
            }
        }

        // --- уроки: инструмент +3, приложение +2, слова до +2, частота и доверие
        let mut lessons: HashMap<i64, (Lesson, f64)> = HashMap::new();
        let tool_key = tool.map(str::trim).filter(|t| !t.is_empty());
        let app_key = app.map(|a| fold(a.trim())).filter(|a| !a.is_empty());
        for l in lessons_by_tool_or_app(&c, tool_key, app_key.as_deref(), 20)? {
            let mut s = 0.0;
            if tool_key.is_some() && l.tool.as_deref() == tool_key {
                s += 3.0;
            }
            if app_key.is_some() && l.app.as_deref().map(|a| fold(a.trim())) == app_key {
                s += 2.0;
            }
            lessons.insert(l.id, (l, s));
        }
        let hits = search_lessons_scored(&c, query, 15)?;
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
        let hits = search_playbooks_scored(&c, query, 10)?;
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

        Ok(MemoryContext { facts, lessons, playbooks })
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

const HEADER: &str = "Память:";
const SECTIONS: [&str; 3] =
    ["Факты о пользователе:", "Уроки из прошлых ошибок (не повторяй их):", "Проверенные сценарии (плейбуки):"];
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
        self.facts.is_empty() && self.lessons.is_empty() && self.playbooks.is_empty()
    }

    /// Ссылки на все записи — передать в [`Memory::mark_used`] после хода.
    pub fn refs(&self) -> Vec<MemoryRef> {
        let f = self.facts.iter().map(|x| MemoryRef { kind: MemoryKind::Fact, id: x.id });
        let l = self.lessons.iter().map(|x| MemoryRef { kind: MemoryKind::Lesson, id: x.id });
        let p = self.playbooks.iter().map(|x| MemoryRef { kind: MemoryKind::Playbook, id: x.id });
        f.chain(l).chain(p).collect()
    }

    /// Русский блок «Память:» не длиннее `budget_tokens` (по [`estimate_tokens`]).
    ///
    /// Приоритет при нехватке места: важные факты → уроки → остальные факты → плейбуки.
    /// Длинные строки обрезаются по символам с «…», не влезающие — пропускаются.
    /// Пустая память → пустая строка.
    pub fn render(&self, budget_tokens: usize) -> String {
        let mut candidates: Vec<(usize, String)> = Vec::new();
        candidates.extend(self.facts.iter().filter(|f| is_important(f)).map(|f| (0, fact_line(f))));
        candidates.extend(self.lessons.iter().map(|l| (1, lesson_line(l))));
        candidates.extend(self.facts.iter().filter(|f| !is_important(f)).map(|f| (0, fact_line(f))));
        candidates.extend(self.playbooks.iter().map(|p| (2, playbook_line(p))));

        let mut used = estimate_tokens(HEADER);
        let mut picked: [Vec<String>; 3] = Default::default();
        for (sec, line) in candidates {
            let line = truncate_chars(&line, MAX_LINE_CHARS);
            let sec_cost = if picked[sec].is_empty() { estimate_tokens(SECTIONS[sec]) + 1 } else { 0 };
            let cost = sec_cost + estimate_tokens(&line) + 1;
            if used + cost <= budget_tokens {
                picked[sec].push(line);
                used += cost;
                continue;
            }
            let avail = budget_tokens.saturating_sub(used + sec_cost + 1);
            if avail >= MIN_TRUNCATED_TOKENS {
                let cut = truncate_chars(&line, avail * 3);
                used += sec_cost + estimate_tokens(&cut) + 1;
                picked[sec].push(cut);
            }
        }
        if picked.iter().all(Vec::is_empty) {
            return String::new();
        }
        let mut out = String::from(HEADER);
        for (i, lines) in picked.iter().enumerate() {
            if lines.is_empty() {
                continue;
            }
            out.push('\n');
            out.push_str(SECTIONS[i]);
            for l in lines {
                out.push('\n');
                out.push_str(l);
            }
        }
        out
    }
}
