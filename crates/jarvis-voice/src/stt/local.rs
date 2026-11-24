//! Локальное распознавание речи на этом ПК: sherpa-onnx + GigaAM v3 CTC (русский, int8, ~215 МБ).
//! Сеть не нужна. Модель лежит в `data/models/stt/gigaam-v3-ctc` (`model*.onnx` + `tokens.txt`).
//!
//! Модель офлайновая: `final` — одно декодирование всей фразы после конца речи (≈0,1–0,3 с на обычном CPU).
//! «Потоковая» сессия имитируется: `partial` — декодирование накопленного звука каждые ~0,5 с в фоне,
//! причём только если распознаватель свободен (очереди не бывает, `final` не ждёт кучу partial).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use async_trait::async_trait;
use jarvis_core::CancellationToken;
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig};
use tokio::sync::mpsc;

use super::{SttEngine, SttEvent, SttResult, SttStream, StreamCmd};
use crate::error::{Result, VoiceError};

const SR: i32 = 16_000;
/// Каталог модели по умолчанию внутри `data_dir`.
pub const DEFAULT_MODEL_SUBDIR: &str = "models/stt/gigaam-v3-ctc";
/// Partial не чаще, чем раз в столько сэмплов нового звука (0,5 с).
const PARTIAL_EVERY: usize = SR as usize / 2;
/// Partial только для первых N секунд фразы: дальше они дорогие, а текст всё равно придёт в final.
const PARTIAL_MAX_SAMPLES: usize = SR as usize * 12;

struct Inner {
    dir: PathBuf,
    threads: i32,
    rec: OnceLock<std::result::Result<Mutex<OfflineRecognizer>, String>>,
}

impl Inner {
    /// Распознаватель (загружается при первом обращении; блокирует — звать из blocking-потока).
    fn rec(&self) -> Result<&Mutex<OfflineRecognizer>> {
        self.rec
            .get_or_init(|| {
                let t0 = Instant::now();
                let r = load(&self.dir, self.threads);
                match &r {
                    Ok(_) => tracing::info!(ms = t0.elapsed().as_millis() as u64, dir = %self.dir.display(), "локальный STT: модель загружена"),
                    Err(e) => tracing::warn!(error = %e, "локальный STT: модель не загрузилась"),
                }
                r.map(Mutex::new)
            })
            .as_ref()
            .map_err(|e| VoiceError::Config(e.clone()))
    }

    fn decode(&self, samples: &[f32]) -> Result<String> {
        let rec = self.rec()?.lock().unwrap_or_else(|e| e.into_inner());
        Ok(decode_with(&rec, samples))
    }

    /// Partial: только если распознаватель свободен прямо сейчас.
    fn try_decode(&self, samples: &[f32]) -> Option<String> {
        let rec = self.rec().ok()?.try_lock().ok()?;
        Some(decode_with(&rec, samples))
    }
}

fn decode_with(rec: &OfflineRecognizer, samples: &[f32]) -> String {
    // < 0,1 с — распознавать нечего (и модель не принимает слишком короткий вход).
    if samples.len() < SR as usize / 10 {
        return String::new();
    }
    let s = rec.create_stream();
    s.accept_waveform(SR, samples);
    rec.decode(&s);
    s.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default()
}

/// Файл `prefix*.onnx` в каталоге; при нескольких — предпочтение `int8`.
fn find_onnx(dir: &Path, prefix: &str) -> std::result::Result<PathBuf, String> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("нет каталога модели {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            n.starts_with(prefix) && n.ends_with(".onnx")
        })
        .collect();
    found.sort_by_key(|p| !p.to_string_lossy().contains("int8"));
    found.into_iter().next().ok_or_else(|| format!("нет {prefix}*.onnx в {}", dir.display()))
}

fn load(dir: &Path, threads: i32) -> std::result::Result<OfflineRecognizer, String> {
    let model = find_onnx(dir, "model")?;
    let tokens = dir.join("tokens.txt");
    if !tokens.is_file() {
        return Err(format!("нет файла {}", tokens.display()));
    }
    let mut c = OfflineRecognizerConfig::default();
    c.model_config.num_threads = threads;
    c.model_config.provider = Some("cpu".into());
    c.decoding_method = Some("greedy_search".into());
    c.model_config.nemo_ctc.model = Some(model.to_string_lossy().into_owned());
    c.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
    OfflineRecognizer::create(&c).ok_or_else(|| format!("sherpa-onnx не смог открыть модель в {}", dir.display()))
}

fn to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|&s| s as f32 / 32768.0).collect()
}

/// Локальный STT (GigaAM v3 CTC через sherpa-onnx).
#[derive(Clone)]
pub struct LocalStt {
    inner: Arc<Inner>,
    name: String,
}

impl LocalStt {
    /// Есть ли в каталоге всё нужное для модели (без загрузки).
    pub fn model_present(dir: &Path) -> bool {
        find_onnx(dir, "model").is_ok() && dir.join("tokens.txt").is_file()
    }

    /// Каталог модели по умолчанию: `<project_root>/<data_dir>/models/stt/gigaam-v3-ctc`.
    pub fn default_dir(project_root: &Path, data_dir: &str) -> PathBuf {
        let data = Path::new(data_dir);
        let data = if data.is_absolute() { data.to_path_buf() } else { project_root.join(data) };
        data.join(DEFAULT_MODEL_SUBDIR)
    }

    /// Создать движок; модель грузится в фоне сразу (≈1–3 с), первая фраза её дождётся.
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        if !Self::model_present(&dir) {
            return Err(VoiceError::Config(format!("локальная модель распознавания не найдена в {}", dir.display())));
        }
        // Половина ядер, но не больше 4: распознавание короткое, остальному ПК тоже нужен CPU.
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let threads = (cores / 2).clamp(1, 4) as i32;
        let inner = Arc::new(Inner { dir, threads, rec: OnceLock::new() });
        let warm = inner.clone();
        std::thread::Builder::new()
            .name("stt-local-load".into())
            .spawn(move || {
                if warm.rec().is_ok() {
                    // Прогрев ONNX Runtime: первое декодирование заметно медленнее остальных.
                    let _ = warm.decode(&vec![0.0; SR as usize / 2]);
                }
            })
            .map_err(|e| VoiceError::Other(format!("поток загрузки модели: {e}")))?;
        Ok(Self { inner, name: "local:gigaam-v3-ctc".into() })
    }
}

#[async_trait]
impl SttEngine for LocalStt {
    fn name(&self) -> &str {
        &self.name
    }

    async fn transcribe(&self, pcm16k: &[i16], cancel: &CancellationToken) -> Result<SttResult> {
        let samples = to_f32(pcm16k);
        let duration_ms = pcm16k.len() as u64 * 1000 / SR as u64;
        let inner = self.inner.clone();
        let job = tokio::task::spawn_blocking(move || inner.decode(&samples));
        tokio::select! {
            _ = cancel.cancelled() => Err(VoiceError::Cancelled),
            r = job => {
                let text = r.map_err(|e| VoiceError::Other(format!("сбой распознавания: {e}")))??;
                Ok(SttResult { text, duration_ms })
            }
        }
    }

    fn open_stream(&self, events: mpsc::Sender<SttEvent>) -> Option<SttStream> {
        let (tx, mut rx) = mpsc::unbounded_channel::<StreamCmd>();
        let inner = self.inner.clone();
        let model = self.name.clone();
        let task = tokio::spawn(async move {
            let _ = events.send(SttEvent::Ready { model }).await;
            let mut audio: Vec<f32> = Vec::new();
            let mut decoded_at = 0usize;
            // Номер фразы: partial от прошлой фразы (досчитался после end/reset) не отправляем.
            let generation = Arc::new(AtomicU64::new(0));
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    StreamCmd::Audio(pcm) => {
                        audio.extend(pcm.iter().map(|&s| s as f32 / 32768.0));
                        if audio.len() >= decoded_at + PARTIAL_EVERY && audio.len() <= PARTIAL_MAX_SAMPLES {
                            decoded_at = audio.len();
                            let (inner, events, gen_now, generation) =
                                (inner.clone(), events.clone(), generation.load(Ordering::SeqCst), generation.clone());
                            let snapshot = audio.clone();
                            tokio::task::spawn_blocking(move || {
                                if let Some(text) = inner.try_decode(&snapshot)
                                    && !text.is_empty()
                                    && generation.load(Ordering::SeqCst) == gen_now
                                {
                                    let _ = events.blocking_send(SttEvent::Partial { text });
                                }
                            });
                        }
                    }
                    StreamCmd::End => {
                        generation.fetch_add(1, Ordering::SeqCst);
                        let samples = std::mem::take(&mut audio);
                        decoded_at = 0;
                        let duration_ms = samples.len() as u64 * 1000 / SR as u64;
                        let inner = inner.clone();
                        let t0 = Instant::now();
                        let ev = match tokio::task::spawn_blocking(move || inner.decode(&samples)).await {
                            Ok(Ok(text)) => {
                                tracing::debug!(ms = t0.elapsed().as_millis() as u64, duration_ms, "локальный STT: final");
                                SttEvent::Final { text, duration_ms }
                            }
                            Ok(Err(e)) => SttEvent::Error { message: e.to_string() },
                            Err(e) => SttEvent::Error { message: format!("сбой распознавания: {e}") },
                        };
                        if events.send(ev).await.is_err() {
                            break;
                        }
                    }
                    StreamCmd::Reset => {
                        generation.fetch_add(1, Ordering::SeqCst);
                        audio.clear();
                        decoded_at = 0;
                    }
                    StreamCmd::Close => break,
                }
            }
        });
        Some(SttStream::new(tx, task))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_dir() -> Option<PathBuf> {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data").join(DEFAULT_MODEL_SUBDIR);
        LocalStt::model_present(&d).then_some(d)
    }

    /// WAV 16 кГц mono s16 → PCM (простейший разбор: ищем чанк `data`).
    fn read_wav(p: &Path) -> Vec<i16> {
        let b = std::fs::read(p).unwrap();
        let pos = b.windows(4).position(|w| w == b"data").expect("нет data") + 8;
        b[pos..].as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect()
    }

    #[test]
    fn missing_model_is_config_error() {
        let e = LocalStt::new(std::env::temp_dir().join("нет-такой-модели")).err().unwrap();
        assert!(matches!(e, VoiceError::Config(_)), "{e:?}");
    }

    // Ручные тесты: тестовый exe лежит не рядом с DLL sherpa-onnx, и Windows подставляет системную
    // onnxruntime.dll (1.17) из System32 → падение. Запуск: скопировать target/debug/*.dll к тестовому exe
    // и выполнить `<exe> stt::local --ignored`. В самом приложении DLL лежат рядом с jarvis.exe.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "нужны DLL sherpa-onnx рядом с тестовым exe (см. комментарий)"]
    async fn recognizes_example_wav_fast() {
        let Some(dir) = model_dir() else {
            eprintln!("модели нет в data/models — тест пропущен");
            return;
        };
        let wav = dir.join("test_wavs").join("example.wav");
        if !wav.is_file() {
            return;
        }
        let pcm = read_wav(&wav);
        let stt = LocalStt::new(&dir).unwrap();
        let cancel = CancellationToken::new();
        // Первая фраза ждёт загрузку модели.
        let r = stt.transcribe(&pcm, &cancel).await.unwrap();
        assert!(r.text.chars().filter(|c| c.is_alphabetic()).count() > 5, "текст: {:?}", r.text);
        // Повторная — уже без загрузки: должна быть быстрой.
        let t0 = Instant::now();
        let r2 = stt.transcribe(&pcm, &cancel).await.unwrap();
        let ms = t0.elapsed().as_millis();
        eprintln!("локальный STT: {} мс на {} мс звука: {:?}", ms, r2.duration_ms, r2.text);
        assert_eq!(r.text, r2.text);
        assert!(ms < 3000, "слишком медленно: {ms} мс");
    }

    /// Ручная проверка распознавания папки WAV (любой частоты): `JARVIS_STT_WAV_DIR=<папка>`. Печатает
    /// «файл → текст» (например, фразы, синтезированные голосом Джарвиса).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "нужны DLL sherpa-onnx рядом с тестовым exe и JARVIS_STT_WAV_DIR"]
    async fn transcribe_wav_dir() {
        let (Some(model), Ok(dir)) = (model_dir(), std::env::var("JARVIS_STT_WAV_DIR")) else {
            eprintln!("нет модели или JARVIS_STT_WAV_DIR — пропуск");
            return;
        };
        let stt = LocalStt::new(&model).unwrap();
        let cancel = CancellationToken::new();
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
        files.retain(|p| p.extension().is_some_and(|e| e == "wav"));
        files.sort();
        for p in files {
            let wav = crate::pcm::wav_decode(&std::fs::read(&p).unwrap()).unwrap();
            let pcm = wav.to_stt_pcm().unwrap();
            let r = stt.transcribe(&pcm, &cancel).await.unwrap();
            println!("STT\t{}\t{}", p.file_stem().unwrap().to_string_lossy(), r.text);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "нужны DLL sherpa-onnx рядом с тестовым exe (см. комментарий выше)"]
    async fn stream_gives_final_after_end() {
        let Some(dir) = model_dir() else { return };
        let wav = dir.join("test_wavs").join("example.wav");
        if !wav.is_file() {
            return;
        }
        let pcm = read_wav(&wav);
        let stt = LocalStt::new(&dir).unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let s = stt.open_stream(tx).unwrap();
        for chunk in pcm.chunks(256) {
            s.send_audio(chunk);
        }
        s.end();
        let fin = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                match rx.recv().await {
                    Some(SttEvent::Final { text, .. }) => break text,
                    Some(_) => {}
                    None => panic!("поток закрылся"),
                }
            }
        })
        .await
        .unwrap();
        assert!(!fin.is_empty());
    }
}
