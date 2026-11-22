//! Настоящие реализации команд экранов (бывшие демо-заглушки): провайдеры, серверы голоса, поиск, аудио,
//! память, база знаний, история.

use std::path::Path;
use std::time::{Duration, Instant};

use jarvis_core::config::{SearchProviderConfig, SttConfig, TtsConfig};
use jarvis_core::{Config, ProviderConfig, Role};
use jarvis_kb::Kb;
use jarvis_llm::{CheckOptions, KeyStatus, ModelCache, ModelRole, ProviderReport, check_provider_with};
use jarvis_memory::{
    FactSource, Memory, MemoryKind, NewFact, NewLesson, NewPlaybook, PlaybookStep, is_tool_name, LessonScope,
};
use jarvis_voice::{ServerStt, TtsClient};
use serde_json::{Value, json};

use super::Core;
use crate::api;

const CHECK_TIMEOUT: Duration = Duration::from_secs(8);

fn ms(d: Duration) -> Option<u32> {
    Some(d.as_millis().min(u32::MAX as u128) as u32)
}

fn date(ts: i64) -> String {
    jarvis_kb::format_rfc3339(ts.max(0) as u64).chars().take(10).collect()
}

fn is_http(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

// ───────────────────────────── провайдеры LLM ─────────────────────────────

pub fn provider_presets(core: &Core) -> Vec<api::ProviderPreset> {
    core.presets
        .providers
        .iter()
        .map(|p| api::ProviderPreset {
            id: p.id.clone(),
            name: p.name.clone(),
            kind: p.kind,
            base_url: p.base_url.clone(),
            key_url: p.key_url.clone(),
            needs_key: p.needs_key,
            note: p.notes.clone(),
        })
        .collect()
}

/// «Проверить»: ключ → модели → живая проверка → рейтинг (jarvis-llm `check_provider`). Отчёт запоминается
/// для «Выбрать лучшее автоматически».
pub async fn provider_check(core: &Core, provider_id: &str, base_url: &str, api_key: String) -> api::ProviderCheck {
    let stored = core.config();
    let base_url = base_url.trim().trim_end_matches('/').to_string();
    if !is_http(&base_url) {
        return check_fail(api::CheckStatus::Network, "Адрес не похож на URL: нужен http:// или https://");
    }
    let preset = core.presets.get(provider_id);
    let kind = stored.provider(provider_id).map(|p| p.kind).or(preset.map(|p| p.kind)).unwrap_or_default();
    if preset.is_none_or(|p| p.needs_key) && api_key.trim().is_empty() && !base_url.contains("://127.0.0.1") && !base_url.contains("://localhost") {
        return check_fail(api::CheckStatus::BadKey, "Вставьте ключ — без него провайдер не ответит");
    }
    let pc = ProviderConfig { id: provider_id.to_string(), kind, base_url, api_key, enabled: true };
    let opts = CheckOptions {
        cache: Some(ModelCache::for_config(&core.root, &stored)),
        presets: Some(core.presets.clone()),
        ..CheckOptions::default()
    };
    let report = check_provider_with(&pc, &opts).await;
    let view = report_view(&report);
    core.reports.lock().unwrap().insert(provider_id.to_string(), report);
    view
}

fn check_fail(status: api::CheckStatus, message: &str) -> api::ProviderCheck {
    api::ProviderCheck { demo: false, status, message: message.into(), models: vec![], ranking: vec![] }
}

/// Отчёт jarvis-llm → таблица экрана настроек.
pub fn report_view(r: &ProviderReport) -> api::ProviderCheck {
    let status = match &r.key_status {
        KeyStatus::Valid => api::CheckStatus::Ok,
        KeyStatus::Invalid => api::CheckStatus::BadKey,
        KeyStatus::Unreachable(_) => api::CheckStatus::Network,
    };
    let answered = r.probes.iter().filter(|p| p.answered()).count();
    let message = match &r.error {
        Some(e) => e.clone(),
        None => format!("{} · моделей: {} · проверено вживую: {answered}", r.key_status.describe_ru(), r.models.len()),
    };
    let first_for = |model_ref: &str| -> Vec<String> {
        r.recommendations_by_role
            .iter()
            .filter(|(_, recs)| recs.first().is_some_and(|x| x.model_ref == model_ref))
            .map(|(role, _)| role.as_str().to_string())
            .collect()
    };
    let mut models: Vec<api::ModelView> = r
        .models
        .iter()
        .map(|m| {
            let mref = m.model_ref();
            let probe = r.probes.iter().find(|p| p.model == mref);
            let answered = probe.is_some_and(|p| p.answered());
            api::ModelView {
                id: m.id.clone(),
                ctx: m.ctx.unwrap_or(0),
                tools: m.tools,
                vision: m.vision,
                reasoning: m.reasoning,
                latency_ms: probe.and_then(|p| p.ttft_ms).map(|v| v.min(u32::MAX as u64) as u32),
                tokens_per_s: probe.and_then(|p| p.tok_per_s),
                tools_ok: probe.filter(|_| answered).map(|p| p.tools_ok),
                vision_ok: probe.and_then(|p| p.vision_ok),
                quality: m.quality,
                recommended_for: first_for(&mref),
            }
        })
        .collect();
    // Проверенные и рекомендованные — наверх, дальше по качеству.
    models.sort_by(|a, b| {
        (b.recommended_for.len(), b.latency_ms.is_some(), b.quality)
            .cmp(&(a.recommended_for.len(), a.latency_ms.is_some(), a.quality))
            .then_with(|| a.id.cmp(&b.id))
    });
    let ranking = ModelRole::ALL
        .iter()
        .map(|role| api::RoleRanking {
            role: role.as_str().into(),
            top: r
                .recommendations_by_role
                .get(role)
                .map(|recs| recs.iter().map(|x| api::RankedModel { model: x.model_ref.clone(), reason: x.label.clone() }).collect())
                .unwrap_or_default(),
        })
        .collect();
    api::ProviderCheck { demo: false, status, message, models, ranking }
}

/// «Выбрать лучшее автоматически» (jarvis-llm `apply_best`) по последним проверкам провайдеров, которые есть в `cfg`.
/// `only` — ограничить провайдерами (кнопка у конкретного провайдера).
pub fn apply_best(core: &Core, cfg: &mut Config, only: Option<&[String]>) -> Result<(), String> {
    let reports: Vec<ProviderReport> = core
        .reports
        .lock()
        .unwrap()
        .values()
        .filter(|r| cfg.provider(&r.provider).is_some() && only.is_none_or(|ids| ids.contains(&r.provider)))
        .cloned()
        .collect();
    if reports.iter().all(|r| r.recommendations_by_role.values().all(|v| v.is_empty())) {
        return Err("Нет результатов проверки: сначала нажмите «Проверить» у провайдера".into());
    }
    jarvis_llm::apply_best(cfg, &reports, 3);
    Ok(())
}

// ───────────────────────────── серверы голоса ─────────────────────────────

/// `kind`: `tts` (`GET /ready`) или `stt` (`GET /health`). CA берётся из сохранённого конфига.
pub async fn voice_check(cfg: &Config, root: &Path, kind: &str, url: &str) -> api::ServiceCheck {
    let url = url.trim().trim_end_matches('/').to_string();
    let fail = |message: String, latency_ms: Option<u32>| api::ServiceCheck { demo: false, ok: false, message, latency_ms };
    if !is_http(&url) {
        return fail("Адрес должен начинаться с https://".into(), None);
    }
    let t0 = Instant::now();
    match kind {
        "tts" => {
            let tc = TtsConfig { url: url.clone(), ..cfg.tts.clone() };
            let client = match TtsClient::new(&tc, root) {
                Ok(c) => c,
                Err(e) => return fail(format!("Сервер синтеза речи ({url}): {e}."), None),
            };
            match tokio::time::timeout(CHECK_TIMEOUT, client.ready()).await {
                Ok(Ok(r)) if r.ready => api::ServiceCheck {
                    demo: false,
                    ok: true,
                    message: format!(
                        "Сервер речи отвечает ✓: /ready → готов, голос {}, {} Гц",
                        if r.voice.is_empty() { tc.voice.as_str() } else { r.voice.as_str() },
                        r.sample_rate
                    ),
                    latency_ms: ms(t0.elapsed()),
                },
                Ok(Ok(_)) => fail("Сервер речи отвечает, но ещё не готов (/ready: ready=false).".into(), ms(t0.elapsed())),
                Ok(Err(e)) => fail(format!("Сервер синтеза речи недоступен ({url}): {e}."), None),
                Err(_) => fail(format!("Сервер синтеза речи не ответил за {} с ({url}).", CHECK_TIMEOUT.as_secs()), None),
            }
        }
        "stt" => {
            let sc = SttConfig { url: url.clone(), ..cfg.stt.clone() };
            let client = match ServerStt::new(&sc, root) {
                Ok(c) => c,
                Err(e) => return fail(format!("Сервер распознавания речи ({url}): {e}."), None),
            };
            match tokio::time::timeout(CHECK_TIMEOUT, client.health()).await {
                Ok(Ok(h)) if h.ok => api::ServiceCheck {
                    demo: false,
                    ok: true,
                    message: format!("Сервер распознавания отвечает ✓: /health → ok, модель {}, {} Гц", h.model, h.sample_rate),
                    latency_ms: ms(t0.elapsed()),
                },
                Ok(Ok(h)) => fail(format!("Сервер распознавания отвечает, но не готов (модель {}).", h.model), ms(t0.elapsed())),
                Ok(Err(e)) => fail(format!("Сервер распознавания речи недоступен ({url}): {e}."), None),
                Err(_) => fail(format!("Сервер распознавания речи не ответил за {} с ({url}).", CHECK_TIMEOUT.as_secs()), None),
            }
        }
        other => fail(format!("Неизвестный сервис «{other}»: нужен tts или stt"), None),
    }
}

// ───────────────────────────── поиск ─────────────────────────────

pub fn search_presets() -> Vec<api::SearchPreset> {
    let p = |id: &str, name: &str, needs_key: bool, extra: &str, url: &str| api::SearchPreset {
        id: id.into(),
        name: name.into(),
        needs_key,
        extra_label: extra.into(),
        key_url: url.into(),
    };
    vec![
        p("brave", "Brave Search", true, "", "https://api-dashboard.search.brave.com/"),
        p("tavily", "Tavily", true, "", "https://app.tavily.com/"),
        p("serper", "Serper (Google)", true, "", "https://serper.dev/"),
        p("google_cse", "Google Programmable Search", true, "Идентификатор поисковой системы (cx)", "https://programmablesearchengine.google.com/"),
        p("jina", "Jina Search (ключ необязателен)", false, "", "https://jina.ai/"),
        p("duckduckgo", "DuckDuckGo (без ключа, запасной)", false, "", ""),
    ]
}

/// Тестовый запрос поисковика (jarvis-web `validate_config`).
pub async fn search_check(id: &str, api_key: String, extra: &str) -> api::ServiceCheck {
    let c = SearchProviderConfig { id: id.to_string(), api_key, extra: extra.trim().to_string(), enabled: true };
    let r = jarvis_web::validate_config(&c, &jarvis_web::http::default_client()).await;
    let mut message = r.summary_ru();
    if let Some(first) = message.get(0..1) {
        message = first.to_uppercase() + &message[1..];
    }
    api::ServiceCheck { demo: false, ok: r.ok, message, latency_ms: Some(r.latency_ms.min(u32::MAX as u64) as u32) }
}

// ───────────────────────────── аудио ─────────────────────────────

/// Устройства WASAPI (только список имён — устройства не открываются).
pub fn audio_devices() -> Result<api::AudioDevices, String> {
    use cpal::traits::HostTrait;
    let host = cpal::default_host();
    let def_in = host.default_input_device().map(|d| jarvis_voice::device::device_name(&d));
    let def_out = host.default_output_device().map(|d| jarvis_voice::device::device_name(&d));
    let list = |names: Vec<String>, def: &Option<String>| -> Vec<api::AudioDevice> {
        names
            .into_iter()
            .map(|n| api::AudioDevice { id: n.clone(), default: def.as_deref() == Some(n.as_str()), name: n })
            .collect()
    };
    let inputs = jarvis_voice::device::input_devices().map_err(|e| e.to_string())?;
    let outputs = jarvis_voice::device::output_devices().map_err(|e| e.to_string())?;
    Ok(api::AudioDevices { demo: false, inputs: list(inputs, &def_in), outputs: list(outputs, &def_out) })
}

// ───────────────────────────── память ─────────────────────────────

const MEMORY_LIST_LIMIT: i64 = 500;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn memory_list(mem: &Memory) -> Result<api::MemoryData, String> {
    let facts = mem
        .list_facts(MEMORY_LIST_LIMIT, 0)
        .map_err(err)?
        .into_iter()
        .map(|f| api::Fact {
            id: f.id.to_string(),
            text: f.text,
            confidence: f.confidence as f32,
            source: match f.source {
                FactSource::Explicit => format!("сказано явно · {}", f.category),
                FactSource::Inferred => format!("выведено · {}", f.category),
            },
            updated: date(f.updated_at),
        })
        .collect();
    let lessons = mem
        .list_lessons(MEMORY_LIST_LIMIT, 0)
        .map_err(err)?
        .into_iter()
        .map(|l| api::Lesson {
            id: l.id.to_string(),
            what_went_wrong: if l.mistake.is_empty() { l.cause.clone() } else { l.mistake.clone() },
            tool: l.tool.clone().or_else(|| l.app.clone()).unwrap_or_else(|| l.scope.as_str().to_string()),
            trigger: l.trigger,
            fix: l.fix,
            seen: l.seen.max(0) as u32,
            last_seen: date(l.last_seen),
        })
        .collect();
    let playbooks = mem
        .list_playbooks(MEMORY_LIST_LIMIT, 0)
        .map_err(err)?
        .into_iter()
        .map(|p| api::Playbook {
            id: p.id.to_string(),
            name: p.name,
            when: p.when,
            steps: p.steps.iter().map(render_step).collect(),
            successes: p.success_count.max(0) as u32,
        })
        .collect();
    Ok(api::MemoryData { demo: false, facts, lessons, playbooks })
}

fn render_step(s: &PlaybookStep) -> String {
    let mut out = s.tool.clone();
    if s.args.as_object().is_some_and(|o| !o.is_empty()) {
        out.push(' ');
        out.push_str(&s.args.to_string());
    }
    if let Some(n) = s.note.as_deref().filter(|n| !n.trim().is_empty()) {
        out.push_str(" — ");
        out.push_str(n.trim());
    }
    out
}

/// Строки шагов из UI → шаги плейбука: `инструмент {json} — примечание`; строка без имени инструмента
/// становится примечанием предыдущего шага.
pub fn parse_steps(lines: &[String]) -> Result<Vec<PlaybookStep>, String> {
    let mut steps: Vec<PlaybookStep> = Vec::new();
    for raw in lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        let (head, rest) = raw.split_once(char::is_whitespace).unwrap_or((raw, ""));
        let rest = rest.trim();
        if is_tool_name(head) {
            let mut args = json!({});
            let mut note = rest.to_string();
            if rest.starts_with('{') {
                let mut it = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
                if let Some(Ok(v)) = it.next()
                    && v.is_object()
                {
                    note = rest[it.byte_offset()..].trim().trim_start_matches(['—', '-']).trim().to_string();
                    args = v;
                }
            }
            steps.push(PlaybookStep { tool: head.to_string(), args, note: (!note.is_empty()).then_some(note) });
        } else if let Some(last) = steps.last_mut() {
            let n = last.note.get_or_insert_with(String::new);
            if !n.is_empty() {
                n.push_str("; ");
            }
            n.push_str(raw);
        } else {
            return Err(format!(
                "Шаг «{raw}» должен начинаться с имени инструмента латиницей, например: open_target {{\"name\":\"блокнот\"}}"
            ));
        }
    }
    if steps.is_empty() {
        return Err("В плейбуке нужен хотя бы один шаг".into());
    }
    Ok(steps)
}

fn parse_id(id: &str) -> Result<i64, String> {
    id.trim().parse::<i64>().map_err(|_| format!("неверный id записи «{id}»"))
}

/// Создать (пустой `id`) или изменить запись памяти из экрана «Память».
pub fn memory_upsert(mem: &Memory, kind: &str, item: Value) -> Result<api::MemoryData, String> {
    let bad = |e: serde_json::Error| format!("неверная запись: {e}");
    match MemoryKind::parse(kind).ok_or_else(|| format!("неизвестный раздел памяти «{kind}»"))? {
        MemoryKind::Fact => {
            let f: api::Fact = serde_json::from_value(item).map_err(bad)?;
            if f.id.trim().is_empty() {
                let mut nf = NewFact::explicit(f.text);
                if f.confidence > 0.0 {
                    nf.confidence = f64::from(f.confidence.clamp(0.0, 1.0));
                }
                mem.upsert_fact(nf).map_err(err)?;
            } else {
                let mut cur = mem.get_fact(parse_id(&f.id)?).map_err(err)?.ok_or("запись не найдена")?;
                cur.text = f.text;
                if f.confidence > 0.0 {
                    cur.confidence = f64::from(f.confidence.clamp(0.0, 1.0));
                }
                mem.update_fact(&cur).map_err(err)?;
            }
        }
        MemoryKind::Lesson => {
            let l: api::Lesson = serde_json::from_value(item).map_err(bad)?;
            let tool = Some(l.tool.trim().to_string()).filter(|t| !t.is_empty());
            if l.id.trim().is_empty() {
                let scope = if tool.as_deref().is_some_and(is_tool_name) { LessonScope::Tool } else { LessonScope::General };
                mem.add_lesson(NewLesson {
                    scope,
                    trigger: l.trigger,
                    mistake: l.what_went_wrong,
                    fix: l.fix,
                    tool,
                    confidence: 0.8,
                    ..NewLesson::default()
                })
                .map_err(err)?;
            } else {
                let mut cur = mem.get_lesson(parse_id(&l.id)?).map_err(err)?.ok_or("запись не найдена")?;
                cur.trigger = l.trigger;
                cur.mistake = l.what_went_wrong;
                cur.fix = l.fix;
                cur.tool = tool;
                cur.is_stub = false;
                mem.update_lesson(&cur).map_err(err)?;
            }
        }
        MemoryKind::Playbook => {
            let p: api::Playbook = serde_json::from_value(item).map_err(bad)?;
            let steps = parse_steps(&p.steps)?;
            if p.id.trim().is_empty() {
                mem.add_playbook(NewPlaybook { name: p.name, when: p.when, steps, params_schema: json!({}) }).map_err(err)?;
            } else {
                let mut cur = mem.get_playbook(parse_id(&p.id)?).map_err(err)?.ok_or("запись не найдена")?;
                cur.name = p.name;
                cur.when = p.when;
                cur.steps = steps;
                mem.update_playbook(&cur).map_err(err)?;
            }
        }
    }
    memory_list(mem)
}

pub fn memory_delete(mem: &Memory, kind: &str, id: &str) -> Result<api::MemoryData, String> {
    let k = MemoryKind::parse(kind).ok_or_else(|| format!("неизвестный раздел памяти «{kind}»"))?;
    mem.delete(k, parse_id(id)?).map_err(err)?;
    memory_list(mem)
}

// ───────────────────────────── база знаний, история ─────────────────────────────

pub fn kb_search(kb: &Kb, query: &str) -> Result<api::KbResult, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(api::KbResult { demo: false, hits: vec![] });
    }
    let hits = kb
        .search(&[q.to_string()], 8)
        .map_err(err)?
        .into_iter()
        .map(|h| api::KbHit {
            section: if h.heading.is_empty() { h.source.clone() } else { format!("{} › {}", h.source, h.heading) },
            title: h.title,
            url: h.url,
            snippet: h.snippet,
            score: h.score as f32,
        })
        .collect();
    Ok(api::KbResult { demo: false, hits })
}

/// Реплики пользователя и Джарвиса из последних сеансов (по времени).
pub fn history_list(mem: &Memory) -> Result<api::HistoryResult, String> {
    const MAX_ITEMS: usize = 300;
    let mut items = Vec::new();
    for s in mem.list_sessions(30, 0).map_err(err)? {
        for m in mem.session_messages(s.id).map_err(err)? {
            let speaker = match m.role {
                Role::User => "user",
                Role::Assistant => "jarvis",
                _ => continue,
            };
            items.push((m.ts, m.id, api::HistoryItem {
                id: m.id.to_string(),
                at: jarvis_kb::format_rfc3339(m.ts.max(0) as u64),
                speaker: speaker.into(),
                text: m.content,
            }));
        }
        if items.len() >= MAX_ITEMS {
            break;
        }
    }
    items.sort_by_key(|(ts, id, _)| (*ts, *id));
    let skip = items.len().saturating_sub(MAX_ITEMS);
    Ok(api::HistoryResult { demo: false, items: items.into_iter().skip(skip).map(|(_, _, i)| i).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_roundtrip() {
        let steps = parse_steps(&[
            r#"open_target {"name":"ms-settings:bluetooth"} — открыть страницу"#.to_string(),
            "проверить заголовок окна «Bluetooth»".to_string(),
            "ui_snapshot".to_string(),
        ])
        .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].tool, "open_target");
        assert_eq!(steps[0].args["name"], "ms-settings:bluetooth");
        assert_eq!(steps[0].note.as_deref(), Some("открыть страницу; проверить заголовок окна «Bluetooth»"));
        let again = parse_steps(&steps.iter().map(render_step).collect::<Vec<_>>()).unwrap();
        assert_eq!(again, steps);
        assert!(parse_steps(&["просто текст".into()]).is_err());
        assert!(parse_steps(&[]).is_err());
    }

    #[test]
    fn memory_crud_on_real_store() {
        let dir = tempfile::tempdir().unwrap();
        let mem = Memory::open(dir.path().join("memory.db")).unwrap();
        let d = memory_upsert(&mem, "facts", json!({"text": "Пользователь пьёт чёрный кофе"})).unwrap();
        assert_eq!(d.facts.len(), 1);
        assert!(!d.demo);
        let id = d.facts[0].id.clone();
        let d = memory_upsert(&mem, "facts", json!({"id": id, "text": "Пользователь пьёт кофе без сахара"})).unwrap();
        assert_eq!(d.facts[0].text, "Пользователь пьёт кофе без сахара");
        let d = memory_upsert(&mem, "lessons", json!({"trigger": "git comit", "what_went_wrong": "опечатка", "fix": "git commit", "tool": "terminal_run"})).unwrap();
        assert_eq!(d.lessons.len(), 1);
        assert_eq!(d.lessons[0].tool, "terminal_run");
        let d = memory_upsert(&mem, "playbooks", json!({"name": "Блютуз", "when": "открой блютуз", "steps": ["open_target {\"name\":\"bluetooth\"}"]})).unwrap();
        assert_eq!(d.playbooks[0].steps, vec![r#"open_target {"name":"bluetooth"}"#.to_string()]);
        let d = memory_delete(&mem, "facts", &id).unwrap();
        assert!(d.facts.is_empty());
        assert!(memory_upsert(&mem, "nope", json!({})).is_err());
        // история
        let sid = mem.start_session(None).unwrap();
        mem.log_message(sid, Role::User, "Джарвис, привет", None).unwrap();
        mem.log_message(sid, Role::Assistant, "Здравствуйте, сэр.", None).unwrap();
        let h = history_list(&mem).unwrap();
        assert_eq!(h.items.iter().map(|i| i.speaker.as_str()).collect::<Vec<_>>(), ["user", "jarvis"]);
    }
}
