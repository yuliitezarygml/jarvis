//! Резервный STT: Groq Whisper (`POST {base}/audio/transcriptions`, multipart: `file`, `model`, `language`,
//! `response_format=json`, `temperature`), ответ `{"text": "…"}`. Ключ — провайдер `groq` из `config.json`.

use std::time::Duration;

use async_trait::async_trait;
use jarvis_core::CancellationToken;
use jarvis_core::stt_protocol::SttResult;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use super::SttEngine;
use crate::error::{Result, VoiceError};
use crate::pcm::{STT_SAMPLE_RATE, wav_encode};
use crate::tls;
use crate::tts::check_status;

pub struct GroqWhisperStt {
    http: reqwest::Client,
    base: String,
    api_key: String,
    model: String,
    language: String,
    prompt: Option<String>,
}

#[derive(Deserialize)]
struct Transcription {
    text: String,
}

impl GroqWhisperStt {
    pub const DEFAULT_MODEL: &'static str = "whisper-large-v3-turbo";
    pub const DEFAULT_BASE: &'static str = "https://api.groq.com/openai/v1";

    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        if api_key.trim().is_empty() {
            return Err(VoiceError::Config("не задан ключ Groq для резервного STT".into()));
        }
        Ok(Self {
            http: tls::public_http_client(Duration::from_secs(30))?,
            base: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.trim().to_string(),
            model: Self::DEFAULT_MODEL.into(),
            language: "ru".into(),
            // Подсказка стиля: имя ассистента пишется кириллицей.
            prompt: Some("Джарвис, открой браузер.".into()),
        })
    }

    /// Провайдер `groq` из конфига (адрес и ключ).
    pub fn from_config(cfg: &jarvis_core::Config) -> Result<Self> {
        let p = cfg
            .provider("groq")
            .ok_or_else(|| VoiceError::Config("в config.json нет провайдера groq".into()))?;
        let base = if p.base_url.is_empty() { Self::DEFAULT_BASE } else { p.base_url.as_str() };
        Self::new(base, &p.api_key)
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
        let file = Part::bytes(wav)
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
        let send = self
            .http
            .post(format!("{}/audio/transcriptions", self.base))
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send();
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(VoiceError::Cancelled),
            r = send => r?,
        };
        let resp = check_status(resp).await?;
        let t: Transcription = resp.json().await.map_err(|e| VoiceError::BadResponse(e.to_string()))?;
        Ok(SttResult { text: t.text.trim().to_string(), duration_ms: pcm16k.len() as u64 * 1000 / STT_SAMPLE_RATE as u64 })
    }
}
