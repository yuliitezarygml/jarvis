//! Симулятор бэкенда: прогоняет все состояния и события, чтобы весь UI можно было увидеть без агента.
//! Команда `simulate_demo` проигрывает `demo_script()` через тот же `TauriSink`, что и настоящий бэкенд.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jarvis_core::event::{ActionStatus, AudioSource, SourceLink, Speaker};
use jarvis_core::{AssistantState, CancellationToken, Event, EventSink, Rect, RiskTier};

/// Детерминированный «шум» для волны и телеметрии (без внешних крейтов).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32) / (u32::MAX >> 1) as f32
    }
}

struct Script {
    items: Vec<(u64, Event)>,
    rng: Lcg,
}

impl Script {
    fn at(&mut self, t: u64, e: Event) {
        self.items.push((t, e));
    }

    fn state(&mut self, t: u64, s: AssistantState) {
        self.at(t, Event::State { state: s });
    }

    /// Волна звука: ~30 кадров/с, огибающая похожа на речь (слоги + паузы).
    fn audio(&mut self, from: u64, to: u64, source: AudioSource, loud: f32) {
        let mut t = from;
        while t < to {
            let phase = (t - from) as f32 / 1000.0;
            let syllable = (phase * 7.3).sin().abs();
            let breath = 0.6 + 0.4 * (phase * 1.7).sin();
            let rms = (loud * syllable * breath + 0.06 * self.rng.next()).clamp(0.0, 1.0);
            self.at(t, Event::AudioLevel { source, rms });
            t += 33;
        }
        self.at(to, Event::AudioLevel { source, rms: 0.0 });
    }

    /// Субтитры, нарастающие по словам; последний кадр — `is_final`.
    fn subtitle(&mut self, from: u64, speaker: Speaker, text: &str, word_ms: u64) -> u64 {
        let words: Vec<&str> = text.split(' ').collect();
        let mut t = from;
        for i in 1..=words.len() {
            let is_final = i == words.len();
            self.at(t, Event::Subtitle { speaker, text: words[..i].join(" "), is_final });
            t += word_ms;
        }
        t
    }

    fn action(&mut self, t: u64, id: &str, tool: &str, summary: &str, status: ActionStatus) {
        self.at(t, Event::Action { id: id.into(), tool: tool.into(), summary: summary.into(), status });
    }

    fn telemetry(&mut self, from: u64, to: u64) {
        let mut t = from;
        let mut cpu: f32 = 9.0;
        let mut ram: u64 = 9_800;
        while t <= to {
            cpu = (cpu + (self.rng.next() - 0.45) * 14.0).clamp(3.0, 96.0);
            ram = (ram as i64 + ((self.rng.next() - 0.5) * 300.0) as i64).clamp(6_000, 15_500) as u64;
            let gpu = Some((cpu * 0.6 + self.rng.next() * 10.0).clamp(0.0, 100.0));
            self.at(t, Event::Telemetry { cpu, ram_used_mb: ram, ram_total_mb: 16_384, gpu });
            t += 1000;
        }
    }
}

/// Полный сценарий демо: `(время от старта в мс, событие)`, отсортировано по времени.
pub fn demo_script() -> Vec<(u64, Event)> {
    use ActionStatus::*;
    use AssistantState::*;
    let mut s = Script { items: Vec::new(), rng: Lcg(0x4A_52_56_53) };

    // 0 — ожидание
    s.state(0, Idle);

    // 1 — слушает
    s.state(1_500, Listening);
    s.audio(1_500, 4_300, AudioSource::Mic, 0.85);
    s.subtitle(1_700, Speaker::User, "Джарвис, открой блокнот и напиши привет", 330);

    // 2 — думает
    s.state(4_400, Thinking);

    // 3 — говорит
    s.state(6_000, Speaking);
    s.audio(6_000, 7_900, AudioSource::Tts, 0.7);
    s.subtitle(6_050, Speaker::Jarvis, "Сию минуту, сэр.", 380);

    // 4 — управляет ПК (T1): цели с прямоугольниками
    s.state(8_000, Controlling);
    s.at(8_000, Event::ControlStart { tier: RiskTier::T1 });
    s.action(8_100, "demo-1", "open_target", "Открываю Блокнот", Running);
    s.at(8_300, Event::ControlTarget { rect: Rect { x: 0, y: 1032, w: 48, h: 48 }, label: "Нажимаю: Пуск".into() });
    s.at(9_400, Event::ControlTarget { rect: Rect { x: 620, y: 180, w: 680, h: 52 }, label: "Ввожу: блокнот".into() });
    s.action(10_400, "demo-1", "open_target", "Открыл Блокнот", Done);
    s.action(10_500, "demo-2", "ui_type", "Пишу «привет»", Running);
    s.at(10_500, Event::ControlTarget { rect: Rect { x: 360, y: 260, w: 1200, h: 560 }, label: "Ввожу: привет".into() });
    s.action(11_900, "demo-2", "ui_type", "Написал «привет»", Done);
    s.action(12_000, "demo-3", "ui_click", "Сохраняю документ", Running);
    s.at(12_000, Event::ControlTarget { rect: Rect { x: 1180, y: 860, w: 120, h: 36 }, label: "Нажимаю: Сохранить".into() });
    s.action(13_200, "demo-3", "ui_click", "Документ сохранён", Done);
    s.at(13_400, Event::ControlStop);

    // 5 — интернет: поиск и источники
    s.state(13_500, Thinking);
    s.action(13_600, "demo-4", "web_search", "Ищу: что нового в Rust", Running);
    s.action(15_000, "demo-4", "web_search", "Нашёл 8 результатов", Done);
    s.at(
        15_100,
        Event::Sources {
            items: vec![
                SourceLink { title: "Rust Blog — Announcing Rust 1.100".into(), url: "https://blog.rust-lang.org/".into() },
                SourceLink { title: "This Week in Rust".into(), url: "https://this-week-in-rust.org/".into() },
                SourceLink { title: "Rust Release Notes".into(), url: "https://doc.rust-lang.org/releases.html".into() },
            ],
        },
    );
    s.state(15_300, Speaking);
    s.audio(15_300, 19_000, AudioSource::Tts, 0.75);
    s.subtitle(15_350, Speaker::Jarvis, "Вот что нашёл, сэр: вышла новая версия компилятора, источники на экране.", 300);

    // 6 — опасное действие (T3) с подтверждением
    s.state(19_400, Controlling);
    s.at(19_400, Event::ControlStart { tier: RiskTier::T3 });
    s.action(19_500, "demo-5", "service_stop", "Остановить службу Spooler", AwaitingConfirm);
    s.at(
        19_600,
        Event::ConfirmRequest {
            id: "demo-confirm".into(),
            summary: "Остановить службу печати (Spooler)?".into(),
            tier: RiskTier::T3,
            timeout_ms: 20_000,
        },
    );
    s.audio(19_700, 22_000, AudioSource::Tts, 0.6);
    s.subtitle(19_700, Speaker::Jarvis, "Сэр, остановить службу печати? Скажите «да» или нажмите кнопку.", 260);
    s.at(26_500, Event::ConfirmResolved { id: "demo-confirm".into(), approved: true });
    s.action(26_600, "demo-5", "service_stop", "Останавливаю службу Spooler", Running);
    s.at(26_700, Event::ControlTarget { rect: Rect { x: 700, y: 400, w: 520, h: 280 }, label: "Останавливаю: Spooler".into() });
    s.action(28_000, "demo-5", "service_stop", "Служба Spooler остановлена", Done);
    s.action(28_100, "demo-6", "powershell_run", "Проверка состояния службы — не удалось", Failed);
    s.at(28_300, Event::ControlStop);

    // 7 — ошибка
    s.at(28_600, Event::Error { message: "Сервер распознавания речи не отвечает (демо)".into() });
    s.state(28_600, Error);

    // 8 — снова ожидание
    s.state(31_500, Idle);
    s.action(31_600, "demo-7", "kb_search", "Отменено пользователем", Cancelled);

    s.telemetry(0, 32_000);
    let mut items = s.items;
    items.sort_by_key(|(t, _)| *t); // стабильная сортировка: порядок в одну миллисекунду сохраняется
    items
}

/// Длительность сценария, мс.
pub fn demo_duration_ms() -> u64 {
    demo_script().last().map(|(t, _)| *t).unwrap_or(0)
}

/// Проиграть сценарий в фоне. При отмене гасит свечение и возвращает `idle`.
pub fn spawn_demo(sink: Arc<dyn EventSink>, cancel: CancellationToken) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("jarvis-demo".into())
        .spawn(move || {
            let start = Instant::now();
            for (at, event) in demo_script() {
                let due = start + Duration::from_millis(at);
                loop {
                    if cancel.is_cancelled() {
                        sink.emit(Event::ControlStop);
                        sink.emit(Event::State { state: AssistantState::Idle });
                        return;
                    }
                    let now = Instant::now();
                    if now >= due {
                        break;
                    }
                    std::thread::sleep((due - now).min(Duration::from_millis(50)));
                }
                sink.emit(event);
            }
        })
        .expect("поток симулятора")
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::CollectSink;
    use std::mem::discriminant;

    #[test]
    fn covers_every_event_and_state() {
        let script = demo_script();
        let samples = [
            Event::State { state: AssistantState::Idle },
            Event::Subtitle { speaker: Speaker::User, text: String::new(), is_final: false },
            Event::AudioLevel { source: AudioSource::Mic, rms: 0.0 },
            Event::ControlStart { tier: RiskTier::T0 },
            Event::ControlTarget { rect: Rect::default(), label: String::new() },
            Event::ControlStop,
            Event::Action { id: String::new(), tool: String::new(), summary: String::new(), status: ActionStatus::Done },
            Event::ConfirmRequest { id: String::new(), summary: String::new(), tier: RiskTier::T3, timeout_ms: 0 },
            Event::ConfirmResolved { id: String::new(), approved: true },
            Event::Telemetry { cpu: 0.0, ram_used_mb: 0, ram_total_mb: 0, gpu: None },
            Event::Sources { items: vec![] },
            Event::Error { message: String::new() },
        ];
        for s in &samples {
            assert!(script.iter().any(|(_, e)| discriminant(e) == discriminant(s)), "нет события {s:?}");
        }
        for st in [
            AssistantState::Idle,
            AssistantState::Listening,
            AssistantState::Thinking,
            AssistantState::Speaking,
            AssistantState::Controlling,
            AssistantState::Error,
        ] {
            assert!(script.iter().any(|(_, e)| *e == Event::State { state: st }), "нет состояния {st:?}");
        }
        for st in [ActionStatus::Running, ActionStatus::Done, ActionStatus::Failed, ActionStatus::Cancelled, ActionStatus::AwaitingConfirm] {
            assert!(script.iter().any(|(_, e)| matches!(e, Event::Action { status, .. } if *status == st)));
        }
    }

    #[test]
    fn script_is_ordered_and_consistent() {
        let script = demo_script();
        assert!(script.windows(2).all(|w| w[0].0 <= w[1].0));
        let last_state = script.iter().rev().find_map(|(_, e)| match e {
            Event::State { state } => Some(*state),
            _ => None,
        });
        assert_eq!(last_state, Some(AssistantState::Idle));
        // каждому ControlStart соответствует ControlStop
        let starts = script.iter().filter(|(_, e)| matches!(e, Event::ControlStart { .. })).count();
        let stops = script.iter().filter(|(_, e)| matches!(e, Event::ControlStop)).count();
        assert_eq!(starts, stops);
        // уровни звука в [0, 1]
        assert!(script.iter().all(|(_, e)| !matches!(e, Event::AudioLevel { rms, .. } if !(0.0..=1.0).contains(rms))));
        assert!(demo_duration_ms() > 20_000 && demo_duration_ms() < 60_000);
    }

    #[test]
    fn cancel_stops_and_cleans_up() {
        let sink = Arc::new(CollectSink::default());
        let cancel = CancellationToken::new();
        cancel.cancel();
        spawn_demo(sink.clone(), cancel).join().unwrap();
        assert_eq!(sink.take(), vec![Event::ControlStop, Event::State { state: AssistantState::Idle }]);
    }
}
