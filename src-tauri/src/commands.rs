use serde::Serialize;

#[derive(Serialize)]
pub struct AppInfo {
    name: &'static str,
    version: &'static str,
    os: &'static str,
    arch: &'static str,
}

#[tauri::command]
pub fn get_app_info() -> AppInfo {
    AppInfo {
        name: "Jarvis",
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

#[tauri::command]
pub fn greet(name: String) -> Result<String, String> {
    let name = name.trim();

    if name.is_empty() {
        return Err("Введи имя, чтобы продолжить.".into());
    }

    if name.chars().count() > 64 {
        return Err("Имя должно содержать не больше 64 символов.".into());
    }

    Ok(format!("Привет, {name}! Rust на связи. Можно начинать."))
}

#[cfg(test)]
mod tests {
    use super::greet;

    #[test]
    fn rejects_blank_names() {
        assert!(greet(" \n\t ".into()).is_err());
    }

    #[test]
    fn validates_unicode_names_by_characters() {
        assert!(greet("Я".repeat(64)).is_ok());
        assert!(greet("Я".repeat(65)).is_err());
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(greet("  Илья  ".into()), greet("Илья".into()));
    }
}
