//! Движок на sherpa-onnx (официальный крейт `sherpa-onnx`, нативная `libsherpa-onnx-c-api.so`).
//!
//! Схема «гибрид»: потоковая модель (T-one / streaming Zipformer) даёт `partial` по ходу речи,
//! офлайн-модель (GigaAM v3 RNNT/CTC, offline Zipformer) по всей фразе даёт `final` после `end`.
//! Любую из двух можно отключить: без офлайн-модели `final` берётся из потоковой, без потоковой
//! `partial` не отправляются.
//!
//! Пул: `pool` экземпляров каждого распознавателя (у каждого свой ONNX Runtime с `threads` потоками).
//! Потоковые сессии распределяются по экземплярам по кругу, декодирование экземпляра — под мьютексом;
//! офлайн-экземпляры выдаются из [`BlockingPool`] с таймаутом ожидания.

use crate::audio::SAMPLE_RATE;
use crate::config::ModelSpec;
use crate::engine::{Engine, Recognition};
use crate::pool::BlockingPool;
use anyhow::{Context, Result, bail, ensure};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineStream,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SR: i32 = SAMPLE_RATE as i32;

/// Файл модели в каталоге: имя начинается с `prefix` и кончается на `.onnx`; при нескольких
/// вариантах предпочитаем `int8`.
fn find_onnx(dir: &Path, prefix: &str) -> Result<String> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("каталог модели {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            n.starts_with(prefix) && n.ends_with(".onnx")
        })
        .collect();
    found.sort_by_key(|p| !p.to_string_lossy().contains("int8"));
    let p = found
        .into_iter()
        .next()
        .with_context(|| format!("нет {prefix}*.onnx в {}", dir.display()))?;
    Ok(p.to_string_lossy().into_owned())
}

fn file(dir: &Path, name: &str) -> Result<String> {
    let p = dir.join(name);
    ensure!(p.is_file(), "нет файла {}", p.display());
    Ok(p.to_string_lossy().into_owned())
}

fn transducer(dir: &Path) -> Result<(String, String, String)> {
    Ok((
        find_onnx(dir, "encoder")?,
        find_onnx(dir, "decoder")?,
        find_onnx(dir, "joiner")?,
    ))
}

struct OnlineSlot {
    rec: OnlineRecognizer,
    lock: Mutex<()>,
}

struct Online {
    slots: Vec<OnlineSlot>,
    next: AtomicUsize,
    /// Тишина перед фразой (T-one ждёт ~0,3 с) и после (досчитать последние слова), в сэмплах.
    left_pad: usize,
    tail_pad: usize,
}

struct Offline {
    pool: BlockingPool<OfflineRecognizer>,
    wait: Duration,
}

struct Inner {
    name: String,
    online: Option<Online>,
    offline: Option<Offline>,
}

pub struct SherpaEngine {
    inner: Arc<Inner>,
}

#[derive(Debug, Clone)]
pub struct EngineOptions {
    pub threads: usize,
    pub pool: usize,
    /// Сколько ждать свободный офлайн-распознаватель.
    pub queue_timeout: Duration,
}

fn online_config(
    spec: &ModelSpec,
    threads: usize,
) -> Result<(OnlineRecognizerConfig, usize, usize)> {
    let mut c = OnlineRecognizerConfig::default();
    c.model_config.num_threads = threads as i32;
    c.model_config.provider = Some("cpu".into());
    c.decoding_method = Some("greedy_search".into());
    c.enable_endpoint = false; // конец фразы определяет клиент (VAD) сообщением end
    let dir = &spec.dir;
    let pads = match spec.kind.as_str() {
        "t-one" | "tone" => {
            c.model_config.t_one_ctc.model = Some(find_onnx(dir, "model")?);
            c.model_config.tokens = Some(file(dir, "tokens.txt")?);
            (SAMPLE_RATE as usize * 3 / 10, SAMPLE_RATE as usize * 6 / 10)
        }
        "zipformer" | "transducer" => {
            let (e, d, j) = transducer(dir)?;
            c.model_config.transducer.encoder = Some(e);
            c.model_config.transducer.decoder = Some(d);
            c.model_config.transducer.joiner = Some(j);
            c.model_config.tokens = Some(file(dir, "tokens.txt")?);
            (0, SAMPLE_RATE as usize * 66 / 100)
        }
        other => bail!("неизвестный тип потоковой модели {other:?} (t-one | zipformer)"),
    };
    Ok((c, pads.0, pads.1))
}

fn offline_config(spec: &ModelSpec, threads: usize) -> Result<OfflineRecognizerConfig> {
    let mut c = OfflineRecognizerConfig::default();
    c.model_config.num_threads = threads as i32;
    c.model_config.provider = Some("cpu".into());
    c.decoding_method = Some("greedy_search".into());
    let dir = &spec.dir;
    match spec.kind.as_str() {
        "gigaam-ctc" | "nemo-ctc" => {
            c.model_config.nemo_ctc.model = Some(find_onnx(dir, "model")?);
        }
        "gigaam-rnnt" | "nemo-transducer" | "zipformer" => {
            let (encoder, decoder, joiner) = transducer(dir)?;
            c.model_config.transducer = OfflineTransducerModelConfig {
                encoder: Some(encoder),
                decoder: Some(decoder),
                joiner: Some(joiner),
            };
            let t = if spec.kind == "zipformer" {
                "transducer"
            } else {
                "nemo_transducer"
            };
            c.model_config.model_type = Some(t.into());
        }
        other => {
            bail!("неизвестный тип офлайн-модели {other:?} (gigaam-rnnt | gigaam-ctc | zipformer)")
        }
    }
    c.model_config.tokens = Some(file(dir, "tokens.txt")?);
    Ok(c)
}

impl SherpaEngine {
    pub fn load(
        streaming_spec: Option<&ModelSpec>,
        offline_spec: Option<&ModelSpec>,
        opt: &EngineOptions,
    ) -> Result<Self> {
        ensure!(
            streaming_spec.is_some() || offline_spec.is_some(),
            "нужна хотя бы одна модель"
        );
        let online = match streaming_spec {
            None => None,
            Some(spec) => {
                let (cfg, left_pad, tail_pad) = online_config(spec, opt.threads)?;
                let slots = (0..opt.pool)
                    .map(|_| {
                        OnlineRecognizer::create(&cfg)
                            .map(|rec| OnlineSlot {
                                rec,
                                lock: Mutex::new(()),
                            })
                            .with_context(|| format!("не удалось загрузить {}", spec.dir.display()))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Some(Online {
                    slots,
                    next: AtomicUsize::new(0),
                    left_pad,
                    tail_pad,
                })
            }
        };
        let offline = match offline_spec {
            None => None,
            Some(spec) => {
                let cfg = offline_config(spec, opt.threads)?;
                let recs = (0..opt.pool)
                    .map(|_| {
                        OfflineRecognizer::create(&cfg)
                            .with_context(|| format!("не удалось загрузить {}", spec.dir.display()))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Some(Offline {
                    pool: BlockingPool::new(recs),
                    wait: opt.queue_timeout,
                })
            }
        };
        // Имя из каталогов моделей: `streaming-t-one-…+nemo-transducer-punct-giga-am-v3-…`.
        let name = [streaming_spec, offline_spec]
            .into_iter()
            .flatten()
            .map(ModelSpec::short_name)
            .collect::<Vec<_>>()
            .join("+");
        Ok(Self {
            inner: Arc::new(Inner {
                name,
                online,
                offline,
            }),
        })
    }

    /// Прогрев: первый запуск ONNX Runtime медленный, делаем его до открытия сокета.
    pub fn warmup(&self) -> Result<()> {
        let tone: Vec<f32> = (0..SAMPLE_RATE as usize)
            .map(|i| (i as f32 * 0.07).sin() * 0.01)
            .collect();
        if let Some(on) = &self.inner.online {
            for slot in 0..on.slots.len() {
                let s = new_stream(on, slot);
                let _g = on.slots[slot]
                    .lock
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                s.accept_waveform(SR, &tone);
                drain(&on.slots[slot].rec, &s);
            }
        }
        if let Some(off) = &self.inner.offline {
            let guards: Vec<_> = (0..off.pool.size())
                .filter_map(|_| off.pool.get(Duration::from_secs(1)))
                .collect();
            for rec in &guards {
                decode_offline_with(rec, &tone);
            }
        }
        Ok(())
    }
}

fn new_stream(on: &Online, slot: usize) -> OnlineStream {
    let s = on.slots[slot].rec.create_stream();
    if on.left_pad > 0 {
        s.accept_waveform(SR, &vec![0.0; on.left_pad]);
    }
    s
}

fn drain(rec: &OnlineRecognizer, s: &OnlineStream) {
    while rec.is_ready(s) {
        rec.decode(s);
    }
}

fn online_text(rec: &OnlineRecognizer, s: &OnlineStream) -> String {
    rec.get_result(s)
        .map(|r| r.text.trim().to_string())
        .unwrap_or_default()
}

/// Досчитать потоковую фразу до конца (хвост тишины + input_finished) и вернуть текст.
fn finish_online(on: &Online, slot: usize, s: &OnlineStream) -> String {
    let rec = &on.slots[slot].rec;
    let _g = on.slots[slot]
        .lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    s.accept_waveform(SR, &vec![0.0; on.tail_pad]);
    s.input_finished();
    drain(rec, s);
    online_text(rec, s)
}

fn decode_offline_with(rec: &OfflineRecognizer, samples: &[f32]) -> String {
    let s = rec.create_stream();
    s.accept_waveform(SR, samples);
    rec.decode(&s);
    s.get_result()
        .map(|r| r.text.trim().to_string())
        .unwrap_or_default()
}

impl Offline {
    fn decode(&self, samples: &[f32]) -> Result<String> {
        // < 0,1 с — распознавать нечего (и некоторые модели не принимают такой короткий вход)
        if samples.len() < SAMPLE_RATE as usize / 10 {
            return Ok(String::new());
        }
        let rec = self
            .pool
            .get(self.wait)
            .context("все офлайн-распознаватели заняты")?;
        Ok(decode_offline_with(&rec, samples))
    }
}

impl Engine for SherpaEngine {
    fn model(&self) -> &str {
        &self.inner.name
    }

    fn open(&self) -> Result<Box<dyn Recognition>> {
        let (slot, stream) = match &self.inner.online {
            Some(on) => {
                let slot = on.next.fetch_add(1, Ordering::Relaxed) % on.slots.len();
                (slot, Some(new_stream(on, slot)))
            }
            None => (0, None),
        };
        Ok(Box::new(SherpaRecognition {
            stream,
            slot,
            audio: Vec::new(),
            inner: self.inner.clone(),
        }))
    }

    fn transcribe(&self, samples: &[f32]) -> Result<String> {
        if let Some(off) = &self.inner.offline {
            return off.decode(samples);
        }
        let on = self
            .inner
            .online
            .as_ref()
            .expect("есть хотя бы одна модель");
        let slot = on.next.fetch_add(1, Ordering::Relaxed) % on.slots.len();
        let s = new_stream(on, slot);
        {
            let _g = on.slots[slot]
                .lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for chunk in samples.chunks(SAMPLE_RATE as usize) {
                s.accept_waveform(SR, chunk);
                drain(&on.slots[slot].rec, &s);
            }
        }
        Ok(finish_online(on, slot, &s))
    }
}

/// Поля освобождаются по порядку объявления: поток раньше распознавателей (`inner`).
struct SherpaRecognition {
    stream: Option<OnlineStream>,
    slot: usize,
    /// Звук текущей фразы для офлайн-модели.
    audio: Vec<f32>,
    inner: Arc<Inner>,
}

impl SherpaRecognition {
    fn restart(&mut self) {
        self.audio.clear();
        if let Some(on) = &self.inner.online {
            self.stream = None; // сначала освобождаем старый поток
            self.stream = Some(new_stream(on, self.slot));
        }
    }
}

impl Recognition for SherpaRecognition {
    fn accept(&mut self, samples: &[f32]) -> Result<String> {
        if self.inner.offline.is_some() {
            self.audio.extend_from_slice(samples);
        }
        let (Some(on), Some(s)) = (&self.inner.online, &self.stream) else {
            return Ok(String::new());
        };
        let slot = &on.slots[self.slot];
        let _g = slot.lock.lock().unwrap_or_else(|e| e.into_inner());
        s.accept_waveform(SR, samples);
        drain(&slot.rec, s);
        Ok(online_text(&slot.rec, s))
    }

    fn finish(&mut self) -> Result<String> {
        let text = match (&self.inner.offline, &self.inner.online, &self.stream) {
            (Some(off), _, _) => off.decode(&self.audio),
            (None, Some(on), Some(s)) => Ok(finish_online(on, self.slot, s)),
            _ => Ok(String::new()),
        };
        self.restart();
        text
    }

    fn reset(&mut self) {
        self.restart();
    }
}
