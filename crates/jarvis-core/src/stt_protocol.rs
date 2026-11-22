//! Протокол STT-сервиса `server/jarvis-stt-rs` (Xeon). Его используют и сервер, и клиент в `jarvis-voice`.
//!
//! Адрес по умолчанию: `https://192.168.50.38:8444` (TLS тем же частным CA, что и TTS; без токена).
//! - `GET  /health` → `Health` JSON.
//! - `POST /stt` — тело: сырой PCM s16le 16 кГц mono (`Content-Type: audio/L16; rate=16000`) или WAV
//!   (`Content-Type: audio/wav`) → `SttResult` JSON.
//! - `WS   /stt/stream` — клиент шлёт **бинарные** кадры PCM s16le 16 кГц mono (любой длины, рекомендуется 20–100 мс)
//!   и **текстовые** кадры `ClientMsg` (JSON); сервер шлёт текстовые кадры `ServerMsg` (JSON).
//!   После `{"type":"end"}` сервер отдаёт `final` по накопленному и готов к следующей фразе в том же соединении.

use serde::{Deserialize, Serialize};

pub const SAMPLE_RATE: u32 = 16_000;
pub const DEFAULT_URL: &str = "https://192.168.50.38:8444";
pub const STREAM_PATH: &str = "/stt/stream";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Конец фразы: выдать final и сбросить состояние распознавателя.
    End,
    /// Сбросить без результата (барж-ин/отмена).
    Reset,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Сразу после подключения.
    Ready { model: String },
    /// Промежуточный текст текущей фразы (может меняться).
    Partial { text: String },
    /// Окончательный текст фразы.
    Final { text: String, duration_ms: u64 },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub model: String,
    pub sample_rate: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SttResult {
    pub text: String,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format() {
        assert_eq!(serde_json::to_string(&ClientMsg::End).unwrap(), r#"{"type":"end"}"#);
        let m: ServerMsg = serde_json::from_str(r#"{"type":"final","text":"привет","duration_ms":900}"#).unwrap();
        assert_eq!(m, ServerMsg::Final { text: "привет".into(), duration_ms: 900 });
    }
}
