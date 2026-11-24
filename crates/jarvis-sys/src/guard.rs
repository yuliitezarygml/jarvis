//! Resource guard (план, раздел 3): RAM > 90 % 30 с, CPU > 95 % 60 с, свободно на диске < 5 % →
//! сообщение по-русски для озвучивания; антидребезг 10 минут на каждый вид (и каждый диск).
//! Чистая логика — тестируется на подставных сэмплах.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::util::fmt_bytes;

#[derive(Debug, Clone, PartialEq)]
pub struct GuardConfig {
    pub ram_pct: f32,
    pub ram_for: Duration,
    pub cpu_pct: f32,
    pub cpu_for: Duration,
    pub disk_free_pct: f32,
    pub cooldown: Duration,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            ram_pct: 90.0,
            ram_for: Duration::from_secs(30),
            cpu_pct: 95.0,
            cpu_for: Duration::from_secs(60),
            disk_free_pct: 5.0,
            cooldown: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskSample {
    pub mount: String,
    pub free: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub at: Instant,
    pub cpu_pct: f32,
    pub ram_used: u64,
    pub ram_total: u64,
    pub disks: Vec<DiskSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Ram,
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", content = "disk", rename_all = "snake_case")]
pub enum AlertKind {
    Ram,
    Cpu,
    Disk(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GuardAlert {
    pub kind: AlertKind,
    pub value_pct: f32,
    /// Кто больше всего потребляет (для RAM/CPU).
    pub top: Option<String>,
    /// Готовая фраза для TTS.
    pub message: String,
}

pub struct ResourceGuard {
    cfg: GuardConfig,
    ram_since: Option<Instant>,
    cpu_since: Option<Instant>,
    last: HashMap<AlertKind, Instant>,
}

fn pct(a: u64, b: u64) -> f32 {
    if b == 0 { 0.0 } else { (a as f64 * 100.0 / b as f64) as f32 }
}

impl ResourceGuard {
    pub fn new(cfg: GuardConfig) -> Self {
        Self { cfg, ram_since: None, cpu_since: None, last: HashMap::new() }
    }

    fn cooled(&self, k: &AlertKind, now: Instant) -> bool {
        self.last.get(k).is_none_or(|t| now.duration_since(*t) >= self.cfg.cooldown)
    }

    /// Учесть сэмпл. `top` вызывается только при срабатывании (поиск «кто ест» дорогой).
    pub fn observe(&mut self, s: &Sample, top: &dyn Fn(Metric) -> Option<String>) -> Vec<GuardAlert> {
        let mut out = Vec::new();
        let now = s.at;

        let ram = pct(s.ram_used, s.ram_total);
        if ram > self.cfg.ram_pct {
            let since = *self.ram_since.get_or_insert(now);
            if now.duration_since(since) >= self.cfg.ram_for && self.cooled(&AlertKind::Ram, now) {
                let who = top(Metric::Ram);
                let message = match &who {
                    Some(w) => format!("Сэр, оперативная память заполнена на {ram:.0} %, больше всего берёт {w}."),
                    None => format!("Сэр, оперативная память заполнена на {ram:.0} %."),
                };
                out.push(GuardAlert { kind: AlertKind::Ram, value_pct: ram, top: who, message });
                self.last.insert(AlertKind::Ram, now);
                self.ram_since = None;
            }
        } else {
            self.ram_since = None;
        }

        if s.cpu_pct > self.cfg.cpu_pct {
            let since = *self.cpu_since.get_or_insert(now);
            if now.duration_since(since) >= self.cfg.cpu_for && self.cooled(&AlertKind::Cpu, now) {
                let who = top(Metric::Cpu);
                let cpu = s.cpu_pct;
                let message = match &who {
                    Some(w) => format!("Сэр, процессор загружен на {cpu:.0} % уже больше минуты, больше всего — {w}."),
                    None => format!("Сэр, процессор загружен на {cpu:.0} % уже больше минуты."),
                };
                out.push(GuardAlert { kind: AlertKind::Cpu, value_pct: cpu, top: who, message });
                self.last.insert(AlertKind::Cpu, now);
                self.cpu_since = None;
            }
        } else {
            self.cpu_since = None;
        }

        for d in &s.disks {
            if d.total == 0 {
                continue;
            }
            let free = pct(d.free, d.total);
            let k = AlertKind::Disk(d.mount.clone());
            if free < self.cfg.disk_free_pct && self.cooled(&k, now) {
                let name = d.mount.trim_end_matches('\\');
                let message = format!("Сэр, на диске {name} осталось всего {free:.0} % свободного места ({}).", fmt_bytes(d.free));
                out.push(GuardAlert { kind: k.clone(), value_pct: free, top: None, message });
                self.last.insert(k, now);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn sample(t0: Instant, secs: u64, cpu: f32, ram_pct: u64) -> Sample {
        Sample { at: t0 + Duration::from_secs(secs), cpu_pct: cpu, ram_used: 16 * GB * ram_pct / 100, ram_total: 16 * GB, disks: vec![] }
    }

    fn chrome(_: Metric) -> Option<String> {
        Some("chrome".into())
    }

    #[test]
    fn ram_needs_30_seconds_and_debounces() {
        let t0 = Instant::now();
        let mut g = ResourceGuard::new(GuardConfig::default());
        for s in 0..30 {
            assert!(g.observe(&sample(t0, s, 10.0, 93), &chrome).is_empty(), "рано на {s} с");
        }
        let a = g.observe(&sample(t0, 30, 10.0, 93), &chrome);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].message, "Сэр, оперативная память заполнена на 93 %, больше всего берёт chrome.");
        // антидребезг 10 минут
        for s in 31..600 {
            assert!(g.observe(&sample(t0, s, 10.0, 95), &chrome).is_empty());
        }
        assert_eq!(g.observe(&sample(t0, 640, 10.0, 95), &chrome).len(), 1);
    }

    #[test]
    fn dip_resets_timer() {
        let t0 = Instant::now();
        let mut g = ResourceGuard::new(GuardConfig::default());
        for s in 0..20 {
            g.observe(&sample(t0, s, 10.0, 95), &chrome);
        }
        g.observe(&sample(t0, 20, 10.0, 50), &chrome);
        for s in 21..50 {
            assert!(g.observe(&sample(t0, s, 10.0, 95), &chrome).is_empty(), "{s}");
        }
        assert_eq!(g.observe(&sample(t0, 51, 10.0, 95), &chrome).len(), 1);
    }

    #[test]
    fn cpu_after_a_minute_and_top_is_lazy() {
        let t0 = Instant::now();
        let mut g = ResourceGuard::new(GuardConfig::default());
        let calls = std::cell::Cell::new(0);
        let top = |_: Metric| {
            calls.set(calls.get() + 1);
            Some("ffmpeg".to_string())
        };
        for s in 0..60 {
            assert!(g.observe(&sample(t0, s, 99.0, 10), &top).is_empty());
        }
        assert_eq!(calls.get(), 0);
        let a = g.observe(&sample(t0, 60, 99.0, 10), &top);
        assert_eq!(a[0].kind, AlertKind::Cpu);
        assert!(a[0].message.contains("99 %") && a[0].message.contains("ffmpeg"));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn disk_low_space_per_disk() {
        let t0 = Instant::now();
        let mut g = ResourceGuard::new(GuardConfig::default());
        let mut s = sample(t0, 0, 5.0, 10);
        s.disks = vec![
            DiskSample { mount: "C:\\".into(), free: 12 * GB, total: 400 * GB },
            DiskSample { mount: "D:\\".into(), free: 300 * GB, total: 1000 * GB },
        ];
        let a = g.observe(&s, &|_| None);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, AlertKind::Disk("C:\\".into()));
        assert_eq!(a[0].message, "Сэр, на диске C: осталось всего 3 % свободного места (12,0 ГБ).");
        s.at += Duration::from_secs(60);
        assert!(g.observe(&s, &|_| None).is_empty());
        s.at += Duration::from_secs(600);
        assert_eq!(g.observe(&s, &|_| None).len(), 1);
    }
}
