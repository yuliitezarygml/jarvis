# jarvis-llm

LLM-провайдеры J.A.R.V.I.S. (план, раздел 2). Опирается на контракты `jarvis-core` (`ChatModel`, `ChatRequest`, `LlmError`, `ProviderConfig`, …).

## Что внутри

| Модуль | Назначение |
|---|---|
| `presets` + `presets/providers.json` | 16 пресетов: groq, opencode-zen, openrouter, openai, anthropic, gemini, mistral, deepseek, together, fireworks, cerebras, xai, zai, ollama, lmstudio, custom. Встроены через `include_str!`; `Presets::load_or_builtin(path)` берёт файл без перекомпиляции |
| `catalog` + `presets/model_catalog.json` | семейства по glob-шаблонам id → quality 1–5, tools, vision, reasoning, ctx, speed; фильтр нечатовых (embedding, whisper, tts, guard, image, rerank, audio…) |
| `openai::OpenAiCompatClient` | `/chat/completions`: обычный ответ и SSE, tool_calls из дельт по `index`, картинки как data URL, вырезание `<think>` |
| `anthropic::AnthropicClient` | Messages API: system отдельно, tool_use/tool_result, картинки, SSE |
| `discovery` | `validate_key`, `list_models`, `list_models_cached` (24 ч, `{root}/{data_dir}/cache/models-<id>.json`) |
| `probe` | `probe`/`probe_many`: TTFT и ток/с на русском стриме, вызов `get_time`, цифра 7 на PNG; не больше 3 параллельно, 15 с на шаг, повтор после 429 |
| `rank` | `rank(models, probes, ModelRole)` → `Recommendation` с подписью «tools ✓ · русский ✓ · 0,4 с · 128k» |
| `chain` | `ModelChain` (failover + circuit breaker 3 сбоя → 60 с), `build_role_model(&Config, ModelRole)` |
| `report` | `check_provider` для экрана настроек, `apply_best` («выбрать лучшее автоматически») |

## Быстрый старт

```rust
use jarvis_llm::{build_role_model, check_provider, ModelRole};

let cfg = jarvis_core::Config::load(r"d:\jarvis\config.json")?;
let brain = build_role_model(&cfg, ModelRole::Brain)?;   // Arc<dyn ChatModel>, цепочка с failover
let report = check_provider(cfg.provider("groq").unwrap()).await;
```

Корень проекта для кэша задаётся в `BuildOptions::root` / `ModelCache::for_config(root, &cfg)`. Если он не задан, берётся `JARVIS_ROOT` или ближайший каталог вверх с `config.json`.

## Особенности провайдеров (проверено 2026-11-22)

- **OpenCode Zen**: base `https://opencode.ai/inference/openai/v1`. Модели `claude-*` и `qwen3.5-plus`/`qwen3.6-plus`/`qwen3.8-flash` идут на `https://opencode.ai/inference/anthropic/v1/messages` с `Authorization: Bearer`. Маршрут берётся из поля `api` в `/models` (принимается только с тем же хостом) или из пресета. Модели с родным Responses или Gemini API вызываются через `/chat/completions`: по документации Zen переводит запросы сам. Бесплатные модели (`*-free`, `big-pickle`) работают только из клиента OpenCode, поэтому в живую проверку не попадают.
- **OpenRouter**: `/models` публичный, поэтому ключ проверяется через `/key`.
- **Gemini и xAI**: на неверный ключ отвечают **400**, клиент распознаёт это как `Auth`.
- **Anthropic**: `x-api-key` + `anthropic-version: 2023-06-01`. Для версии 2026-10-21 и новее `temperature` не отправляется.
- **Z.AI**: `/models` в документации не описан. Если он отвечает 404, берутся `fallback_models`, а ключ проверяется минимальным чатом.
- Ключ вырезается из текстов ошибок (`mask_key`). `Debug` клиентов ключ не печатает. В кэше ключа нет.

## Тесты

```
cargo test                                         # мок-сервер axum, без сети
cargo test --test live -- --ignored --nocapture    # живые: ключи groq / opencode-zen из d:\jarvis\config.json
```

Без ключа живой тест печатает «нет ключа» и проходит.
