# jarvis-sys

Системный модуль J.A.R.V.I.S.: политика рисков, PowerShell/cmd, файлы с откатом, ресурсы ПК, поиск и открытие.
Только Windows (чистая логика — `policy`, `confirm`, `redact`, `guard` — собирается везде).

```rust
let cfg = jarvis_sys::SysConfig::from_core(&jarvis_core::Config::load("config.json")?, project_root);
let sys = jarvis_sys::Sys::new(cfg);            // Корзина Windows, ShellExecuteW, реальные источники
registry.extend(jarvis_sys::tools(sys.clone())); // 38 инструментов
```

## Модули

| Модуль | Что делает |
|---|---|
| `policy` | `classify_command(shell, cmd) -> Classification { tier, reasons, protected_path }` для PowerShell и cmd. Лексер цепочек (`;` `\|` `&&` `\|\|` `&`, переводы строк), вложенный код (`( )`, `$( )`, `{ }`, `@{ }`, `$( )` в строках), перенаправления в файл, алиасы, `cd` внутри команды, вложенные `powershell -Command`/`cmd /c`, `Start-Process`, методы .NET. **Неизвестное = T3.** Пути: профиль/рабочие папки/TEMP — T2, вне — T3, защищённые (`C:\Windows`, `Program Files`, `ProgramData`, `System32`, корень диска, корень профиля) — T3 + `protected_path`. |
| `confirm` | `ConfirmGate`: `request(summary, tier, timeout) -> ConfirmTicket` (`ticket.id`, `ticket.await -> ConfirmDecision`), `on_transcript(text, AudioSource, tts_finished_at)`, `on_click(id, approved)`, `cancel_all()`. События `ConfirmRequest`/`ConfirmResolved`. |
| `redact` | `redact_secrets(text) -> (String, usize)` — ключи gsk_/ghp_/github_pat_/sk-/sk-ant-/xox?-/AKIA/AWS secret/AIza/hf_/Stripe, JWT, Bearer/Basic, `password=`/`token:`/`--password x`, пароли в URL и строках подключения, PEM-ключи. Маркер `[REDACTED:вид]`. |
| `exec` | `run_powershell`/`run_cmd`/`run_program`: Job Object + `CREATE_SUSPENDED` (таймаут/отмена убивают всё дерево), UTF-8 (`[Console]::OutputEncoding`; для cmd — `chcp 65001` и вложенный cmd через `!JARVIS_CMD!`), лимит захвата 2 МБ, модели — ≤12 000 символов на поток, секреты вырезаются. |
| `undo` | Журнал `{data_dir}/undo/entries/*.json`: Корзина (`trash`), резервные копии `undo/files`, `reg export` в `undo/registry`, значения реестра, схема питания. `undo_last`. |
| `files` | `file_read/write/move/copy/delete` с журналом. Защищённый путь требует `confirm_protected: true` (после второго «да»). `file_write` отказывается писать текст с `[REDACTED:`. |
| `resources` | `sys_snapshot`, процессы, службы (SCM API / sc.exe), схемы питания (PowerProf API), автозагрузка (StartupApproved, как Диспетчер задач), обслуживание, winget, планировщик, `spawn_telemetry`. GPU — PDH `\GPU Engine(*)\Utilization Percentage` или NVML (`--features nvml`). |
| `guard` | `ResourceGuard::observe(&Sample, &top_fn) -> Vec<GuardAlert>`: RAM > 90 % 30 с, CPU > 95 % 60 с, диск < 5 %; антидребезг 10 мин; фраза для TTS. |
| `open` | `resolve(sources, name) -> Vec<Candidate>`: окна → «Пуск» (`Get-StartApps`, кэш 10 мин) → App Paths → PATH → известные папки → `ms-settings:` (57 страниц с русскими синонимами, включая `windowsdefender:`) → поиск файла. Запуск — через `Opener` (подменяется в тестах). |
| `search` | `file_search`: `ignore::WalkBuilder` параллельно + `grep-searcher`, бюджет ≤ 5 с, нумерованные результаты (`result=N` в `open_target`/`reveal_in_explorer`). |

## Инструменты и уровни

| Инструмент | Tier |
|---|---|
| `powershell_run`, `cmd_run` | по классификатору (база T3) |
| `undo_last` | T2 (T3, если откат трогает HKLM) |
| `file_read` | T0 |
| `file_write`, `file_move`, `file_copy`, `file_delete` | T2 в профиле/рабочих папках, T3 вне, T3 + повторное подтверждение для защищённых; удаление папки — T3 |
| `sys_snapshot`, `process_list`, `service_list`, `service_status`, `power_plan_get`, `startup_list`, `network_info`, `ping`, `winget_search`, `scheduled_tasks_list`, `file_search` | T0 |
| `process_kill`, `process_priority`, `process_suspend`, `process_resume` | T2 свой процесс, T3 системный/критичный/чужой/сам Джарвис (последнее запрещено) |
| `service_start`, `service_stop`, `service_set_startup` | T3 |
| `power_plan_set`, `dns_flush`, `temp_cleanup` | T2 (`temp_cleanup include_system` — T3, `dry_run` — T0) |
| `startup_disable` | T2 HKCU/папка пользователя, T3 HKLM/общая папка |
| `winget_install`, `winget_upgrade`, `winget_uninstall` | T3 |
| `open_target`, `reveal_in_explorer`, `url_open`, `clipboard_get`, `clipboard_set` | T1 (`open_target` с путём к exe — T3) |

Агент узнаёт о защищённом пути через `Sys::classify_call(tool, args) -> Option<Classification>`.

## Тесты

`cargo test` — ничего не меняет вне временных каталогов: не пишет в реестр (только `reg export` HKCU\Console во временную папку), не трогает службы, убивает/приостанавливает только свои дочерние `ping`, ничего не открывает (`open_target` проверяется на разрешении и с `MockOpener`). Тест настоящей Корзины — `#[ignore]` (`cargo test -- --ignored system_recycle_bin`).
Фейковые токены в тестах собираются через `concat!`, чтобы их не ловил сканер секретов GitHub.
