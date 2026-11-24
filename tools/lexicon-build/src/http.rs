//! Вежливый HTTP-клиент с кэшем на диске: пауза между запросами, повторы с backoff,
//! сырые ответы в `data/persona/cache/`, чтобы повторный запуск ничего не качал заново.

use std::fs;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

pub const USER_AGENT: &str = "JarvisLexiconBuild/0.1 (personal offline dictionary builder)";
const MIN_GAP: Duration = Duration::from_millis(1000);
const ATTEMPTS: u32 = 6;

pub struct Client {
    agent: ureq::Agent,
    cache_dir: PathBuf,
    last: Option<Instant>,
    /// Без сети: только кэш (`--offline`).
    pub offline: bool,
    pub fetched: usize,
    pub from_cache: usize,
}

/// FNV-1a 64: стабильное имя файла кэша по URL и параметрам.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Client {
    pub fn new(cache_dir: PathBuf, offline: bool) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self { agent, cache_dir, last: None, offline, fetched: 0, from_cache: 0 }
    }

    fn throttle(&mut self) {
        if let Some(t) = self.last {
            let gap = t.elapsed();
            if gap < MIN_GAP {
                sleep(MIN_GAP - gap);
            }
        }
        self.last = Some(Instant::now());
    }

    /// GET с параметрами запроса. Ответ кэшируется по ключу (URL + параметры).
    pub fn get(&mut self, url: &str, params: &[(&str, &str)]) -> Result<String, String> {
        let mut key = url.to_string();
        for (k, v) in params {
            key.push('&');
            key.push_str(k);
            key.push('=');
            key.push_str(v);
        }
        let path = self.cache_dir.join(format!("{:016x}.txt", fnv(&key)));
        if let Ok(s) = fs::read_to_string(&path) {
            self.from_cache += 1;
            return Ok(s);
        }
        if self.offline {
            return Err(format!("нет в кэше (offline): {key}"));
        }
        let mut backoff = Duration::from_secs(2);
        let mut last_err = String::new();
        for attempt in 1..=ATTEMPTS {
            self.throttle();
            let mut req = self.agent.get(url);
            for (k, v) in params {
                req = req.query(*k, *v);
            }
            match req.call() {
                Ok(mut resp) => {
                    let status = resp.status().as_u16();
                    if status == 200 {
                        let body = resp
                            .body_mut()
                            .with_config()
                            .limit(64 * 1024 * 1024)
                            .read_to_string()
                            .map_err(|e| e.to_string())?;
                        // MediaWiki при перегрузке отвечает 200 с error.code=maxlag.
                        if body.contains("\"code\":\"maxlag\"") {
                            last_err = "maxlag".into();
                        } else {
                            fs::create_dir_all(&self.cache_dir).map_err(|e| e.to_string())?;
                            fs::write(&path, &body).map_err(|e| e.to_string())?;
                            self.fetched += 1;
                            return Ok(body);
                        }
                    } else {
                        let retry_after = resp
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.trim().parse::<u64>().ok());
                        last_err = format!("HTTP {status}");
                        if !(status == 429 || status >= 500) {
                            return Err(last_err);
                        }
                        if let Some(s) = retry_after {
                            backoff = backoff.max(Duration::from_secs(s.min(120)));
                        }
                    }
                }
                Err(e) => last_err = e.to_string(),
            }
            eprintln!("  повтор {attempt}/{ATTEMPTS} через {:?}: {last_err}", backoff);
            sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(120));
        }
        Err(format!("{key}: {last_err}"))
    }
}
