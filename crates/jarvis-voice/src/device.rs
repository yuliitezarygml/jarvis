//! Выбор аудиоустройств cpal (WASAPI на Windows) по имени.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::error::{Result, VoiceError};

pub fn device_name(d: &cpal::Device) -> String {
    d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| d.to_string())
}

/// Имена устройств вывода.
pub fn output_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devs = host.output_devices().map_err(|e| VoiceError::Device(e.to_string()))?;
    Ok(devs.map(|d| device_name(&d)).collect())
}

/// Имена устройств ввода (микрофонов).
pub fn input_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devs = host.input_devices().map_err(|e| VoiceError::Device(e.to_string()))?;
    Ok(devs.map(|d| device_name(&d)).collect())
}

/// Индекс лучшего совпадения: точное имя (без регистра), иначе подстрока.
pub fn pick_by_name(names: &[String], wanted: &str) -> Option<usize> {
    let w = wanted.trim().to_lowercase();
    if w.is_empty() {
        return None;
    }
    names
        .iter()
        .position(|n| n.to_lowercase() == w)
        .or_else(|| names.iter().position(|n| n.to_lowercase().contains(&w)))
}

/// Предупреждения для HUD: выбранный в настройках микрофон/динамики сейчас не подключены
/// (тогда `Capture`/`Player` берут устройство Windows по умолчанию). Устройства не открываются.
pub fn missing_device_warnings(audio: &jarvis_core::config::AudioConfig) -> Vec<String> {
    missing_warnings(audio, input_devices().ok().as_deref(), output_devices().ok().as_deref())
}

/// Чистая часть `missing_device_warnings` (список устройств не удалось получить — `None`, молчим).
pub fn missing_warnings(
    audio: &jarvis_core::config::AudioConfig,
    inputs: Option<&[String]>,
    outputs: Option<&[String]>,
) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(w), Some(list)) = (audio.input(), inputs)
        && pick_by_name(list, w).is_none()
    {
        out.push(format!("Микрофон «{w}» не найден — слушаю микрофон Windows по умолчанию. Выберите другой в Настройках → «Аудио»."));
    }
    if let (Some(w), Some(list)) = (audio.output(), outputs)
        && pick_by_name(list, w).is_none()
    {
        out.push(format!("Устройство вывода «{w}» не найдено — говорю через динамики Windows по умолчанию. Выберите другое в Настройках → «Аудио»."));
    }
    out
}

pub(crate) fn find_output(wanted: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(w) = wanted.filter(|w| !w.trim().is_empty()) {
        let devs: Vec<cpal::Device> = host.output_devices().map_err(|e| VoiceError::Device(e.to_string()))?.collect();
        let names: Vec<String> = devs.iter().map(device_name).collect();
        if let Some(i) = pick_by_name(&names, w) {
            return Ok(devs.into_iter().nth(i).unwrap());
        }
        tracing::warn!(wanted = w, "устройство вывода не найдено, беру по умолчанию");
    }
    host.default_output_device().ok_or_else(|| VoiceError::Device("нет устройства вывода звука".into()))
}

pub(crate) fn find_input(wanted: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(w) = wanted.filter(|w| !w.trim().is_empty()) {
        let devs: Vec<cpal::Device> = host.input_devices().map_err(|e| VoiceError::Device(e.to_string()))?.collect();
        let names: Vec<String> = devs.iter().map(device_name).collect();
        if let Some(i) = pick_by_name(&names, w) {
            return Ok(devs.into_iter().nth(i).unwrap());
        }
        tracing::warn!(wanted = w, "микрофон не найден, беру по умолчанию");
    }
    host.default_input_device().ok_or_else(|| VoiceError::Device("нет микрофона".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick() {
        let n: Vec<String> = ["Динамики (Realtek)", "Наушники (USB Audio)", "USB"].map(String::from).to_vec();
        assert_eq!(pick_by_name(&n, "usb"), Some(2), "точное совпадение важнее подстроки");
        assert_eq!(pick_by_name(&n, "наушники"), Some(1));
        assert_eq!(pick_by_name(&n, "HDMI"), None);
        assert_eq!(pick_by_name(&n, " "), None);
    }

    #[test]
    fn warnings_only_for_missing_selected_devices() {
        use jarvis_core::config::AudioConfig;
        let ins: Vec<String> = vec!["Микрофон (USB Audio)".into()];
        let outs: Vec<String> = vec!["Динамики (Realtek)".into()];
        let mut a = AudioConfig::default();
        assert!(missing_warnings(&a, Some(&ins), Some(&outs)).is_empty(), "по умолчанию — без предупреждений");
        a.input_device = Some("Микрофон (USB Audio)".into());
        a.output_device = Some("Наушники (Bluetooth)".into());
        let w = missing_warnings(&a, Some(&ins), Some(&outs));
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("Наушники (Bluetooth)"), "{w:?}");
        assert!(missing_warnings(&a, None, None).is_empty(), "список недоступен — молчим");
    }
}
