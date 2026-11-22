# jarvis-pc

Взаимодействие J.A.R.V.I.S. с ПК: мышь/клавиатура (`SendInput`), UI Automation, экран (снимок, set-of-marks,
встроенный OCR Windows), терминал (контекст ошибки, свой ConPTY, ввод команд, хук оболочки).
Все инструменты реализуют `jarvis_core::Tool`. Координаты — физические пиксели виртуального рабочего стола
(мультимонитор, поток переводится в Per-Monitor-DPI v2 на время операции).

## Подключение

```rust
let tk = jarvis_pc::PcToolkit::new(
    jarvis_pc::PcOptions::from_config(&cfg, project_root)
        .with_classifier(Arc::new(|cmd| jarvis_sys::classify(cmd))), // по умолчанию всё T3
);
registry.extend(tk.tools());
jarvis_pc::exclude_from_capture(hud.hwnd()?.0 as isize)?;   // HUD и overlay не попадают в снимки
// Ctrl+Alt+K: cancel.cancel(); jarvis_pc::release_all_held();
// многошаговая задача без мигания рамки: let _hold = tk.glow().hold(events, RiskTier::T1);
```

## Инструменты

| Инструмент | Tier | Что делает |
|---|---|---|
| `mouse_move`, `mouse_click`, `mouse_drag`, `mouse_scroll` | T1 | плавная траектория 150–400 мс (ease-in-out, дуга), клик по `x,y` или по `id` элемента UIA |
| `key_type` | T1 | текст через `KEYEVENTF_UNICODE` (кириллица, эмодзи, `\n` → Enter) |
| `key_hotkey` | T1 | `ctrl+shift+s`, `alt+f4`, `ctrl++`, последовательности `ctrl+k ctrl+c`, `ctrl+ы` = `ctrl+s` |
| `media_key` | T1 | play_pause/next/prev/stop/volume_up/volume_down/mute |
| `ui_snapshot`, `ui_find`, `ui_window_list` | T0 | ≤150 элементов: `[id] роль «имя» (x,y wxh) состояние`; id стабильны до следующего снимка |
| `ui_invoke`, `ui_set_value`, `ui_toggle`, `ui_expand`, `ui_select`, `ui_scroll`, `ui_focus_window` | T1 | паттерны UIA; `ui_set_value` без ValuePattern → фокус + Ctrl+A + ввод |
| `screen_capture` | T0 | монитор/экран/окно/область → JPEG/PNG ≤1280 px в `Attachment::Image` |
| `screen_marks` | T0 | снимок окна с пронумерованными рамками по UIA + легенда «[3] Кнопка «Сохранить»» |
| `screen_ocr` | T0 | Windows.Media.Ocr (ru/en/auto), строки с экранными рамками |
| `terminal_context` | T0 | источники: pty → хук `last.json` (что свежее, ≤30 мин) → UIA TextPattern → OCR; хук без stderr дополняется текстом с экрана |
| `terminal_run` | T1 + классификатор | ConPTY (`portable-pty`), PowerShell через `-EncodedCommand` или cmd через bat; таймаут/отмена убивают дерево (Job Object) |
| `terminal_type` | T1 (+ классификатор при `enter`) | фокус окна терминала и ввод одной строки |
| `shell_hook_install`, `shell_hook_uninstall` | T2 | хук в `$PROFILE` / `~/.bashrc` с бэкапом в `{data_dir}/terminal/backup/` |

Выводы с чужим текстом (UIA, OCR, терминал, скриншоты) помечены `untrusted = true`.
Свечение: T1+ шлют `ControlStart{tier}` → `ControlTarget{rect,label}` → `ControlStop` (guard `ControlSession`).

## Формат `{data_dir}/terminal/last.json`

```json
{"version":1,"shell":"powershell","shell_version":"5.1.26100","command":"git comit -m x","exit_code":1,
 "success":false,"error_text":null,"cwd":"C:\\proj","time":"2026-11-22T16:40:00+03:00",
 "time_unix_ms":1795354800123,"pid":1234,"history_id":42}
```

## Тесты

`cargo test` — только чтение и свои дочерние процессы (UIA панели задач, скриншот в память, OCR нарисованного
Arial текста, `terminal_run`). Тесты живого ввода в `tests/input_live.rs` помечены `#[ignore]`:
запускать вручную, когда за ПК никто не работает: `cargo test --test input_live -- --ignored --test-threads=1`.

## Ограничения

- Терминал VS Code (xterm.js) почти не отдаёт текст через UIA — используется OCR; хук PowerShell не видит
  stderr внешних программ (`git`), его добирает UIA/OCR, если команда видна на экране.
- cmd.exe без хука (нет PROMPT_COMMAND); bat-файл `terminal_run` трактует `%` по правилам bat.
- Профиль PowerShell не загрузится при политике `Restricted`.
