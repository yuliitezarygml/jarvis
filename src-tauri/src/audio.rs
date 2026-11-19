use anyhow::{bail, ensure, Result};
use sha2::{Digest, Sha256};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineStream,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[cfg(any(target_os = "macos", windows))]
mod device {
    unsafe extern "C" {
        pub fn jv_mic_start(continuous: i32) -> i32;
        pub fn jv_mic_status() -> i32;
        pub fn jv_mic_end();
        pub fn jv_read(p: *mut f32, cap: i32) -> i32;
        pub fn jv_play(p: *const i16, n: i32) -> i32;
        pub fn jv_stop();
        pub fn jv_pending() -> i32;
        pub fn jv_gpu(out: *mut i8, n: i32);
    }
}
#[cfg(any(target_os = "macos", windows))]
pub fn mic_start(continuous: bool) -> Result<()> {
    match unsafe { device::jv_mic_start(i32::from(continuous)) } {
        1 => Ok(()),
        -1 => bail!("Разрешите доступ к микрофону в системных настройках конфиденциальности"),
        -2 => bail!("Истекло время ожидания разрешения микрофона"),
        _ => bail!("Не удалось открыть устройство ввода. Проверьте выбранный микрофон."),
    }
}
#[cfg(any(target_os = "macos", windows))]
pub fn mic_status() -> i32 {
    unsafe { device::jv_mic_status() }
}
#[cfg(any(target_os = "macos", windows))]
pub fn mic_end() {
    unsafe { device::jv_mic_end() }
}
#[cfg(any(target_os = "macos", windows))]
pub fn read(buf: &mut [f32]) -> usize {
    unsafe { device::jv_read(buf.as_mut_ptr(), buf.len() as i32) as usize }
}
#[cfg(any(target_os = "macos", windows))]
pub fn play(buf: &[i16]) -> Result<bool> {
    match unsafe { device::jv_play(buf.as_ptr(), buf.len() as i32) } {
        1 => Ok(true),
        0 => Ok(false),
        _ => bail!("Выход звука недоступен"),
    }
}
#[cfg(any(target_os = "macos", windows))]
pub fn stop() {
    unsafe { device::jv_stop() }
}
#[cfg(any(target_os = "macos", windows))]
pub fn pending() -> i32 {
    unsafe { device::jv_pending() }
}
#[cfg(any(target_os = "macos", windows))]
pub fn gpu() -> String {
    let mut b = [0i8; 160];
    unsafe {
        device::jv_gpu(b.as_mut_ptr(), 160);
        std::ffi::CStr::from_ptr(b.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}
#[cfg(not(any(target_os = "macos", windows)))]
mod fallback {
    use anyhow::{bail, Result};
    pub fn mic_start(continuous: bool) -> Result<()> {
        bail!("Голосовой ввод поддерживается в macOS")
    }
    pub fn mic_status() -> i32 {
        -1
    }
    pub fn mic_end() {}
    pub fn read(_: &mut [f32]) -> usize {
        0
    }
    pub fn play(_: &[i16]) -> Result<bool> {
        bail!("Голосовой вывод поддерживается в macOS")
    }
    pub fn stop() {}
    pub fn pending() -> i32 {
        0
    }
    pub fn gpu() -> String {
        String::new()
    }
}
#[cfg(not(any(target_os = "macos", windows)))]
pub use fallback::*;
pub struct Recognizer {
    root: PathBuf,
    engine: Mutex<Option<OfflineRecognizer>>,
    online: Mutex<Option<Arc<OnlineRecognizer>>>,
}
impl Recognizer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            engine: Mutex::new(None),
            online: Mutex::new(None),
        }
    }
    pub fn available(&self) -> bool {
        self.streaming_available() || self.offline_available()
    }
    pub fn streaming_available(&self) -> bool {
        let dir = self.root.join("streaming");
        dir.join("model.onnx").is_file() && dir.join("tokens.txt").is_file()
    }
    pub fn offline_available(&self) -> bool {
        [
            "encoder.int8.onnx",
            "decoder.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ]
        .iter()
        .all(|s| self.root.join(s).is_file())
    }
    pub fn streaming_engine(&self) -> Option<Arc<OnlineRecognizer>> {
        let mut guard = self.online.lock().unwrap();
        if guard.is_none() && self.streaming_available() {
            let dir = self.root.join("streaming");
            // Avoid passing a partial download or mismatched tokens to the native runtime.
            for line in include_str!("../../scripts/stt-streaming.sha256").lines() {
                let (expected, name) = line.split_once("  ")?;
                if name == "LICENSE" {
                    continue;
                }
                let mut file = std::fs::File::open(dir.join(name)).ok()?;
                let mut hash = Sha256::new();
                std::io::copy(&mut file, &mut hash).ok()?;
                if format!("{:x}", hash.finalize()) != expected {
                    return None;
                }
            }
            let mut config = OnlineRecognizerConfig::default();
            config.model_config.t_one_ctc.model =
                Some(dir.join("model.onnx").to_string_lossy().into_owned());
            config.model_config.tokens =
                Some(dir.join("tokens.txt").to_string_lossy().into_owned());
            config.model_config.num_threads = 2;
            config.feat_config.sample_rate = 8000;
            config.decoding_method = Some("greedy_search".into());
            *guard = OnlineRecognizer::create(&config).map(Arc::new);
        }
        guard.clone()
    }
    fn open(dir: &Path) -> Result<OfflineRecognizer> {
        let f = |n: &str| Some(dir.join(n).to_string_lossy().into_owned());
        let mut c = OfflineRecognizerConfig::default();
        c.model_config.transducer = OfflineTransducerModelConfig {
            encoder: f("encoder.int8.onnx"),
            decoder: f("decoder.onnx"),
            joiner: f("joiner.int8.onnx"),
        };
        c.model_config.tokens = f("tokens.txt");
        c.model_config.num_threads = 2;
        OfflineRecognizer::create(&c)
            .ok_or_else(|| anyhow::anyhow!("Не удалось загрузить русскую STT-модель"))
    }
    pub fn recognize(&self, samples: &[f32]) -> Result<String> {
        ensure!(
            self.offline_available(),
            "Резервная модель Zipformer не установлена"
        );
        let mut guard = self.engine.lock().unwrap();
        if guard.is_none() {
            *guard = Some(Self::open(&self.root)?);
        }
        let r = guard.as_ref().unwrap();
        let s = r.create_stream();
        s.accept_waveform(16000, samples);
        r.decode(&s);
        Ok(s.get_result()
            .map(|r| r.text.trim().to_owned())
            .unwrap_or_default())
    }
    pub fn finalize(&self, samples: &[f32], live: Option<LiveUtterance>) -> Result<String> {
        // The original 16 kHz model can refine words that the 8 kHz stream missed.
        let offline = self.recognize(samples);
        if matches!(&offline, Ok(text) if !text.is_empty()) {
            return offline;
        }
        match live {
            Some(stream) => Ok(stream.finish()),
            None => offline,
        }
    }
}
pub struct LiveUtterance {
    stream: OnlineStream,
    engine: Arc<OnlineRecognizer>,
    text: String,
}
impl LiveUtterance {
    pub fn new(engine: &Arc<OnlineRecognizer>) -> Self {
        Self {
            stream: engine.create_stream(),
            engine: engine.clone(),
            text: String::new(),
        }
    }
    pub fn push(&mut self, samples: &[f32]) -> Option<&str> {
        // sherpa resamples 16 kHz microphone PCM to the T-one input rate.
        self.stream.accept_waveform(16000, samples);
        self.decode()
    }
    fn decode(&mut self) -> Option<&str> {
        if !self.engine.is_ready(&self.stream) {
            return None;
        }
        while self.engine.is_ready(&self.stream) {
            self.engine.decode(&self.stream);
        }
        let result = self.engine.get_result(&self.stream)?;
        let text = result.text.trim();
        if self.text == text {
            return None;
        }
        self.text = text.to_owned();
        Some(&self.text)
    }
    pub fn finish(mut self) -> String {
        // Flush the last acoustic chunk, including a word cut off by the PTT release.
        self.stream.accept_waveform(16000, &[0.0; 24000]);
        self.stream.input_finished();
        self.decode();
        self.text
    }
}
pub fn level(samples: &[f32]) -> f32 {
    (samples.iter().map(|v| v * v).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

#[cfg(test)]
mod acceptance {
    use super::*;
    #[test]
    #[ignore = "Requires the installed Russian streaming model and Jarvis WAVs"]
    fn streaming_russian_before_end_and_between_commands() {
        let root = PathBuf::from(std::env::var_os("JARVIS_STT_DIR").expect("JARVIS_STT_DIR"));
        let recognizer = Recognizer::new(root);
        let engine = recognizer.streaming_engine().unwrap();
        for (name, words) in [("old-long", "диагностика"), ("old-short", "добрый вечер")]
        {
            let wave =
                sherpa_onnx::Wave::read(&format!("../jarvis-tts-rs/examples/audio/{name}.wav"))
                    .unwrap();
            assert_eq!(wave.sample_rate(), 24000);
            let samples: Vec<_> = (0..wave.samples().len() * 2 / 3)
                .map(|i| wave.samples()[i * 3 / 2])
                .collect();
            let mut live = LiveUtterance::new(&engine);
            let mut changes = 0;
            for (i, chunk) in samples.chunks(800).enumerate() {
                if let Some(text) = live.push(chunk) {
                    changes += 1;
                    println!("{} ms audio: {text}", (i + 1) * 50);
                }
            }
            let text = recognizer.finalize(&samples, Some(live)).unwrap();
            println!("Final: {text}");
            if name == "old-long" {
                assert!(changes >= 3);
                assert!(text.ends_with("следующую команду"));
            }
            assert!(text.contains(words));
            if name == "old-short" {
                assert_eq!(text, "добрый вечер сэр");
            }
        }
    }
    #[test]
    #[ignore = "Requires the installed Russian model and real Jarvis WAV"]
    fn original_voice_recognized_in_russian() {
        let root = std::env::var_os("JARVIS_STT_DIR")
            .map(PathBuf::from)
            .expect("JARVIS_STT_DIR");
        let wave =
            sherpa_onnx::Wave::read("../jarvis-tts-rs/examples/audio/old-short.wav").unwrap();
        assert_eq!(wave.sample_rate(), 24000);
        let audio: Vec<f32> = (0..wave.samples().len() * 2 / 3)
            .map(|i| {
                let pos = i * 3 / 2;
                if i % 2 == 0 {
                    wave.samples()[pos]
                } else {
                    (wave.samples()[pos] + wave.samples().get(pos + 1).copied().unwrap_or_default())
                        / 2.
                }
            })
            .collect();
        // This isolated installation has the old files and an incomplete streaming download.
        let fallback =
            std::env::temp_dir().join(format!("jarvis-fallback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(fallback.join("streaming")).unwrap();
        for file in [
            "encoder.int8.onnx",
            "decoder.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ] {
            std::fs::copy(root.join(file), fallback.join(file)).unwrap();
        }
        std::fs::write(fallback.join("streaming/model.onnx"), b"incomplete").unwrap();
        std::fs::write(fallback.join("streaming/tokens.txt"), b"").unwrap();
        let recognizer = Recognizer::new(fallback.clone());
        assert!(recognizer.available());
        assert!(recognizer.streaming_engine().is_none());
        let text = recognizer.finalize(&audio, None).unwrap();
        std::fs::remove_dir_all(fallback).unwrap();
        println!("Русская расшифровка: {text}");
        assert!(text.to_lowercase().contains("добрый вечер"));
    }
}
