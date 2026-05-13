use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

pub struct PerKeyLimiter {
    counts: Mutex<HashMap<String, u64>>,
    limit: u64,
    last_cleanup: Mutex<Instant>,
}

impl PerKeyLimiter {
    pub fn new(limit: u64) -> Self {
        Self {
            counts: Mutex::new(HashMap::new()),
            limit,
            last_cleanup: Mutex::new(Instant::now()),
        }
    }

    pub fn try_acquire(self: &Arc<Self>, key: &str) -> bool {
        self.maybe_cleanup();
        let mut map = self.counts.lock().expect("per_key_counts poisoned");
        let count = map.entry(key.to_string()).or_insert(0);
        if *count >= self.limit {
            return false;
        }
        *count += 1;
        true
    }

    fn release(&self, key: &str) {
        let mut map = self.counts.lock().expect("per_key_counts poisoned");
        if let Some(count) = map.get_mut(key) {
            *count = count.saturating_sub(1);
        }
    }

    fn maybe_cleanup(&self) {
        let needs_cleanup = {
            let mut last = self.last_cleanup.lock().expect("last_cleanup poisoned");
            if last.elapsed() < CLEANUP_INTERVAL {
                false
            } else {
                *last = Instant::now();
                true
            }
        };
        if needs_cleanup {
            let mut map = self.counts.lock().expect("per_key_counts poisoned");
            map.retain(|_, v| *v > 0);
        }
    }

    pub fn acquire_guard(self: &Arc<Self>, key: &str) -> Option<PerKeyGuard> {
        if self.try_acquire(key) {
            Some(PerKeyGuard {
                limiter: Arc::clone(self),
                key_hash: key.to_string(),
            })
        } else {
            None
        }
    }
}

pub struct PerKeyGuard {
    limiter: Arc<PerKeyLimiter>,
    key_hash: String,
}

impl Drop for PerKeyGuard {
    fn drop(&mut self) {
        self.limiter.release(&self.key_hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn acquire_allows_under_limit() {
        let limiter = Arc::new(PerKeyLimiter::new(3));
        assert!(limiter.try_acquire("key-a"));
        assert!(limiter.try_acquire("key-a"));
        assert!(limiter.try_acquire("key-a"));
    }

    #[test]
    fn acquire_blocks_when_limit_exceeded() {
        let limiter = Arc::new(PerKeyLimiter::new(2));
        assert!(limiter.try_acquire("key-a"));
        assert!(limiter.try_acquire("key-a"));
        assert!(!limiter.try_acquire("key-a"));
    }

    #[test]
    fn release_allows_reacquire() {
        let limiter = Arc::new(PerKeyLimiter::new(1));
        assert!(limiter.try_acquire("key-a"));
        assert!(!limiter.try_acquire("key-a"));
        limiter.release("key-a");
        assert!(limiter.try_acquire("key-a"));
    }

    #[test]
    fn different_keys_independent() {
        let limiter = Arc::new(PerKeyLimiter::new(1));
        assert!(limiter.try_acquire("key-a"));
        assert!(limiter.try_acquire("key-b"));
        assert!(!limiter.try_acquire("key-a"));
        limiter.release("key-a");
        assert!(limiter.try_acquire("key-a"));
    }

    #[test]
    fn guard_acquire_and_drop_releases() {
        let limiter = Arc::new(PerKeyLimiter::new(1));
        {
            let _guard = limiter.acquire_guard("key-x").expect("should acquire");
            assert!(!limiter.try_acquire("key-x"));
        }
        assert!(limiter.try_acquire("key-x"));
    }

    #[test]
    fn guard_returns_none_when_full() {
        let limiter = Arc::new(PerKeyLimiter::new(1));
        let _g1 = limiter.acquire_guard("key-x").expect("should acquire");
        assert!(limiter.acquire_guard("key-x").is_none());
    }

    #[test]
    fn saturating_release_prevents_underflow() {
        let limiter = Arc::new(PerKeyLimiter::new(1));
        limiter.release("key-a");
        limiter.release("key-a");
        assert!(limiter.try_acquire("key-a"));
    }
}
