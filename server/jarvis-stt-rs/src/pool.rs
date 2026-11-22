//! Блокирующий пул объектов (распознавателей): `get` ждёт свободный экземпляр не дольше таймаута.

use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

pub struct BlockingPool<T> {
    items: Mutex<Vec<T>>,
    cv: Condvar,
    size: usize,
}

impl<T> BlockingPool<T> {
    pub fn new(items: Vec<T>) -> Self {
        let size = items.len();
        Self {
            items: Mutex::new(items),
            cv: Condvar::new(),
            size,
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn available(&self) -> usize {
        self.items.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Взять свободный экземпляр; `None`, если за `timeout` никто не освободился.
    pub fn get(&self, timeout: Duration) -> Option<PoolGuard<'_, T>> {
        let deadline = Instant::now() + timeout;
        let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(item) = items.pop() {
                return Some(PoolGuard {
                    pool: self,
                    item: Some(item),
                });
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            items = self
                .cv
                .wait_timeout(items, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

pub struct PoolGuard<'a, T> {
    pool: &'a BlockingPool<T>,
    item: Option<T>,
}

impl<T> Deref for PoolGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.item.as_ref().expect("pool item present until drop")
    }
}

impl<T> DerefMut for PoolGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.item.as_mut().expect("pool item present until drop")
    }
}

impl<T> Drop for PoolGuard<'_, T> {
    fn drop(&mut self) {
        if let Some(item) = self.item.take() {
            self.pool
                .items
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(item);
            self.pool.cv.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn get_returns_and_times_out() {
        let pool = BlockingPool::new(vec![1, 2]);
        let a = pool.get(Duration::from_millis(10)).unwrap();
        let b = pool.get(Duration::from_millis(10)).unwrap();
        assert_eq!(*a + *b, 3);
        let t = Instant::now();
        assert!(pool.get(Duration::from_millis(50)).is_none());
        assert!(t.elapsed() >= Duration::from_millis(45));
        drop(a);
        assert_eq!(pool.available(), 1);
        assert!(pool.get(Duration::ZERO).is_some());
    }

    #[test]
    fn limits_concurrency_across_threads() {
        let pool = Arc::new(BlockingPool::new(vec![(), ()]));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (pool, active, peak) = (pool.clone(), active.clone(), peak.clone());
                std::thread::spawn(move || {
                    let _g = pool.get(Duration::from_secs(5)).expect("slot");
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(pool.available(), 2);
    }
}
