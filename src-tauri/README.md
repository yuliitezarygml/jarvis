# J.A.R.V.I.S. — оболочка Tauri 2 и HUD (Next.js)

Окна, трей, глобальные хоткеи, мост событий `jarvis_core::Event` → фронтенд, конфиг, симулятор и демо-заглушки
экранов. Фронтенд — Next.js (App Router, TypeScript, static export) в корне репозитория (`src/`, `public/`).

## Запуск

```powershell
cd d:\jarvis
npm install            # один раз
cargo tauri dev        # поднимает `npm run dev` (http://localhost:3000) и приложение
```

Отладочная сборка запускается **без UAC** (манифест `asInvoker`). Релиз:

```powershell
cargo tauri build      # npm run build → out/ → src-tauri/target/release/jarvis.exe (+ установщик NSIS)
```

Релизный exe требует прав администратора (`requireAdministrator`, один запрос UAC при старте).

Только интерфейс, без Tauri: `npm run dev` и открыть `http://localhost:3000/` — команды обслуживают
заглушки из `src/lib/mock.ts`, `http://localhost:3000/hud/?demo` сразу запускает демо-сценарий.
Вкладки браузера синхронизируются через `BroadcastChannel`: демо в `/hud/` подсвечивает `/overlay/` в соседней вкладке.

### Проверки

```powershell
npm test               # vitest: редьюсер событий, цвета свечения, форматирование, геометрия, настройки
npm run typecheck      # tsc --noEmit
npm run build          # static export в out/
cd src-tauri
cargo build
cargo test             # сериализация событий TauriSink (mock runtime), config_path, маски ключей, симулятор, …
```

## Что увидит пользователь

При старте открываются два окна (режим берётся из `config.json` → `hud.mode`, по умолчанию `corner`):

| Окно | Адрес | Свойства |
|---|---|---|
| `corner` | `/corner/` | 220×220, прозрачное, всегда сверху, без панели задач, без рамки и тени. Клик — во весь экран; перетаскивание — переместить, позиция сохраняется в `hud.corner_x/corner_y` (физические пиксели) |
| `hud` | `/hud/` | полноэкранное, без рамки; реактор, телеметрия, лента действий, источники, субтитры, диалог подтверждения. `Esc` — в угол |
| `overlay` | `/overlay/` | прозрачное на весь основной монитор, всегда сверху, без фокуса (`focusable(false)`), без панели задач, клики насквозь (`set_ignore_cursor_events(true)`). Пока Джарвис не управляет ПК — пустое и ничего не рисует |
| `settings` | `/settings/` (+ `/memory/`, `/kb/`, `/log/`) | обычное окно, создаётся по требованию (трей, кнопка в HUD) |

На `hud`, `corner`, `overlay` выставлен `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` — окна не попадают
в скриншоты и запись экрана (в том числе в скриншоты, которые агент отправляет vision-модели).

- **Ctrl+Alt+J** — весь экран ↔ угол (то же: левый клик по значку в трее, меню трея).
- **Ctrl+Alt+K** — аварийная остановка: `UiHooks::emergency_stop`, отмена демо, открытые действия → `cancelled`,
  открытый запрос подтверждения → «нет», `ControlStop`, `State{idle}`, событие `jarvis://emergency_stop`.
- **Трей**: «Во весь экран», «В угол», «Настройки», «Выход» (единственный способ закрыть приложение;
  служебные окна на Alt+F4 не закрываются, HUD сворачивается в угол).
- **Симулятор**: панель «Симулятор» слева внизу в HUD (видна в отладочной сборке; в релизе — `Ctrl+Shift+D`).
  «Запустить демо» проигрывает ~32 с: все 6 состояний реактора, субтитры, волну звука, управление ПК T1 с целями
  на экране, поиск с источниками, T3 с подтверждением (красная рамка, таймер 20 с), ошибку.
  Отдельные кнопки шлют одиночные события.

## Мост событий

Rust: `TauriSink<R>` реализует `jarvis_core::EventSink` и делает `app.emit("jarvis://event", event)` во все окна.
Payload — JSON `jarvis_core::Event` как есть (`#[serde(tag = "type", rename_all = "snake_case")]`):

```json
{"type":"state","state":"thinking"}
{"type":"subtitle","speaker":"jarvis","text":"Сию минуту, сэр.","is_final":true}
{"type":"audio_level","source":"mic","rms":0.42}
{"type":"control_start","tier":"T3"}
{"type":"control_target","rect":{"x":1180,"y":860,"w":120,"h":36},"label":"Нажимаю: Сохранить"}
{"type":"control_stop"}
{"type":"action","id":"a1","tool":"ui_click","summary":"Сохраняю документ","status":"running"}
{"type":"confirm_request","id":"c1","summary":"Остановить службу Spooler?","tier":"T3","timeout_ms":20000}
{"type":"confirm_resolved","id":"c1","approved":true}
{"type":"telemetry","cpu":12.5,"ram_used_mb":8000,"ram_total_mb":16384,"gpu":null}
{"type":"sources","items":[{"title":"Rust Blog","url":"https://blog.rust-lang.org/"}]}
{"type":"error","message":"Сервер распознавания речи не отвечает"}
```

`rect` — физические пиксели виртуального рабочего стола; оверлей переводит их в CSS-пиксели своего монитора.
`audio_level` ~30 раз в секунду; `telemetry` ~1 раз в секунду.

Фронтенд: `src/lib/reducer.ts` (чистый `reduce(state, event, now)`; неизвестный или битый JSON игнорируется),
`src/lib/store.ts` (`useJarvis(selector)`, `useTauriEvent`), типы — `src/lib/types.ts` (зеркало `jarvis-core`).
Канвас-реактор читает стор в каждом кадре и не перерисовывает React.

Прочие события оболочки: `jarvis://cursor` `{x,y}` (только окну `overlay`, 60 Гц во время управления — след курсора),
`jarvis://hud_mode` (`"fullscreen" | "corner"`), `jarvis://emergency_stop` `{source}`, `jarvis://settings_tab` (id вкладки).

`EventCache` хранит снимок экрана (состояние, свечение и цель, открытое подтверждение, источники, телеметрия,
50 последних действий); окно, открытое позже, получает его командой `events_replay`.

Таймаут подтверждения страхует оболочка: если за `timeout_ms + 1,5 с` никто не ответил — `UiHooks::confirm(id, false)`
и `ConfirmResolved{approved:false}` (fail-closed).

## Команды Tauri (`src-tauri/src/commands.rs`)

Аргументы из JS — в camelCase (`providerId`), в Rust — snake_case (правило Tauri 2).

| Команда | Сигнатура | |
|---|---|---|
| `set_hud_mode` | `(mode: HudMode) -> Result<()>` | async; режим сохраняется в config.json |
| `get_hud_mode` | `() -> HudMode` | |
| `toggle_hud_mode` | `() -> Result<HudMode>` | async |
| `open_settings` | `(tab: Option<String>) -> Result<()>` | async; `providers`, `voice`, `audio`, `permissions`, `windows`, `search` |
| `overlay_geometry` | `() -> Result<Option<OverlayGeometry{x,y,width,height,scale}>>` | async |
| `config_get` | `() -> Config` | ключи замаскированы (`mask_key`) |
| `config_set` | `(config: Config) -> Result<Config>` | маска вместо ключа = «не менять»; возвращает замаскированный |
| `config_path_get` | `() -> String` | |
| `app_info` | `() -> AppInfo{version, config_path, debug_build, elevated}` | |
| `emergency_stop` | `()` | то же, что Ctrl+Alt+K |
| `confirm_respond` | `(id: String, approved: bool) -> Result<()>` | ошибка, если запрос уже закрыт |
| `events_replay` | `() -> Vec<Event>` | снимок для нового окна |
| `open_url` | `(url: String) -> Result<()>` | только http(s), браузер по умолчанию |
| `simulate_demo` | `() -> u64` | длительность сценария, мс; повторный вызов перезапускает |
| `simulate_stop` | `()` | |
| `simulate_event` | `(event: Event)` | одно событие через настоящий `TauriSink` |
| `provider_presets` | `() -> Vec<ProviderPreset>` | **демо** (копия; настоящие — `providers.json` модуля LLM) |
| `provider_check` | `(provider_id, base_url, api_key) -> ProviderCheck{demo,status,message,models[],ranking[]}` | **демо**; маска ключа → берётся сохранённый (`config_store::resolve_key`) |
| `voice_check` | `(kind: "tts"\|"stt", url) -> ServiceCheck{demo,ok,message,latency_ms}` | **демо** |
| `search_presets` | `() -> Vec<SearchPreset>` | **демо** |
| `search_check` | `(id, api_key, extra) -> ServiceCheck` | **демо** |
| `audio_devices` | `() -> AudioDevices{demo,inputs,outputs}` | **демо** |
| `memory_list` | `() -> MemoryData{demo,facts,lessons,playbooks}` | **демо** (в памяти процесса) |
| `memory_upsert` | `(kind: "facts"\|"lessons"\|"playbooks", item: Value) -> Result<MemoryData>` | **демо**; пустой `id` — новая запись |
| `memory_delete` | `(kind, id) -> Result<MemoryData>` | **демо** |
| `kb_search` | `(query) -> KbResult{demo,hits[]}` | **демо** |
| `history_list` | `() -> HistoryResult{demo,items[]}` | **демо** |

Все «демо»-ответы содержат `demo: true`, UI показывает плашку «демо-данные». Интегратор заменяет тела команд,
сохраняя типы ответа (`src-tauri/src/demo.rs`).

## Подключение бэкенда (интегратору)

```rust
use jarvis_lib::{commands::SinkState, hooks::{AppHooks, UiHooks}};
// события для HUD/оверлея:
let sink: std::sync::Arc<dyn jarvis_core::EventSink> = std::sync::Arc::new(app.state::<SinkState>().0.clone());
// остановка и подтверждения:
app.state::<AppHooks>().set(std::sync::Arc::new(MyHooks)); // impl UiHooks { emergency_stop, confirm }
```

Ctrl+Alt+K регистрирует оболочка — агенту не нужно регистрировать его повторно (`RegisterHotKey` вернёт ошибку);
он получает остановку через `UiHooks::emergency_stop`.

## Конфиг

`config_store::config_path()`: env `JARVIS_HOME` (папка с `config.json`) → вверх от каталога exe до первой папки,
где есть `config.json` (в dev это `d:\jarvis`) → текущий каталог. Битый `config.json` не роняет приложение: берутся
умолчания, файл копируется в `config.json.broken`, в HUD — ошибка.

## Файлы

```
src-tauri/
  build.rs              манифест: release requireAdministrator, debug asInvoker (+ Common Controls v6);
                        встраивается линкером во все exe пакета, включая тесты
  tauri.conf.json       frontendDist ../out, devUrl :3000, CSP, иконки
  capabilities/         core:default + перетаскивание окна
  icons/                сгенерированы `cargo tauri icon icons/app-icon.svg`
  config/xeon-tts-ca.pem
  src/lib.rs            сборка приложения, регистрация команд
  src/windows_manager.rs окна, режимы, WDA_EXCLUDEFROMCAPTURE, позиция виджета, след курсора
  src/sink.rs           TauriSink, EventCache, имена событий
  src/commands.rs       команды, аварийная остановка, таймаут подтверждения
  src/hotkeys.rs, tray.rs, hooks.rs, config_store.rs, simulator.rs, demo.rs
src/app/{hud,corner,overlay,settings,memory,kb,log}/page.tsx
src/components/         reactor/ (Canvas 2D), hud/, settings/, DevPanel, AppShell
src/lib/                types, reducer, store, bridge, glow, format, geometry, settings, simulator, mock
public/fonts/           Exo 2, Orbitron, JetBrains Mono (woff2, SIL OFL 1.1 — тексты лицензий рядом)
```

## Известные риски (проверить на этой машине)

- Прозрачность WebView2: виджет и оверлей должны быть без белого/чёрного фона. Если фон появится — проверить, что
  у корня документа нет `color-scheme`/фона, и попробовать `no_redirection_bitmap(true)`.
- Click-through: `set_ignore_cursor_events(true)` даёт `WS_EX_TRANSPARENT | WS_EX_LAYERED` на всё окно оверлея;
  проверить клики сквозь рамку и что оверлей не забирает фокус.
- `WDA_EXCLUDEFROMCAPTURE` (Windows 10 2004+): проверить скриншотом (Win+Shift+S), что окна не видны.
- Оверлей всегда показан (пустой): показ скрытого окна в tao идёт через `SW_SHOW` и может активировать его.
- Цели `ControlTarget` на втором мониторе не подсвечиваются (оверлей только на основном).
- Полноэкранный HUD и корнер не проверены на нескольких мониторах с разным масштабом.
