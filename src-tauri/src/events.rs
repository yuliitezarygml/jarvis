use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Start,
    Delta,
    Done,
    Error,
    Warning,
    Cancelled,
    Thinking,
    Executing,
    #[cfg(windows)]
    Confirmation,
    Speaking,
    Level,
    FirstText,
    Mic,
    MicLevel,
    MicError,
    VoiceStart,
    Recognizing,
    Speech,
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "state", content = "text", rename_all = "snake_case")]
pub enum SpeechUpdate<'a> {
    Listening,
    Fallback,
    Partial(&'a str),
    Final(&'a str),
    Cleared,
}

#[derive(Clone, Serialize)]
pub struct Event {
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    pub kind: Kind,
    pub data: Value,
}
impl Event {
    pub fn speech(session: &str, update: SpeechUpdate<'_>) -> Self {
        Self::new(session, None, Kind::Speech, serde_json::json!(update))
    }
    pub fn new(session: &str, turn: Option<&str>, kind: Kind, data: Value) -> Self {
        Self {
            session: session.to_owned(),
            turn: turn.map(str::to_owned),
            kind,
            data,
        }
    }
}
