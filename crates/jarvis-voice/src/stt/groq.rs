//! Облачный STT: Groq Whisper (`POST {base}/audio/transcriptions`, multipart: `file`, `model`, `language`,
//! `prompt`, `response_format=json`, `temperature`), ответ `{"text": "…"}`.
//!
//! Ключи — провайдеры Groq из `config.json` (`groq`, `groq-2`, …): при HTTP 429 (лимит) запрос один раз
//! повторяется со следующим ключом, и дальше запросы начинаются с него. Модель и язык — `config.stt`,
//! `prompt` — подсказка стиля + словарь названий (`stt::hints`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use jarvis_core::CancellationToken;
use jarvis_core::stt_protocol::SttResult;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use super::SttEngine;
use super::hints::{build_prompt, stt_hints};
use crate::error::{Result, VoiceError};
use crate::pcm::{STT_SAMPLE_RATE, wav_encode};
use crate::tls;
use crate::tts::check_status;

#[derive(Debug, Clone)]
struct GroqKey {
    id: String,
    base: String,
    key: String,
}

pub struct GroqWhisperStt {
    http: reqwest::Client,
    keys: Vec<GroqKey>,
    /// С какого ключа начинать (после 429 — со следующего).
    next: AtomicUsize,
    model: String,
    language: String,
    prompt: Option<String>,
}

#[derive(Deserialize)]
struct Transcription {
    text: String,
}

impl GroqWhisperStt {
    pub const DEFAULT_MODEL: &'static str = jarvis_core::config::SttConfig::DEFAULT_CLOUD_MODEL;
    pub const TURBO_MODEL: &'static str = "whisper-large-v3-turbo";
    pub const DEFAULT_BASE: &'static str = "https://api.groq.com/openai/v1";
    /// Подсказка стиля: имя ассистента пишется кириллицей, команды — с заглавной и запятой.
    pub const STYLE_PROMPT: &'static str = "Джарвис, открой браузер.";
    /// Предел `prompt` у Groq — 224 токена; оценка токенов грубая, поэтому с запасом.
    pub const PROMPT_TOKEN_BUDGET: usize = 200;

    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        if api_key.trim().is_empty() {
            return Err(VoiceError::Config("не задан ключ Groq для распознавания".into()));
        }
        Ok(Self {
            http: tls::public_http_client(Duration::from_secs(30))?,
            keys: vec![GroqKey { id: "groq".into(), base: base_url.trim_end_matches('/').to_string(), key: api_key.trim().to_string() }],
            next: AtomicUsize::new(0),
            model: Self::DEFAULT_MODEL.into(),
            language: "ru".into(),
            prompt: Some(Self::STYLE_PROMPT.into()),
        })
    }

    /// Ещё один ключ (запасной при 429).
    pub fn with_key(mut self, id: &str, base_url: &str, api_key: &str) -> Self {
        if !api_key.trim().is_empty() {
            let base = if base_url.trim().is_empty() { Self::DEFAULT_BASE } else { base_url };
            self.keys.push(GroqKey { id: id.into(), base: base.trim_end_matches('/').to_string(), key: api_key.trim().to_string() });
        }
        self
    }

    /// Провайдеры Groq из конфига (адрес и ключ), модель/язык/словарь из `config.stt`.
    pub fn from_config(cfg: &jarvis_core::Config) -> Result<Self> {
        Self::from_config_with_hints(cfg, &[])
    }

    /// То же + подсказки, которые передаёт приложение (имена из памяти и т. п.).
    pub fn from_config_with_hints(cfg: &jarvis_core::Config, extra_hints: &[String]) -> Result<Self> {
        let providers = groq_providers(cfg);
        let Some((first, rest)) = providers.split_first() else {
            return Err(VoiceError::Config("в config.json нет провайдера groq с ключом".into()));
        };
        let base = if first.base_url.is_empty() { Self::DEFAULT_BASE } else { first.base_url.as_str() };
        let mut s = Self::new(base, &first.api_key)?;
        s.keys[0].id = first.id.clone();
        for p in rest {
            s = s.with_key(&p.id, &p.base_url, &p.api_key);
        }
        let model = cfg.stt.cloud_model.trim();
        let lang = cfg.stt.language.trim();
        Ok(s.with_model(if model.is_empty() { Self::DEFAULT_MODEL } else { model })
            .with_language(if lang.is_empty() { "ru" } else { lang })
            .with_hints(&stt_hints(&cfg.stt.hints, extra_hints)))
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = lang.into();
        self
    }

    pub fn with_prompt(mut self, prompt: Option<String>) -> Self {
        self.prompt = prompt;
        self
    }

    /// `prompt` = подсказка стиля + словарь (обрезается под предел Groq).
    pub fn with_hints(self, hints: &[String]) -> Self {
        let p = build_prompt(Self::STYLE_PROMPT, hints, Self::PROMPT_TOKEN_BUDGET);
        self.with_prompt(Some(p))
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn prompt(&self) -> Option<&str> {
        self.prompt.as_deref()
    }

    /// Сколько ключей Groq в запасе (для 429).
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    async fn request(&self, key: &GroqKey, wav: &[u8], cancel: &CancellationToken) -> Result<String> {
        let file = Part::bytes(wav.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| VoiceError::Other(e.to_string()))?;
        let mut form = Form::new()
            .part("file", file)
            .text("model", self.model.clone())
            .text("language", self.language.clone())
            .text("response_format", "json")
            .text("temperature", "0");
        if let Some(p) = &self.prompt {
            form = form.text("prompt", p.clone());
        }
        let send = self.http.post(format!("{}/audio/transcriptions", key.base)).bearer_auth(&key.key).multipart(form).send();
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(VoiceError::Cancelled),
            r = send => r?,
        };
        let resp = check_status(resp).await?;
        let t: Transcription = tokio::select! {
            _ = cancel.cancelled() => return Err(VoiceError::Cancelled),
            r = resp.json() => r.map_err(|e| VoiceError::BadResponse(e.to_string()))?,
        };
        Ok(t.text.trim().to_string())
    }
}

/// Включённые провайдеры Groq с ключом: `groq` первым, затем `groq-2`, `groq_…` и любые с адресом api.groq.com.
fn groq_providers(cfg: &jarvis_core::Config) -> Vec<&jarvis_core::config::ProviderConfig> {
    let mut v: Vec<_> = cfg
        .providers
        .iter()
        .filter(|p| p.enabled && !p.api_key.trim().is_empty())
        .filter(|p| p.id == "groq" || p.id.starts_with("groq-") || p.id.starts_with("groq_") || p.base_url.contains("api.groq.com"))
        .collect();
    v.sort_by_key(|p| p.id != "groq");
    v
}

#[async_trait]
impl SttEngine for GroqWhisperStt {
    fn name(&self) -> &str {
        "groq_whisper"
    }

    async fn transcribe(&self, pcm16k: &[i16], cancel: &CancellationToken) -> Result<SttResult> {
        if pcm16k.len() < (STT_SAMPLE_RATE / 100) as usize {
            return Err(VoiceError::InvalidInput("слишком короткий звук для Whisper".into()));
        }
        let wav = wav_encode(pcm16k, STT_SAMPLE_RATE, 1);
        let n = self.keys.len();
        let start = self.next.load(Ordering::Relaxed) % n;
        // Не больше двух попыток: при 429 — один повтор со следующим ключом.
        let attempts = n.min(2);
        let mut last = VoiceError::Other("нет ключей Groq".into());
        for i in 0..attempts {
            let idx = (start + i) % n;
            let key = &self.keys[idx];
            match self.request(key, &wav, cancel).await {
                Ok(text) => {
                    return Ok(SttResult { text, duration_ms: pcm16k.len() as u64 * 1000 / STT_SAMPLE_RATE as u64 });
                }
                Err(VoiceError::Http { status: 429, body }) if i + 1 < attempts => {
                    let next = (idx + 1) % n;
                    tracing::warn!(key = %key.id, next = %self.keys[next].id, "Groq STT: лимит (HTTP 429), повтор с другим ключом");
                    self.next.store(next, Ordering::Relaxed);
                    last = VoiceError::Http { status: 429, body };
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }
}
