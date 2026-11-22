/// Ошибки голосового модуля. Тексты по-русски: они могут попасть в HUD.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VoiceError {
    #[error("TLS: {0}")]
    Tls(String),
    #[error("сеть недоступна: {0}")]
    Network(String),
    #[error("таймаут: {0}")]
    Timeout(&'static str),
    #[error("отменено")]
    Cancelled,
    #[error("ключ не принят провайдером (HTTP {0})")]
    Auth(u16),
    #[error("сервер ответил HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("некорректный ответ сервера: {0}")]
    BadResponse(String),
    #[error("некорректные входные данные: {0}")]
    InvalidInput(String),
    #[error("не настроено: {0}")]
    Config(String),
    #[error("аудиоустройство: {0}")]
    Device(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = VoiceError> = std::result::Result<T, E>;

impl VoiceError {
    /// Временная ошибка, после которой есть смысл повторить/переподключиться.
    pub fn is_transient(&self) -> bool {
        match self {
            VoiceError::Network(_) | VoiceError::Timeout(_) => true,
            VoiceError::Http { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }
}

impl From<reqwest::Error> for VoiceError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            VoiceError::Timeout("HTTP-запрос")
        } else if e.is_decode() {
            VoiceError::BadResponse(e.to_string())
        } else {
            // Полная цепочка причин: TLS-ошибки reqwest прячет в source().
            let mut msg = e.to_string();
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                msg.push_str(": ");
                msg.push_str(&s.to_string());
                src = s.source();
            }
            VoiceError::Network(msg)
        }
    }
}
