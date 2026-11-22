//! Интеграционные тесты крейта (внутри крейта — чтобы можно было «состарить» записи SQL-ом).
//! Базы создаются только во временных каталогах.

mod consolidate_tests;
mod lessons_tests;
mod reflect_tests;
mod retrieve_tests;
mod storage_tests;
mod tools_tests;

use async_trait::async_trait;
use jarvis_core::{CancellationToken, ChatMessage, ChatModel, ChatRequest, ChatResponse, LlmError};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::Memory;

pub(crate) fn temp_mem() -> (tempfile::TempDir, Memory) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mem = Memory::open(dir.path().join("data").join("memory.db")).expect("open");
    (dir, mem)
}

/// Выполнить произвольный SQL (например, «состарить» запись).
pub(crate) fn exec(mem: &Memory, sql: &str, params: impl rusqlite::Params) -> usize {
    mem.conn().execute(sql, params).expect(sql)
}

pub(crate) const DAY: i64 = 86_400;

/// Мок модели: возвращает заданный текст, запоминает запросы.
pub(crate) struct MockModel {
    pub reply: Result<String, LlmError>,
    pub calls: AtomicUsize,
    pub last: Mutex<Option<ChatRequest>>,
}

impl MockModel {
    pub fn ok(reply: impl Into<String>) -> Self {
        Self { reply: Ok(reply.into()), calls: AtomicUsize::new(0), last: Mutex::new(None) }
    }
    pub fn err(e: LlmError) -> Self {
        Self { reply: Err(e), calls: AtomicUsize::new(0), last: Mutex::new(None) }
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ChatModel for MockModel {
    fn id(&self) -> String {
        "mock/reflector".into()
    }
    async fn chat(&self, req: ChatRequest, _cancel: CancellationToken) -> Result<ChatResponse, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some(req);
        let text = self.reply.clone()?;
        Ok(ChatResponse { message: ChatMessage::assistant(text), model: self.id(), usage: None })
    }
}
mod zz_repro;
