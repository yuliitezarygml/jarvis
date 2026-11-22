# jarvis-voice

Голос J.A.R.V.I.S. (клиентская часть): микрофон → VAD → STT → фильтр «Джарвис» → команда;
ответ LLM → нормализатор → фразы → TTS → динамики, с барж-ином и защитой от эха.
Rust 2024, свой target (пустая таблица `[workspace]`), зависит только от `jarvis-core`.

## Модули и главный API
| Модуль | Что | Главное |
|---|---|---|
| `tts` | клиент `jarvis-tts-rs` (Xeon :8443) | `TtsClient::new(&TtsConfig, root)`, `ready()`, `synthesize_stream(text, voice, cancel) -> TtsStream { request_id, sample_rate, Stream<Result<Bytes>> }`, `cancel(id)`; трейт `TtsEngine` |
| `stt` | распознавание | трейт `SttEngine { transcribe(pcm, &cancel), open_stream(events) -> Option<SttStream> }`; `ServerStt` (WS `/stt/stream` + `POST /stt` + `/health`), `GroqWhisperStt::from_config(&Config)`, `engine_from_config()` |
| `normalizer` | текст для TTS | `SpeechNormalizer::normalize(&str)`; `cardinal/ordinal/decimal/plural/with_noun` |
| `chunker` | дельты LLM → фразы ≤260 | `PhraseChunker::push(delta) -> Vec<String>`, `flush()`, `split_long()` |
| `player` | вывод cpal | `Player::start(PlayerConfig, events)`, трейт `AudioOutput { play, finish, stop, drain, monitor }` |
| `capture` | микрофон cpal | `Capture::start(CaptureConfig, events) -> (Capture, Receiver<Vec<i16>>)` (кадры 256 @16 кГц) |
| `vad` | начало/конец речи | `Vad::push(frame) -> Option<VadEvent>` (`SpeechStart/Speech/SpeechEnd(Utterance)/Discarded`) |
| `echo` | полу-дуплекс, фильтр эха | `EchoGuard::{mic_muted, note_spoken, is_echo, is_clean_confirmation}`, трейт `PlaybackMonitor` |
| `wake` | «Джарвис, …», стоп, да/нет | `WakeFilter::classify(text, WakeContext) -> Intent` |
| `pipeline` | всё вместе | `VoicePipeline::{new, from_config, start_listening(cb), listen_frames, speak(rx), say, barge_in, set_awaiting_confirm}` |
| `pcm`, `resample`, `device`, `tls` | утилиты | WAV, `S16Decoder`, `LevelMeter`, `StreamResampler` (rubato FFT), выбор устройств по имени, закреплённый CA |

## Как это работает
- **TLS.** К серверу Xeon — rustls + ring, корни только из `ca_path` (сертификат на IP). Groq — системные корни.
- **TTS.** Тело строго `{"text","voice"}` (сервер с `deny_unknown_fields`), id задаёт клиент в `x-request-id`.
  Отмена токеном, таймаут или брошенный недочитанным поток → `DELETE /tts/{id}` сразу.
- **Речь.** Чанкер не рвёт «3.14», «т. е.», URL, «1 000 000»; блок кода и таблица — целиком.
  Следующая фраза синтезируется, пока играет текущая (`prefetch_phrases`), звук уходит буферами по 100 мс,
  очередь плеера ≤5 буферов, воспроизведение — с первого буфера. `barge_in()` / новая команда / «стоп» — мгновенная остановка.
- **Слух.** VAD (earshot или энергия), pre-roll 300 мс, конец после 800–1200 мс тишины, минимум речи 250 мс.
  Потоковый STT получает звук во время речи; если `final` не пришёл за `final_timeout` — та же фраза через `transcribe`.
  WebSocket переподключается сам и досылает звук текущей фразы.
- **Эхо.** Аппаратного AEC нет: во время звука (+300 мс) реплики, на ≥70 % состоящие из слов недавней речи TTS, отбрасываются.
  `barge_in_while_speaking = false` включает чистый полу-дуплекс. «Да, сэр» засчитывается, только если сказано после окончания речи.

## Тесты
- `cargo test` — юнит-тесты, 100+ кейсов нормализатора, моки TTS/STT (WS по протоколу ядра, `POST /stt`, Groq multipart), пайплайн на подменах.
- `cargo test --test tts_live -- --ignored --nocapture` — живой TTS (только HTTP, звук не играет).
- `cargo test --test stt_live -- --ignored --nocapture` — живой STT :8444 (фраза синтезируется TTS, микрофон не нужен).
- `cargo test --test audio_devices -- --ignored --test-threads=1` — **играет звук и включает микрофон**, только вручную.
