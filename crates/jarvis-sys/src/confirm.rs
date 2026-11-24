//! Подтверждение опасных (T3) действий: голосом («да, сэр») или кнопкой в HUD (план, раздел 3).
//!
//! Правила защиты от эха и самоподтверждения:
//! * решение принимается только в состоянии ожидания (есть активный запрос);
//! * «да» — только с микрофона (`AudioSource::Mic`), только после окончания речи TTS и паузы `echo_tail`,
//!   только если фраза целиком — подтверждение («да», «да, сэр», «подтверждаю», «выполняй»),
//!   только если запрос один (при нескольких — кнопкой);
//! * «нет / отмена / стоп» — с микрофона в любой момент (можно перебить TTS);
//! * фраза, повторяющая текст запроса, считается эхом и игнорируется;
//! * по таймауту — отказ.

use jarvis_core::event::AudioSource;
use jarvis_core::{Event, EventSink, RiskTier};
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Как получено решение.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmVia {
    Voice,
    Click,
    Timeout,
    /// Отмена агентом (Ctrl+Alt+K, «стоп», завершение задачи).
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ConfirmDecision {
    pub approved: bool,
    pub via: ConfirmVia,
}

/// Почему реплика не повлияла на запрос.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreReason {
    NoPending,
    /// Транскрипт собственной речи TTS — самоподтверждение.
    NotMic,
    /// TTS ещё говорит.
    TtsSpeaking,
    /// Слишком быстро после конца речи TTS — вероятно эхо.
    TooSoonAfterTts,
    /// Реплика повторяет текст запроса.
    Echo,
    /// Не «да» и не «нет».
    NotADecision,
    /// Ждут несколько запросов — подтверждать голосом нельзя.
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptVerdict {
    Approved { id: String },
    Denied { ids: Vec<String> },
    Ignored(IgnoreReason),
}

struct Pending {
    id: String,
    summary: String,
    tier: RiskTier,
    tx: oneshot::Sender<ConfirmDecision>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    pending: Vec<Pending>,
}

/// Шлюз подтверждений. Клонируется дёшево (общий стейт).
#[derive(Clone)]
pub struct ConfirmGate {
    inner: Arc<Mutex<Inner>>,
    events: Arc<dyn EventSink>,
    echo_tail: Duration,
}

/// Запрос подтверждения: `id` для HUD и future решения (`ticket.await`).
pub struct ConfirmTicket {
    pub id: String,
    rx: oneshot::Receiver<ConfirmDecision>,
    timeout: Duration,
    gate: ConfirmGate,
}

impl IntoFuture for ConfirmTicket {
    type Output = ConfirmDecision;
    type IntoFuture = Pin<Box<dyn Future<Output = ConfirmDecision> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            match tokio::time::timeout(self.timeout, self.rx).await {
                Ok(Ok(d)) => d,
                Ok(Err(_)) => ConfirmDecision { approved: false, via: ConfirmVia::Cancel },
                Err(_) => {
                    self.gate.resolve(&self.id, false, ConfirmVia::Timeout);
                    ConfirmDecision { approved: false, via: ConfirmVia::Timeout }
                }
            }
        })
    }
}

/// Нормализация: нижний регистр, ё→е, без пунктуации.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('ё', "е")
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|w| w.trim_matches('-').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

const DENY_WORDS: &[&str] = &[
    "нет", "не", "отмена", "отмени", "отменить", "отменяй", "стоп", "отбой", "хватит", "стой", "no", "cancel", "stop",
];

/// Реплика без обращений («джарвис», «сэр», «пожалуйста»).
fn core_words(text: &str) -> Vec<String> {
    words(text)
        .into_iter()
        .filter(|w| !matches!(w.as_str(), "джарвис" | "jarvis" | "сэр" | "сер" | "пожалуйста"))
        .collect()
}

fn is_approval(core: &[String]) -> bool {
    let c: Vec<&str> = core.iter().map(String::as_str).collect();
    matches!(
        c.as_slice(),
        ["да"] | ["да", "да"] | ["подтверждаю"] | ["выполняй"] | ["да", "подтверждаю"] | ["да", "выполняй"] | ["так", "точно"]
    )
}

fn is_denial(core: &[String]) -> bool {
    !core.is_empty() && core.len() <= 5 && core.iter().any(|w| DENY_WORDS.contains(&w.as_str()))
}

/// Реплика из ≥3 слов, ≥60 % которых есть в тексте запроса, — эхо озвученного вопроса.
fn is_echo(text: &str, summary: &str) -> bool {
    let t = words(text);
    if t.len() < 3 {
        return false;
    }
    let s = words(summary);
    let hit = t.iter().filter(|w| s.contains(w)).count();
    hit * 10 >= t.len() * 6
}

impl ConfirmGate {
    pub fn new(events: Arc<dyn EventSink>) -> Self {
        Self { inner: Arc::default(), events, echo_tail: Duration::from_millis(400) }
    }

    /// Пауза после конца TTS, в течение которой «да» считается эхом.
    pub fn with_echo_tail(mut self, d: Duration) -> Self {
        self.echo_tail = d;
        self
    }

    /// Запросить подтверждение. Отправляет `Event::ConfirmRequest`.
    pub fn request(&self, summary: impl Into<String>, tier: RiskTier, timeout: Duration) -> ConfirmTicket {
        let summary = summary.into();
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut g = self.inner.lock().unwrap();
            g.next += 1;
            let id = format!("confirm-{}", g.next);
            g.pending.push(Pending { id: id.clone(), summary: summary.clone(), tier, tx });
            id
        };
        self.events.emit(Event::ConfirmRequest {
            id: id.clone(),
            summary,
            tier,
            timeout_ms: timeout.as_millis() as u64,
        });
        ConfirmTicket { id, rx, timeout, gate: self.clone() }
    }

    /// Есть ли активные запросы (состояние `awaiting_confirm`).
    pub fn awaiting(&self) -> bool {
        !self.inner.lock().unwrap().pending.is_empty()
    }

    /// Активные запросы: (id, текст, уровень).
    pub fn pending(&self) -> Vec<(String, String, RiskTier)> {
        self.inner.lock().unwrap().pending.iter().map(|p| (p.id.clone(), p.summary.clone(), p.tier)).collect()
    }

    /// Финальный транскрипт реплики. `tts_finished_at` — когда TTS закончил говорить (`None` — ещё говорит).
    pub fn on_transcript(&self, text: &str, source: AudioSource, tts_finished_at: Option<Instant>) -> TranscriptVerdict {
        self.on_transcript_at(text, source, tts_finished_at, Instant::now())
    }

    /// То же с явным «сейчас» (для тестов).
    pub fn on_transcript_at(
        &self,
        text: &str,
        source: AudioSource,
        tts_finished_at: Option<Instant>,
        now: Instant,
    ) -> TranscriptVerdict {
        let (ids, summaries): (Vec<String>, Vec<String>) = {
            let g = self.inner.lock().unwrap();
            g.pending.iter().map(|p| (p.id.clone(), p.summary.clone())).unzip()
        };
        if ids.is_empty() {
            return TranscriptVerdict::Ignored(IgnoreReason::NoPending);
        }
        if source != AudioSource::Mic {
            return TranscriptVerdict::Ignored(IgnoreReason::NotMic);
        }
        if summaries.iter().any(|s| is_echo(text, s)) {
            return TranscriptVerdict::Ignored(IgnoreReason::Echo);
        }
        let core = core_words(text);
        if is_denial(&core) {
            for id in &ids {
                self.resolve(id, false, ConfirmVia::Voice);
            }
            return TranscriptVerdict::Denied { ids };
        }
        if !is_approval(&core) {
            return TranscriptVerdict::Ignored(IgnoreReason::NotADecision);
        }
        let Some(done) = tts_finished_at else {
            return TranscriptVerdict::Ignored(IgnoreReason::TtsSpeaking);
        };
        if now < done + self.echo_tail {
            return TranscriptVerdict::Ignored(IgnoreReason::TooSoonAfterTts);
        }
        if ids.len() > 1 {
            return TranscriptVerdict::Ignored(IgnoreReason::Ambiguous);
        }
        if self.resolve(&ids[0], true, ConfirmVia::Voice) {
            TranscriptVerdict::Approved { id: ids[0].clone() }
        } else {
            TranscriptVerdict::Ignored(IgnoreReason::NoPending)
        }
    }

    /// Кнопка в HUD. `false` — запроса с таким id нет (уже решён/истёк).
    pub fn on_click(&self, id: &str, approved: bool) -> bool {
        self.resolve(id, approved, ConfirmVia::Click)
    }

    /// Отменить все запросы (Ctrl+Alt+K, «стоп»).
    pub fn cancel_all(&self) {
        let ids: Vec<String> = self.inner.lock().unwrap().pending.iter().map(|p| p.id.clone()).collect();
        for id in ids {
            self.resolve(&id, false, ConfirmVia::Cancel);
        }
    }

    fn resolve(&self, id: &str, approved: bool, via: ConfirmVia) -> bool {
        let p = {
            let mut g = self.inner.lock().unwrap();
            let Some(i) = g.pending.iter().position(|p| p.id == id) else { return false };
            g.pending.remove(i)
        };
        let _ = p.tx.send(ConfirmDecision { approved, via });
        self.events.emit(Event::ConfirmResolved { id: id.to_string(), approved });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::CollectSink;

    const SUMMARY: &str = "Остановить службу Spooler. Скажите «да, сэр» или «нет».";

    fn gate() -> (ConfirmGate, Arc<CollectSink>) {
        let sink = Arc::new(CollectSink::default());
        (ConfirmGate::new(sink.clone()), sink)
    }

    fn after_tts() -> Option<Instant> {
        Some(Instant::now() - Duration::from_secs(2))
    }

    #[tokio::test]
    async fn voice_approval_after_tts() {
        let (g, sink) = gate();
        let t = g.request(SUMMARY, RiskTier::T3, Duration::from_secs(20));
        let id = t.id.clone();
        assert!(g.awaiting());
        assert_eq!(g.on_transcript("Да, сэр!", AudioSource::Mic, after_tts()), TranscriptVerdict::Approved { id: id.clone() });
        assert_eq!(t.await, ConfirmDecision { approved: true, via: ConfirmVia::Voice });
        let ev = sink.take();
        assert!(matches!(&ev[0], Event::ConfirmRequest { tier: RiskTier::T3, timeout_ms: 20000, .. }));
        assert_eq!(ev[1], Event::ConfirmResolved { id, approved: true });
        assert!(!g.awaiting());
    }

    #[test]
    fn accepted_phrases() {
        for p in ["да", "Да, сэр", "Джарвис, да", "подтверждаю", "Выполняй!", "да, выполняй", "так точно, сэр"] {
            let (g, _) = gate();
            let _t = g.request("x", RiskTier::T3, Duration::from_secs(5));
            assert!(matches!(g.on_transcript(p, AudioSource::Mic, after_tts()), TranscriptVerdict::Approved { .. }), "{p}");
        }
    }

    #[test]
    fn swearing_is_never_approval() {
        // Ругань и посылания — не «да», даже если начинаются с «да».
        for p in ["да пошёл ты", "да пошел ты нахуй", "да иди ты", "да блять", "да похуй", "делай что хочешь", "мне похер", "ну давай уже, бля"] {
            let (g, _) = gate();
            let _t = g.request("x", RiskTier::T3, Duration::from_secs(5));
            assert!(!matches!(g.on_transcript(p, AudioSource::Mic, after_tts()), TranscriptVerdict::Approved { .. }), "{p}");
            assert!(g.awaiting(), "{p}: запрос должен ждать ясного «да»");
        }
    }

    #[test]
    fn self_confirmation_from_tts_is_ignored() {
        let (g, _) = gate();
        let _t = g.request(SUMMARY, RiskTier::T3, Duration::from_secs(5));
        assert_eq!(g.on_transcript("да, сэр", AudioSource::Tts, after_tts()), TranscriptVerdict::Ignored(IgnoreReason::NotMic));
        assert!(g.awaiting());
    }

    #[test]
    fn echo_while_tts_speaks_or_right_after() {
        let (g, _) = gate();
        let _t = g.request(SUMMARY, RiskTier::T3, Duration::from_secs(5));
        assert_eq!(g.on_transcript("да сэр", AudioSource::Mic, None), TranscriptVerdict::Ignored(IgnoreReason::TtsSpeaking));
        let now = Instant::now();
        assert_eq!(
            g.on_transcript_at("да сэр", AudioSource::Mic, Some(now - Duration::from_millis(100)), now),
            TranscriptVerdict::Ignored(IgnoreReason::TooSoonAfterTts)
        );
        // микрофон услышал озвученный вопрос целиком
        assert_eq!(
            g.on_transcript("остановить службу spooler скажите да сэр или нет", AudioSource::Mic, after_tts()),
            TranscriptVerdict::Ignored(IgnoreReason::Echo)
        );
        assert!(g.awaiting());
    }

    #[test]
    fn yes_without_pending_and_non_decisions() {
        let (g, sink) = gate();
        assert_eq!(g.on_transcript("да", AudioSource::Mic, after_tts()), TranscriptVerdict::Ignored(IgnoreReason::NoPending));
        let _t = g.request("удалить", RiskTier::T3, Duration::from_secs(5));
        for p in ["да, удали всё и ещё папку", "какая погода", "давай подумаем"] {
            assert_eq!(g.on_transcript(p, AudioSource::Mic, after_tts()), TranscriptVerdict::Ignored(IgnoreReason::NotADecision), "{p}");
        }
        assert!(sink.take().len() == 1);
    }

    #[tokio::test]
    async fn denial_even_during_tts() {
        for p in ["нет", "Отмена!", "стоп", "нет, сэр", "да нет", "не надо"] {
            let (g, _) = gate();
            let t = g.request(SUMMARY, RiskTier::T3, Duration::from_secs(5));
            assert!(matches!(g.on_transcript(p, AudioSource::Mic, None), TranscriptVerdict::Denied { .. }), "{p}");
            assert_eq!(t.await, ConfirmDecision { approved: false, via: ConfirmVia::Voice });
        }
    }

    #[tokio::test]
    async fn click_and_unknown_id() {
        let (g, _) = gate();
        let t = g.request("x", RiskTier::T3, Duration::from_secs(5));
        assert!(!g.on_click("nope", true));
        assert!(g.on_click(&t.id.clone(), true));
        assert_eq!(t.await, ConfirmDecision { approved: true, via: ConfirmVia::Click });
    }

    #[tokio::test]
    async fn timeout_denies_and_emits() {
        let (g, sink) = gate();
        let t = g.request("x", RiskTier::T3, Duration::from_millis(30));
        let id = t.id.clone();
        assert_eq!(t.await, ConfirmDecision { approved: false, via: ConfirmVia::Timeout });
        assert!(!g.awaiting());
        assert!(sink.take().contains(&Event::ConfirmResolved { id: id.clone(), approved: false }));
        // поздний клик уже ничего не делает
        assert!(!g.on_click(&id, true));
    }

    #[tokio::test]
    async fn several_pending_voice_yes_is_ambiguous() {
        let (g, _) = gate();
        let a = g.request("a", RiskTier::T3, Duration::from_secs(5));
        let b = g.request("b", RiskTier::T3, Duration::from_secs(5));
        assert_eq!(g.on_transcript("да", AudioSource::Mic, after_tts()), TranscriptVerdict::Ignored(IgnoreReason::Ambiguous));
        assert!(g.on_click(&b.id.clone(), true));
        g.cancel_all();
        assert_eq!(a.await.via, ConfirmVia::Cancel);
        assert!(b.await.approved);
    }
}
