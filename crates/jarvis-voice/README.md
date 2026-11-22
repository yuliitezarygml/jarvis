# jarvis-voice

Голос J.A.R.V.I.S. (клиентская часть). Rust 2024, свой target (`[workspace]` пустой).

## Модули
- `tts` — `TtsClient` (reqwest + rustls/ring, доверие только CA из `tts.ca_path`): `ready()`, `synthesize_stream(text, voice, cancel) -> TtsStream { request_id, sample_rate, impl Stream<Item=Result<Bytes>> }`, `cancel(id)`. Отмена токеном или брошенный поток → `DELETE /tts/{id}` автоматически. Трейт `TtsEngine`.
- `normalizer` — `SpeechNormalizer::normalize()`: числа со склонением и согласованием, порядковые, даты, время, десятичные, проценты, валюты, единицы, Markdown, сокращения, эмодзи, ссылки.
- `chunker` — `PhraseChunker::push(delta)/flush()`, фразы ≤260 символов; `split_long()`.
- `stt` — трейт `SttEngine`; `ServerStt` (WS `/stt/stream` с переподключением и досылкой буфера фразы, `POST /stt`, `/health`); `GroqWhisperStt` (multipart `/audio/transcriptions`, `whisper-large-v3-turbo`, `language=ru`).
- `vad` — `Vad` (earshot, кадры 256 сэмплов/16 мс, pre-roll 300 мс, тишина конца 800–1200 мс, мин. длительность), `EnergyScorer`.
- `echo` — `EchoGuard` (полу-дуплекс + хвост 300 мс, фильтр слов ≥70 %), трейт `PlaybackMonitor`.
- `wake` — `WakeFilter::classify()` → `Intent::{Command, WakeOnly, Stop, Confirm, Deny, Ignored}`.
- `pcm`, `resample` — s16le, WAV, RMS/`LevelMeter`, `FrameAssembler`, `StreamResampler` (rubato FFT).
- `tls` — закреплённый CA, `default_project_root()` (`JARVIS_ROOT` или поиск `config.example.json`).

## Не сделано
`Player` и `Capture` (cpal) и `VoicePipeline` — не реализованы (лимит времени). API cpal 0.18 проверен (Stream: Send, `device.description()`, `build_output_stream(config, cb, err, timeout)`).

## Тесты
- `cargo test` — юнит-тесты, таблица нормализатора (≥100 кейсов), моки TTS.
- `cargo test --test tts_live -- --ignored --nocapture` — живой TTS (только HTTP, звук не играет).
