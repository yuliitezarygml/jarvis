//! Граница движка распознавания. Все методы блокирующие (вычисления на CPU): сервер вызывает их
//! из `spawn_blocking`. Реализации: [`crate::sherpa`] (feature `sherpa`) и [`MockEngine`] для тестов.

use std::time::Duration;

pub trait Engine: Send + Sync + 'static {
    /// Имя модели для `Health.model` и `ServerMsg::Ready`.
    fn model(&self) -> &str;
    /// Новая потоковая сессия (одна фраза за раз, после `finish`/`reset` — следующая).
    fn open(&self) -> anyhow::Result<Box<dyn Recognition>>;
    /// Распознать целую запись (16 кГц mono).
    fn transcribe(&self, samples: &[f32]) -> anyhow::Result<String>;
}

pub trait Recognition: Send {
    /// Принять очередной кусок 16 кГц mono, вернуть текущий промежуточный текст (может быть пустым).
    fn accept(&mut self, samples: &[f32]) -> anyhow::Result<String>;
    /// Конец фразы: окончательный текст; состояние сбрасывается для следующей фразы.
    fn finish(&mut self) -> anyhow::Result<String>;
    /// Сброс без результата.
    fn reset(&mut self);
}

/// Детерминированный тестовый движок: каждые 200 мс звука (3200 сэмплов) — одно «слово».
pub struct MockEngine {
    pub model: String,
    pub finish_delay: Duration,
}

impl Default for MockEngine {
    fn default() -> Self {
        Self { model: "mock".into(), finish_delay: Duration::ZERO }
    }
}

impl MockEngine {
    pub fn text_for(samples: usize) -> String {
        vec!["слово"; samples / 3200].join(" ")
    }
}

struct MockRecognition {
    samples: usize,
    delay: Duration,
}

impl Engine for MockEngine {
    fn model(&self) -> &str {
        &self.model
    }
    fn open(&self) -> anyhow::Result<Box<dyn Recognition>> {
        Ok(Box::new(MockRecognition { samples: 0, delay: self.finish_delay }))
    }
    fn transcribe(&self, samples: &[f32]) -> anyhow::Result<String> {
        Ok(Self::text_for(samples.len()))
    }
}

impl Recognition for MockRecognition {
    fn accept(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        self.samples += samples.len();
        Ok(MockEngine::text_for(self.samples))
    }
    fn finish(&mut self) -> anyhow::Result<String> {
        std::thread::sleep(self.delay);
        Ok(MockEngine::text_for(std::mem::take(&mut self.samples)))
    }
    fn reset(&mut self) {
        self.samples = 0;
    }
}
