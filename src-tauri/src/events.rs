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
    Speaking,
    Level,
    FirstText,
    Mic,
    MicLevel,
    MicError,
    VoiceStart,
    Recognizing,
    Transcript,
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
    pub fn new(session: &str, turn: Option<&str>, kind: Kind, data: Value) -> Self {
        Self {
            session: session.to_owned(),
            turn: turn.map(str::to_owned),
            kind,
            data,
        }
    }
}
