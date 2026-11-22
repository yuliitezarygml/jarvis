use anyhow::Result;
use jarvis_stt_rs::{config::Config, engine::Engine, server};
use std::sync::Arc;
use std::time::Instant;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let config = Config::from_env()?;
    let started = Instant::now();
    let engine = load_engine(&config)?;
    let load_ms = started.elapsed().as_millis() as u64;
    tracing::info!(model = engine.model(), threads = config.threads, pool = config.pool, load_ms, "модель загружена");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(64)
        .thread_name("stt-io")
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let state = server::AppState::new(engine, config.limits.clone());
        server::serve(config.bind, config.tls.clone(), state).await
    })
}

#[cfg(feature = "sherpa")]
fn load_engine(config: &Config) -> Result<Arc<dyn Engine>> {
    use jarvis_stt_rs::sherpa::{EngineOptions, SherpaEngine};
    let opt = EngineOptions { threads: config.threads, pool: config.pool, queue_timeout: config.limits.queue_timeout };
    let engine = SherpaEngine::load(config.streaming.as_ref(), config.offline.as_ref(), &opt)?;
    let t = Instant::now();
    engine.warmup()?;
    tracing::info!(warmup_ms = t.elapsed().as_millis() as u64, "прогрев");
    Ok(Arc::new(engine))
}

#[cfg(not(feature = "sherpa"))]
fn load_engine(_config: &Config) -> Result<Arc<dyn Engine>> {
    anyhow::bail!("собрано без движка: пересоберите с `--features sherpa` (Linux, см. README)")
}
