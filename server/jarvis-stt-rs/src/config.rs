//! Конфигурация из переменных окружения (unit systemd задаёт их через `Environment=`).

use anyhow::{Context, Result, bail, ensure};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Модели по умолчанию (выбраны замером, см. README): GigaAM v3 CTC даёт и final, и partial
/// («симулированный стриминг» каждые `JARVIS_STT_PARTIAL_MS`); потоковая модель не нужна.
pub const DEFAULT_STREAMING: &str = "none";
pub const DEFAULT_OFFLINE: &str = "gigaam-ctc:sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16";

/// Модель: семейство и каталог с файлами.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub kind: String,
    pub dir: PathBuf,
}

impl ModelSpec {
    /// `kind:каталог` (каталог относительно `root`, если не абсолютный); `none`/пусто → `None`.
    pub fn parse(spec: &str, root: &std::path::Path) -> Result<Option<Self>> {
        let spec = spec.trim();
        if spec.is_empty() || spec.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        let (kind, dir) = spec
            .split_once(':')
            .with_context(|| format!("ожидается kind:каталог, получено {spec:?}"))?;
        ensure!(
            !kind.is_empty() && !dir.is_empty(),
            "пустой kind или каталог в {spec:?}"
        );
        let dir = PathBuf::from(dir);
        let dir = if dir.is_absolute() {
            dir
        } else {
            root.join(dir)
        };
        Ok(Some(Self {
            kind: kind.to_ascii_lowercase(),
            dir,
        }))
    }

    /// Короткое имя для `ready`/`health`: каталог без префикса `sherpa-onnx-`.
    pub fn short_name(&self) -> String {
        let name = self
            .dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        name.strip_prefix("sherpa-onnx-")
            .unwrap_or(&name)
            .to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    /// Одновременные WebSocket-сессии.
    pub max_sessions: usize,
    /// Одновременные `POST /stt`.
    pub max_requests: usize,
    /// Предел длины одной фразы/записи.
    pub max_utterance: Duration,
    /// Закрыть WS, если клиент молчит (включая pong) дольше.
    pub idle_timeout: Duration,
    /// Интервал ping от сервера.
    pub ping_interval: Duration,
    /// Сколько ждать свободный распознаватель.
    pub queue_timeout: Duration,
    /// Предел ожидания результата декодирования.
    pub decode_timeout: Duration,
    /// Предел чтения тела `POST /stt`.
    pub body_timeout: Duration,
    pub max_body_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_sessions: 8,
            max_requests: 4,
            max_utterance: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(120),
            ping_interval: Duration::from_secs(30),
            queue_timeout: Duration::from_secs(5),
            decode_timeout: Duration::from_secs(20),
            body_timeout: Duration::from_secs(15),
            max_body_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub tls: Option<(PathBuf, PathBuf)>,
    pub model_dir: PathBuf,
    pub streaming: Option<ModelSpec>,
    pub offline: Option<ModelSpec>,
    /// Потоки ONNX Runtime на один распознаватель.
    pub threads: usize,
    /// Экземпляров каждого распознавателя (параллельных декодирований).
    pub pool: usize,
    /// Без потоковой модели: partial из офлайн-модели каждые N мс звука (0 — partial нет).
    pub partial_ms: u64,
    pub limits: Limits,
}

fn num<T: std::str::FromStr>(
    get: &impl Fn(&str) -> Option<String>,
    key: &str,
    default: T,
) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match get(key) {
        None => Ok(default),
        Some(v) => v
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("{key}={v:?}: {e}")),
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let bind: SocketAddr = get("JARVIS_STT_BIND")
            .unwrap_or_else(|| "127.0.0.1:8182".into())
            .parse()
            .context("JARVIS_STT_BIND")?;
        let tls = match (get("JARVIS_TLS_CERT"), get("JARVIS_TLS_KEY")) {
            (Some(c), Some(k)) => Some((PathBuf::from(c), PathBuf::from(k))),
            (None, None) => None,
            _ => bail!("задайте оба файла TLS: JARVIS_TLS_CERT и JARVIS_TLS_KEY"),
        };
        ensure!(
            bind.ip().is_loopback() || tls.is_some(),
            "адрес не loopback требует TLS"
        );
        let model_dir = PathBuf::from(
            get("JARVIS_STT_MODEL_DIR").unwrap_or_else(|| "/opt/jarvis-stt-rs/models".into()),
        );
        let streaming = ModelSpec::parse(
            &get("JARVIS_STT_STREAMING").unwrap_or_else(|| DEFAULT_STREAMING.into()),
            &model_dir,
        )?;
        let offline = ModelSpec::parse(
            &get("JARVIS_STT_OFFLINE").unwrap_or_else(|| DEFAULT_OFFLINE.into()),
            &model_dir,
        )?;
        ensure!(
            streaming.is_some() || offline.is_some(),
            "нужна хотя бы одна модель (JARVIS_STT_STREAMING/OFFLINE)"
        );
        let threads: usize = num(&get, "JARVIS_STT_THREADS", 4)?;
        ensure!((1..=16).contains(&threads), "JARVIS_STT_THREADS: 1..16");
        let pool: usize = num(&get, "JARVIS_STT_POOL", 2)?;
        ensure!((1..=8).contains(&pool), "JARVIS_STT_POOL: 1..8");
        let partial_ms: u64 = num(&get, "JARVIS_STT_PARTIAL_MS", 300)?;
        ensure!(
            partial_ms == 0 || (100..=5000).contains(&partial_ms),
            "JARVIS_STT_PARTIAL_MS: 0 или 100..5000"
        );
        let d = Limits::default();
        let secs = |key: &str, def: Duration| -> Result<Duration> {
            let v: f64 = num(&get, key, def.as_secs_f64())?;
            ensure!(v > 0.0 && v <= 3600.0, "{key}: 0..3600 с");
            Ok(Duration::from_secs_f64(v))
        };
        let limits = Limits {
            max_sessions: num(&get, "JARVIS_STT_MAX_SESSIONS", d.max_sessions)?,
            max_requests: num(&get, "JARVIS_STT_MAX_REQUESTS", d.max_requests)?,
            max_utterance: secs("JARVIS_STT_MAX_UTTERANCE_S", d.max_utterance)?,
            idle_timeout: secs("JARVIS_STT_IDLE_TIMEOUT_S", d.idle_timeout)?,
            ping_interval: secs("JARVIS_STT_PING_S", d.ping_interval)?,
            queue_timeout: secs("JARVIS_STT_QUEUE_TIMEOUT_S", d.queue_timeout)?,
            decode_timeout: secs("JARVIS_STT_DECODE_TIMEOUT_S", d.decode_timeout)?,
            ..d
        };
        ensure!(
            (1..=64).contains(&limits.max_sessions),
            "JARVIS_STT_MAX_SESSIONS: 1..64"
        );
        ensure!(
            (1..=32).contains(&limits.max_requests),
            "JARVIS_STT_MAX_REQUESTS: 1..32"
        );
        ensure!(
            limits.max_utterance <= Duration::from_secs(120),
            "JARVIS_STT_MAX_UTTERANCE_S ≤ 120"
        );
        Ok(Self {
            bind,
            tls,
            model_dir,
            streaming,
            offline,
            threads,
            pool,
            partial_ms,
            limits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Result<Config> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(move |k| m.get(k).cloned())
    }

    #[test]
    fn defaults() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.bind.to_string(), "127.0.0.1:8182");
        assert!(c.tls.is_none());
        assert_eq!((c.threads, c.pool, c.partial_ms), (4, 2, 300));
        assert!(c.streaming.is_none());
        let s = c.offline.unwrap();
        assert_eq!(s.kind, "gigaam-ctc");
        assert!(s.dir.starts_with("/opt/jarvis-stt-rs/models"));
        assert_eq!(s.short_name(), "nemo-ctc-giga-am-v3-russian-2025-12-16");
        assert!(cfg(&[("JARVIS_STT_PARTIAL_MS", "50")]).is_err());
    }

    #[test]
    fn validation() {
        assert!(
            cfg(&[("JARVIS_STT_BIND", "0.0.0.0:8182")]).is_err(),
            "LAN без TLS"
        );
        assert!(
            cfg(&[("JARVIS_TLS_CERT", "/c")]).is_err(),
            "только один файл TLS"
        );
        let c = cfg(&[
            ("JARVIS_STT_BIND", "0.0.0.0:1"),
            ("JARVIS_TLS_CERT", "/c"),
            ("JARVIS_TLS_KEY", "/k"),
        ])
        .unwrap();
        assert!(c.tls.is_some());
        assert!(cfg(&[("JARVIS_STT_THREADS", "0")]).is_err());
        assert!(cfg(&[("JARVIS_STT_THREADS", "x")]).is_err());
        assert!(
            cfg(&[
                ("JARVIS_STT_STREAMING", "none"),
                ("JARVIS_STT_OFFLINE", "none")
            ])
            .is_err()
        );
        assert!(cfg(&[("JARVIS_STT_STREAMING", "no-colon")]).is_err());
    }

    #[test]
    fn model_specs() {
        let abs = std::env::temp_dir().join("zip");
        let c = cfg(&[
            ("JARVIS_STT_MODEL_DIR", "/m"),
            (
                "JARVIS_STT_STREAMING",
                &format!("Zipformer:{}", abs.display()),
            ),
            ("JARVIS_STT_OFFLINE", "none"),
            ("JARVIS_STT_MAX_UTTERANCE_S", "12.5"),
        ])
        .unwrap();
        assert_eq!(
            c.streaming,
            Some(ModelSpec {
                kind: "zipformer".into(),
                dir: abs
            })
        );
        assert_eq!(c.offline, None);
        assert_eq!(c.limits.max_utterance, Duration::from_millis(12500));
        let s = ModelSpec::parse("gigaam-ctc:giga", std::path::Path::new("/m"))
            .unwrap()
            .unwrap();
        assert_eq!(s.dir, PathBuf::from("/m").join("giga"));
    }
}
