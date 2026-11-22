# jarvis-memory

Память и самообучение J.A.R.V.I.S. (план, раздел 6; сигнатуры ошибок и привычки пользователя — 3б).
Это накопление опыта в памяти, а **не** дообучение весов модели.

## Хранилище

`{корень проекта}/{Config.data_dir}/memory.db` (`Memory::open_for_config`), SQLite (bundled, WAL,
`busy_timeout` 5 с, запись в `BEGIN IMMEDIATE`). Одно соединение под `Mutex`; из async-кода — через
`Memory::run(|m| …)` (spawn_blocking) или `*_async`-обёртки. Миграции по `PRAGMA user_version`,
идемпотентны (повторная проверка версии под блокировкой записи, `IF NOT EXISTS`).

| Таблица | Назначение | FTS5 (стеммированные колонки) |
|---|---|---|
| `sessions`, `messages` | эпизоды диалогов | `messages_fts(s_content)` |
| `facts` | факты о пользователе: категория, confidence, источник explicit/inferred, important, created/updated/last_used, use_count | `facts_fts(s_text, s_category)` |
| `lessons` | память ошибок: signature, exact_signature, error_text, scope `tool/app/user_habits/general`, trigger, mistake, cause, fix, tool, app, seen, first/last_seen, confidence, success/fail_after_fix, is_stub | `lessons_fts(s_trigger, s_body, s_meta)` |
| `playbooks` | name (+ уникальный name_key), when, steps JSON `[{tool,args,note?}]`, params_schema, success/fail_count, version, promoted | `playbooks_fts(s_name, s_when, s_steps)` |
| `actions` | аудит: ts, session_id, tool, args JSON, tier `T0..T3`, status, result_summary, approved_by | — |

FTS5 — external content поверх основных таблиц, синхронизация триггерами, `unicode61 remove_diacritics 0`.

## Поиск и морфология

`text::stems`: lowercase, `ё→е`, слова с дефисом/подчёркиванием дают склейку и части
(`wi-fi` → `wifi wi fi`), стоп-слова, Snowball-стемминг (`rust-stemmers`, ru/en по алфавиту слова).
Одинаково для индекса и запроса; в запросе основы ≥3 символов префиксные (`"люб"*` находит «любл»).
«я люблю чёрный кофе» находится по «какой кофе я любил». Ограничение: супплетивные формы
(«пил» ↔ «пью») стеммер не связывает.

Дедупликация: кандидаты из FTS, похожесть — коэффициент Дайса по основам
(факты ≥0.8, уроки ≥0.6, плейбуки «имя + условия» ≥0.75).

## Сигнатуры ошибок

`error_signature(text)` → 16 hex (SHA-256). Нормализация убирает ANSI, пути Windows/Unix, URL, e-mail,
GUID, IP, даты/время, hex-адреса, числа, имена файлов, содержимое кавычек, эхо команд PowerShell и кадры
кода rustc. Коды-идентификаторы (`E0425`, `CS0103`, HRESULT `0x8…`, NTSTATUS `0xC…`) сохраняются.
`ErrorSig::of` даёт ещё **точную** сигнатуру (с содержимым кавычек): рецепт «gti → git» не применится
к «pyhton». `lesson_for_error(text)` → `LessonMatch { lesson, exact }`; `lesson_for_signature(sig)` —
мгновенный рецепт без LLM.

## Использование агентом

```rust
let mem = Memory::open_for_config(root, &cfg)?;
// перед ходом
let ctx = mem.retrieve_context_async(user_text, Some(tool), app).await?;
let block = ctx.render(DEFAULT_BUDGET_TOKENS);      // «Память: …», ≤1500 токенов
mem.mark_used_async(ctx.refs()).await?;
// перед инструментом
for l in mem.warnings_for_tool("powershell_run", app, 3)? { /* l.warning() → «В прошлый раз…» */ }
// ошибка инструмента
if let Some(m) = mem.lesson_for_error_async(err.clone()).await? { /* m.exact → применить m.lesson.fix */ }
mem.add_lesson_async(lesson_stub_from_tool_error(tool, app, &err)).await?;
// «нет, не так»
if let Some(stub) = lesson_stub_from_correction(&user_text, Some(&last_action)) { mem.add_lesson(stub)?; }
// после задачи
Reflector::new(reflection_model).reflect_and_apply(&mem, &TaskTranscript::from_chat(&history), cancel).await?;
// раз в ночь/неделю
mem.consolidate()?;
```

Рендер блока «Память:»: группы важные факты → уроки → прочие факты → плейбуки; сначала каждая группа
получает долю бюджета (35/40/10/15 %), затем остаток по приоритету; не влезающее обрезается с «…».
Оценка токенов — `символы / 3` (с запасом для кириллицы).

Рефлектор: русский промпт со строгой JSON-схемой, разбор терпит ```json-блоки, текст вокруг,
висячие запятые, `"80%"`; пустые/шаблонные поля (`...`, `string`, `<…>`) отбрасываются; выведенные
факты ≤0.9 confidence; уроки применяются через `refine_lesson` (уточнение заготовки не увеличивает `seen`),
ошибка упавшего шага подставляется в урок того же инструмента, чтобы он слился с заготовкой по сигнатуре.

Консолидация: слияние почти-дублей (факты, уроки — с защитой разных точных сигнатур, плейбуки с
одинаковыми шагами), затухание неиспользуемого >60 дней (×0.7 за период; явные и важные факты не
затухают), удаление затухших слабее 0.1 (кроме уроков, которые хоть раз помогли), повышение плейбуков
с ≥3 успехами и ≥80 % успеха, понижение провальных и давно неиспользуемых.

## Инструменты

`memory_tools(mem)`: `memory_remember` (T0, явный факт), `memory_recall` (T0, поиск),
`memory_forget` (T2: по id+kind или по запросу — удаляются только факты, покрывающие ≥60 % слов
запроса; иначе возвращаются кандидаты с id).

## Конкурентность и FTS5

Если тот же файл пишет другое соединение (другой процесс или второй `Memory::open`), кэш FTS5 в
читающем соединении может устареть (на SQLite 3.54.1 воспроизводилось как «fts5: corruption found
reading blob …» при целом файле). `Memory` перед каждой операцией сверяет `PRAGMA data_version`
и после чужих коммитов переоткрывает соединение. Рекомендуется один `Memory` на процесс (он `Clone`).

## Тесты

`cargo test` — базы только во временных каталогах (`tempfile`). Покрыто: морфология поиска,
≥15 пар сигнатур, дедуп/слияние уроков и фактов, версии плейбуков, бюджет рендера, идемпотентность
миграций, конкурентный доступ (задачи + два хэндла), consolidate, экспорт, рефлектор на мок-модели
(JSON в ```json и с мусором), инструменты, приёмка этапа 5 («запомни… → после перезапуска знает»).
