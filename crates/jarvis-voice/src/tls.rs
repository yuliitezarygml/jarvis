//! TLS: доверяем **только** CA из файла (сервер Xeon с сертификатом на IP), криптопровайдер — ring.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustls::ClientConfig;
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::error::{Result, VoiceError};

/// Путь из конфига (относительно корня проекта) → абсолютный.
pub fn resolve_path(project_root: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() { path.to_path_buf() } else { project_root.join(path) }
}

/// Корень проекта по умолчанию: `JARVIS_ROOT` или ближайший предок текущего каталога/крейта с `config.example.json`.
pub fn default_project_root() -> PathBuf {
    if let Some(r) = std::env::var_os("JARVIS_ROOT") {
        return PathBuf::from(r);
    }
    let starts = [std::env::current_dir().ok(), Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")))];
    for start in starts.into_iter().flatten() {
        let mut cur: Option<&Path> = Some(&start);
        while let Some(dir) = cur {
            if dir.join("config.example.json").exists() {
                return dir.to_path_buf();
            }
            cur = dir.parent();
        }
    }
    PathBuf::from(".")
}

pub fn ring_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Процессный провайдер по умолчанию (нужен reqwest для публичных хостов, например Groq).
/// Если кто-то уже установил свой — оставляем его.
pub fn ensure_default_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Конфиг клиента, доверяющий только сертификатам из PEM (ALPN не задан — годится и для WebSocket).
pub fn pinned_config_from_pem(pem: &[u8]) -> Result<ClientConfig> {
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(pem) {
        let cert = cert.map_err(|e| VoiceError::Tls(format!("не удалось разобрать PEM: {e}")))?;
        roots.add(cert).map_err(|e| VoiceError::Tls(format!("сертификат CA не принят: {e}")))?;
    }
    if roots.is_empty() {
        return Err(VoiceError::Tls("в файле CA нет ни одного сертификата".into()));
    }
    let cfg = ClientConfig::builder_with_provider(ring_provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| VoiceError::Tls(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(cfg)
}

pub fn pinned_config(ca_path: &Path) -> Result<ClientConfig> {
    let pem = std::fs::read(ca_path)
        .map_err(|e| VoiceError::Tls(format!("не удалось прочитать CA {}: {e}", ca_path.display())))?;
    pinned_config_from_pem(&pem)
}

/// HTTP-клиент к серверу голоса. Для `https://` нужен CA; для `http://` (локальные тесты/моки) CA не читается.
pub fn pinned_http_client(base_url: &str, ca_path: &Path, connect_timeout: Duration) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .no_proxy() // сервер в LAN: системный прокси не нужен и не должен видеть трафик
        .tcp_nodelay(true)
        .user_agent(concat!("jarvis-voice/", env!("CARGO_PKG_VERSION")));
    if base_url.starts_with("https://") && is_private_host(base_url) {
        let mut cfg = pinned_config(ca_path)?;
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        b = b.tls_backend_preconfigured(cfg);
    } else {
        // `http://` или публичный адрес (туннель Cloudflare): обычные сертификаты из хранилища Windows.
        // Без TLS reqwest всё равно собирает rustls-бэкенд — нужен провайдер.
        ensure_default_provider();
    }
    b.build().map_err(|e| VoiceError::Tls(format!("не удалось создать HTTP-клиент: {e}")))
}

/// HTTP-клиент для публичных хостов (Groq): системное хранилище сертификатов Windows.
pub fn public_http_client(timeout: Duration) -> Result<reqwest::Client> {
    ensure_default_provider();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .user_agent(concat!("jarvis-voice/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| VoiceError::Tls(format!("не удалось создать HTTP-клиент: {e}")))
}

/// Адрес в домашней сети (частный/loopback IP, `localhost`, `*.local`, `*.lan`, `*.home.arpa`):
/// для него TLS проверяется закреплённым CA, для остальных — обычными корнями.
pub fn is_private_host(base_url: &str) -> bool {
    use std::net::IpAddr;
    let Ok(u) = reqwest::Url::parse(base_url) else { return false };
    match u.host() {
        Some(url::Host::Ipv4(ip)) => is_private_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_private_ip(IpAddr::V6(ip)),
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || [".local", ".lan", ".home.arpa", ".localhost"].iter().any(|s| d.ends_with(s))
        }
        None => false,
    }
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => {
            let s0 = v6.segments()[0];
            v6.is_loopback() || (s0 & 0xfe00) == 0xfc00 || (s0 & 0xffc0) == 0xfe80
        }
    }
}

/// `https://host:port` → `wss://host:port` (и `http` → `ws`).
pub fn to_ws_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_vs_public_hosts() {
        for u in ["https://192.168.50.38:8443", "https://10.0.0.5/x", "https://localhost:8181", "https://127.0.0.1", "https://xeon.lan:8443", "https://[::1]:8443"] {
            assert!(is_private_host(u), "{u}");
        }
        for u in ["https://minute-aerial-slim-same.trycloudflare.com", "https://api.groq.com/v1", "https://8.8.8.8", "https://192.169.1.1"] {
            assert!(!is_private_host(u), "{u}");
        }
    }

    /// Живая проверка: туннель Cloudflare к серверу голоса отвечает через обычные сертификаты.
    /// `JARVIS_TTS_PUBLIC_URL=https://….trycloudflare.com cargo test -p jarvis-voice tunnel_ready -- --ignored`
    #[tokio::test]
    #[ignore = "ходит в интернет"]
    async fn tunnel_ready() {
        let Ok(url) = std::env::var("JARVIS_TTS_PUBLIC_URL") else { return };
        let root = default_project_root();
        let ca = resolve_path(&root, &jarvis_core::config::TtsConfig::default().ca_path);
        let c = pinned_http_client(&url, &ca, Duration::from_secs(10)).unwrap();
        let r = c.get(format!("{url}/ready")).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }

    #[test]
    fn project_ca_parses() {
        let root = default_project_root();
        let ca = resolve_path(&root, &jarvis_core::config::TtsConfig::default().ca_path);
        if !ca.exists() {
            eprintln!("CA не найден ({}), пропуск", ca.display());
            return;
        }
        pinned_config(&ca).expect("CA сервера Xeon должен разбираться");
    }

    #[test]
    fn rejects_empty_and_garbage_pem() {
        assert!(matches!(pinned_config_from_pem(b""), Err(VoiceError::Tls(_))));
        assert!(pinned_config_from_pem(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n").is_err());
        assert!(pinned_config(Path::new("Z:/нет/такого/файла.pem")).is_err());
    }

    #[test]
    fn urls() {
        assert_eq!(to_ws_url("https://192.168.50.38:8444/", "/stt/stream"), "wss://192.168.50.38:8444/stt/stream");
        assert_eq!(to_ws_url("http://127.0.0.1:1", "/x"), "ws://127.0.0.1:1/x");
        let root = Path::new("D:/jarvis");
        assert_eq!(resolve_path(root, "a/b.pem"), root.join("a/b.pem"));
    }

    #[test]
    fn private_hosts_use_pinned_ca() {
        for u in ["https://192.168.50.38:8443", "http://127.0.0.1:9", "https://10.0.0.5", "https://[::1]:8443", "https://xeon.local:8443", "https://localhost"] {
            assert!(is_private_host(u), "{u}");
        }
        for u in ["https://minute-aerial-slim-same.trycloudflare.com", "https://8.8.8.8", "https://api.groq.com", "мусор"] {
            assert!(!is_private_host(u), "{u}");
        }
    }
}
