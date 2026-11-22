//! Асинхронные обёртки горячего пути агента (spawn_blocking). Остальное — через [`Memory::run`].

use jarvis_core::Role;

use crate::db::Memory;
use crate::error::Result;
use crate::model::{Lesson, LessonMatch, MemoryRef, NewAction, NewFact, NewLesson, Saved};

impl Memory {
    pub async fn upsert_fact_async(&self, f: NewFact) -> Result<Saved> {
        self.run(move |m| m.upsert_fact(f)).await
    }

    pub async fn add_lesson_async(&self, l: NewLesson) -> Result<Saved> {
        self.run(move |m| m.add_lesson(l)).await
    }

    pub async fn lesson_for_signature_async(&self, sig: String) -> Result<Option<Lesson>> {
        self.run(move |m| m.lesson_for_signature(&sig)).await
    }

    pub async fn lesson_for_error_async(&self, error_text: String) -> Result<Option<LessonMatch>> {
        self.run(move |m| m.lesson_for_error(&error_text)).await
    }

    pub async fn record_action_async(&self, a: NewAction) -> Result<i64> {
        self.run(move |m| m.record_action(a)).await
    }

    pub async fn log_message_async(
        &self,
        session_id: i64,
        role: Role,
        content: String,
        tool_name: Option<String>,
    ) -> Result<i64> {
        self.run(move |m| m.log_message(session_id, role, &content, tool_name.as_deref())).await
    }

    pub async fn mark_used_async(&self, refs: Vec<MemoryRef>) -> Result<()> {
        self.run(move |m| m.mark_used(&refs)).await
    }
}
