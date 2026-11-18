use crate::events::Kind;
use crate::{audio, core::Core, provider};
use anyhow::{bail, ensure, Context, Result};
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
fn request(http: &Client, method: reqwest::Method, path: &str) -> Result<RequestBuilder> {
    let url = std::env::var("JARVIS_TTS_URL").unwrap_or_else(|_| "http://127.0.0.1:18181".into());
    provider::base(&url)?;
    Ok(http
        .request(method, format!("{}{path}", url.trim_end_matches('/')))
        .bearer_auth(std::env::var("JARVIS_TTS_TOKEN").context("Токен TTS не настроен")?))
}
pub async fn ready(http: &Client) -> Result<Value> {
    let r = request(http, reqwest::Method::GET, "/ready")?
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .context("TTS недоступен. Проверьте SSH-туннель")?;
    provider::status(r.status())?;
    Ok(r.json().await?)
}
pub struct Sentences(String);
impl Sentences {
    pub fn new() -> Self {
        Self(String::new())
    }
    pub fn push(&mut self, s: &str, flush: bool) -> Vec<String> {
        self.0.push_str(s);
        let mut out = vec![];
        loop {
            let mut cut = None;
            let mut iter = self.0.char_indices().peekable();
            let mut last_space = None;
            let mut prev = ' ';
            for (count, (i, c)) in std::iter::from_fn(|| iter.next()).enumerate() {
                let end = i + c.len_utf8();
                let next = self.0[end..].chars().next();
                if c.is_whitespace() {
                    last_space = Some(end);
                }
                let decimal =
                    c == '.' && prev.is_ascii_digit() && next.is_some_and(|v| v.is_ascii_digit());
                if !decimal
                    && ((matches!(c, '!' | '?' | '…' | '\n'))
                        || (c == '.' && (next.is_some_and(char::is_whitespace) || flush)))
                {
                    cut = Some(end);
                    break;
                }
                if count >= 240 {
                    cut = Some(last_space.filter(|p| *p > 0).unwrap_or(end));
                    break;
                }
                prev = c;
            }
            let n = match cut {
                Some(n) => n,
                None if flush && !self.0.is_empty() => self.0.len(),
                _ => break,
            };
            let part = self.0.drain(..n).collect::<String>();
            if part.chars().any(char::is_alphanumeric) {
                out.push(part.trim().to_owned());
            }
        }
        out
    }
}
pub async fn speech(
    core: Arc<Core>,
    turn: String,
    cancel: CancellationToken,
    mut rx: mpsc::Receiver<String>,
    start: Instant,
) -> Result<()> {
    let mut seq = 0;
    let mut first = true;
    while let Some(text) = tokio::select! {_=cancel.cancelled()=>None,p=rx.recv()=>p} {
        seq += 1;
        let id = format!("{turn}-{seq}");
        let play = phrase(&core, &turn, &id, &text, &mut first, start);
        tokio::select! {biased;_=cancel.cancelled()=>{if let Ok(req)=request(&core.http,reqwest::Method::DELETE,&format!("/tts/{id}")){let _=req.timeout(Duration::from_secs(2)).send().await;}bail!("Отменено")},r=play=>r?}
    }
    let drain = Instant::now();
    while !cancel.is_cancelled() && audio::pending() > 0 {
        ensure!(
            drain.elapsed() < Duration::from_secs(4),
            "Аудиоустройство перестало воспроизводить звук"
        );
        tokio::time::sleep(Duration::from_millis(12)).await;
    }
    Ok(())
}
async fn phrase(
    core: &Core,
    turn: &str,
    id: &str,
    text: &str,
    first: &mut bool,
    start: Instant,
) -> Result<()> {
    let r = request(&core.http, reqwest::Method::POST, "/tts/stream")?
        .header("X-Request-Id", id)
        .json(&json!({"text":text,"format":"pcm"}))
        .send()
        .await
        .context("TTS недоступен. Проверьте SSH-туннель")?;
    provider::status(r.status())?;
    ensure!(
        r.headers()
            .get("x-audio-format")
            .and_then(|h| h.to_str().ok())
            == Some("s16le;rate=24000;channels=1"),
        "Неожиданный формат аудио"
    );
    let mut stream = r.bytes_stream();
    let mut low = None;
    let mut heard = false;
    while let Some(b) = stream.next().await {
        let b = b?;
        let mut pcm = Vec::with_capacity(b.len().div_ceil(2));
        for &v in &b {
            if let Some(lo) = low.take() {
                pcm.push(i16::from_le_bytes([lo, v]));
            } else {
                low = Some(v);
            }
        }
        for block in pcm.chunks(1920) {
            if block.is_empty() {
                continue;
            }
            let wait = Instant::now();
            loop {
                ensure!(
                    wait.elapsed() < Duration::from_secs(4),
                    "Аудиоустройство не освобождает буфер"
                );
                let accepted = core
                    .for_turn(turn, || audio::play(block))
                    .context("Отменено")??;
                if accepted {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if *first {
                *first = false;
                core.event(
                    turn,
                    Kind::Speaking,
                    json!({"ms":start.elapsed().as_millis()}),
                );
            }
            heard = true;
            let rms = (block
                .iter()
                .map(|x| (*x as f32 / 32768.).powi(2))
                .sum::<f32>()
                / block.len() as f32)
                .sqrt();
            core.event(turn, Kind::Level, json!(rms));
        }
    }
    ensure!(heard && low.is_none(), "Получен неполный поток PCM");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_before_final_token() {
        let mut b = Sentences::new();
        assert!(b.push("Температура 2.", false).is_empty());
        assert_eq!(b.push("5. След", false), ["Температура 2.5."]);
        assert_eq!(b.push("ом!", false), ["Следом!"]);
    }
}
