//! Группы инструментов и ленивая загрузка: модели уходит не весь реестр (≈70 описаний, 8–10 тыс. токенов),
//! а «базовые» инструменты + группы, которые выбрал роутер по словам пользователя, + группы, использованные
//! в прошлом ходе. Остальное модель подключает сама мета-инструментом [`TOOLS_LOAD`].
//!
//! Группа — свойство имени инструмента (`group_of`), реестр не меняется. Незнакомые имена без известного
//! префикса попадают в `core`, чтобы новый инструмент никогда не «пропал» из запроса.

use std::collections::BTreeSet;

use jarvis_core::{RiskTier, ToolSpec};
use serde_json::{Value, json};

/// Имя мета-инструмента (обрабатывается агентом, в `ToolRegistry` его нет).
pub const TOOLS_LOAD: &str = "tools_load";

/// Группа, которая отправляется всегда.
pub const CORE: &str = "core";

/// Группа инструментов: id и одна строка описания (для `tools_load`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolGroup {
    pub id: &'static str,
    pub about: &'static str,
}

/// Все группы в порядке показа модели.
pub const GROUPS: &[ToolGroup] = &[
    ToolGroup { id: CORE, about: "состояние ПК, открыть программу/файл/сайт, поиск файлов, PowerShell, веб-поиск, память" },
    ToolGroup { id: "pc_input", about: "мышь, клавиатура, медиаклавиши, буфер обмена" },
    ToolGroup { id: "ui", about: "окна и их элементы (UIA): найти, нажать, ввести, прокрутить" },
    ToolGroup { id: "screen", about: "скриншот, OCR, метки на экране" },
    ToolGroup { id: "terminal", about: "терминал: вывод и ошибки, команды, cmd" },
    ToolGroup { id: "files", about: "файлы: читать, писать, копировать, удалять, проводник, отмена" },
    ToolGroup { id: "process", about: "процессы: список, завершить, приоритет" },
    ToolGroup { id: "system", about: "службы, автозагрузка, планировщик, питание, очистка" },
    ToolGroup { id: "apps", about: "программы (winget): найти, установить, обновить, удалить" },
    ToolGroup { id: "network", about: "сеть: IP, ping, DNS" },
    ToolGroup { id: "web", about: "открыть и прочитать страницу, исследование темы" },
    ToolGroup { id: "memory", about: "память: забыть факт" },
    ToolGroup { id: "kb", about: "база знаний Windows: как настроить/исправить, команды" },
];

/// Инструменты, которые всегда в запросе (держать мало: ~10). `kb_search` (≈900 симв.) — в группе `kb`:
/// вопросы «как настроить Windows» её подключают по словам, остальные — через `tools_load`.
const CORE_TOOLS: &[&str] = &[
    "sys_snapshot",
    "open_target",
    "url_open",
    "file_search",
    "powershell_run",
    "web_search",
    "memory_remember",
    "memory_recall",
];

/// Явные имена вне общего префикса группы.
const NAMED: &[(&str, &str)] = &[
    ("media_key", "pc_input"),
    ("cmd_run", "terminal"),
    ("reveal_in_explorer", "files"),
    ("undo_last", "files"),
    ("scheduled_tasks_list", "system"),
    ("temp_cleanup", "system"),
    ("network_info", "network"),
    ("ping", "network"),
    ("dns_flush", "network"),
];

/// Префиксы имён → группа.
const PREFIXES: &[(&str, &str)] = &[
    ("mouse_", "pc_input"),
    ("key_", "pc_input"),
    ("clipboard_", "pc_input"),
    ("ui_", "ui"),
    ("screen_", "screen"),
    ("terminal_", "terminal"),
    ("shell_hook_", "terminal"),
    ("file_", "files"),
    ("process_", "process"),
    ("service_", "system"),
    ("startup_", "system"),
    ("power_plan_", "system"),
    ("winget_", "apps"),
    ("net_", "network"),
    ("web_", "web"),
    ("memory_", "memory"),
    ("kb_", "kb"),
];

/// Группа инструмента по имени. Незнакомое — `core` (не теряем новые инструменты).
pub fn group_of(name: &str) -> &'static str {
    if CORE_TOOLS.contains(&name) {
        return CORE;
    }
    if let Some((_, g)) = NAMED.iter().find(|(n, _)| *n == name) {
        return g;
    }
    PREFIXES.iter().find(|(p, _)| name.starts_with(p)).map(|(_, g)| *g).unwrap_or(CORE)
}

/// Id группы из `GROUPS` по строке (без учёта регистра и пробелов).
pub fn parse_group(s: &str) -> Option<&'static str> {
    let s = s.trim().to_lowercase();
    GROUPS.iter().find(|g| g.id == s).map(|g| g.id)
}

/// Правила роутера: основы слов (рус./англ.) → группы. Основа совпадает, если с неё начинается слово запроса;
/// основа с пробелом ищется как фраза; `слово=` — только целое слово (короткие: `ip=`, `git=`).
const ROUTES: &[(&[&str], &[&str])] = &[
    (
        &[
            "мыш", "клик", "курсор", "перетащ", "нажм", "нажать", "напечат", "напиши", "введи", "ввести",
            "впиши", "набери", "набрать", "клавиш", "горяч", "сочетани", "хоткей", "mouse", "click", "drag", "press",
            "hotkey", "keyboard", "type=",
        ],
        &["pc_input", "ui"],
    ),
    (
        &[
            "окн", "окош", "вкладк", "кнопк", "меню", "поле", "поля", "полю", "галочк", "флажок", "чекбокс", "переключ", "прокрут",
            "пролистай", "листай", "скролл", "сверни", "разверни", "закрой", "закрыть", "выбери", "отметь", "window",
            "button", "tab=", "scroll", "menu", "checkbox",
        ],
        &["ui", "pc_input"],
    ),
    (
        &[
            "громк", "тише", "звук", "музык", "пауз", "трек", "песн", "плеер", "буфер", "скопируй", "вставь", "volume",
            "mute", "clipboard", "paste",
        ],
        &["pc_input"],
    ),
    (
        &[
            "экран", "скрин", "снимок", "видишь", "посмотри", "глянь", "что на", "распознай", "screen", "screenshot",
            "ocr",
        ],
        &["screen"],
    ),
    (
        &[
            "терминал", "консол", "команд", "cmd", "ошибк", "error", "сборк", "собери", "компил", "git=",
            "cargo", "npm", "лог=", "логи=", "логах=", "terminal", "console", "build", "compile",
        ],
        &["terminal"],
    ),
    (
        &[
            "файл", "папк", "каталог", "директор", "документ", "переимен", "перемест", "копир", "удали", "удалить",
            "корзин", "проводник", "рабочем стол", "загрузк", "отмени", "найди", "file", "folder", "directory",
            "rename", "delete", "explorer", "undo",
        ],
        &["files"],
    ),
    (
        &[
            "процесс", "диспетчер", "завис", "тормоз", "грузит", "нагруж", "закрой", "закрыть", "заверш", "убей",
            "убить", "приоритет", "приостанов", "заморозь", "процессор", "памят", "озу", "оператив", "kill", "process",
            "cpu", "ram=",
        ],
        &["process"],
    ),
    (
        &[
            "служб", "сервис", "автозагруз", "автозапуск", "планировщ", "питани", "электропит", "энергосбер",
            "производительн", "батаре", "временн", "очист", "мусор", "кэш", "service", "startup", "power=", "cleanup",
            "cache",
        ],
        &["system"],
    ),
    (
        &[
            "установи", "установить", "поставь", "программ", "приложени", "обнови", "обновить", "обновлен", "winget",
            "install", "uninstall", "upgrade", "update",
        ],
        &["apps"],
    ),
    (
        &[
            "сеть", "сети", "сетев", "интернет", "wifi", "wi-fi", "вайфай", "пинг", "ping", "dns", "днс", "ip=", "айпи",
            "network",
        ],
        &["network"],
    ),
    (
        &[
            "загугли", "погугли", "гугл", "интернет", "сайт", "новост", "стать", "страниц", "ссылк", "url", "http",
            "www", "википед", "исследуй", "разузнай", "узнай", "почитай", "погод", "курс=", "курса=", "google", "website", "web=",
            "news", "research", "weather",
        ],
        &["web"],
    ),
    (&["запомни", "помнишь", "забудь", "вспомни", "remember", "forget", "recall"], &["memory"]),
    (
        &[
            "windows", "виндовс", "винд", "как настро", "как отключ", "как включ", "как исправ", "как смен",
            "как помен", "как убрать", "как сделать", "как ускор", "почему", "не работает", "настройк", "параметр",
            "реестр", "политик", "защитник", "defender", "брандмауэр", "firewall", "bitlocker", "powercfg", "gpedit",
            "regedit", "registry", "how to",
        ],
        &["kb"],
    ),
];

/// Нормализованный текст: нижний регистр, «ё» → «е», слова через один пробел.
fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('ё', "е")
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Группы по тексту пользователя (без `core`).
pub fn route(text: &str) -> BTreeSet<&'static str> {
    let words = normalize(text);
    let joined = format!(" {} ", words.join(" "));
    let mut out = BTreeSet::new();
    for (stems, groups) in ROUTES {
        let hit = stems.iter().any(|s| {
            if let Some(exact) = s.strip_suffix('=') {
                words.iter().any(|w| w == exact)
            } else if s.contains(' ') {
                joined.contains(&format!(" {s}"))
            } else {
                words.iter().any(|w| w.starts_with(s))
            }
        });
        if hit {
            out.extend(groups.iter().copied());
        }
    }
    out
}

/// Спецификация `tools_load` для групп, которые ещё можно подключить (`available` — их id).
pub fn tools_load_spec(available: &[&'static str]) -> ToolSpec {
    let mut description = String::from("Подключить группы инструментов, если нужного нет среди доступных. Группы:");
    for g in GROUPS.iter().filter(|g| available.contains(&g.id)) {
        description.push_str(&format!("\n{} — {}", g.id, g.about));
    }
    ToolSpec {
        name: TOOLS_LOAD.into(),
        description,
        parameters: json!({
            "type": "object",
            "properties": {
                "groups": { "type": "array", "items": { "type": "string", "enum": available } }
            },
            "required": ["groups"]
        }),
        tier: RiskTier::T0,
    }
}

/// Имена групп из аргументов `tools_load`: массив строк, одна строка или строка через запятую.
/// Возвращает (известные, неизвестные).
pub fn parse_load_args(args: &Value) -> (Vec<&'static str>, Vec<String>) {
    let raw: Vec<String> = match args.get("groups").or(args.get("group")) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for part in raw.iter().flat_map(|s| s.split([',', ' ', ';'])).filter(|s| !s.trim().is_empty()) {
        match parse_group(part) {
            Some(g) if !known.contains(&g) => known.push(g),
            Some(_) => {}
            None => unknown.push(part.trim().to_string()),
        }
    }
    (known, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Полный список имён реестра приложения (из `measure_request_size`) + kb.
    const ALL: &[&str] = &[
        "screen_capture", "mouse_click", "screen_ocr", "file_search", "mouse_move", "ui_scroll", "ui_find",
        "ui_snapshot", "terminal_run", "terminal_context", "open_target", "screen_marks", "memory_remember",
        "powershell_run", "mouse_drag", "terminal_type", "mouse_scroll", "file_write", "web_search", "memory_forget",
        "web_open", "temp_cleanup", "process_list", "web_research", "cmd_run", "key_type", "key_hotkey", "media_key",
        "ui_set_value", "shell_hook_install", "file_move", "file_copy", "ui_expand", "ui_invoke", "memory_recall",
        "process_kill", "file_delete", "process_priority", "ui_focus_window", "web_fetch", "startup_disable",
        "ui_toggle", "power_plan_set", "service_set_startup", "ui_select", "shell_hook_uninstall", "file_read",
        "scheduled_tasks_list", "undo_last", "reveal_in_explorer", "process_resume", "process_suspend",
        "service_list", "winget_install", "ping", "url_open", "winget_upgrade", "ui_window_list", "service_start",
        "service_stop", "winget_uninstall", "startup_list", "service_status", "clipboard_set", "sys_snapshot",
        "clipboard_get", "winget_search", "power_plan_get", "network_info", "dns_flush", "kb_search",
        "kb_lookup_command", "kb_status",
    ];

    #[test]
    fn every_real_tool_has_a_known_group_and_core_is_small() {
        let core: Vec<_> = ALL.iter().filter(|n| group_of(n) == CORE).copied().collect();
        let mut expected: Vec<&str> = CORE_TOOLS.to_vec();
        expected.sort();
        let mut got = core.clone();
        got.sort();
        assert_eq!(got, expected, "в core только явный список");
        assert!(core.len() <= 12);
        for n in ALL {
            assert!(GROUPS.iter().any(|g| g.id == group_of(n)), "{n}");
        }
        assert_eq!(group_of("mouse_click"), "pc_input");
        assert_eq!(group_of("ui_find"), "ui");
        assert_eq!(group_of("cmd_run"), "terminal");
        assert_eq!(group_of("undo_last"), "files");
        assert_eq!(group_of("winget_install"), "apps");
        assert_eq!(group_of("web_open"), "web");
        assert_eq!(group_of("web_search"), CORE);
        assert_eq!(group_of("kb_search"), "kb");
        // новое имя с известным префиксом — в свою группу, совсем незнакомое — в core
        assert_eq!(group_of("ui_drag_and_drop"), "ui");
        assert_eq!(group_of("brand_new_tool"), CORE);
    }

    #[test]
    fn every_group_has_tools() {
        for g in GROUPS {
            assert!(ALL.iter().any(|n| group_of(n) == g.id), "пустая группа {}", g.id);
        }
    }

    fn r(text: &str) -> Vec<&'static str> {
        route(text).into_iter().collect()
    }

    #[test]
    fn router_picks_groups() {
        assert!(r("привет").is_empty());
        assert!(r("Который час?").is_empty());
        assert_eq!(r("кликни по кнопке ОК"), ["pc_input", "ui"]);
        assert_eq!(r("сделай погромче музыку"), ["pc_input"]);
        assert_eq!(r("Что ты видишь на экране?"), ["screen"]);
        assert_eq!(r("что за ошибка в терминале"), ["terminal"]);
        assert_eq!(r("найди файл отчёт в загрузках"), ["files"]);
        assert_eq!(r("какой процесс жрёт память"), ["process"]);
        assert_eq!(r("отключи службу печати"), ["system"]);
        assert_eq!(r("установи 7-zip через winget"), ["apps"]);
        assert_eq!(r("загугли новости про Rust"), ["web"]);
        assert_eq!(r("запомни, что я пью чёрный кофе"), ["memory"]);
        assert_eq!(r("Как отключить залипание клавиш в Windows?"), ["kb", "pc_input", "ui"]);
        assert!(r("закрой блокнот").contains(&"process"));
        assert!(r("какой у меня IP").contains(&"network"));
        // короткая основа не ловит середину слова
        assert!(!r("типа того").contains(&"network"));
        assert!(!r("наведи курсор").contains(&"web"), "курсор — не «курс»");
        assert!(!r("запусти в powershell").contains(&"system"), "powershell — не «power»");
        assert!(!r("получи список").contains(&"ui"));
    }

    #[test]
    fn tools_load_spec_lists_only_available_groups() {
        let s = tools_load_spec(&["ui", "web"]);
        assert_eq!(s.name, TOOLS_LOAD);
        assert_eq!(s.tier, RiskTier::T0);
        assert!(s.description.contains("\nui — ") && s.description.contains("\nweb — "));
        assert!(!s.description.contains("screen —"));
        assert_eq!(s.parameters["properties"]["groups"]["items"]["enum"], json!(["ui", "web"]));
    }

    #[test]
    fn load_args_are_lenient() {
        assert_eq!(parse_load_args(&json!({"groups": ["ui", "Screen"]})), (vec!["ui", "screen"], vec![]));
        assert_eq!(parse_load_args(&json!({"groups": "ui, web"})), (vec!["ui", "web"], vec![]));
        assert_eq!(parse_load_args(&json!({"groups": ["ui", "ui", "nope"]})), (vec!["ui"], vec!["nope".to_string()]));
        assert_eq!(parse_load_args(&json!({})), (vec![], vec![]));
    }
}
