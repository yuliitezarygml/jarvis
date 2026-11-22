//! Поиск и открытие (план, раздел 3а): `open_target` разрешает имя по порядку — открытые окна → «Пуск»
//! (`Get-StartApps`) → App Paths → PATH → известные папки → `ms-settings:` → поиск файла. Запуск —
//! через внедряемый [`Opener`] (по умолчанию `ShellExecuteW`), поэтому разрешение тестируется без запуска.

pub mod catalog;
mod tools;

pub use tools::{ClipboardGet, ClipboardSet, OpenTarget, RevealInExplorer, UrlOpen};

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::util::norm_text;
use crate::win::{self, KnownFolder, WindowInfo};

/// Что открыть.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenRequest {
    /// Файл/папка/программа — ShellExecute «open».
    Path { path: PathBuf },
    /// URL, `ms-settings:…`, `shell:…`.
    Uri { uri: String },
    /// Приложение «Пуска» по AppUserModelID (`shell:AppsFolder\…`).
    App { app_id: String },
    /// Переключиться на открытое окно.
    Focus { hwnd: isize },
    /// Показать файл в Проводнике (выделить).
    Reveal { path: PathBuf },
}

pub trait Opener: Send + Sync {
    fn open(&self, req: &OpenRequest) -> io::Result<()>;
}

/// Боевой запуск через `ShellExecuteW`.
pub struct ShellOpener;

impl Opener for ShellOpener {
    fn open(&self, req: &OpenRequest) -> io::Result<()> {
        match req {
            OpenRequest::Path { path } => win::shell_execute(&path.to_string_lossy(), None, "open"),
            OpenRequest::Uri { uri } => win::shell_execute(uri, None, "open"),
            OpenRequest::App { app_id } => win::shell_execute(&format!("shell:AppsFolder\\{app_id}"), None, "open"),
            OpenRequest::Focus { hwnd } => {
                if win::focus_window(*hwnd) {
                    Ok(())
                } else {
                    Err(io::Error::other("не удалось переключиться на окно"))
                }
            }
            OpenRequest::Reveal { path } => win::shell_execute("explorer.exe", Some(&format!("/select,\"{}\"", path.display())), "open"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct StartApp {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "AppID")]
    pub app_id: String,
}

/// Источники кандидатов (в тестах — подставные).
pub trait TargetSources: Send + Sync {
    fn windows(&self) -> Vec<WindowInfo>;
    fn start_apps(&self) -> Vec<StartApp>;
    /// (имя exe без расширения, полный путь) из реестра App Paths.
    fn app_paths(&self) -> Vec<(String, PathBuf)>;
    /// Найти `name(.exe|.com|.msc)` в PATH.
    fn path_exe(&self, name: &str) -> Option<PathBuf>;
    fn known_folder(&self, f: KnownFolder) -> Option<PathBuf>;
    /// Поиск файлов по имени (последний шаг).
    fn find_files(&self, query: &str) -> Vec<PathBuf>;
}

/// Реальные источники: EnumWindows, `Get-StartApps` (кэш 10 мин), реестр, PATH, SHGetKnownFolderPath, file_search.
#[derive(Default)]
pub struct SystemSources {
    start_cache: Mutex<Option<(Instant, Vec<StartApp>)>>,
    pub search_roots: Vec<PathBuf>,
}

impl SystemSources {
    pub fn with_roots(roots: Vec<PathBuf>) -> Self {
        Self { start_cache: Mutex::new(None), search_roots: roots }
    }
}

fn load_start_apps() -> Vec<StartApp> {
    use std::os::windows::process::CommandExt;
    let script = "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); Get-StartApps | Select-Object Name,AppID | ConvertTo-Json -Compress";
    let Ok(out) = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let t = text.trim();
    if t.starts_with('[') {
        serde_json::from_str(t).unwrap_or_default()
    } else {
        serde_json::from_str::<StartApp>(t).map(|a| vec![a]).unwrap_or_default()
    }
}

impl TargetSources for SystemSources {
    fn windows(&self) -> Vec<WindowInfo> {
        win::enum_windows()
    }

    fn start_apps(&self) -> Vec<StartApp> {
        let mut g = self.start_cache.lock().unwrap();
        if let Some((t, v)) = g.as_ref()
            && t.elapsed() < Duration::from_secs(600)
        {
            return v.clone();
        }
        let v = load_start_apps();
        *g = Some((Instant::now(), v.clone()));
        v
    }

    fn app_paths(&self) -> Vec<(String, PathBuf)> {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        let mut out = Vec::new();
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let Ok(k) = RegKey::predef(hive).open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths") else { continue };
            for name in k.enum_keys().flatten() {
                let Ok(sub) = k.open_subkey(&name) else { continue };
                let Ok(path) = sub.get_value::<String, _>("") else { continue };
                let stem = name.trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
                out.push((stem, PathBuf::from(path.trim_matches('"'))));
            }
        }
        out
    }

    fn path_exe(&self, name: &str) -> Option<PathBuf> {
        if name.contains(['\\', '/', ' ']) || name.is_empty() {
            return None;
        }
        let path = std::env::var_os("PATH")?;
        // «services.msc» ищем как есть, «notepad» — с расширениями
        let exts: &[&str] = if name.contains('.') { &[""] } else { &[".exe", ".com", ".msc"] };
        for dir in std::env::split_paths(&path) {
            for ext in exts {
                let p = dir.join(format!("{name}{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }

    fn known_folder(&self, f: KnownFolder) -> Option<PathBuf> {
        win::known_folder(f)
    }

    fn find_files(&self, query: &str) -> Vec<PathBuf> {
        let q = crate::search::SearchQuery { name: query.to_string(), limit: 10, budget: Duration::from_secs(2), ..Default::default() };
        crate::search::search(&self.search_roots, &q).hits.into_iter().map(|h| h.path).collect()
    }
}

// ───────────────────────────── нечёткое сравнение ─────────────────────────────

/// Транслитерация кириллицы для сравнения «телеграм» ↔ «telegram».
pub fn translit(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        let r = match c {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' | 'ё' | 'э' => "e", 'ж' => "zh", 'з' => "z",
            'и' | 'й' | 'ы' => "i", 'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o", 'п' => "p", 'р' => "r",
            'с' => "s", 'т' => "t", 'у' => "u", 'ф' => "f", 'х' => "h", 'ц' => "c", 'ч' => "ch", 'ш' => "sh", 'щ' => "sch",
            'ъ' | 'ь' => "", 'ю' => "yu", 'я' => "ya",
            other => {
                o.push(other);
                continue;
            }
        };
        o.push_str(r);
    }
    o
}

fn score(q: &str, t: &str) -> f32 {
    if q == t {
        return 1.0;
    }
    let tw: Vec<&str> = t.split(' ').collect();
    let qw: Vec<&str> = q.split(' ').collect();
    if t.starts_with(q) && t[q.len()..].starts_with(' ') {
        return 0.92;
    }
    if tw.contains(&q) {
        return 0.88;
    }
    if qw.len() > 1 && qw.iter().all(|w| tw.contains(w)) {
        return 0.86;
    }
    if q.chars().count() >= 4 && t.starts_with(q) {
        return 0.82;
    }
    if q.chars().count() >= 4 && t.contains(q) {
        return 0.75;
    }
    let jw = strsim::jaro_winkler(q, t) as f32;
    let word_jw = if qw.len() == 1 { tw.iter().map(|w| strsim::jaro_winkler(q, w) as f32).fold(0.0, f32::max) * 0.95 } else { 0.0 };
    let best = jw.max(word_jw);
    if best >= 0.86 { best * 0.9 } else { 0.0 }
}

/// Похожесть запроса на название, 0…1.
pub fn sim(query: &str, target: &str) -> f32 {
    let q = norm_text(query);
    let t = norm_text(target);
    if q.is_empty() || t.is_empty() {
        return 0.0;
    }
    score(&q, &t).max(score(&translit(&q), &translit(&t)) * 0.97)
}

// ───────────────────────────── разрешение ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetKind {
    Window { hwnd: isize, pid: u32, exe: String },
    StartApp { app_id: String },
    AppPath { exe: PathBuf },
    PathExe { exe: PathBuf },
    Folder { path: PathBuf },
    Settings { uri: String },
    Uri { uri: String },
    File { path: PathBuf },
}

impl TargetKind {
    pub fn request(&self) -> OpenRequest {
        match self {
            Self::Window { hwnd, .. } => OpenRequest::Focus { hwnd: *hwnd },
            Self::StartApp { app_id } => OpenRequest::App { app_id: app_id.clone() },
            Self::AppPath { exe } | Self::PathExe { exe } => OpenRequest::Path { path: exe.clone() },
            Self::Folder { path } | Self::File { path } => OpenRequest::Path { path: path.clone() },
            Self::Settings { uri } | Self::Uri { uri } => OpenRequest::Uri { uri: uri.clone() },
        }
    }

    /// Ключ для устранения дублей (одна и та же программа из разных источников).
    fn key(&self) -> String {
        match self {
            Self::Window { hwnd, .. } => format!("w{hwnd}"),
            Self::StartApp { app_id } => format!("a{}", app_id.to_lowercase()),
            Self::AppPath { exe } | Self::PathExe { exe } => format!("e{}", exe.to_string_lossy().to_lowercase()),
            Self::Folder { path } | Self::File { path } => format!("p{}", path.to_string_lossy().to_lowercase()),
            Self::Settings { uri } | Self::Uri { uri } => format!("u{uri}"),
        }
    }

    /// Исполняемый файл, найденный поиском, — запуск требует подтверждения.
    pub fn is_executable_file(&self) -> bool {
        match self {
            Self::File { path } => {
                let e = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                matches!(e.as_str(), "exe" | "com" | "bat" | "cmd" | "ps1" | "vbs" | "js" | "msi" | "reg" | "scr" | "hta" | "lnk" | "jar" | "wsf")
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub title: String,
    pub kind: TargetKind,
    pub score: f32,
    /// Откуда: window, start, app_paths, path, folder, settings, file, uri.
    pub source: &'static str,
    #[serde(skip)]
    stage: u8,
}

const VERBS: &[&str] = &[
    "открой", "открыть", "открою", "запусти", "запустить", "покажи", "показать", "включи", "перейди", "в", "на", "мне",
    "пожалуйста", "джарвис", "сэр", "приложение", "программу", "программа", "open", "launch", "show", "start", "the",
];

struct Query {
    text: String,
    settings_hint: bool,
    folder_hint: bool,
}

fn parse_query(raw: &str) -> Query {
    let mut settings_hint = false;
    let mut folder_hint = false;
    let words: Vec<String> = norm_text(raw)
        .split(' ')
        .filter(|w| {
            if matches!(*w, "настройки" | "настройку" | "настройка" | "параметры" | "settings") {
                settings_hint = true;
                return false;
            }
            if matches!(*w, "папку" | "папка" | "папки" | "каталог" | "folder") {
                folder_hint = true;
                return false;
            }
            !VERBS.contains(w)
        })
        .map(String::from)
        .collect();
    let mut text = words.join(" ");
    if text.is_empty() && settings_hint {
        text = "настройки".into();
        settings_hint = false;
    }
    Query { text, settings_hint, folder_hint }
}

/// Кандидаты для имени, лучшие первыми.
pub fn resolve(src: &dyn TargetSources, name: &str) -> Vec<Candidate> {
    let raw = name.trim();
    let mut out: Vec<Candidate> = Vec::new();
    let push = |out: &mut Vec<Candidate>, title: String, kind: TargetKind, score: f32, source: &'static str, stage: u8| {
        if score > 0.0 {
            out.push(Candidate { title, kind, score: score.min(1.1), source, stage });
        }
    };
    // явный URI или существующий путь
    let low = raw.to_lowercase();
    if low.starts_with("http://") || low.starts_with("https://") || low.starts_with("ms-settings:") || low.starts_with("shell:") || low.starts_with("mailto:") {
        push(&mut out, raw.to_string(), TargetKind::Uri { uri: raw.to_string() }, 1.1, "uri", 0);
        return out;
    }
    let p = Path::new(raw.trim_matches('"'));
    if p.is_absolute() && p.exists() {
        let kind = if p.is_dir() { TargetKind::Folder { path: p.into() } } else { TargetKind::File { path: p.into() } };
        push(&mut out, raw.to_string(), kind, 1.1, "path", 0);
        return out;
    }

    let q = parse_query(raw);
    if q.text.is_empty() {
        return out;
    }
    // синонимы приложений: «блокнот» → notepad
    let mut terms: Vec<String> = vec![q.text.clone()];
    for (names, canon) in catalog::APP_ALIASES {
        if names.iter().any(|n| sim(&q.text, n) >= 0.9) {
            terms.extend(canon.iter().map(|c| c.to_string()));
        }
    }
    let best = |t: &str| terms.iter().map(|q| sim(q, t)).fold(0.0f32, f32::max);
    let app_penalty = if q.settings_hint || q.folder_hint { 0.85 } else { 1.0 };

    // 1. окна
    for w in src.windows() {
        let stem = w.exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
        let s = best(&w.title).max(best(&stem));
        push(&mut out, w.title.clone(), TargetKind::Window { hwnd: w.hwnd, pid: w.pid, exe: w.exe.clone() }, s * app_penalty, "window", 1);
    }
    // 2. «Пуск»
    for a in src.start_apps() {
        let s = best(&a.name);
        push(&mut out, a.name.clone(), TargetKind::StartApp { app_id: a.app_id.clone() }, s * app_penalty, "start", 2);
    }
    // 3. App Paths
    for (stem, path) in src.app_paths() {
        let s = best(&stem);
        push(&mut out, stem.clone(), TargetKind::AppPath { exe: path }, s * app_penalty, "app_paths", 3);
    }
    // 4. PATH
    for t in &terms {
        if t.contains(' ') || !t.is_ascii() {
            continue;
        }
        if let Some(exe) = src.path_exe(t) {
            push(&mut out, t.clone(), TargetKind::PathExe { exe }, 0.95 * app_penalty, "path", 4);
        }
    }
    // 5. известные папки
    for (f, names) in catalog::FOLDERS {
        let s = names.iter().map(|n| sim(&q.text, n)).fold(0.0f32, f32::max);
        if s >= 0.7
            && let Some(path) = src.known_folder(*f)
        {
            let bonus = if s >= 0.999 { 0.01 } else { 0.0 } + if q.folder_hint { 0.05 } else { 0.0 };
            push(&mut out, names[0].to_string(), TargetKind::Folder { path }, s + bonus, "folder", 5);
        }
    }
    // 6. страницы настроек
    for (uri, names) in catalog::SETTINGS {
        let s = names.iter().map(|n| sim(&q.text, n)).fold(0.0f32, f32::max);
        if s >= 0.7 {
            let s = if q.settings_hint { s + 0.05 } else { s * 0.97 };
            push(&mut out, format!("Параметры: {}", names[0]), TargetKind::Settings { uri: uri.to_string() }, s, "settings", 6);
        }
    }
    // 7. поиск файла — только если ничего хорошего
    if out.iter().all(|c| c.score < 0.85) && !q.settings_hint {
        for path in src.find_files(&q.text) {
            let title = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let s = 0.55 + 0.35 * sim(&q.text, path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default().as_str());
            push(&mut out, title, TargetKind::File { path }, s, "file", 7);
        }
    }
    out.retain(|c| c.score >= 0.5);
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then(a.stage.cmp(&b.stage)));
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.kind.key()));
    out.truncate(8);
    out
}

/// Однозначен ли лучший кандидат (иначе — уточняющий вопрос).
pub fn is_confident(c: &[Candidate]) -> bool {
    match c {
        [] => false,
        [a] => a.score >= 0.7,
        [a, b, ..] => a.score >= 0.8 && (a.score - b.score >= 0.05 || a.stage < b.stage && a.score >= 0.95),
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    #[derive(Default)]
    pub struct MockOpener(pub Mutex<Vec<OpenRequest>>);
    impl Opener for MockOpener {
        fn open(&self, req: &OpenRequest) -> io::Result<()> {
            self.0.lock().unwrap().push(req.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct MockSources {
        pub windows: Vec<WindowInfo>,
        pub apps: Vec<StartApp>,
        pub files: Vec<PathBuf>,
    }
    impl TargetSources for MockSources {
        fn windows(&self) -> Vec<WindowInfo> {
            self.windows.clone()
        }
        fn start_apps(&self) -> Vec<StartApp> {
            self.apps.clone()
        }
        fn app_paths(&self) -> Vec<(String, PathBuf)> {
            vec![("chrome".into(), "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe".into())]
        }
        fn path_exe(&self, name: &str) -> Option<PathBuf> {
            matches!(name, "notepad" | "calc" | "cmd").then(|| PathBuf::from(format!("C:\\Windows\\System32\\{name}.exe")))
        }
        fn known_folder(&self, f: KnownFolder) -> Option<PathBuf> {
            Some(PathBuf::from(format!("C:\\Users\\test\\{f:?}")))
        }
        fn find_files(&self, q: &str) -> Vec<PathBuf> {
            self.files.iter().filter(|p| p.to_string_lossy().to_lowercase().contains(&q.to_lowercase())).cloned().collect()
        }
    }

    pub fn sources() -> MockSources {
        MockSources {
            windows: vec![WindowInfo { hwnd: 42, pid: 7, title: "Отчёт.docx - Word".into(), exe: "WINWORD.EXE".into() }],
            apps: vec![
                StartApp { name: "Блокнот".into(), app_id: "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App".into() },
                StartApp { name: "Калькулятор".into(), app_id: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into() },
                StartApp { name: "Telegram Desktop".into(), app_id: "TelegramDesktop".into() },
                StartApp { name: "Параметры".into(), app_id: "windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel".into() },
            ],
            files: vec!["D:\\docs\\Договор аренды.pdf".into(), "D:\\docs\\setup.exe".into()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::sources;
    use super::*;

    fn top(name: &str) -> Candidate {
        resolve(&sources(), name).into_iter().next().unwrap_or_else(|| panic!("нет кандидатов для «{name}»"))
    }

    #[test]
    fn notepad_by_russian_name() {
        let c = top("блокнот");
        assert!(
            matches!(&c.kind, TargetKind::StartApp { app_id } if app_id.contains("Notepad"))
                || matches!(&c.kind, TargetKind::PathExe { exe } if exe.ends_with("notepad.exe")),
            "{c:?}"
        );
        assert!(is_confident(&resolve(&sources(), "открой блокнот")));
    }

    #[test]
    fn settings_bluetooth() {
        assert_eq!(top("настройки bluetooth").kind, TargetKind::Settings { uri: "ms-settings:bluetooth".into() });
        assert_eq!(top("блютуз").kind, TargetKind::Settings { uri: "ms-settings:bluetooth".into() });
        assert_eq!(top("открой настройки звука").kind, TargetKind::Settings { uri: "ms-settings:sound".into() });
        assert_eq!(top("настройки вай фай").kind, TargetKind::Settings { uri: "ms-settings:network-wifi".into() });
        assert_eq!(top("ночной свет").kind, TargetKind::Settings { uri: "ms-settings:nightlight".into() });
    }

    #[test]
    fn folders() {
        assert_eq!(top("загрузки").kind, TargetKind::Folder { path: "C:\\Users\\test\\Downloads".into() });
        assert_eq!(top("покажи папку загрузок").kind, TargetKind::Folder { path: "C:\\Users\\test\\Downloads".into() });
        assert_eq!(top("рабочий стол").kind, TargetKind::Folder { path: "C:\\Users\\test\\Desktop".into() });
    }

    #[test]
    fn windows_translit_and_files() {
        assert!(matches!(top("ворд").kind, TargetKind::Window { hwnd: 42, .. }));
        assert!(matches!(top("телеграм").kind, TargetKind::StartApp { ref app_id } if app_id == "TelegramDesktop"));
        assert!(matches!(top("хром").kind, TargetKind::AppPath { .. }));
        let c = top("договор");
        assert!(matches!(&c.kind, TargetKind::File { path } if path.to_string_lossy().contains("Договор")), "{c:?}");
        let exe = top("setup");
        assert!(exe.kind.is_executable_file());
        assert!(matches!(top("https://ya.ru").kind, TargetKind::Uri { .. }));
    }

    #[test]
    fn fuzzy() {
        assert!(sim("загрузок", "загрузки") > 0.8);
        assert!(sim("телеграм", "Telegram Desktop") > 0.8);
        assert_eq!(sim("абв", "xyz"), 0.0);
    }

    /// Живое разрешение (только чтение: окна, «Пуск», реестр, PATH) — ничего не запускается.
    #[test]
    fn live_resolution_without_launch() {
        let src = SystemSources::with_roots(vec![]);
        let c = resolve(&src, "блокнот");
        let first = c.first().expect("блокнот не найден");
        let ok = match &first.kind {
            TargetKind::StartApp { app_id } => app_id.to_lowercase().contains("notepad"),
            TargetKind::PathExe { exe } | TargetKind::AppPath { exe } => exe.to_string_lossy().to_lowercase().ends_with("notepad.exe"),
            TargetKind::Window { exe, .. } => exe.to_lowercase() == "notepad.exe",
            _ => false,
        };
        assert!(ok, "{c:#?}");
        let s = resolve(&src, "настройки bluetooth");
        assert_eq!(s[0].kind, TargetKind::Settings { uri: "ms-settings:bluetooth".into() }, "{s:#?}");
        let d = resolve(&src, "загрузки");
        let dl = win::known_folder(KnownFolder::Downloads).unwrap();
        assert!(d.iter().take(3).any(|c| c.kind == TargetKind::Folder { path: dl.clone() }), "{d:#?}");
    }
}
