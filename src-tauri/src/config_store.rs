//! Где лежит `config.json` и как UI читает/пишет его без утечки ключей.
//!
//! Путь: env `JARVIS_HOME` (папка с `config.json`) → вверх от каталога exe до первой папки,
//! где есть `config.json` → текущий каталог.
//! Ключи провайдеров во фронтенд уходят только замаскированными (`jarvis_core::config::mask_key`);
//! при сохранении маска означает «ключ не менялся».

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use jarvis_core::Config;
use jarvis_core::config::mask_key;

pub const CONFIG_FILE: &str = "config.json";
pub const HOME_ENV: &str = "JARVIS_HOME";

/// Чистая функция выбора пути (для тестов): `home` — значение `JARVIS_HOME`, `exe` — путь к exe.
pub fn resolve_config_path(home: Option<&Path>, exe: Option<&Path>, cwd: &Path) -> PathBuf {
    if let Some(h) = home.filter(|h| !h.as_os_str().is_empty()) {
        return h.join(CONFIG_FILE);
    }
    if let Some(dir) = exe.and_then(Path::parent) {
        for d in dir.ancestors() {
            let candidate = d.join(CONFIG_FILE);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    cwd.join(CONFIG_FILE)
}

/// Путь к `config.json` для текущего процесса.
pub fn config_path() -> PathBuf {
    let home = std::env::var_os(HOME_ENV).map(PathBuf::from);
    let exe = std::env::current_exe().ok();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_config_path(home.as_deref(), exe.as_deref(), &cwd)
}

/// Корень проекта (папка, где лежит `config.json`) — относительно него резолвятся `data_dir`, `ca_path`.
pub fn project_root(config_path: &Path) -> PathBuf {
    config_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn mask(key: &str) -> String {
    if key.is_empty() { String::new() } else { mask_key(key) }
}

/// Похоже ли значение на маску (а не на настоящий ключ).
fn looks_masked(s: &str) -> bool {
    !s.is_empty() && (s.contains('…') || s.chars().all(|c| c == '•'))
}

/// Копия конфига для UI: все секреты замаскированы.
pub fn masked(cfg: &Config) -> Config {
    let mut c = cfg.clone();
    for p in &mut c.providers {
        p.api_key = mask(&p.api_key);
    }
    for s in &mut c.web_search {
        s.api_key = mask(&s.api_key);
    }
    c
}

/// Ключ, присланный из UI: маска текущего ключа (или любая маска) → оставить сохранённый; иначе — новое значение.
fn merge_key(incoming: &str, existing: Option<&str>) -> String {
    match existing {
        Some(old) if incoming == mask(old) => old.to_string(),
        Some(old) if looks_masked(incoming) => old.to_string(),
        None if looks_masked(incoming) => String::new(),
        _ => incoming.trim().to_string(),
    }
}

/// Слить конфиг из UI с сохранённым: настоящие ключи не теряются, если UI прислал маски.
pub fn merge_from_ui(stored: &Config, mut incoming: Config) -> Config {
    for p in &mut incoming.providers {
        let old = stored.providers.iter().find(|o| o.id == p.id).map(|o| o.api_key.as_str());
        p.api_key = merge_key(&p.api_key, old);
    }
    for s in &mut incoming.web_search {
        let old = stored.web_search.iter().find(|o| o.id == s.id).map(|o| o.api_key.as_str());
        s.api_key = merge_key(&s.api_key, old);
    }
    incoming
}

/// Ключ для проверки провайдера: если UI прислал маску — берём сохранённый.
pub fn resolve_key(stored: &Config, provider_id: &str, key_from_ui: &str) -> String {
    let old = stored.providers.iter().find(|p| p.id == provider_id).map(|p| p.api_key.as_str());
    merge_key(key_from_ui, old)
}

pub fn resolve_search_key(stored: &Config, search_id: &str, key_from_ui: &str) -> String {
    let old = stored.web_search.iter().find(|s| s.id == search_id).map(|s| s.api_key.as_str());
    merge_key(key_from_ui, old)
}

/// Конфиг в памяти + файл. Все окна читают/пишут через него (команды `config_get`/`config_set`).
pub struct ConfigStore {
    path: PathBuf,
    cfg: Mutex<Config>,
}

impl ConfigStore {
    /// Битый `config.json` не роняет приложение: берём умолчания, а битый файл копируем
    /// в `config.json.broken` (чтобы следующее сохранение не уничтожило ключи пользователя).
    pub fn open(path: PathBuf) -> (Self, Option<String>) {
        let (cfg, err) = match Config::load(&path) {
            Ok(c) => (c, None),
            Err(e) => {
                let backup = path.with_extension("json.broken");
                let _ = std::fs::copy(&path, &backup);
                (Config::default(), Some(format!("{e}; копия: {}", backup.display())))
            }
        };
        (Self { path, cfg: Mutex::new(cfg) }, err)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> Config {
        self.cfg.lock().unwrap().clone()
    }

    /// Изменить и сохранить на диск. Возвращает новый конфиг.
    pub fn update(&self, f: impl FnOnce(&mut Config)) -> Result<Config, String> {
        let mut guard = self.cfg.lock().unwrap();
        let mut next = guard.clone();
        f(&mut next);
        next.save(&self.path).map_err(|e| e.to_string())?;
        *guard = next.clone();
        Ok(next)
    }

    /// Сохранить конфиг, пришедший из UI (с масками вместо ключей).
    pub fn set_from_ui(&self, incoming: Config) -> Result<Config, String> {
        let stored = self.get();
        let merged = merge_from_ui(&stored, incoming);
        self.update(|c| *c = merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::config::{ProviderConfig, SearchProviderConfig};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jarvis-tauri-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn env_home_wins() {
        let root = tmp("env");
        let p = resolve_config_path(Some(&root), Some(&root.join("x/y/app.exe")), Path::new("C:/cwd"));
        assert_eq!(p, root.join("config.json"));
    }

    #[test]
    fn empty_env_is_ignored() {
        let root = tmp("emptyenv");
        let p = resolve_config_path(Some(Path::new("")), None, &root);
        assert_eq!(p, root.join("config.json"));
    }

    #[test]
    fn walks_up_from_exe() {
        let root = tmp("walk");
        let exe_dir = root.join("src-tauri/target/debug");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::write(root.join("config.json"), "{}").unwrap();
        let p = resolve_config_path(None, Some(&exe_dir.join("jarvis.exe")), Path::new("C:/elsewhere"));
        assert_eq!(p, root.join("config.json"));
        // ближайший к exe config.json важнее
        std::fs::write(root.join("src-tauri/config.json"), "{}").unwrap();
        let p = resolve_config_path(None, Some(&exe_dir.join("jarvis.exe")), Path::new("C:/elsewhere"));
        assert_eq!(p, root.join("src-tauri/config.json"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn falls_back_to_cwd() {
        let root = tmp("cwd");
        let exe_dir = root.join("bin");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let cwd = root.join("work");
        // нигде нет config.json (temp_dir наверху его тоже не содержит)
        let p = resolve_config_path(None, Some(&exe_dir.join("j.exe")), &cwd);
        assert!(p == cwd.join("config.json") || p.is_file(), "{p:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    fn cfg_with_keys() -> Config {
        let mut c = Config::default();
        c.providers = vec![ProviderConfig {
            id: "groq".into(),
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key: "gsk_abcdefghijklmnop1234".into(),
            enabled: true,
            ..Default::default()
        }];
        c.web_search = vec![SearchProviderConfig {
            id: "brave".into(),
            api_key: "BSAxxxxxxxxxxxxx9999".into(),
            enabled: true,
            ..Default::default()
        }];
        c
    }

    #[test]
    fn masked_hides_secrets() {
        let m = masked(&cfg_with_keys());
        let j = serde_json::to_string(&m).unwrap();
        assert!(!j.contains("abcdefghijklmnop"));
        assert!(!j.contains("xxxxxxxx"));
        assert_eq!(m.providers[0].api_key, "gsk…1234");
        assert_eq!(masked(&Config::default()).providers[0].api_key, "");
    }

    #[test]
    fn merge_keeps_secret_when_mask_comes_back() {
        let stored = cfg_with_keys();
        let ui = masked(&stored);
        let merged = merge_from_ui(&stored, ui);
        assert_eq!(merged, stored);
    }

    #[test]
    fn merge_accepts_new_and_cleared_keys() {
        let stored = cfg_with_keys();
        let mut ui = masked(&stored);
        ui.providers[0].api_key = "  gsk_NEWKEY000000000000  ".into();
        ui.web_search[0].api_key = String::new();
        ui.providers.push(ProviderConfig { id: "zen".into(), api_key: "zen…dead".into(), ..Default::default() });
        let merged = merge_from_ui(&stored, ui);
        assert_eq!(merged.providers[0].api_key, "gsk_NEWKEY000000000000");
        assert_eq!(merged.web_search[0].api_key, "");
        assert_eq!(merged.providers[1].api_key, "", "маска без сохранённого ключа не становится ключом");
    }

    #[test]
    fn resolve_key_for_checks() {
        let stored = cfg_with_keys();
        assert_eq!(resolve_key(&stored, "groq", "gsk…1234"), "gsk_abcdefghijklmnop1234");
        assert_eq!(resolve_key(&stored, "groq", "gsk_other"), "gsk_other");
        assert_eq!(resolve_search_key(&stored, "brave", &mask_key("BSAxxxxxxxxxxxxx9999")), "BSAxxxxxxxxxxxxx9999");
    }

    #[test]
    fn store_roundtrip_on_disk() {
        let root = tmp("store");
        let path = root.join("config.json");
        let (store, err) = ConfigStore::open(path.clone());
        assert!(err.is_none());
        store.update(|c| c.hud.corner_x = Some(42)).unwrap();
        let again = Config::load(&path).unwrap();
        assert_eq!(again.hud.corner_x, Some(42));

        std::fs::write(&path, "{ broken").unwrap();
        let (store2, err2) = ConfigStore::open(path);
        assert!(err2.is_some());
        assert_eq!(store2.get(), Config::default());
        std::fs::remove_dir_all(&root).ok();
    }
}
