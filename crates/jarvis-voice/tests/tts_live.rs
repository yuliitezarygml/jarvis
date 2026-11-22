//! Живые тесты против TTS-сервера Xeon (`https://192.168.50.38:8443`, TLS частным CA). Звук не воспроизводят.
//! Запуск: `cargo test --test tts_live -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use jarvis_core::{CancellationToken, Config};
use jarvis_voice::VoiceError;
use jarvis_voice::tls::default_project_root;
use jarvis_voice::tts::TtsClient;

fn client() -> TtsClient {
    let root = default_project_root();
    let cfg = Config::load(root.join("config.json")).map(|c| c.tts).unwrap_or_default();
    TtsClient::new(&cfg, &root).expect("клиент TTS")
}

const LONG: &str = "Сэр, это длинная проверочная фраза. Я буду говорить её достаточно долго, \
    чтобы сервер не успел закончить синтез до отмены. Раз, два, три, четыре, пять, шесть, семь, восемь, девять, десять. \
    Продолжаю говорить, пока вы не попросите меня остановиться. Ещё немного текста для надёжности проверки.";

#[tokio::test]
#[ignore = "сеть: реальный TTS-сервер Xeon"]
async fn tts_live_ready() {
    let c = client();
    let r = c.ready().await.expect("/ready");
    eprintln!("{} /ready → {r:?}", c.base_url());
    assert!(r.ready);
    assert_eq!(r.voice, "jarvis-ru");
    assert_eq!(r.sample_rate, 24_000);
}

#[tokio::test]
#[ignore = "сеть: реальный TTS-сервер Xeon"]
async fn tts_live_synthesize_pcm() {
    let c = client();
    let t0 = Instant::now();
    let mut s = c.synthesize_stream("Добрый день, сэр. Все системы в норме.", None, CancellationToken::new()).await.expect("синтез");
    let first_header = t0.elapsed();
    let mut total = 0usize;
    let mut first_chunk = None;
    let mut chunks = 0;
    while let Some(chunk) = s.next().await {
        let chunk = chunk.expect("кусок PCM");
        first_chunk.get_or_insert(t0.elapsed());
        total += chunk.len();
        chunks += 1;
    }
    let secs = total as f64 / (24_000.0 * 2.0);
    eprintln!(
        "id={} rate={} заголовки через {:?}, первый звук через {:?}, всего {:?}; {chunks} кусков, {total} байт = {secs:.2} с аудио",
        s.request_id,
        s.sample_rate,
        first_header,
        first_chunk.unwrap(),
        t0.elapsed()
    );
    assert!(total > 0);
    assert_eq!(total % 2, 0, "s16le: чётное число байт");
    assert!((0.8..15.0).contains(&secs), "длительность фразы правдоподобна: {secs}");
}

#[tokio::test]
#[ignore = "сеть: реальный TTS-сервер Xeon"]
async fn tts_live_cancel() {
    let c = client();
    // 1) Явный DELETE во время синтеза.
    let mut s = c.synthesize_stream(LONG, None, CancellationToken::new()).await.expect("синтез");
    let id = s.request_id.clone();
    assert!(s.next().await.expect("первый кусок").is_ok());
    let t = Instant::now();
    let cancelled = c.cancel(&id).await.expect("DELETE");
    eprintln!("DELETE /tts/{id} → {cancelled} за {:?}", t.elapsed());
    assert!(cancelled, "сервер должен знать активный запрос");
    // Поток после отмены заканчивается (ошибкой или концом) быстро, а не дочитывает всю фразу.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = 0;
        while let Some(chunk) = s.next().await {
            match chunk {
                Ok(b) => bytes += b.len(),
                Err(_) => break,
            }
        }
        bytes
    })
    .await;
    eprintln!("после отмены дочитано {ended:?} байт");
    assert!(ended.is_ok(), "поток не завершился после DELETE");

    // 2) Отмена токеном (барж-ин): поток сразу даёт Cancelled, клиент сам шлёт DELETE.
    let token = CancellationToken::new();
    let mut s = c.synthesize_stream(LONG, None, token.clone()).await.expect("синтез");
    let id = s.request_id.clone();
    assert!(s.next().await.expect("первый кусок").is_ok());
    token.cancel();
    let t = Instant::now();
    assert_eq!(s.next().await.expect("ответ").err(), Some(VoiceError::Cancelled));
    assert!(t.elapsed() < Duration::from_millis(100));
    tokio::time::sleep(Duration::from_millis(700)).await;
    // DELETE уже ушёл из guard — повторный ничего не находит.
    let again = c.cancel(&id).await.expect("повторный DELETE");
    eprintln!("повторный DELETE /tts/{id} → {again}");
    assert!(!again, "синтез должен быть уже отменён клиентом");
}
