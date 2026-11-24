//! Голос J.A.R.V.I.S. (клиент): захват, VAD, STT-клиенты, фильтр «Джарвис», нормализатор речи, чанкер,
//! TTS-клиент, воспроизведение, барж-ин, защита от эха и тон голоса (громкость/темп). См. README.md.

pub mod capture;
pub mod chunker;
pub mod device;
pub mod echo;
pub mod error;
pub mod normalizer;
pub mod pcm;
pub mod pipeline;
pub mod player;
pub mod resample;
pub mod stt;
pub mod tls;
pub mod tone;
pub mod tts;
pub mod vad;
pub mod wake;

pub use capture::{Capture, CaptureConfig};
pub use chunker::PhraseChunker;
pub use echo::{EchoGuard, EchoGuardConfig, PlaybackMonitor};
pub use error::{Result, VoiceError};
pub use normalizer::SpeechNormalizer;
pub use pipeline::{ListenHandle, SpeakOutcome, ToneConfig, VoiceConfig, VoicePipeline};
pub use player::{AudioOutput, Player, PlayerConfig};
pub use stt::{GroqWhisperStt, ServerStt, SttEngine, SttEvent, SttStream};
pub use tone::{VoiceFeatures, VoiceProfile, VoiceTone};
pub use tts::{TtsClient, TtsEngine, TtsStream};
pub use vad::{Utterance, Vad, VadConfig, VadEvent};
pub use wake::{Intent, WakeContext, WakeFilter};
