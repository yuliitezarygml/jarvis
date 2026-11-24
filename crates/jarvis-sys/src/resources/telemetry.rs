//! Сэмплер телеметрии: раз в секунду `Event::Telemetry` (ЦП, ОЗУ, GPU, сеть) + проверка resource guard.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jarvis_core::{CancellationToken, Event, EventSink};
use sysinfo::{Networks, System};
use tokio::sync::mpsc::UnboundedSender;

use crate::guard::{DiskSample, GuardAlert, Metric, ResourceGuard, Sample};

/// Кто больше всего потребляет (имя без .exe), по сгруппированным процессам.
pub fn top_consumer(m: Metric) -> Option<String> {
    let mut v = super::group_by_name(super::processes(Duration::from_millis(300)));
    super::sort_procs(&mut v, if m == Metric::Cpu { "cpu" } else { "memory" });
    v.into_iter().find(|p| !matches!(p.name.to_lowercase().as_str(), "system idle process" | "idle" | "memory compression")).map(|p| super::display_name(&p.name))
}

/// Запустить сэмплер в отдельном потоке. Останавливается по `cancel`.
/// `guard` — опционально: сработавшие предупреждения уходят в канал (их озвучивает хост).
pub fn spawn_telemetry(
    events: Arc<dyn EventSink>,
    interval: Duration,
    guard: Option<(ResourceGuard, UnboundedSender<GuardAlert>)>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("jarvis-telemetry".into())
        .spawn(move || {
            let mut sys = System::new();
            // Сеть: sysinfo отдаёт байты с прошлого обновления — первое обновление только «обнуляет» счётчики.
            let mut nets = Networks::new_with_refreshed_list();
            let mut net_at = Instant::now();
            let mut gpu = crate::win::GpuCounter::new();
            let mut guard = guard;
            let mut disks: Vec<DiskSample> = Vec::new();
            let mut disks_at: Option<Instant> = None;
            sys.refresh_cpu_usage();
            while !cancel.is_cancelled() {
                std::thread::sleep(interval);
                if cancel.is_cancelled() {
                    break;
                }
                sys.refresh_cpu_usage();
                sys.refresh_memory();
                let cpu = sys.global_cpu_usage();
                let g = gpu.as_mut().and_then(|c| c.sample());
                nets.refresh(true);
                let secs = net_at.elapsed().as_secs_f64();
                net_at = Instant::now();
                let bytes: u64 = nets.iter().map(|(_, d)| d.received() + d.transmitted()).sum();
                events.emit(Event::Telemetry {
                    cpu,
                    ram_used_mb: sys.used_memory() / (1024 * 1024),
                    ram_total_mb: sys.total_memory() / (1024 * 1024),
                    gpu: g,
                    net_kbps: net_kbps(bytes, secs),
                });
                if let Some((gd, tx)) = guard.as_mut() {
                    if disks_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(30)) {
                        disks = super::disks().into_iter().filter(|d| !d.removable).map(|d| DiskSample { mount: d.mount, free: d.free, total: d.total }).collect();
                        disks_at = Some(Instant::now());
                    }
                    let s = Sample { at: Instant::now(), cpu_pct: cpu, ram_used: sys.used_memory(), ram_total: sys.total_memory(), disks: disks.clone() };
                    for a in gd.observe(&s, &top_consumer) {
                        let _ = tx.send(a);
                    }
                }
            }
        })
        .expect("поток телеметрии")
}

/// Байты за интервал → кбит/с. Интервал слишком короткий — `None` (нечего делить).
pub fn net_kbps(bytes: u64, secs: f64) -> Option<f32> {
    (secs >= 0.05).then(|| (bytes as f64 * 8.0 / 1000.0 / secs) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_rate_kbps() {
        assert_eq!(net_kbps(125_000, 1.0), Some(1000.0), "125 КБ за секунду = 1000 кбит/с");
        assert_eq!(net_kbps(0, 1.0), Some(0.0));
        assert_eq!(net_kbps(1000, 0.0), None);
    }
    use jarvis_core::CollectSink;

    #[test]
    fn emits_telemetry() {
        let sink = Arc::new(CollectSink::default());
        let cancel = CancellationToken::new();
        let h = spawn_telemetry(sink.clone(), Duration::from_millis(200), None, cancel.clone());
        let t = Instant::now();
        while sink.0.lock().unwrap().len() < 2 && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        cancel.cancel();
        h.join().unwrap();
        let ev = sink.take();
        assert!(ev.len() >= 2, "{ev:?}");
        assert!(matches!(ev[0], Event::Telemetry { ram_total_mb, .. } if ram_total_mb > 0));
    }

    #[test]
    fn top_consumer_works() {
        assert!(top_consumer(Metric::Ram).is_some());
    }
}
