//! Проверка построенных URL HEAD-запросами (с переходом по редиректам; при 405 — GET).

use std::time::Duration;
use ureq::ResponseExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlCheck {
    pub source: String,
    pub url: String,
    /// HTTP-код итогового ответа, 0 — сетевая ошибка.
    pub status: u16,
    pub final_url: String,
    pub error: String,
}

impl UrlCheck {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub fn check(urls: &[(String, String)]) -> Vec<UrlCheck> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(10)
        .user_agent("JarvisKbBuild/1.0")
        .build()
        .into();
    urls.iter()
        .filter(|(_, u)| u.starts_with("http"))
        .map(|(source, url)| {
            let mut res = agent.head(url).call();
            if matches!(&res, Ok(r) if r.status().as_u16() == 405) {
                res = agent.get(url).call();
            }
            match res {
                Ok(r) => UrlCheck {
                    source: source.clone(),
                    url: url.clone(),
                    status: r.status().as_u16(),
                    final_url: r.get_uri().to_string(),
                    error: String::new(),
                },
                Err(e) => UrlCheck { source: source.clone(), url: url.clone(), status: 0, final_url: String::new(), error: e.to_string() },
            }
        })
        .collect()
}
