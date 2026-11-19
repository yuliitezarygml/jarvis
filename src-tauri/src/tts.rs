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
const XEON_API: &str = "https://192.168.50.38:8443";

fn api_url() -> String {
    std::env::var("JARVIS_TTS_URL").unwrap_or_else(|_| XEON_API.to_owned())
}

pub fn client() -> Result<Client> {
    let url = provider::base(&api_url())?;
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(90));
    if url.origin().ascii_serialization() == XEON_API {
        let ca = reqwest::Certificate::from_pem(include_bytes!("../config/xeon-tts-ca.pem"))?;
        builder = builder
            .no_proxy()
            .tls_built_in_root_certs(false)
            .add_root_certificate(ca);
    }
    Ok(builder.build()?)
}

fn request(http: &Client, method: reqwest::Method, path: &str) -> Result<RequestBuilder> {
    let url = api_url();
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
        .context("Нет соединения с API голосового сервера")?;
    provider::status(r.status())?;
    Ok(r.json().await?)
}
pub async fn speech(
    core: Arc<Core>,
    turn: String,
    cancel: CancellationToken,
    mut rx: mpsc::UnboundedReceiver<String>,
    start: Instant,
) -> Result<()> {
    let mut seq = 0;
    let mut first = true;
    let mut reader = crate::speech_normalizer::SpeechNormalizer::default();
    loop {
        let delta = tokio::select! {biased; _=cancel.cancelled()=>return Ok(()), p=rx.recv()=>p};
        let phrases = match &delta {
            Some(raw) => reader.push(raw),
            None => reader.finish(),
        };
        for text in phrases {
            seq += 1;
            let id = format!("{turn}-{seq}");
            let play = phrase(&core, &turn, &id, &text, &mut first, start);
            tokio::select! {biased;_=cancel.cancelled()=>{if let Ok(req)=request(&core.http,reqwest::Method::DELETE,&format!("/tts/{id}")){let _=req.timeout(Duration::from_secs(2)).send().await;}bail!("Отменено")},r=play=>r?}
        }
        if delta.is_none() {
            break;
        }
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
        .context("Нет соединения с API голосового сервера")?;
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
                    if !heard {
                        core.remember_playback(text);
                    }
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
