//! Живая проверка модели: (а) стрим по-русски → TTFT, ток/с, русский; (б) вызов `get_time`; (в) цифра на картинке.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use jarvis_core::{
    CancellationToken, ChatEvent, ChatMessage, ChatModel, ChatRequest, ChatResponse, LlmError, RiskTier, ToolSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

pub const PROMPT_RU: &str = "Ответь по-русски одним словом: какого цвета небо?";
pub const PROMPT_TOOL: &str = "Который сейчас час? Обязательно вызови инструмент get_time.";
pub const PROMPT_VISION: &str = "Какая цифра изображена на картинке? Ответь одной цифрой.";
/// Цифра на тестовой картинке.
pub const VISION_DIGIT: u8 = 7;

#[derive(Debug, Clone)]
pub struct ProbeOptions {
    /// Таймаут каждого шага.
    pub timeout: Duration,
    /// Проверять зрение (только для моделей с vision).
    pub check_vision: bool,
    /// Максимум параллельных проверок в `probe_many`.
    pub max_parallel: usize,
    /// Сколько максимум ждать по Retry-After перед одной повторной попыткой.
    pub max_rate_limit_wait: Duration,
    pub cancel: CancellationToken,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            check_vision: true,
            max_parallel: 3,
            max_rate_limit_wait: Duration::from_secs(10),
            cancel: CancellationToken::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    /// «provider/model».
    pub model: String,
    /// Время до первого видимого токена.
    pub ttft_ms: Option<u64>,
    /// Скорость генерации (по usage, иначе оценка по длине текста).
    pub tok_per_s: Option<f32>,
    pub tools_ok: bool,
    pub russian_ok: bool,
    /// `None` — зрение не проверялось.
    pub vision_ok: Option<bool>,
    /// Ответ на шаге (а) — для отладки и UI.
    #[serde(default)]
    pub answer: String,
    /// Первая ошибка (с указанием шага).
    pub error: Option<String>,
}

impl ProbeResult {
    fn new(model: String) -> Self {
        Self {
            model,
            ttft_ms: None,
            tok_per_s: None,
            tools_ok: false,
            russian_ok: false,
            vision_ok: None,
            answer: String::new(),
            error: None,
        }
    }

    /// Модель ответила на базовый стрим.
    pub fn answered(&self) -> bool {
        self.ttft_ms.is_some()
    }

    fn note_error(&mut self, step: &str, e: &LlmError) {
        if self.error.is_none() {
            self.error = Some(format!("{step}: {e}"));
        }
    }
}

/// Тестовый инструмент.
pub fn get_time_tool() -> ToolSpec {
    ToolSpec {
        name: "get_time".into(),
        description: "Возвращает текущее время. Вызывай, когда спрашивают, который час.".into(),
        parameters: json!({
            "type": "object",
            "properties": { "timezone": { "type": "string", "description": "IANA-таймзона, например Europe/Moscow" } },
        }),
        tier: RiskTier::T0,
    }
}

/// Текст по-русски: кириллица преобладает среди букв.
pub fn is_russian(text: &str) -> bool {
    let (mut cyr, mut lat) = (0usize, 0usize);
    for c in text.chars() {
        if matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё') {
            cyr += 1;
        } else if c.is_ascii_alphabetic() {
            lat += 1;
        }
    }
    cyr >= 2 && cyr * 10 >= (cyr + lat) * 6
}

/// Ответ на картинку содержит нужную цифру (и никаких других цифр), либо её название.
pub fn vision_answer_ok(text: &str, digit: u8) -> bool {
    let want = char::from(b'0' + digit % 10);
    let digits: Vec<char> = text.chars().filter(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        return digits.iter().all(|d| *d == want);
    }
    let lower = text.to_lowercase();
    let names = [
        ("ноль", "zero"),
        ("один", "one"),
        ("два", "two"),
        ("три", "three"),
        ("четыре", "four"),
        ("пять", "five"),
        ("шесть", "six"),
        ("семь", "seven"),
        ("восемь", "eight"),
        ("девять", "nine"),
    ];
    let (ru, en) = names[(digit % 10) as usize];
    lower.contains(ru) || lower.contains(en)
}

/// Грубая оценка токенов, если провайдер не прислал usage (~3,5 символа на токен).
fn estimate_tokens(text: &str) -> u32 {
    ((text.chars().count() as f32) / 3.5).ceil() as u32
}

/// Вызов с таймаутом шага, отменой и одной повторной попыткой после 429 (ждём Retry-After, но не дольше лимита).
async fn with_retry<F, Fut, T>(opts: &ProbeOptions, mut f: F) -> Result<T, LlmError>
where
    F: FnMut(CancellationToken) -> Fut,
    Fut: std::future::Future<Output = Result<T, LlmError>>,
{
    for attempt in 0..2 {
        let token = opts.cancel.child_token();
        let r = match tokio::time::timeout(opts.timeout, f(token.clone())).await {
            Ok(r) => r,
            Err(_) => {
                token.cancel();
                Err(LlmError::Timeout)
            }
        };
        match r {
            Err(LlmError::RateLimited { retry_after_ms }) if attempt == 0 => {
                let wait = Duration::from_millis(retry_after_ms.unwrap_or(2000)).min(opts.max_rate_limit_wait);
                tracing::info!("429 при проверке — жду {wait:?}");
                tokio::select! {
                    _ = opts.cancel.cancelled() => return Err(LlmError::Cancelled),
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            other => return other,
        }
    }
    unreachable!("цикл возвращает на второй попытке")
}

struct StreamMeasure {
    resp: ChatResponse,
    ttft: Option<Duration>,
    total: Duration,
}

async fn measure_stream(model: &dyn ChatModel, req: ChatRequest, cancel: CancellationToken) -> Result<StreamMeasure, LlmError> {
    let (tx, mut rx) = mpsc::channel::<ChatEvent>(256);
    let start = Instant::now();
    let mut first: Option<Duration> = None;
    let fut = model.chat_stream(req, tx, cancel);
    let recv = async {
        while let Some(ev) = rx.recv().await {
            if let ChatEvent::TextDelta(t) = ev
                && first.is_none()
                && !t.trim().is_empty()
            {
                first = Some(start.elapsed());
            }
        }
    };
    let (res, ()) = tokio::join!(fut, recv);
    let resp = res?;
    Ok(StreamMeasure { resp, ttft: first, total: start.elapsed() })
}

/// Проверить одну модель.
pub async fn probe(model: &dyn ChatModel, opts: &ProbeOptions) -> ProbeResult {
    let mut r = ProbeResult::new(model.id());

    // (а) стрим по-русски.
    let req = ChatRequest { messages: vec![ChatMessage::user(PROMPT_RU)], max_tokens: Some(PROBE_MAX_TOKENS), ..Default::default() };
    match with_retry(opts, |c| measure_stream(model, req.clone(), c)).await {
        Ok(m) => {
            let text = m.resp.message.content.trim().to_string();
            let ttft = m.ttft.unwrap_or(m.total);
            r.ttft_ms = Some(ttft.as_millis() as u64);
            let tokens = m
                .resp
                .usage
                .as_ref()
                .map(|u| u.output_tokens)
                .filter(|n| *n > 0)
                .unwrap_or_else(|| estimate_tokens(&text));
            let gen_time = m.total.saturating_sub(ttft);
            let span = if gen_time >= Duration::from_millis(100) { gen_time } else { m.total };
            if tokens > 0 && !span.is_zero() {
                r.tok_per_s = Some(((tokens as f64 / span.as_secs_f64()) * 10.0).round() as f32 / 10.0);
            }
            r.russian_ok = is_russian(&text);
            r.answer = text.chars().take(200).collect();
            if text.is_empty() {
                r.error = Some("стрим: пустой ответ".into());
            }
        }
        Err(e) => {
            r.note_error("стрим", &e);
            return r; // модель не отвечает — дальше не проверяем
        }
    }

    // (б) инструмент.
    let req = ChatRequest {
        messages: vec![ChatMessage::user(PROMPT_TOOL)],
        tools: vec![get_time_tool()],
        max_tokens: Some(PROBE_MAX_TOKENS),
        ..Default::default()
    };
    match with_retry(opts, |c| model.chat(req.clone(), c)).await {
        Ok(resp) => r.tools_ok = resp.message.tool_calls.iter().any(|c| c.name == "get_time"),
        Err(e) => r.note_error("инструмент", &e),
    }

    // (в) зрение.
    if opts.check_vision {
        let mut msg = ChatMessage::user(PROMPT_VISION);
        msg.images.push(crate::png::digit_image(VISION_DIGIT));
        let req = ChatRequest { messages: vec![msg], max_tokens: Some(PROBE_MAX_TOKENS), ..Default::default() };
        r.vision_ok = Some(match with_retry(opts, |c| model.chat(req.clone(), c)).await {
            Ok(resp) => vision_answer_ok(&resp.message.content, VISION_DIGIT),
            Err(e) => {
                r.note_error("зрение", &e);
                false
            }
        });
    }
    r
}

/// Проверить несколько моделей: не больше `opts.max_parallel` одновременно, порядок результатов = порядок входа.
/// Второй элемент пары — проверять ли зрение у этой модели.
pub async fn probe_many(models: Vec<(Arc<dyn ChatModel>, bool)>, opts: &ProbeOptions) -> Vec<ProbeResult> {
    let par = opts.max_parallel.max(1);
    futures_util::stream::iter(models)
        .map(|(m, vision)| {
            let o = ProbeOptions { check_vision: vision && opts.check_vision, ..opts.clone() };
            async move { probe(m.as_ref(), &o).await }
        })
        .buffered(par)
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_detection() {
        assert!(is_russian("Голубое"));
        assert!(is_russian("Синее."));
        assert!(!is_russian("Blue"));
        assert!(!is_russian("The sky is голубое blue blue"));
        assert!(!is_russian(""));
    }

    #[test]
    fn vision_answers() {
        assert!(vision_answer_ok("7", 7));
        assert!(vision_answer_ok("На картинке цифра 7.", 7));
        assert!(vision_answer_ok("Семь", 7));
        assert!(!vision_answer_ok("1", 7));
        assert!(!vision_answer_ok("17", 7));
        assert!(!vision_answer_ok("не вижу", 7));
    }
}
