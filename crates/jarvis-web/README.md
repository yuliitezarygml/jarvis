# jarvis-web

Интернет для J.A.R.V.I.S. (план, разделы 3в и 3г): поиск, чтение страниц и исследование с источниками.

## Что умеет

| Часть | Что делает |
|---|---|
| `search` | trait `SearchProvider` + DuckDuckGo HTML (без ключа), Brave, Tavily, Serper, Google Programmable Search, Jina Search; `SearchChain` с failover в порядке `Config.web_search`; `validate()` для кнопки «Проверить» |
| `fetch` | `Fetcher::fetch(url) -> Page`: reqwest + rustls, UA `JarvisBot/1.0`, таймаут 15 с, тело до 2 МБ (PDF до 10 МБ), кодировка, readability, HTML → Markdown, PDF → текст, лимит 48 000 символов (~12k токенов) с пометкой обрезки, запасной Jina Reader для JS-страниц |
| `research` | `Researcher::research(question, &dyn ChatModel, cancel)`: план из 2–4 запросов (строгий JSON) → поиск → 3–5 страниц с разных доменов → параллельное чтение → синтез по-русски со ссылками [1], [2] |
| `tools` | `web_search` (T0), `web_fetch` (T0, `untrusted`), `web_research` (T0, `untrusted`), `web_open` (T1, через внедряемый `Opener`) |

## Подключение

```rust
use std::sync::Arc;
use jarvis_web::{SystemOpener, WebStack};

let stack = WebStack::from_config(&config);                       // цепочка поиска + Fetcher
registry.extend(stack.tools(brain_model.clone(), Arc::new(SystemOpener)));
```

Кнопка «Проверить» в настройках:

```rust
let report = jarvis_web::validate_config(&cfg.web_search[i], &jarvis_web::http::default_client()).await;
report.summary_ru(); // «работает ✓ (8 результатов, 0,6 с)» или «не работает ✗: ключ не принят: …»
```

## Поисковики (`config.json` → `web_search[]`)

| `id` | Ключ | Запрос (по официальной документации) |
|---|---|---|
| `duckduckgo` | не нужен | `POST https://html.duckduckgo.com/html/` (форма `q`, `b`, `kl`), разбор `div.result`, `a.result__a`, `.result__snippet`; ссылки `//duckduckgo.com/l/?uddg=…` раскрываются; реклама (`result--ad`, `y.js`) отбрасывается; капча (`anomaly-modal`, HTTP 202) → `SearchError::Blocked` |
| `brave` | `api_key` | `GET https://api.search.brave.com/res/v1/web/search?q=&count=` (≤ 20), `X-Subscription-Token`; `web.results[] {title, url, description}` |
| `tavily` | `api_key` | `POST https://api.tavily.com/search`, `Authorization: Bearer`, `{query, max_results (≤ 20), search_depth: "basic"}`; `results[] {title, url, content}` |
| `serper` | `api_key` | `POST https://google.serper.dev/search`, `X-API-KEY`, `{q, num}`; `organic[] {title, link, snippet}` |
| `google_cse` | `api_key` + `extra` = cx | `GET https://customsearch.googleapis.com/customsearch/v1?key=&cx=&q=&num=` (≤ 10); `items[] {title, link, snippet}` |
| `jina` | необязателен | `GET https://s.jina.ai/<запрос>`, `Accept: application/json`, `X-Respond-With: no-content`; `data[] {title, url, description}` |

`SearchChain::from_config` пропускает выключенные записи и записи без обязательного ключа (`skipped()` объясняет почему);
если не осталось ни одной — подставляет DuckDuckGo. Ошибка или пустая выдача → следующий провайдер; итог `AllFailed`
перечисляет причины по каждому.

## Защита от prompt injection (3г)

- Содержимое страниц и сниппеты — данные: `web_search`, `web_fetch`, `web_research` возвращают `ToolOutput { untrusted: true }`,
  агент оборачивает их в `<untrusted>`.
- В `research` каждая страница (и заголовок, и текст) идёт в синтез через `jarvis_core::wrap_untrusted`; вопрос пользователя — снаружи.
  Системный промпт синтеза явно запрещает выполнять инструкции из источников и просит предупредить о подозрительных.
- План и синтез вызываются **без инструментов** (`ChatRequest.tools` пуст); `tool_calls` в ответе модели игнорируются.
- `web_fetch` не ходит во внутреннюю сеть: IP-литералы (127/8, 10/8, 172.16/12, 192.168/16, link-local, CGNAT, ULA…),
  `localhost`/`*.local`/`*.lan`, все шаги редиректов и DNS-ответы (свой резолвер) проверяются. Только `http`/`https`.
- `web_open` открывает только `http`/`https` (не `file:`, `javascript:`, `ms-settings:`).
- Jina Reader (адрес страницы уходит в Jina) включается, только если пользователь сам включил `jina` в `web_search`.

## Крейты

| Крейт | Зачем |
|---|---|
| `reqwest 0.13` (`rustls-no-provider`) + `rustls 0.23` (`ring`) | HTTP; провайдер ring как в `jarvis-tts-rs` — без `aws-lc-sys` (cmake/NASM на Windows); сертификаты из хранилища Windows (`rustls-platform-verifier`) |
| `dom_smoothie 0.20` | порт Mozilla Readability (выделение основного текста), MSRV 1.85 |
| `dom_query 0.30` | тот же DOM (html5ever), что у dom_smoothie: разбор выдачи DuckDuckGo и сниппетов без второго парсера |
| `htmd 0.6` | HTML → Markdown (по мотивам turndown.js), тот же html5ever |
| `pdf-extract 0.12` | PDF → текст (на lopdf); паники перехватываются |
| `encoding_rs 0.8` | кодировки (windows-1251, koi8-r…), правила WHATWG |
| `open 5` | браузер по умолчанию (`ShellExecuteExW`) в `SystemOpener` |
| `wiremock 0.6` (dev) | мок-сервер для провайдеров и страниц |

## Тесты

```
cargo test                                         # всё без сети
cargo test --test live -- --ignored --nocapture    # живые: DuckDuckGo «rust programming language», fetch rust-lang.org
```

Фикстуры (`tests/fixtures`): `ddg_rust.html` и `ddg_anomaly.html` — реальные ответы html.duckduckgo.com (2026-11-22),
`ddg_legacy_uddg.html` — старая разметка с редиректами `uddg` и рекламой, `article.html`, `docs_page.html`, `js_app.html`,
`injection.html`, JSON-ответы Brave/Tavily/Serper/Google/Jina в формате их документации. Крошечный PDF собирается в тесте.

## Ограничения

- DuckDuckGo HTML на UA `JarvisBot/1.0` отдаёт капчу, поэтому только для него используется браузерный UA; при частых
  запросах капча возможна и с ним (`Blocked` → следующий в цепочке).
- Google Custom Search JSON API закрыт для новых клиентов, у существующих работает до 2027-01-01 — оставлен ради тех, у кого есть ключ.
- Headless Chrome (CDP) для тяжёлых JS-страниц не реализован — только Jina Reader.
- «Родной» поиск LLM (Groq browser_search, OpenAI/Anthropic/Gemini web search) — не здесь, это часть адаптеров LLM.
- Защита от внутренней сети при системном HTTPS-прокси: имя резолвит прокси, поэтому работает только проверка IP-литералов;
  если прокси задан именем (`localhost:7890`), чтение страниц будет отклонено — тогда нужен `allow_private_hosts`.
