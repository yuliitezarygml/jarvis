mod api;
mod audio;
mod engine;
mod linguistics;
mod numeric;
mod scheduler;
mod stress;
mod text;

use anyhow::{Context, Result, ensure};
use std::{net::SocketAddr, path::PathBuf, time::Instant};

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let bind: SocketAddr = std::env::var("JARVIS_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8181".into())
        .parse()?;
    let secret = std::env::var("JARVIS_BEARER_TOKEN").ok();
    if let Some(s) = &secret {
        ensure!(s.len() >= 24, "bearer token requires at least 24 bytes");
    }
    ensure!(
        bind.ip().is_loopback() || secret.is_some(),
        "non-loopback bind requires JARVIS_BEARER_TOKEN"
    );
    let cert = std::env::var_os("JARVIS_TLS_CERT");
    let key = std::env::var_os("JARVIS_TLS_KEY");
    ensure!(cert.is_some() == key.is_some(), "configure both TLS files");
    ensure!(bind.ip().is_loopback() || cert.is_some(), "LAN API requires TLS");
    let threads: usize = std::env::var("JARVIS_THREADS")
        .unwrap_or_else(|_| "4".into())
        .parse()?;
    ensure!((1..=40).contains(&threads), "JARVIS_THREADS must be 1..40");
    let threads = threads.min(std::thread::available_parallelism()?.get());
    pocket_native::set_num_threads(threads);
    // Configure the global tensor pool before any async worker threads start.
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()?;
    let dir = PathBuf::from(std::env::var_os("JARVIS_ASSETS").unwrap_or_else(|| ".assets".into()));
    let started = Instant::now();
    let mut frontend = text::Frontend::load(&dir).context("load Russian text models")?;
    if std::env::args().nth(1).as_deref() == Some("prepare") {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            println!("{}", frontend.prepare(&line?, &false.into())?);
        }
        return Ok(());
    }
    let engine = engine::Engine::load(&dir).context("load acoustic model and voice")?;
    let load_ms = started.elapsed().as_millis() as u64;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let state = api::StateData {
            pool: scheduler::Pool::new(engine, frontend, load_ms),
            secret,
        };
        let router = api::router(state);
        if let (Some(cert), Some(key)) = (cert, key) {
            rustls::crypto::ring::default_provider().install_default().ok();
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            tracing::info!(%bind,threads,load_ms,"HTTPS ready");
            axum_server::bind_rustls(bind, tls).serve(router.into_make_service()).await?;
        } else {
            let listener = tokio::net::TcpListener::bind(bind).await?;
            tracing::info!(%bind,threads,load_ms,"ready");
            axum::serve(listener, router).await?;
        }
        Ok(())
    })
}
