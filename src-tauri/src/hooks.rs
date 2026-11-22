//! Точки подключения бэкенда к интерфейсу. Сейчас — заглушки; интегратор подменяет `UiHooks`
//! своей реализацией (агент, политика рисков, голос) через `AppHooks::set`.

use std::sync::{Arc, Mutex, RwLock};

use jarvis_core::CancellationToken;

/// Что интерфейс просит у бэкенда.
pub trait UiHooks: Send + Sync {
    /// Ctrl+Alt+K / кнопка «Стоп»: отменить текущую задачу, отпустить клавиши/кнопки мыши, остановить TTS.
    fn emergency_stop(&self, source: &str);
    /// Ответ пользователя на `ConfirmRequest` (кнопки в HUD или таймаут).
    fn confirm(&self, id: &str, approved: bool);
}

/// Заглушка: держит общий `CancellationToken` (его же получает симулятор) и пишет в лог.
#[derive(Default)]
pub struct StubHooks {
    token: Mutex<CancellationToken>,
}

impl StubHooks {
    /// Текущий токен отмены. После `emergency_stop` выдаётся новый.
    pub fn token(&self) -> CancellationToken {
        self.token.lock().unwrap().clone()
    }
}

impl UiHooks for StubHooks {
    fn emergency_stop(&self, source: &str) {
        let mut t = self.token.lock().unwrap();
        t.cancel();
        *t = CancellationToken::new();
        eprintln!("[jarvis] аварийная остановка ({source}) — заглушка: токен отмены сброшен");
    }

    fn confirm(&self, id: &str, approved: bool) {
        eprintln!("[jarvis] подтверждение {id}: {} — заглушка", if approved { "да" } else { "нет" });
    }
}

/// Состояние Tauri: текущая реализация хуков + токен отмены для демо/симулятора.
pub struct AppHooks {
    inner: RwLock<Arc<dyn UiHooks>>,
    stub: Arc<StubHooks>,
}

impl Default for AppHooks {
    fn default() -> Self {
        let stub = Arc::new(StubHooks::default());
        Self { inner: RwLock::new(stub.clone()), stub }
    }
}

impl AppHooks {
    /// Подменить реализацию (интегратор).
    pub fn set(&self, hooks: Arc<dyn UiHooks>) {
        *self.inner.write().unwrap() = hooks;
    }

    pub fn get(&self) -> Arc<dyn UiHooks> {
        self.inner.read().unwrap().clone()
    }

    /// Токен отмены для симулятора: отменяется при любой аварийной остановке.
    pub fn demo_token(&self) -> CancellationToken {
        self.stub.token()
    }

    pub fn emergency_stop(&self, source: &str) {
        // Симулятор (токен заглушки) отменяем всегда, даже если реализацию подменили.
        self.stub.emergency_stop(source);
        let current = self.get();
        let is_stub = std::ptr::addr_eq(Arc::as_ptr(&current), Arc::as_ptr(&self.stub));
        if !is_stub {
            current.emergency_stop(source);
        }
    }

    pub fn confirm(&self, id: &str, approved: bool) {
        self.get().confirm(id, approved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn stop_cancels_and_renews_token() {
        let h = AppHooks::default();
        let t1 = h.demo_token();
        h.emergency_stop("test");
        assert!(t1.is_cancelled());
        assert!(!h.demo_token().is_cancelled());
    }

    #[derive(Default)]
    struct Counting(AtomicUsize, AtomicUsize);
    impl UiHooks for Counting {
        fn emergency_stop(&self, _s: &str) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn confirm(&self, _id: &str, _a: bool) {
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn replaced_hooks_receive_calls_and_demo_still_stops() {
        let h = AppHooks::default();
        let c = Arc::new(Counting::default());
        h.set(c.clone());
        let t = h.demo_token();
        h.emergency_stop("hotkey");
        h.confirm("x", true);
        assert_eq!(c.0.load(Ordering::SeqCst), 1);
        assert_eq!(c.1.load(Ordering::SeqCst), 1);
        assert!(t.is_cancelled());
    }
}
