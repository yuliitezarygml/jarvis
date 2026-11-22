# J.A.R.V.I.S. — оболочка Tauri 2 и HUD (Next.js)

Окна, трей, глобальные хоткеи, мост событий `jarvis_core::Event` → фронтенд, конфиг, симулятор (dev-панель) и
ядро-оркестратор, которое собирает все модули в одно приложение. Фронтенд — Next.js (App Router, TypeScript, static export) в корне репозитория (`src/`, `public/`).

## Запуск

```powershell
cd d:\jarvis
npm install            # один раз
cargo tauri dev        # поднимает `npm run dev` (http://localhost:3000) и приложение
```

Отладочная сборка запускается **без UAC** (манифест `asInvoker`). Релиз:

```powershell
cargo tauri build      # npm run build → out/ → target/release/jarvis.exe (+ установщик NSIS)
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
cargo build                       # весь workspace (из корня репозитория)
cargo test -p jarvis              # оболочка + интеграция ядра без GUI (мок-модель, настоящие инструменты)
cargo test -p jarvis live_voice_check -- --ignored --nocapture   # серверы TTS/STT (только /ready и /health)
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
| `agent_submit` | `(text) -> Result<()>` | текстовая команда из HUD («Сказать Джарвису…»), отменяет предыдущую задачу |
| `mic_get` / `mic_set` | `() -> MicStatus` / `(enabled) -> Result<MicStatus>` | микрофон `{enabled, listening, voice_ready, error}`; по умолчанию слушает |
| `assistant_status` | `() -> AssistantStatus{llm_ready, llm_problem, brain_model, mic, issues[], tools, busy}` | плашка «нет ключа LLM» в HUD |
| `provider_presets` | `() -> Vec<ProviderPreset>` | пресеты jarvis-llm (`crates/jarvis-llm/presets/providers.json`, иначе встроенные) |
| `provider_check` | `(provider_id, base_url, api_key) -> ProviderCheck{status,message,models[],ranking[]}` | jarvis-llm `check_provider`: ключ, модели, живая проверка, рейтинг; маска ключа → сохранённый |
| `provider_apply_best` | `(config, provider_ids?) -> Result<Config>` | jarvis-llm `apply_best` по последним проверкам → config.json → модели пересобираются сразу |
| `voice_check` | `(kind: "tts"\|"stt", url) -> ServiceCheck{ok,message,latency_ms}` | TTS `GET /ready`, STT `GET /health` (CA из конфига) |
| `search_presets` | `() -> Vec<SearchPreset>` | |
| `search_check` | `(id, api_key, extra) -> ServiceCheck` | jarvis-web `validate_config` (тестовый запрос) |
| `audio_devices` | `() -> AudioDevices{inputs,outputs}` | имена устройств WASAPI, `default` — устройство Windows по умолчанию |
| `memory_list` | `() -> MemoryData{facts,lessons,playbooks}` | `data/memory.db` (jarvis-memory) |
| `memory_upsert` | `(kind: "facts"\|"lessons"\|"playbooks", item) -> Result<MemoryData>` | пустой `id` — новая запись; шаг плейбука: `инструмент {json} — примечание` |
| `memory_delete` | `(kind, id) -> Result<MemoryData>` | |
| `kb_search` | `(query) -> KbResult{hits[]}` | `data/kb.sqlite` (jarvis-kb) |
| `history_list` | `() -> HistoryResult{items[]}` | сеансы и реплики памяти, по времени |

Типы ответов — `src/api.rs` (зеркало `src/lib/types.ts`); поле `demo` оставлено для совместимости и всегда `false`.
Демо осталось только в симуляторе (`simulate_*`, dev-панель).

## Ядро-оркестратор (`src/runtime/`)

`Core::start(config, TauriSink, CoreOptions::app(root), tokio-handle)` в `setup`:

```text
микрофон ─► VoicePipeline ─► Intent ─┬─ Command ─► Core::submit ─► Agent::run ─► ToolRegistry (sys/pc/web/kb/memory)
поле HUD ─► agent_submit ────────────┘   Stop ─► Core::stop_all (отмена, release_all_held, барж-ин)
                                         Confirm/Deny ─► ConfirmGate ◄── кнопки HUD (confirm_respond → UiHooks::confirm)
Agent ─ SpeechSink ─► SpeechQueue (очередь реплик) ─► VoicePipeline::speak (TTS Xeon)
Agent ─ events ─► GlowSink (GlowControl::hold на всю задачу) ─► TauriSink ─► окна
Agent ─ MemoryHooks ─► jarvis-memory (блок «Память», known_fix только при exact, заготовки уроков, журнал, рефлектор в фоне)
spawn_telemetry (jarvis-sys) ─► TauriSink; алерты ResourceGuard ─► субтитр + голос
```

| Файл | Что |
|---|---|
| `runtime/mod.rs` | `Core`: открытие модулей (ошибка модуля → `issues` и сообщение в HUD, не паника), задачи (новая отменяет старую), голос и микрофон, телеметрия, `apply_config` |
| `runtime/models.rs` | `ModelSlot` — модель роли с заменой на лету; без ключа — `NO_KEY_TEXT` |
| `runtime/bridges.rs` | `GateConfirmer` (озвучить → ждать «да, сэр»/кнопку 20 с, защищённый путь — второй раз), `MemoryBridge`, `GlowSink` |
| `runtime/speech.rs` | `SpeechQueue` (`SpeechSink` → `speak` по очереди), `VoiceEvents` (фильтр событий голоса) |
| `runtime/registry.rs` | один `ToolRegistry`; при дубле имени остаётся модуль раньше в порядке sys → pc → web → kb → memory |
| `runtime/services.rs` | тела команд экранов (провайдеры, голос, поиск, аудио, память, KB, журнал) |
| `runtime/tests.rs` | интеграция без GUI: мок-модель → настоящие инструменты → события → ответ; T3 с отказом |

`config_set` применяет сразу: провайдеры и модели, автономию/зрение, поиск, серверы TTS/STT (голос пересобирается),
resource guard. Только после перезапуска: `data_dir`, `search_roots` (в HUD — сообщение).

Ctrl+Alt+K регистрирует оболочка; ядро получает остановку через `UiHooks::emergency_stop` (`CoreHooks`).

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
  src/api.rs            типы ответов команд экранов
  src/runtime/          ядро-оркестратор (см. выше)
  src/hotkeys.rs, tray.rs, hooks.rs, config_store.rs, simulator.rs
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
