use anyhow::{bail, ensure, Result};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

#[cfg(target_os = "macos")]
mod device {
    unsafe extern "C" {
        pub fn jv_mic_start() -> i32;
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
pub fn mic_start() -> Result<()> {
    match unsafe { device::jv_mic_start() } {
        1 => Ok(()),
        -1 => bail!("Разрешите доступ к микрофону в настройках конфиденциальности macOS"),
        -2 => bail!("Истекло время ожидания разрешения микрофона"),
        _ => bail!("Не удалось открыть микрофон с эхоподавлением. Проверьте устройство ввода."),
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
    pub fn mic_start() -> Result<()> {
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
}
impl Recognizer {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            engine: Mutex::new(None),
        }
    }
    pub fn available(&self) -> bool {
        [
            "encoder.int8.onnx",
            "decoder.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ]
        .iter()
        .all(|s| self.root.join(s).is_file())
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
        ensure!(self.available(), "Установите русскую модель STT");
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
pub fn level(samples: &[f32]) -> f32 {
    (samples.iter().map(|v| v * v).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}
