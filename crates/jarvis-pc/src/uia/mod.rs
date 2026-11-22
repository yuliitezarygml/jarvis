//! UI Automation: выделенный поток с COM (MTA) и каналом команд. Кэш элементов последнего снимка живёт в потоке,
//! копия снимка — в [`Uia::last_snapshot`] (для меток set-of-marks и клика мышью по id).

pub mod element;
pub(crate) mod tools;
pub(crate) mod worker;

use jarvis_core::{Rect, ToolError};
use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use element::{UiElement, UiSnapshot};
use worker::UiaState;

use crate::com::ComInit;
use crate::dpi::DpiScope;

type Job = Box<dyn FnOnce(Result<&mut UiaState, ToolError>) + Send>;

/// Таймаут по умолчанию для одной команды UIA (зависшее приложение может блокировать вызовы).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Uia {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
    last: Mutex<Option<Arc<UiSnapshot>>>,
}

impl Uia {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { tx: Mutex::new(None), last: Mutex::new(None) })
    }

    fn spawn() -> Result<mpsc::Sender<Job>, ToolError> {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("jarvis-uia".into())
            .spawn(move || {
                let _dpi = DpiScope::per_monitor();
                let _com = ComInit::mta();
                let mut state =
                    UiaState::new().map_err(|e| ToolError::Unsupported(format!("UI Automation недоступен: {e}")));
                while let Ok(job) = rx.recv() {
                    let arg = state.as_mut().map_err(|e| e.clone());
                    // Паника в одной команде не должна убивать поток (ответ просто не придёт).
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(move || job(arg)));
                }
            })
            .map_err(|e| ToolError::Failed(format!("не удалось запустить поток UIA: {e}")))?;
        Ok(tx)
    }

    fn sender(&self) -> Result<mpsc::Sender<Job>, ToolError> {
        let mut g = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(Self::spawn()?);
        }
        Ok(g.as_ref().expect("sender").clone())
    }

    /// Сбросить поток (после таймаута/падения): следующий вызов создаст новый. Кэш id теряется.
    fn reset(&self) {
        *self.tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Выполнить функцию в потоке UIA.
    pub(crate) async fn call<R, F>(&self, timeout: Duration, f: F) -> Result<R, ToolError>
    where
        R: Send + 'static,
        F: FnOnce(&mut UiaState) -> Result<R, ToolError> + Send + 'static,
    {
        let (rtx, rrx) = tokio::sync::oneshot::channel::<Result<R, ToolError>>();
        let job: Job = Box::new(move |st| {
            let _ = rtx.send(st.and_then(f));
        });
        if let Err(mpsc::SendError(job)) = self.sender()?.send(job) {
            // Поток умер — пересоздаём и пробуем ещё раз.
            self.reset();
            self.sender()?.send(job).map_err(|_| ToolError::Failed("поток UIA недоступен".into()))?;
        }
        match tokio::time::timeout(timeout, rrx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => {
                self.reset();
                Err(ToolError::Failed("команда UIA прервалась (сбой в потоке UIA)".into()))
            }
            Err(_) => {
                self.reset();
                Err(ToolError::Timeout)
            }
        }
    }

    /// Копия последнего снимка (id → роль/имя/прямоугольник).
    pub fn last_snapshot(&self) -> Option<Arc<UiSnapshot>> {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn set_last(&self, s: UiSnapshot) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(s));
    }

    /// Элемент последнего снимка по id.
    pub fn element(&self, id: u32) -> Option<UiElement> {
        self.last_snapshot().and_then(|s| s.elements.iter().find(|e| e.id == id).cloned())
    }

    /// Актуальный прямоугольник и имя элемента (из потока UIA; при сбое — из снимка).
    pub(crate) async fn element_rect(&self, id: u32) -> Result<(Rect, String, String), ToolError> {
        match self.call(Duration::from_secs(5), move |s| s.describe(id)).await {
            Ok(r) => Ok(r),
            Err(e @ ToolError::InvalidArgs(_)) => Err(e),
            Err(e) => self.element(id).map(|el| (el.rect, el.name, el.role)).ok_or(e),
        }
    }

    /// Снимок окна (`window` — см. [`crate::win::resolve_window`]).
    pub async fn snapshot(&self, window: Option<String>, limit: usize, include_text: bool) -> Result<Arc<UiSnapshot>, ToolError> {
        let snap = self
            .call(DEFAULT_TIMEOUT, move |s| {
                let win = crate::win::resolve_window(window.as_deref())?;
                s.snapshot(win, limit, include_text)
            })
            .await?;
        self.set_last(snap);
        Ok(self.last_snapshot().expect("снимок"))
    }
}
