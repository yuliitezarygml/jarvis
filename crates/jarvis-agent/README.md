# jarvis-agent

«Мозг» J.A.R.V.I.S.: цикл агента «наблюдай → действуй → проверь» (`docs/PLAN.md`, раздел 3), политика
подтверждений T0–T3, защита от недоверенных данных (3г), события свечения и ленты действий (4),
блок «Память» в промпте (6) и краткосрочная память диалога.

Зависит только от `jarvis-core`. Всё остальное внешнее (голос, HUD, память, редактор секретов) подключается
через трейты из `hooks`, реализации подставляет интегратор.

## Подключение

```rust
use std::sync::Arc;
use jarvis_agent::{Agent, AgentConfig};

let agent = Agent::builder(brain_model /* Arc<dyn ChatModel> */, Arc::new(registry))
    .config(AgentConfig::from_core(&core_config))       // autonomy + vision_enabled из config.json
    .events(tauri_sink)                                 // Arc<dyn EventSink>
    .confirmer(voice_hud_confirmer)                     // Arc<dyn Confirmer>
    .memory(memory_hooks)                               // Arc<dyn MemoryHooks> (jarvis-memory)
    .speech(tts_sink)                                   // Arc<dyn SpeechSink> (нормализатор → TTS)
    .redactor(Arc::new(|s: &str| jarvis_sys::redact_secrets(s)))
    .protected_paths(Arc::new(|tool: &str, args: &serde_json::Value| /* C:\Windows, Program Files */ false))
    .build();

let outcome = agent.run("Джарвис, открой блокнот", cancel_token.clone()).await;
// outcome.final_text, outcome.steps, outcome.transcript (JSON для рефлектора), outcome.error, outcome.stop_reason
```

Значения по умолчанию безопасные: подтверждение — всегда «нет» (`FixedConfirmer(false)`, fail-closed),
память и речь выключены, редактор — тождество, защищённых путей нет, события — в `NullSink`.

`run(&self)` можно звать из разных задач: запуски выполняются по очереди. Для барж-ина сначала отмените
токен текущего запуска, затем запускайте новый — старый завершится сразу.
Будущее `run` — `Send`, его можно отдать в `tokio::spawn` через `Arc<Agent>`.

## Публичный API

| Что | Назначение |
|---|---|
| `Agent`, `AgentBuilder` | `builder(model, tools)`, `run(text, cancel) -> TaskOutcome`, `config()/set_config()`, `clear_history()`, `history_len()`, `tools()` |
| `TaskOutcome` | `final_text`, `steps` (число обращений к модели), `transcript`, `error`, `stop_reason` |
| `AgentConfig` | `max_steps` 25 · `tool_timeout` 30 с · `tool_timeouts` (`powershell_run`/`cmd_run`/`terminal_run` 60 с, `web_research` 90 с) · `autonomy` · `vision_enabled` · `history_budget_tokens` 12 000 · `max_history_turns` 40 · `max_tool_output_chars` 8 000 · `loop_repeat_limit` 3 · `persona` · `temperature`/`max_tokens`; `from_core(&Config)`, `timeout_for(name)` |
| `Confirmer` | `async confirm(summary, tier, protected_path) -> bool`; `FixedConfirmer(bool)` |
| `MemoryHooks` | `async context(query, tool_hint) -> String`, `known_fix(error) -> Option<String>`, `on_tool_error(tool, args, error)`, `on_task_done(&Transcript)`; `NoopMemory` |
| `SpeechSink` | `delta(&str)`, `flush()`, `cancel()` (по умолчанию пусто); `NullSpeech` |
| `Redactor` | `Arc<dyn Fn(&str) -> String>`; `identity_redactor()` |
| `ProtectedPathCheck` | `Arc<dyn Fn(&str, &Value) -> bool>`; `no_protected_paths()` |
| `Transcript`, `TranscriptStep`, `StepStatus`, `StopReason` | протокол задачи (serde JSON, `to_json()`) |
| `PERSONA_PROMPT`, `build_system_prompt` | промпт персоны и сборка системного промпта с блоком «Память» |
| `CANCELLED_TEXT`, `DENIED_TEXT`, `KNOWN_FIX_PREFIX`, `VISION_OFF_MARK`, `humanize_llm_error`, `estimate_tokens` | константы и утилиты |
| `text::*` | `truncate_output`, `summarize_call`, `canonical_json`, `redact_value`, `preview` |

## Как работает шаг

1. `State{thinking}` → блок «Память» (`MemoryHooks::context(запрос, последний инструмент)`, кэшируется, пока
   инструмент не сменился) → системный промпт → история в бюджете → `chat_stream` с `tools = registry.specs()`.
2. Текстовые дельты → `SpeechSink::delta` + `Subtitle{jarvis, накопленный текст, is_final:false}`, `State{speaking}`;
   конец сообщения → `flush()` + финальный субтитр.
3. На каждый `ToolCall` (по порядку):
   - **зацикливание**: тот же вызов (имя + аргументы с сортированными ключами) 3-й раз подряд **не выполняется**;
     модели — ответ «смени подход» и системная заметка, в transcript — заметка и шаг `loop_blocked`;
   - **нет инструмента** → модели текст ошибки со списком доступных, `Action{failed}`;
   - `tier = tool.tier_for(args)`; если `requires_confirmation(tier, autonomy)` **или** защищённый путь
     (даже при `Autonomy::Never`) → `Action{awaiting_confirm}`, `flush()` речи, `Confirmer::confirm`;
     отказ → инструмент не вызывается, модели `DENIED_TEXT`, `Action{cancelled}`;
   - `tier.glows()` → `ControlStart{tier}` + `State{controlling}` … `ControlStop` (и при ошибке/таймауте);
   - `Action{running}` → `done` / `failed` / `cancelled`; вызов под `timeout_for(name)` и `ctx.cancel`
     (дочерний токен, отменяется и по таймауту);
   - результат: `untrusted` → `Redactor` → обрезка (¾ начала + ¼ конца, пометка) → `wrap_untrusted(имя)`;
     доверенный — только обрезка; пустой → «(выполнено, вывода нет)»;
   - картинки: при `vision_enabled` — в отдельное user-сообщение сразу после всех tool-ответов шага
     (в контексте остаются только картинки последнего шага); иначе пометка `VISION_OFF_MARK`;
   - ошибка: `known_fix` → (если есть) «Известное решение из памяти: …» в ответ модели → `on_tool_error`
     (аргументы и текст ошибки уже через `Redactor`).
4. Финалы: ответ без вызовов → `State{idle}`; **отмена** (во время модели, подтверждения или инструмента) →
   немедленный выход, `SpeechSink::cancel()`, `ControlStop` (всегда), субтитр `CANCELLED_TEXT` (не озвучивается —
   интегратор может сказать `final_text`), `State{idle}`; **лимит шагов** → озвученная фраза о лимите, `idle`;
   **ошибка модели** → `Event::Error`, озвученное «Прошу прощения, сэр: …», `State{error}`, `outcome.error`.
   В любом случае ход сохраняется в историю, затем `on_task_done(&transcript)`.

`ConfirmRequest`/`ConfirmResolved` агент **не** шлёт: это делает реализация `Confirmer` (она же держит таймаут
20 с, принимает «да» только с микрофона после окончания TTS и при `protected_path` спрашивает повторно/строже).

## Краткосрочная память

Бюджет `history_budget_tokens` (оценка «символы/4», картинка ≈1000) на все сообщения запроса:
системный промпт — всегда; текущая задача — всегда (если не влезает, сворачиваются результаты инструментов её
прошлых шагов); прошлые ходы — от новых к старым: сначала компактно (результаты инструментов свёрнуты до
превью 200 симв., недоверенное превью снова в `<untrusted>`, картинки и заметки убраны), затем минимально
(запрос + ответ); первый не влезший ход обрывает историю. Незакрытые вызовы при отмене закрываются ответом
«Отменено пользователем», поэтому история всегда валидна для провайдеров.

## Тесты

`cargo test` — 17 модульных + 29 сценарных (`tests/agent_loop.rs`, мок-модель со сценарием, фиктивные
инструменты всех tier, `CollectSink`): простой ответ, цепочка инструментов, T3 отказ/согласие, tier от аргументов,
`Never`/`AskAllChanges`/`AskDangerous`, свечение T1 и его отсутствие для T0, `<untrusted>` + redactor, картинки при
зрении вкл/выкл, зацикливание и сброс счётчика, таймаут и переопределение для `powershell_run` (виртуальное время),
отмена во время инструмента/модели/подтверждения, max_steps, known_fix и блок «Память», неизвестный инструмент,
ошибка модели, защищённый путь, обрезка вывода, история и её обрезка, fail-closed по умолчанию, `Send`.

## Ограничения и риски

- Оценка токенов грубая; описания инструментов в бюджет не входят — бюджет по умолчанию взят с запасом.
- Системная заметка о зацикливании — сообщение `Role::System` в середине диалога; адаптер Anthropic должен
  перенести её в `system` или превратить в текст user-сообщения. Картинки идут отдельным user-сообщением
  после tool-ответов — адаптер Anthropic склеит их в одно user-сообщение с `tool_result`.
- Блокировка 3-го одинакового вызова мешает законным повторам (например, трижды «прокрутить вниз») —
  модель должна менять аргументы.
- Параллельные tool calls одного сообщения выполняются последовательно.
- Блок «Память» не оборачивается в `<untrusted>`: `jarvis-memory` должна сама не пускать туда команды из
  недоверенных источников (рефлектор видел страницы и вывод терминала).
- Инструмент, игнорирующий `ctx.cancel`, после таймаута/отмены может продолжать работу в фоне.
- «Проверка» результата в цикле делается моделью (по промпту), кодом не навязывается.
