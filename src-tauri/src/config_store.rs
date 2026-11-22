//! Где лежит `config.json` и как UI читает/пишет его без утечки ключей.
//!
//! Путь: env `JARVIS_HOME` (папка с `config.json`) → вверх от каталога exe до первой папки,
//! где есть `config.json` → текущий каталог.
//! Ключи провайдеров во фронтенд уходят только замаскированными (`jarvis_core::config::mask_key`);
//! при сохранении маска означает «ключ не менялся».

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use jarvis_core::Config;
use jarvis_core::config::mask_key;
use serde_json::{Map, Value};

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

// ───────────────────────────── трёхстороннее слияние ─────────────────────────────

fn item_id(v: &Value) -> Option<&str> {
    v.get("id").and_then(Value::as_str)
}

/// Массив записей с уникальными `id` (провайдеры, поисковики) — сливается по записям, а не целиком.
fn keyed(items: &[Value]) -> bool {
    let mut seen = std::collections::HashSet::new();
    items.iter().all(|v| item_id(v).is_some_and(|id| seen.insert(id)))
}

fn find<'a>(items: &'a [Value], id: &str) -> Option<&'a Value> {
    items.iter().find(|v| item_id(v) == Some(id))
}

/// Трёхстороннее слияние JSON: `base` — что видел UI при открытии, `ours` — что прислал UI,
/// `theirs` — что сейчас в файле (могли поменять руками, HUD или другое окно).
/// Поле, которое UI не трогал, берётся из файла; поле, которое трогали обе стороны, — из UI.
pub fn merge3(base: &Value, ours: &Value, theirs: &Value) -> Value {
    if ours == base || ours == theirs {
        return theirs.clone();
    }
    if theirs == base {
        return ours.clone();
    }
    match (base, ours, theirs) {
        (Value::Object(b), Value::Object(o), Value::Object(t)) => {
            let mut out = Map::new();
            for k in o.keys().chain(t.keys()) {
                if out.contains_key(k) {
                    continue;
                }
                let get = |m: &Map<String, Value>| m.get(k).cloned().unwrap_or(Value::Null);
                out.insert(k.clone(), merge3(&get(b), &get(o), &get(t)));
            }
            Value::Object(out)
        }
        (Value::Array(b), Value::Array(o), Value::Array(t)) if keyed(b) && keyed(o) && keyed(t) => {
            let mut out = Vec::new();
            for oi in o {
                let id = item_id(oi).unwrap_or_default();
                match (find(b, id), find(t, id)) {
                    (bi, Some(ti)) => out.push(merge3(bi.unwrap_or(&Value::Null), oi, ti)),
                    // удалили снаружи, а в UI запись не меняли — удаление остаётся
                    (Some(bi), None) if bi == oi => {}
                    _ => out.push(oi.clone()),
                }
            }
            // добавили снаружи (UI о них не знал)
            for ti in t {
                let id = item_id(ti).unwrap_or_default();
                if find(o, id).is_none() && find(b, id).is_none() {
                    out.push(ti.clone());
                }
            }
            Value::Array(out)
        }
        // настоящий конфликт — побеждает то, что пользователь только что сохранил
        _ => ours.clone(),
    }
}

/// `merge3` для конфигов (ключи во всех трёх уже настоящие, не маски).
pub fn merge3_config(base: &Config, ours: &Config, theirs: &Config) -> Config {
    let v = |c: &Config| serde_json::to_value(c).unwrap_or(Value::Null);
    serde_json::from_value(merge3(&v(base), &v(ours), &v(theirs))).unwrap_or_else(|_| ours.clone())
}

// ───────────────────────────── хранилище ─────────────────────────────

/// Отпечаток файла: если он изменился, файл правили снаружи (редактор, другой процесс).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp(path: &Path) -> Option<Stamp> {
    std::fs::metadata(path).ok().map(|m| Stamp { modified: m.modified().ok(), len: m.len() })
}

struct Inner {
    cfg: Config,
    /// Отпечаток файла после последнего чтения/записи.
    stamp: Option<Stamp>,
    /// Файл на диске сейчас не разбирается (недописан или испорчен руками).
    broken: bool,
}

/// Конфиг в памяти + файл. Все окна читают/пишут через него (команды `config_get`/`config_set`).
///
/// Правки `config.json` снаружи (редактор, другой процесс) не теряются: перед каждым чтением и записью файл
/// перечитывается, если изменился его отпечаток (время + размер), а `spawn_watcher` применяет такие правки
/// к ядру на лету. Сохранение из UI сливается с файлом трёхсторонне (`merge3`).
pub struct ConfigStore {
    path: PathBuf,
    inner: Mutex<Inner>,
    /// Растёт, когда конфиг в памяти поменялся из-за правки файла снаружи.
    external_rev: AtomicU64,
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
        let inner = Inner { cfg, stamp: stamp(&path), broken: false };
        (Self { path, inner: Mutex::new(inner), external_rev: AtomicU64::new(0) }, err)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Перечитать файл, если его изменили снаружи. `true` — конфиг в памяти изменился.
    fn refresh(&self, g: &mut Inner) -> bool {
        let now = stamp(&self.path);
        if now == g.stamp || now.is_none() {
            return false; // не менялся или удалён (тогда следующая запись создаст его заново)
        }
        g.stamp = now;
        match Config::load(&self.path) {
            Ok(c) => {
                g.broken = false;
                if c == g.cfg {
                    return false;
                }
                g.cfg = c;
                self.external_rev.fetch_add(1, Ordering::AcqRel);
                true
            }
            Err(e) => {
                eprintln!("[jarvis] config.json изменён снаружи, но не читается ({e}) — оставляю прежние настройки");
                g.broken = true;
                false
            }
        }
    }

    fn commit(&self, g: &mut Inner, next: Config) -> Result<Config, String> {
        if g.broken {
            // не затирать молча то, что человек писал руками
            let _ = std::fs::copy(&self.path, self.path.with_extension("json.broken"));
            g.broken = false;
        }
        next.save(&self.path).map_err(|e| e.to_string())?;
        g.stamp = stamp(&self.path);
        g.cfg = next.clone();
        Ok(next)
    }

    /// Текущий конфиг (с правками файла снаружи, если они были).
    pub fn get(&self) -> Config {
        let mut g = self.lock();
        self.refresh(&mut g);
        g.cfg.clone()
    }

    /// Проверить файл на правки снаружи (поток `spawn_watcher`).
    pub fn reload_if_changed(&self) -> bool {
        let mut g = self.lock();
        self.refresh(&mut g)
    }

    /// Сколько раз конфиг в памяти менялся из-за правок файла снаружи.
    pub fn external_revision(&self) -> u64 {
        self.external_rev.load(Ordering::Acquire)
    }

    /// Прочитать свежий файл → изменить → сохранить (read-modify-write). Возвращает новый конфиг.
    pub fn update(&self, f: impl FnOnce(&mut Config)) -> Result<Config, String> {
        let mut g = self.lock();
        self.refresh(&mut g);
        let mut next = g.cfg.clone();
        f(&mut next);
        self.commit(&mut g, next)
    }

    /// Сохранить конфиг, пришедший из UI (с масками вместо ключей). `base` — конфиг, который UI получил
    /// при открытии (тоже с масками): поля, которые пользователь не трогал, берутся из файла.
    pub fn set_from_ui(&self, incoming: Config, base: Option<Config>) -> Result<Config, String> {
        self.set_from_ui_with(incoming, base, |_| Ok(()))
    }

    /// `set_from_ui` + доработка результата перед записью (например, «Выбрать лучшее автоматически»).
    pub fn set_from_ui_with(
        &self,
        incoming: Config,
        base: Option<Config>,
        edit: impl FnOnce(&mut Config) -> Result<(), String>,
    ) -> Result<Config, String> {
        let mut g = self.lock();
        self.refresh(&mut g);
        let theirs = g.cfg.clone();
        let ours = merge_from_ui(&theirs, incoming);
        let mut next = match base {
            Some(b) => merge3_config(&merge_from_ui(&theirs, b), &ours, &theirs),
            None => ours,
        };
        edit(&mut next)?;
        self.commit(&mut g, next)
    }
}

/// Событие «config.json изменили снаружи» (payload — замаскированный конфиг): экран настроек обновляет черновик.
pub const CONFIG_CHANGED_EVENT: &str = "jarvis://config_changed";

/// Раз в секунду проверять `config.json`: правки снаружи применяются к ядру без перезапуска
/// (модели, провайдеры, голос, поиск…) и отправляются окнам событием `CONFIG_CHANGED_EVENT`.
pub fn spawn_watcher(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    let _ = std::thread::Builder::new().name("jarvis-config-watch".into()).spawn(move || {
        let mut seen = app.state::<ConfigStore>().external_revision();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let store = app.state::<ConfigStore>();
            store.reload_if_changed();
            let rev = store.external_revision();
            if rev == seen {
                continue;
            }
            seen = rev;
            let cfg = store.get();
            eprintln!("[jarvis] config.json изменён снаружи — применяю");
            app.state::<crate::commands::CoreState>().0.apply_config(&cfg);
            let _ = app.emit(CONFIG_CHANGED_EVENT, masked(&cfg));
        }
    });
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

    /// Записать файл «руками» так, чтобы отпечаток гарантированно сменился (другой размер).
    fn edit_on_disk(path: &Path, f: impl FnOnce(&mut Config)) {
        let mut c = Config::load(path).unwrap();
        f(&mut c);
        let mut s = serde_json::to_string_pretty(&c).unwrap();
        let old_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) as usize;
        while s.len() == old_len || s.len() == old_len + 1 {
            s.push(' ');
        }
        std::fs::write(path, s).unwrap();
    }

    /// Регрессия 2026-11-22: groq включили в config.json руками, пока приложение работало, а запись позиции
    /// виджета/режима HUD при выходе вернула старую копию из памяти (enabled=false).
    #[test]
    fn external_edit_survives_internal_write() {
        let root = tmp("external");
        let path = root.join("config.json");
        let mut start = cfg_with_keys();
        start.providers[0].enabled = false;
        start.save(&path).unwrap();
        let (store, _) = ConfigStore::open(path.clone());
        let rev0 = store.external_revision();

        edit_on_disk(&path, |c| c.providers[0].enabled = true);
        store.update(|c| c.hud.corner_x = Some(7)).unwrap();
        let disk = Config::load(&path).unwrap();
        assert!(disk.providers[0].enabled, "правка снаружи не затёрта");
        assert_eq!(disk.hud.corner_x, Some(7));
        assert!(store.external_revision() > rev0, "наблюдатель узнает о правке и применит её к ядру");

        // свои записи не считаются правкой снаружи
        let rev1 = store.external_revision();
        assert!(!store.reload_if_changed());
        assert_eq!(store.external_revision(), rev1);

        edit_on_disk(&path, |c| c.autonomy = jarvis_core::Autonomy::Never);
        assert!(store.reload_if_changed());
        assert_eq!(store.get().autonomy, jarvis_core::Autonomy::Never);
        std::fs::remove_dir_all(&root).ok();
    }

    /// Экран настроек открыт со старым конфигом; файл поменяли снаружи; пользователь сохранил другое поле.
    #[test]
    fn ui_save_merges_with_external_edit() {
        let root = tmp("ui-merge");
        let path = root.join("config.json");
        let mut start = cfg_with_keys();
        start.providers.push(ProviderConfig { id: "zen".into(), api_key: "zen_key_0000000000001".into(), enabled: true, ..Default::default() });
        start.providers[0].enabled = false;
        start.save(&path).unwrap();
        let (store, _) = ConfigStore::open(path.clone());
        let base = masked(&store.get()); // то, что увидел UI

        edit_on_disk(&path, |c| {
            c.providers[0].enabled = true; // включили groq руками
            c.hud.mode = jarvis_core::config::HudMode::Fullscreen;
        });
        let mut draft = base.clone();
        draft.vision_enabled = false; // пользователь поменял только это
        draft.providers[1].base_url = "https://zen.example/v1".into();
        let saved = store.set_from_ui(draft, Some(base)).unwrap();
        assert!(saved.providers[0].enabled, "UI не трогал groq — берётся из файла");
        assert_eq!(saved.providers[0].api_key, "gsk_abcdefghijklmnop1234", "ключи не теряются");
        assert_eq!(saved.providers[1].api_key, "zen_key_0000000000001");
        assert_eq!(saved.providers[1].base_url, "https://zen.example/v1");
        assert!(!saved.vision_enabled);
        assert_eq!(saved.hud.mode, jarvis_core::config::HudMode::Fullscreen);
        assert_eq!(Config::load(&path).unwrap(), saved);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn merge3_rules() {
        use serde_json::json;
        let base = json!({"a": 1, "b": 1, "list": [{"id": "x", "v": 1}, {"id": "y", "v": 1}], "roles": ["m1"]});
        let ours = json!({"a": 2, "b": 1, "list": [{"id": "x", "v": 2}, {"id": "y", "v": 1}, {"id": "new", "v": 0}], "roles": ["m1"]});
        let theirs = json!({"a": 3, "b": 5, "list": [{"id": "x", "v": 1}, {"id": "z", "v": 9}], "roles": ["m2"]});
        let m = merge3(&base, &ours, &theirs);
        assert_eq!(m["a"], 2, "конфликт — побеждает UI");
        assert_eq!(m["b"], 5, "UI не трогал — из файла");
        assert_eq!(m["roles"], json!(["m2"]));
        assert_eq!(
            m["list"],
            json!([{"id": "x", "v": 2}, {"id": "new", "v": 0}, {"id": "z", "v": 9}]),
            "y удалили снаружи (UI не менял), new добавил UI, z добавили снаружи"
        );
    }
}
