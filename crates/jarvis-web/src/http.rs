//! HTTP-клиент: reqwest + rustls (провайдер ring, сертификаты из хранилища Windows), UA `JarvisBot/1.0`,
//! таймаут 15 с, чтение тела с лимитом, защита от запросов во внутреннюю сеть.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect;

/// User-Agent всех запросов Джарвиса (кроме DuckDuckGo HTML, см. [`BROWSER_USER_AGENT`]).
pub const USER_AGENT: &str = "JarvisBot/1.0";

/// DuckDuckGo HTML отдаёт капчу (HTTP 202, `anomaly-modal`) на любой небраузерный UA, включая
/// `JarvisBot/1.0`, — проверено 2026-11-22. Поэтому только для него используется браузерный UA.
pub const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 8;

/// rustls собран только с ring; ставим его провайдером процесса (повторный вызов безвреден).
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Клиент для API поисковиков: UA `JarvisBot/1.0`, таймаут 15 с.
pub fn build_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    install_crypto_provider();
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(redirect::Policy::limited(MAX_REDIRECTS))
        .build()
}

/// Общий клиент по умолчанию (дешёвый клон, общий пул соединений).
/// Паникует, только если не удалось инициализировать TLS (как `reqwest::Client::new`).
pub fn default_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| build_client(DEFAULT_TIMEOUT).expect("не удалось создать HTTP-клиент (TLS)")).clone()
}

/// Клиент для чтения произвольных страниц: дополнительно не ходит во внутреннюю сеть
/// (localhost, 10/8, 192.168/16, link-local…), в том числе через редиректы и DNS.
pub(crate) fn build_page_client(timeout: Duration, allow_private: bool) -> reqwest::Result<reqwest::Client> {
    install_crypto_provider();
    let mut b = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(format!("больше {MAX_REDIRECTS} перенаправлений"));
            }
            match check_url(attempt.url(), allow_private) {
                Ok(()) => attempt.follow(),
                Err(e) => attempt.error(PrivateAddressError(e)),
            }
        }));
    if !allow_private {
        b = b.dns_resolver(Arc::new(PublicOnlyResolver));
    }
    b.build()
}

/// Ошибка «адрес во внутренней сети» — распознаётся в цепочке ошибок reqwest.
#[derive(Debug)]
pub(crate) struct PrivateAddressError(pub String);
impl std::fmt::Display for PrivateAddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for PrivateAddressError {}

/// Есть ли в цепочке источников ошибка [`PrivateAddressError`].
pub(crate) fn private_address_error(e: &reqwest::Error) -> Option<String> {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(p) = err.downcast_ref::<PrivateAddressError>() {
            return Some(p.0.clone());
        }
        cur = err.source();
    }
    None
}

/// Резолвер, который отбрасывает внутренние адреса (защита от DNS-rebinding и имён вроде `router.lan`).
struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let public: Vec<SocketAddr> = addrs.into_iter().filter(|a| is_public_ip(a.ip())).collect();
            if public.is_empty() {
                return Err(Box::new(PrivateAddressError(format!("{host} указывает во внутреннюю сеть")))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// Только http/https; IP-литералы внутренней сети запрещены (если не `allow_private`).
pub(crate) fn check_url(url: &url::Url, allow_private: bool) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => {}
        s => return Err(format!("схема «{s}:» не поддерживается, только http/https")),
    }
    if allow_private {
        return Ok(());
    }
    match url.host() {
        None => Err("в адресе нет хоста".into()),
        Some(url::Host::Ipv4(ip)) if !is_public_ip(IpAddr::V4(ip)) => Err(format!("{ip} — адрес внутренней сети")),
        Some(url::Host::Ipv6(ip)) if !is_public_ip(IpAddr::V6(ip)) => Err(format!("{ip} — адрес внутренней сети")),
        Some(url::Host::Domain(d)) if is_local_name(d) => Err(format!("{d} — локальное имя")),
        _ => Ok(()),
    }
}

fn is_local_name(d: &str) -> bool {
    let d = d.trim_end_matches('.').to_ascii_lowercase();
    d == "localhost" || [".localhost", ".local", ".lan", ".internal", ".home.arpa"].iter().any(|s| d.ends_with(s))
}

/// Публичный ли адрес (не loopback/частный/link-local/CGNAT/multicast/документационный).
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15 бенчмарки
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
                || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
                || (s[0] == 0x2001 && s[1] == 0x0db8)) // документационный
        }
    }
}

/// Прочитать тело не больше `limit` байт. Возвращает (байты, обрезано ли).
pub(crate) async fn read_limited(resp: &mut reqwest::Response, limit: usize) -> reqwest::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::with_capacity(resp.content_length().map_or(64 * 1024, |l| (l as usize).min(limit)));
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > limit {
            let room = limit - buf.len();
            buf.extend_from_slice(&chunk[..room]);
            return Ok((buf, true));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((buf, false))
}

/// Человекочитаемое сообщение из JSON-ошибки API: `error.message`, `message`, `detail`, `error` и т.п.
pub(crate) fn api_error_message(body: &str) -> String {
    let fallback = || crate::text::truncate_plain(body.trim(), 200);
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return fallback();
    };
    let candidates = [
        v.pointer("/error/message"),
        v.pointer("/error/detail"),
        v.pointer("/detail/error"),
        v.pointer("/message"),
        v.pointer("/readableMessage"),
        v.pointer("/detail"),
        v.pointer("/error"),
    ];
    for c in candidates.into_iter().flatten() {
        if let Some(s) = c.as_str().filter(|s| !s.trim().is_empty()) {
            return crate::text::truncate_plain(s.trim(), 200);
        }
    }
    fallback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ip_filter() {
        for ip in ["127.0.0.1", "10.1.2.3", "192.168.50.38", "172.16.0.1", "169.254.1.1", "100.64.0.1", "0.0.0.0"] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["::1", "fe80::1", "fd00::1", "::ffff:192.168.1.1"] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn url_checks() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert!(check_url(&u("https://example.com/"), false).is_ok());
        assert!(check_url(&u("http://192.168.50.38:8443/tts"), false).is_err());
        assert!(check_url(&u("http://localhost:3000/"), false).is_err());
        assert!(check_url(&u("http://router.lan/"), false).is_err());
        assert!(check_url(&u("http://[::1]/"), false).is_err());
        assert!(check_url(&u("file:///C:/Windows/win.ini"), false).is_err());
        assert!(check_url(&u("file:///C:/Windows/win.ini"), true).is_err());
        assert!(check_url(&u("http://127.0.0.1:8080/"), true).is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolver_drops_internal_addresses() {
        use std::str::FromStr;
        // localhost резолвится из hosts без сети → только 127.0.0.1/::1 → отказ.
        let err = match PublicOnlyResolver.resolve(Name::from_str("localhost").unwrap()).await {
            Err(e) => e,
            Ok(addrs) => panic!("ожидался отказ, получено {:?}", addrs.collect::<Vec<_>>()),
        };
        assert!(err.downcast_ref::<PrivateAddressError>().is_some(), "{err}");
    }

    #[test]
    fn api_messages() {
        assert_eq!(api_error_message(r#"{"error":{"code":400,"message":"API key not valid."}}"#), "API key not valid.");
        assert_eq!(api_error_message(r#"{"detail":{"error":"Unauthorized: missing or invalid API key."}}"#),
            "Unauthorized: missing or invalid API key.");
        assert_eq!(api_error_message(r#"{"message":"Unauthorized.","statusCode":403}"#), "Unauthorized.");
        assert_eq!(api_error_message("plain text"), "plain text");
    }
}
