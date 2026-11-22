# jarvis-stt-rs — распознавание русской речи на Xeon

STT-сервис J.A.R.V.I.S.: Rust (edition 2024), axum + rustls, движок **sherpa-onnx v2.0.2** (официальный крейт
`sherpa-onnx` + готовая `linux-x64-shared-lib`), модель **GigaAM v3 CTC** (`sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16`).
Протокол — `crates/jarvis-core/src/stt_protocol.rs` (крейт подключён по пути, не меняется).

Рабочий экземпляр: **`https://192.168.50.38:8444`** (TLS тем же частным CA, что и TTS; без токена).

## Архитектура

```
ПК (Tauri) ──TLS──▶ 192.168.50.38:8444  jarvis-stt-rs-front.socket ─▶ systemd-socket-proxyd (DynamicUser)
                                                                       │ TCP
                                                                       ▼
                    127.0.0.1:8182  jarvis-stt-rs.service (User=jarvis-rust-tts, TLS сам, rustls/ring)
                      axum: GET /health · GET /metrics · POST /stt · WS /stt/stream
                      сессия WS ─ PcmDecoder (s16le, половинки сэмплов между кадрами) ─ Recognition
                      Engine (trait) ─ SherpaEngine: пул 2 × OfflineRecognizer GigaAM v3 CTC (по 4 потока ORT)
```

- `src/server.rs` — HTTP/WS, лимиты, таймауты, логи (`tracing`, тексты распознавания — только на уровне `debug`).
- `src/engine.rs` — трейт `Engine`/`Recognition` (блокирующие вызовы, сервер зовёт их в `spawn_blocking`) и `MockEngine` для тестов.
- `src/sherpa.rs` (feature `sherpa`) — sherpa-onnx: потоковые модели (T-one, streaming Zipformer) и офлайн
  (GigaAM v3 CTC/RNNT, offline Zipformer), пул экземпляров (`pool.rs`), прогрев при старте.
- `src/audio.rs` — PCM s16le, разбор WAV (PCM 8/16/24/32, float, EXTENSIBLE, стерео→моно), ресемплер windowed-sinc (Кайзер), шум для замеров.
- `src/text.rs` — нормализация и WER/CER; `src/config.rs` — переменные окружения; `src/metrics.rs` — `/metrics`.
- `src/bin/stt-bench.rs` — замер моделей; `bench/` — фразы, генерация звука через TTS, сырые результаты.

**Как получаются partial и final.** Выбранная схема — «симулированный стриминг» одной офлайн-моделью: пока идёт
фраза, каждые `JARVIS_STT_PARTIAL_MS` (300 мс) звука декодируется весь накопленный буфер → `partial`; по `end` —
весь буфер ещё раз → `final` (если буфер не вырос после последнего partial, берётся готовый текст). Пока идёт шаг
(0,1–0,3 с), кадры копятся в сокете, частота partial сама снижается под нагрузкой. Схема «потоковая модель для
partial + офлайн для final» тоже поддерживается (`JARVIS_STT_STREAMING=zipformer:…`), но проиграла по замерам.

## API (кратко; формат — `stt_protocol.rs`)

| Запрос | Ответ |
|---|---|
| `GET /health` | `{"ok":true,"model":"nemo-ctc-giga-am-v3-russian-2025-12-16","sample_rate":16000}` |
| `POST /stt` | тело `audio/L16; rate=16000` (little-endian, как в протоколе; `rate` 8000–48000 и `channels` пересчитываются) или WAV (`audio/wav`, либо по сигнатуре RIFF) → `{"text","duration_ms"}`. Ошибки JSON `{"error":…}`: 400 `empty_audio`, 413 `audio_too_long`/`body_too_large`, 415, 422 `invalid_audio`, 503 `busy`, 504 `decode_timeout` |
| `WS /stt/stream` | при подключении `ready{model}`; бинарные кадры PCM s16le 16 кГц любой длины → `partial{text}` при изменении текста; `{"type":"end"}` → `final{text,duration_ms}` и готовность к следующей фразе; `{"type":"reset"}` — сброс без ответа; неизвестный текст → `error`, соединение живо |
| `GET /metrics` | счётчики Prometheus: сессии, запросы, partial/final, ошибки, суммарная/максимальная задержка final |

`duration_ms` — длительность звука фразы. Фраза длиннее `JARVIS_STT_MAX_UTTERANCE_S` (30 с) получает `final` без
`end`, остаток идёт в следующую фразу. Сверх `JARVIS_STT_MAX_SESSIONS` WebSocket получает HTTP 503 до апгрейда.
Сервер шлёт ping раз в 30 с и закрывает соединение после 120 с без кадров от клиента (pong считается).

## Конфигурация (env)

| Переменная | По умолчанию | Смысл |
|---|---|---|
| `JARVIS_STT_BIND` | `127.0.0.1:8182` | адрес; не-loopback требует TLS |
| `JARVIS_TLS_CERT`, `JARVIS_TLS_KEY` | — | PEM; на Xeon — `/etc/jarvis-tts-rs/tls/server.{pem,key}` |
| `JARVIS_STT_MODEL_DIR` | `/opt/jarvis-stt-rs/models` | корень моделей |
| `JARVIS_STT_OFFLINE` | `gigaam-ctc:sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16` | `kind:каталог`, kind = `gigaam-ctc` \| `gigaam-rnnt` \| `zipformer` \| `none` |
| `JARVIS_STT_STREAMING` | `none` | `t-one:…` \| `zipformer:…` \| `none` |
| `JARVIS_STT_PARTIAL_MS` | `300` | partial из офлайн-модели (только без потоковой); `0` — без partial |
| `JARVIS_STT_THREADS` | `4` | потоков ONNX Runtime на экземпляр |
| `JARVIS_STT_POOL` | `2` | экземпляров каждой модели (параллельных декодирований) |
| `JARVIS_STT_MAX_SESSIONS` / `_MAX_REQUESTS` | `8` / `4` | одновременные WS / `POST /stt` |
| `JARVIS_STT_MAX_UTTERANCE_S`, `_IDLE_TIMEOUT_S`, `_PING_S`, `_QUEUE_TIMEOUT_S`, `_DECODE_TIMEOUT_S` | 30, 120, 30, 5, 20 | лимиты и таймауты, с |
| `RUST_LOG` | `info` | `debug` — с текстами распознавания |

## Выбор модели

Кандидаты из списка pretrained sherpa-onnx (релиз `asr-models`): потоковые **T-one** (`streaming-t-one-russian-2025-09-08`, 8 кГц),
**streaming Zipformer small ru (Vosk)** fp32/int8; офлайн **GigaAM v3** CTC, RNNT и RNNT с пунктуацией (`…-2025-12-16`), **Zipformer ru int8** (`2025-04-20`).

Методика: 20 команд (`bench/phrases.tsv`: «Джарвис, открой блокнот», «посмотри ошибку в терминале», «сколько будет
семь плюс пять», «вай-фай», числа, …; 92 слова) синтезированы нашим TTS (`bench/make-audio.sh`, 24 кГц), переведены в 16 кГц
нашим ресемплером, обрамлены тишиной 0,3 с + 0,8 с (как с VAD клиента); условия: чисто, белый шум SNR 10 и 5 дБ
(к уровню речи). Звук подаётся в реальном времени по «виртуальным часам» (`stt-bench`, см. шапку файла): `first` — от начала
речи до первого partial, `lag` — от конца речи до последнего изменения partial, `final` — от `end` до готового текста.
WER/CER — после нормализации (`text.rs`: регистр, пунктуация, «ё», числа цифрами → словами, «Wi-Fi»/«вай фай» → «вайфай»,
«D»/«де» → «дэ»; ошибки вида «джарвиз» не прощаются). Сырые таблицы — [`bench/results.md`](bench/results.md).

| Модель (режим, потоки) | WER чисто / SNR10 / SNR5 | CER чисто / 10 / 5 | first p50, мс | lag p50 / p95, мс | partial/с | final p50 / p95, мс | RTF |
|---|---|---|---|---|---|---|---|
| T-one, потоковая (2) | 12,0 / 15,2 / 20,7 % | 1,8 / 4,2 / 6,2 % | 957 | 928 / 1042 | 3,8 | 60 / 84 | 0,11 |
| Zipformer small ru fp32, потоковая (2) | 12,0 / 21,7 / 26,1 % | 2,2 / 4,2 / 6,8 % | 430 | 515 / 931 | 2,3 | 30 / 58 | 0,05 |
| Zipformer small ru int8, потоковая (2) | 13,0 / 23,9 / 25,0 % | 3,6 / 4,4 / 6,4 % | 426 | 513 / 929 | 2,4 | 28 / 53 | 0,05 |
| Zipformer ru int8, офлайн (4) | 10,9 / 10,9 / 13,0 % | 1,6 / 2,6 / 4,8 % | — | — | — | 85 / 103 | 0,03 |
| GigaAM v3 RNNT + пунктуация, офлайн (4) | 4,3 / 12,0 / 15,2 % | 2,8 / 5,6 / 6,8 % | — | — | — | 236 / 280 | 0,08 |
| GigaAM v3 RNNT, офлайн (4) | 4,3 / 8,7 / 12,0 % | 0,8 / 2,6 / 3,6 % | — | — | — | 227 / 272 | 0,08 |
| GigaAM v3 CTC, офлайн (4) | 4,3 / 9,8 / **8,7 %** | 0,8 / **1,6 / 2,4 %** | — | — | — | 213 / 249 | 0,07 |
| GigaAM v3 RNNT, partial каждые 300 мс (4) | 4,3 / 8,7 / — | 0,8 / 2,6 / — | 261 | 104 / 590 | 3,4 | 301 / 461 | 0,50 |
| **GigaAM v3 CTC, partial каждые 300 мс (4)** — выбрано | **4,3 / 9,8 / —** | **0,8 / 1,6 / —** | **256** | **148 / 863** | **3,6** | **250 / 428** | 0,46 |
| GigaAM v3 CTC, partial каждые 300 мс (6) | 4,3 / 9,8 / — | 0,8 / 1,6 / — | 248 | 123 / 815 | 3,6 | 197 / 349 | 0,38 |

WER partial (последний partial перед `end`) у выбранной схемы — 5,4 % / 8,7 % против 12–26 % у потоковых моделей.
С ПК по сети (развёрнутый сервис, куски 40 мс в реальном времени, `remote_latency_report`): **WER 3,3 %, CER 1,2 %,
final p50 253 / p95 382 мс после `end`**, lag конца фразы p50 −136 / p95 167 мс (partial обычно готов раньше, чем
кончилась речь), 2,7 partial/с, первый partial ≈ 0,39 с от начала звука; `POST /stt` фразы ~2 с — 141–178 мс.

Почему так:
- Потоковые модели заметно хуже (WER 12–26 % против 4–10 %) и медленнее догоняют речь: T-one ~0,9 с, Zipformer ~0,5 с — оба не укладываются в «partial ≤ 300 мс». Комбинация «потоковая для partial + GigaAM для final» дала бы точный final, но partial с отставанием 0,5 с и ошибками в каждом четвёртом слове на шуме.
- GigaAM v3 CTC и RNNT по WER на чистом звуке равны; CTC устойчивее к шуму (CER 1,6/2,4 % против 2,6/3,6 %) и немного быстрее. Вариант с пунктуацией пишет английские слова латиницей с ошибками («Bluetoot», «Hrom») — CER выше.
- Симулированный стриминг на GigaAM CTC укладывается: partial ~3,6 раза в секунду, lag p50 ~0,15 с, final p95 < 0,5 с. Цена — CPU: пока человек говорит, сессия занимает в среднем ~2 ядра (RTF 0,46 при 4 потоках); в простое 0 %.
- Пул 2 × 4 потока при `CPUQuota=600%`: одна речь — полная скорость, две одновременно — квота делится, задержки растут, но не падают.

Повторить замер на Xeon (модели — в `/opt/jarvis-stt-rs/bench/models`):

```sh
bench/make-audio.sh bench/phrases.tsv /opt/jarvis-stt-rs/bench/audio
/opt/jarvis-stt-rs/bin/stt-bench --streaming zipformer:sherpa-onnx-streaming-zipformer-small-ru-vosk-2025-08-16 \
  --offline gigaam-ctc:sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16 --threads 4 --snr 10 --snr 5 --sim-ms 300
```

## Сборка и тесты

Windows/любая ОС, без движка (протокол, WAV/PCM, ресемплинг, WebSocket-логика на `MockEngine`):

```powershell
cd server\jarvis-stt-rs
cargo test                     # 17 юнит-тестов + 3 теста сервера
cargo test --test remote -- --ignored --nocapture --test-threads=1   # проверка развёрнутого сервиса с ПК
```

`tests/remote.rs`: health, `POST /stt` (L16 и WAV 24 кГц) со звуком от TTS, WebSocket с partial/final в реальном
времени (две фразы в одном соединении), отчёт по всем 20 фразам. TLS — через CA `src-tauri/config/xeon-tts-ca.pem`
(переопределяется `JARVIS_STT_CA`, адреса — `JARVIS_STT_URL`, `JARVIS_TTS_URL`).

Linux (Xeon), с движком: `deploy/build-on-xeon.sh [--install]` — toolchain `/opt/jarvis-tts-rs/build-tools`
(только чтение; свои `CARGO_HOME`/target в `/opt/jarvis-stt-rs/build-tools`), `SHERPA_ONNX_LIB_DIR=/opt/jarvis-stt-rs/lib`,
`-C target-cpu=haswell`. `--install` кладёт бинари в `/opt/jarvis-stt-rs/bin` (прежние → `*.prev`) и через patchelf
(0.18.0, `/opt/jarvis-stt-rs/build-tools/patchelf`) оставляет rpath только `$ORIGIN/../lib` — gcc из conda дописывает путь к своему env.

## Деплой на Xeon (как сделано)

Раскладка: `/opt/jarvis-stt-rs/bin` (root, 0755), `lib` (`libsherpa-onnx-c-api.so`, `libonnxruntime.so` из
`sherpa-onnx-v2.0.2-linux-x64-shared-lib.tar.bz2`), `models` (root:jarvis-rust-tts, 0750/0640), `src` (копия исходников
для сборки), `build-tools`, `bench` (кандидаты и звук, ~1,8 ГБ — можно удалить), `backup/before-deploy-*` (состояние до деплоя:
список `/etc/systemd/system`, unit-файлы jarvis, слушающие порты; STT-юнитов до деплоя не было, 8182/8444 были свободны).
Существующие файлы TTS не менялись.

```sh
# исходники: server/jarvis-stt-rs и crates/jarvis-core → /opt/jarvis-stt-rs/src/{server/jarvis-stt-rs,crates/jarvis-core}
R=https://github.com/k2-fsa/sherpa-onnx/releases/download
wget $R/v2.0.2/sherpa-onnx-v2.0.2-linux-x64-shared-lib.tar.bz2      # bzip2 есть в /opt/jarvis-tts-rs/build-tools/env/bin
wget $R/asr-models/sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16.tar.bz2
# распаковать: .so → /opt/jarvis-stt-rs/lib, модель → /opt/jarvis-stt-rs/models; права как выше
cd /opt/jarvis-stt-rs/src/server/jarvis-stt-rs && deploy/build-on-xeon.sh --install
install -o root -g root -m 0644 deploy/jarvis-stt-rs.service deploy/jarvis-stt-rs-front.service \
  deploy/jarvis-stt-rs-front.socket /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now jarvis-stt-rs.service jarvis-stt-rs-front.socket
```

Юниты (копии в `deploy/`): `jarvis-stt-rs.service` — `Type=notify` (READY после открытия сокета, загрузка ~3–6 с),
`User/Group=jarvis-rust-tts` (читает TLS-ключ TTS), `127.0.0.1:8182`, `CPUQuota=600%`, `MemoryMax=3G` (`MemoryHigh=2560M`,
факт ~0,6 ГБ), `TasksMax=128`, `NoNewPrivileges`, `PrivateTmp`, `ProtectHome`, `ProtectSystem=strict`, `Restart=on-failure`;
`jarvis-stt-rs-front.socket` — `ListenStream=192.168.50.38:8444`, `FreeBind=true`; `jarvis-stt-rs-front.service` —
`systemd-socket-proxyd 127.0.0.1:8182`, `DynamicUser=yes`. Оба включены на загрузку.

Проверка: `journalctl -u jarvis-stt-rs -f`, `curl --cacert ca.pem https://192.168.50.38:8444/health`, `/metrics`,
и что TTS жив: `curl --cacert ca.pem https://192.168.50.38:8443/ready` → 200.

## Обновление

- Код: скопировать исходники в `/opt/jarvis-stt-rs/src/…`, `deploy/build-on-xeon.sh --install`, `systemctl restart jarvis-stt-rs`
  (front перезапустится сам). Вернуть прежний бинарь: `mv /opt/jarvis-stt-rs/bin/jarvis-stt-rs.prev /opt/jarvis-stt-rs/bin/jarvis-stt-rs && systemctl restart jarvis-stt-rs`.
- Модель/параметры: распаковать в `models/` (права root:jarvis-rust-tts 0750/0640), `systemctl edit jarvis-stt-rs` →
  `[Service]` `Environment=JARVIS_STT_OFFLINE=…`, рестарт. Откат — `systemctl revert jarvis-stt-rs`.
- sherpa-onnx: версия `.so` в `lib/` обязана совпадать с крейтом (`=2.0.2` в Cargo.toml) — менять вместе и пересобирать.

## Откат (вернуть сервер как было)

```sh
systemctl disable --now jarvis-stt-rs-front.socket jarvis-stt-rs-front.service jarvis-stt-rs.service
rm -f /etc/systemd/system/jarvis-stt-rs.service /etc/systemd/system/jarvis-stt-rs-front.service \
      /etc/systemd/system/jarvis-stt-rs-front.socket
rm -rf /etc/systemd/system/jarvis-stt-rs.service.d     # если создавались drop-in
systemctl daemon-reload && systemctl reset-failed
rm -rf /opt/jarvis-stt-rs                               # бинари, библиотеки, модели, toolchain-кэш, замеры, бэкап
ss -ltn | grep -E ':8182|:8444' || echo "порты свободны"
curl --cacert /etc/jarvis-tts-rs/tls/ca.pem https://192.168.50.38:8443/ready   # TTS не затрагивался
```

Пользователь `jarvis-rust-tts`, TLS-файлы `/etc/jarvis-tts-rs/tls/*` и юниты TTS принадлежат TTS — их не трогать.

## Риски и ограничения

- Замер — на синтетическом голосе нашего TTS и белом шуме; живой микрофон, эхо динамиков и реальный шум дадут WER выше. Перепроверить на записях пользователя.
- «Джарвис» иногда распознаётся как «джарвиз» (1–2 из 20) — фильтр wake word на клиенте должен быть нечётким.
- Нет пунктуации и регистра, числа — словами («двести сорок три»); английские названия — русскими буквами («блютуз», «хром»).
- CPU: ~2 ядра на говорящую сессию (re-decode буфера каждые 300 мс, сложность растёт с длиной фразы); на фразах 10–30 с partial реже, final дольше (~0,07 с на секунду звука при 4 потоках). Несколько одновременных говорящих делят `CPUQuota=600%`; TTS живёт рядом (его квота 1000%) — при одновременном синтезе и распознавании оба замедляются (проверено: TTS 200, первый байт 0,1–0,33 с во время WS-потока).
- Сервис без токена: любой в LAN может распознавать речь (как и TTS, решение пользователя).
- Бинарь и `.so` — prebuilt sherpa-onnx/onnxruntime с GitHub (sha256 `.so`: c-api `f6a739c0…ba1aa`, onnxruntime `4b3607ae…e566e`).
