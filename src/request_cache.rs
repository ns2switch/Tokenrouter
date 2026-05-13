use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Serialize)]
pub struct CacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub memory_bytes: usize,
    pub max_entries: usize,
}

struct CacheEntry {
    response_body: String,
    expires_at: Instant,
    byte_size: usize,
    tokens: (i64, i64),
}

pub struct RequestCache {
    entries: Mutex<HashMap<String, CacheEntry>>,
    memory_bytes: Mutex<usize>,
    max_entries: usize,
    max_response_bytes: usize,
    ttl: Duration,
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
}

impl RequestCache {
    pub fn new(max_entries: usize, max_response_bytes: usize, ttl_seconds: u64) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            memory_bytes: Mutex::new(0),
            max_entries,
            max_response_bytes,
            ttl: Duration::from_secs(ttl_seconds),
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn get(&self, hash: &str) -> Option<(String, i64, i64)> {
        let mut map = self.entries.lock().expect("request_cache poisoned");
        let now = Instant::now();

        map.retain(|_, e| e.expires_at > now);

        match map.get(hash) {
            Some(entry) if entry.expires_at > now => {
                self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some((entry.response_body.clone(), entry.tokens.0, entry.tokens.1))
            }
            _ => {
                self.misses
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                None
            }
        }
    }

    pub fn put(&self, hash: &str, response_body: &str, input_tokens: i64, output_tokens: i64) {
        let body_size = response_body.len();
        if body_size > self.max_response_bytes {
            return;
        }

        let mut map = self.entries.lock().expect("request_cache poisoned");
        let mut mem = self.memory_bytes.lock().expect("memory_bytes poisoned");
        let now = Instant::now();

        if map.len() >= self.max_entries {
            map.retain(|_, e| e.expires_at > now);
            if map.len() >= self.max_entries {
                let dropped: usize = map.values().map(|e| e.byte_size).sum::<usize>() / map.len();
                if let Some(key) = map.keys().next().cloned() {
                    map.remove(&key);
                    *mem = mem.saturating_sub(dropped);
                }
            }
        }

        map.insert(
            hash.to_string(),
            CacheEntry {
                response_body: response_body.to_string(),
                expires_at: now + self.ttl,
                byte_size: body_size,
                tokens: (input_tokens, output_tokens),
            },
        );
        *mem += body_size;
    }

    pub fn stats(&self) -> CacheStats {
        let map = self.entries.lock().expect("request_cache poisoned");
        let mem = self.memory_bytes.lock().expect("memory_bytes poisoned");
        CacheStats {
            entries: map.len(),
            hits: self.hits.load(std::sync::atomic::Ordering::Relaxed),
            misses: self.misses.load(std::sync::atomic::Ordering::Relaxed),
            memory_bytes: *mem,
            max_entries: self.max_entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_and_get_hit() {
        let cache = RequestCache::new(10, 1024, 60);
        cache.put("hash1", "response-body", 100, 50);
        let result = cache.get("hash1");
        assert!(result.is_some());
        let (body, in_tok, out_tok) = result.unwrap();
        assert_eq!(body, "response-body");
        assert_eq!(in_tok, 100);
        assert_eq!(out_tok, 50);
    }

    #[test]
    fn get_miss_for_unknown_key() {
        let cache = RequestCache::new(10, 1024, 60);
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn expired_entry_not_returned() {
        let cache = RequestCache::new(10, 1024, 0);
        cache.put("hash1", "body", 10, 5);
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(cache.get("hash1").is_none());
    }

    #[test]
    fn max_response_bytes_bypassed() {
        let cache = RequestCache::new(10, 5, 60);
        cache.put("hash1", "123456", 10, 5);
        assert!(cache.get("hash1").is_none());
    }

    #[test]
    fn max_entries_eviction() {
        let cache = RequestCache::new(3, 1024, 60);
        cache.put("a", "body-a", 1, 1);
        cache.put("b", "body-b", 2, 2);
        cache.put("c", "body-c", 3, 3);
        cache.put("d", "body-d", 4, 4);
        assert!(cache.get("d").is_some());
        assert_eq!(cache.stats().entries, 3);
    }

    #[test]
    fn stats_tracks_hits_and_misses() {
        let cache = RequestCache::new(10, 1024, 60);
        cache.put("h1", "data", 5, 5);
        cache.get("h1");
        cache.get("h1");
        cache.get("missing");
        let s = cache.stats();
        assert_eq!(s.hits, 2);
        assert_eq!(s.misses, 1);
        assert_eq!(s.entries, 1);
    }

    #[test]
    fn stats_reports_memory_usage() {
        let cache = RequestCache::new(10, 1024, 60);
        cache.put("h1", "hello", 5, 5);
        let s = cache.stats();
        assert!(s.memory_bytes >= 5);
        assert_eq!(s.max_entries, 10);
    }
}
