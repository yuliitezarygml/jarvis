use crate::{audio, engine::Engine, text};
use anyhow::{Result, ensure};
use bytes::Bytes;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

pub type Frame = std::io::Result<Bytes>;
pub struct Control {
    pub stop: AtomicBool,
    pub cancel: CancellationToken,
}
impl Control {
    pub fn stop(&self) {
        self.stop.store(true, Relaxed);
        self.cancel.cancel();
    }
}
#[derive(Default)]
pub struct Metrics {
    pub jobs: AtomicU64,
    pub failed: AtomicU64,
    pub queue_us: AtomicU64,
    pub load_ms: AtomicU64,
}
struct Job {
    input: mpsc::Receiver<String>,
    out: mpsc::Sender<Frame>,
    control: Arc<Control>,
    queued: Instant,
    id: String,
    _slot: OwnedSemaphorePermit,
}
#[derive(Clone)]
pub struct Pool {
    interactive: mpsc::Sender<Job>,
    normal: mpsc::Sender<Job>,
    slots: Arc<Semaphore>,
    active: Arc<Mutex<HashMap<String, Arc<Control>>>>,
    pub metrics: Arc<Metrics>,
}
pub struct Ticket {
    pub id: String,
    pub control: Arc<Control>,
    pub rx: mpsc::Receiver<Frame>,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.control.stop();
    }
}
#[derive(Debug)]
pub enum SubmitError {
    Busy,
    Duplicate,
    Offline,
}
impl Pool {
    pub fn new(engine: Engine, load_ms: u64) -> Self {
        let (interactive, mut a) = mpsc::channel(5);
        let (normal, mut b) = mpsc::channel(5);
        let pool = Self {
            interactive,
            normal,
            slots: Arc::new(Semaphore::new(5)),
            active: Default::default(),
            metrics: Default::default(),
        };
        pool.metrics.load_ms.store(load_ms, Relaxed);
        let active = pool.active.clone();
        let metrics = pool.metrics.clone();
        let rt = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            while let Some(job) = rt.block_on(async {
                tokio::select! {biased; j=a.recv()=>j,j=b.recv()=>j}
            }) {
                metrics
                    .queue_us
                    .fetch_add(job.queued.elapsed().as_micros() as u64, Relaxed);
                metrics.jobs.fetch_add(1, Relaxed);
                let result = run(&engine, &job.control, job.input, &job.out, &rt);
                if let Err(e) = result {
                    metrics.failed.fetch_add(1, Relaxed);
                    tracing::warn!(id=%job.id,error=%e,"request ended");
                    rt.block_on(async{tokio::select!{_=job.control.cancel.cancelled()=>(),_=job.out.send(Err(std::io::Error::other(e.to_string())))=>()}});
                }
                active.lock().unwrap().remove(&job.id);
            }
        });
        pool
    }
    pub fn submit(
        &self,
        id: String,
        interactive: bool,
        input: mpsc::Receiver<String>,
    ) -> Result<Ticket, SubmitError> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| SubmitError::Busy)?;
        let control = Arc::new(Control {
            stop: false.into(),
            cancel: CancellationToken::new(),
        });
        let (out, rx) = mpsc::channel(4);
        let mut active = self.active.lock().unwrap();
        if active.contains_key(&id) {
            return Err(SubmitError::Duplicate);
        }
        let tx = if interactive {
            &self.interactive
        } else {
            &self.normal
        };
        tx.try_send(Job {
            input,
            out,
            queued: Instant::now(),
            id: id.clone(),
            control: control.clone(),
            _slot: slot,
        })
        .map_err(|_| SubmitError::Offline)?;
        active.insert(id.clone(), control.clone());
        let timer = Arc::downgrade(&control);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(120)).await;
            if let Some(c) = timer.upgrade() {
                c.stop();
            }
        });
        Ok(Ticket { id, rx, control })
    }
    pub fn cancel(&self, id: &str) -> bool {
        self.active.lock().unwrap().get(id).is_some_and(|c| {
            c.stop();
            true
        })
    }
}
fn run(
    engine: &Engine,
    c: &Control,
    mut input: mpsc::Receiver<String>,
    out: &mpsc::Sender<Frame>,
    rt: &tokio::runtime::Handle,
) -> Result<()> {
    let mut samples = 0;
    loop {
        let text = rt.block_on(async {
            tokio::select! {biased;_=c.cancel.cancelled()=>None,t=input.recv()=>t}
        });
        let Some(text) = text else { break };
        ensure!(!c.stop.load(Relaxed), "cancelled");
        let normalized = text::prepare(&text)?;
        engine.synth(&normalized,&c.stop,&mut |frame|{
            samples+=frame.len();
            ensure!(samples<=audio::HZ*90,"audio exceeds 90 s");
            let frame=audio::pcm(frame)?;
            rt.block_on(async{tokio::select!{biased;_=c.cancel.cancelled()=>anyhow::bail!("cancelled"),r=out.send(Ok(frame))=>Ok(r?)}})
        })?;
    }
    ensure!(!c.stop.load(Relaxed), "cancelled");
    ensure!(samples > 0, "empty audio");
    Ok(())
}
