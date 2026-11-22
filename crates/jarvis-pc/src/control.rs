//! Свечение «Джарвис управляет ПК» (план, раздел 4): события `ControlStart` / `ControlTarget` / `ControlStop`.
//!
//! Каждый инструмент T1+ открывает [`ControlSession`]: при создании — `ControlStart{tier}`, при `Drop` — `ControlStop`
//! (даже при ошибке или отмене). Если агент выполняет многошаговую задачу и не хочет мигания рамки между шагами,
//! он берёт [`GlowHold`] через [`GlowControl::hold`]: пока hold жив, инструменты шлют только `ControlTarget`,
//! а `ControlStop` уйдёт один раз — когда отпустят последний hold.

use jarvis_core::{Event, EventSink, Rect, RiskTier};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Общий для всех инструментов переключатель «свечение удерживает агент».
#[derive(Default)]
pub struct GlowControl {
    held: AtomicUsize,
}

impl GlowControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Удерживать свечение, пока жив возвращённый guard (шлёт `ControlStart` при первом hold).
    pub fn hold(self: &Arc<Self>, events: Arc<dyn EventSink>, tier: RiskTier) -> GlowHold {
        if self.held.fetch_add(1, Ordering::SeqCst) == 0 {
            events.emit(Event::ControlStart { tier });
        }
        GlowHold { glow: self.clone(), events }
    }

    pub fn is_held(&self) -> bool {
        self.held.load(Ordering::SeqCst) > 0
    }

    /// Сессия управления для одного вызова инструмента.
    pub fn session(&self, events: Arc<dyn EventSink>, tier: RiskTier) -> ControlSession {
        if !tier.glows() {
            return ControlSession { events, emits_bounds: false, glows: false };
        }
        let emits_bounds = !self.is_held();
        if emits_bounds {
            events.emit(Event::ControlStart { tier });
        }
        ControlSession { events, emits_bounds, glows: true }
    }
}

/// Агент держит свечение; `Drop` последнего hold шлёт `ControlStop`.
pub struct GlowHold {
    glow: Arc<GlowControl>,
    events: Arc<dyn EventSink>,
}

impl Drop for GlowHold {
    fn drop(&mut self) {
        if self.glow.held.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.events.emit(Event::ControlStop);
        }
    }
}

/// Guard одного действия: `ControlStart` при создании, `ControlStop` при `Drop` (если свечение не удерживает агент).
pub struct ControlSession {
    events: Arc<dyn EventSink>,
    emits_bounds: bool,
    glows: bool,
}

impl ControlSession {
    /// Самостоятельная сессия без общего [`GlowControl`].
    pub fn start(events: Arc<dyn EventSink>, tier: RiskTier) -> Self {
        GlowControl::default().session(events, tier)
    }

    /// Подсветить цель: «Нажимаю: Сохранить».
    pub fn target(&self, rect: Rect, label: impl Into<String>) {
        if self.glows {
            self.events.emit(Event::ControlTarget { rect, label: label.into() });
        }
    }

    /// Не гасить свечение при выходе — его погасит тот, кто держит (агент шлёт `ControlStop` сам).
    pub fn keep(mut self) {
        self.emits_bounds = false;
    }

    /// Явно завершить (то же, что `drop`).
    pub fn finish(self) {}
}

impl Drop for ControlSession {
    fn drop(&mut self) {
        if self.emits_bounds {
            self.events.emit(Event::ControlStop);
        }
    }
}

/// Квадрат вокруг точки — цель для подсветки, когда известна только координата.
pub(crate) fn point_rect(x: i32, y: i32) -> Rect {
    const HALF: i32 = 16;
    Rect { x: x - HALF, y: y - HALF, w: HALF * 2, h: HALF * 2 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::CollectSink;

    #[test]
    fn session_emits_start_target_stop() {
        let sink = Arc::new(CollectSink::default());
        let glow = GlowControl::new();
        {
            let s = glow.session(sink.clone(), RiskTier::T1);
            s.target(Rect { x: 1, y: 2, w: 3, h: 4 }, "Нажимаю: Сохранить");
        }
        let ev = sink.take();
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[0], Event::ControlStart { tier: RiskTier::T1 });
        assert!(matches!(&ev[1], Event::ControlTarget { label, .. } if label == "Нажимаю: Сохранить"));
        assert_eq!(ev[2], Event::ControlStop);
    }

    #[test]
    fn t0_does_not_glow() {
        let sink = Arc::new(CollectSink::default());
        let s = GlowControl::default().session(sink.clone(), RiskTier::T0);
        s.target(Rect::default(), "x");
        drop(s);
        assert!(sink.take().is_empty());
    }

    #[test]
    fn hold_suppresses_flicker() {
        let sink = Arc::new(CollectSink::default());
        let glow = GlowControl::new();
        let hold = glow.hold(sink.clone(), RiskTier::T2);
        for _ in 0..3 {
            let s = glow.session(sink.clone(), RiskTier::T1);
            s.target(Rect::default(), "шаг");
        }
        let nested = glow.hold(sink.clone(), RiskTier::T1);
        drop(nested);
        let ev = sink.take();
        assert_eq!(ev[0], Event::ControlStart { tier: RiskTier::T2 });
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::ControlTarget { .. })).count(), 3);
        assert!(!ev.contains(&Event::ControlStop));
        drop(hold);
        assert_eq!(sink.take(), vec![Event::ControlStop]);
        assert!(!glow.is_held());
    }

    #[test]
    fn keep_leaves_glow_on() {
        let sink = Arc::new(CollectSink::default());
        let s = ControlSession::start(sink.clone(), RiskTier::T1);
        s.keep();
        assert_eq!(sink.take(), vec![Event::ControlStart { tier: RiskTier::T1 }]);
    }
}
