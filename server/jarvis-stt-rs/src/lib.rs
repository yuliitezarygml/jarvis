//! J.A.R.V.I.S. STT-сервис: распознавание русской речи на сервере (sherpa-onnx).
//!
//! Протокол — `jarvis_core::stt_protocol`. Движок — за трейтом [`engine::Engine`]; реализация sherpa-onnx
//! включается feature `sherpa` (Linux-сервер), без неё крейт собирается и тестируется где угодно.

pub mod audio;
pub mod config;
pub mod engine;
pub mod metrics;
pub mod pool;
pub mod server;
#[cfg(feature = "sherpa")]
pub mod sherpa;
pub mod text;

pub use jarvis_core::stt_protocol as protocol;
