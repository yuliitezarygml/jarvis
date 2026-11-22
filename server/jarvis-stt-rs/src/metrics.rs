//! Счётчики для `GET /metrics` (текстовый формат Prometheus).

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

#[derive(Default)]
pub struct Metrics {
    pub sessions_active: AtomicU64,
    pub sessions_total: AtomicU64,
    pub sessions_rejected: AtomicU64,
    pub requests_total: AtomicU64,
    pub requests_rejected: AtomicU64,
    pub partials_total: AtomicU64,
    pub finals_total: AtomicU64,
    pub errors_total: AtomicU64,
    pub audio_ms_total: AtomicU64,
    /// Время от `end` до отправки `final` (и время декодирования `POST /stt`).
    pub final_latency_us_total: AtomicU64,
    pub final_latency_us_max: AtomicU64,
}

impl Metrics {
    pub fn final_done(&self, latency: Duration, audio_ms: u64) {
        let us = latency.as_micros() as u64;
        self.finals_total.fetch_add(1, Relaxed);
        self.audio_ms_total.fetch_add(audio_ms, Relaxed);
        self.final_latency_us_total.fetch_add(us, Relaxed);
        self.final_latency_us_max.fetch_max(us, Relaxed);
    }

    pub fn render(&self) -> String {
        let rows = [
            ("jarvis_stt_sessions_active", &self.sessions_active),
            ("jarvis_stt_sessions_total", &self.sessions_total),
            (
                "jarvis_stt_sessions_rejected_total",
                &self.sessions_rejected,
            ),
            ("jarvis_stt_requests_total", &self.requests_total),
            (
                "jarvis_stt_requests_rejected_total",
                &self.requests_rejected,
            ),
            ("jarvis_stt_partials_total", &self.partials_total),
            ("jarvis_stt_finals_total", &self.finals_total),
            ("jarvis_stt_errors_total", &self.errors_total),
            ("jarvis_stt_audio_milliseconds_total", &self.audio_ms_total),
            (
                "jarvis_stt_final_latency_microseconds_total",
                &self.final_latency_us_total,
            ),
            (
                "jarvis_stt_final_latency_microseconds_max",
                &self.final_latency_us_max,
            ),
        ];
        rows.iter()
            .map(|(k, v)| format!("{k} {}\n", v.load(Relaxed)))
            .collect()
    }
}
