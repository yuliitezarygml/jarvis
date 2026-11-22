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
}
