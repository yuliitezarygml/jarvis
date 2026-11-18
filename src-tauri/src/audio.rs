use anyhow::{bail, ensure, Result};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineStream,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
pub fn mic_start(continuous: bool) -> Result<()> {
    match unsafe { device::jv_mic_start(i32::from(continuous)) } {
        1 => Ok(()),
        -1 => bail!("Разрешите доступ к микрофону в настройках конфиденциальности macOS"),
        -2 => bail!("Истекло время ожидания разрешения микрофона"),
        _ => bail!("Не удалось открыть устройство ввода. Проверьте выбранный микрофон macOS."),
    }
}
#[cfg(target_os = "macos")]
pub fn mic_status() -> i32 {
    unsafe { device::jv_mic_status() }
}
#[cfg(target_os = "macos")]
pub fn mic_end() {
    unsafe { device::jv_mic_end() }
}
#[cfg(target_os = "macos")]
pub fn read(buf: &mut [f32]) -> usize {
    unsafe { device::jv_read(buf.as_mut_ptr(), buf.len() as i32) as usize }
}
#[cfg(target_os = "macos")]
pub fn play(buf: &[i16]) -> Result<bool> {
    match unsafe { device::jv_play(buf.as_ptr(), buf.len() as i32) } {
        1 => Ok(true),
        0 => Ok(false),
        _ => bail!("Выход звука недоступен"),
    }
}
#[cfg(target_os = "macos")]
pub fn stop() {
    unsafe { device::jv_stop() }
}
#[cfg(target_os = "macos")]
pub fn pending() -> i32 {
    unsafe { device::jv_pending() }
}
#[cfg(target_os = "macos")]
pub fn gpu() -> String {
    let mut b = [0i8; 160];
    unsafe {
        device::jv_gpu(b.as_mut_ptr(), 160);
        std::ffi::CStr::from_ptr(b.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}
#[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
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
    fn offline_available(&self) -> bool {
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
            let mut config = OnlineRecognizerConfig::default();
            config.model_config.t_one_ctc.model = Some(dir.join("model.onnx").to_string_lossy().into_owned());
            config.model_config.tokens = Some(dir.join("tokens.txt").to_string_lossy().into_owned());
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
        ensure!(self.offline_available(), "Резервная модель Zipformer не установлена");
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
}
pub struct LiveUtterance {
    stream: OnlineStream,
    engine: Arc<OnlineRecognizer>,
    text: String,
}
impl LiveUtterance {
    pub fn new(engine: &Arc<OnlineRecognizer>) -> Self {
        Self { stream: engine.create_stream(), engine: engine.clone(), text: String::new() }
    }
    pub fn push(&mut self, samples: &[f32]) -> Option<&str> {
        // sherpa resamples 16 kHz microphone PCM to the T-one input rate.
        self.stream.accept_waveform(16000, samples);
        self.decode()
    }
    fn decode(&mut self) -> Option<&str> {
        if !self.engine.is_ready(&self.stream) { return None; }
        while self.engine.is_ready(&self.stream) { self.engine.decode(&self.stream); }
        let result = self.engine.get_result(&self.stream)?;
        let text = result.text.trim();
        if self.text == text { return None; }
        self.text = text.to_owned();
        Some(&self.text)
    }
    pub fn finish(mut self) -> String {
        // Flush the last acoustic chunk, including a word cut off by the PTT release.
        self.stream.accept_waveform(16000, &[0.0; 9600]);
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
        let text = Recognizer::new(root).recognize(&audio).unwrap();
        println!("Русская расшифровка: {text}");
        assert!(text.to_lowercase().contains("добрый вечер"));
    }
}
