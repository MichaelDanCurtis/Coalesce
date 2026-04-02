use dashmap::DashMap;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Deduplicates identical concurrent requests.
/// If request A is in-flight and an identical request B arrives,
/// B waits for A's result instead of making a second upstream call.
pub struct RequestDedup {
    /// In-flight requests keyed by content hash
    in_flight: Arc<DashMap<String, broadcast::Sender<DedupResult>>>,
    /// Recent results cache (TTL-based)
    cache: Arc<DashMap<String, CachedResult>>,
    /// How long to cache results
    cache_ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct DedupResult {
    pub body: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
struct CachedResult {
    result: DedupResult,
    cached_at: Instant,
}

impl RequestDedup {
    pub fn new(cache_ttl_secs: u64) -> Self {
        Self {
            in_flight: Arc::new(DashMap::new()),
            cache: Arc::new(DashMap::new()),
            cache_ttl: Duration::from_secs(cache_ttl_secs),
        }
    }

    /// Hash a request body for dedup key
    pub fn hash_request(body: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(body);
        hex::encode(hasher.finalize())
    }

    /// Check if we have a cached result for this hash
    pub fn get_cached(&self, hash: &str) -> Option<DedupResult> {
        if let Some(entry) = self.cache.get(hash) {
            if entry.cached_at.elapsed() < self.cache_ttl {
                return Some(entry.result.clone());
            }
            // Expired — remove it
            drop(entry);
            self.cache.remove(hash);
        }
        None
    }

    /// Try to acquire an in-flight slot. Returns:
    /// - `DedupAction::Execute(guard)` — you should execute, then call guard.complete()
    /// - `DedupAction::Wait(receiver)` — another request is in-flight, wait for its result
    pub fn try_dedup(&self, hash: &str) -> DedupAction {
        // Check cache first
        if let Some(result) = self.get_cached(hash) {
            return DedupAction::Cached(result);
        }

        // Try to be the first in-flight request
        if let Some(sender) = self.in_flight.get(hash) {
            // Someone else is already executing — subscribe to their result
            let rx = sender.subscribe();
            return DedupAction::Wait(rx);
        }

        // We're first — create a broadcast channel
        let (tx, _) = broadcast::channel(1);
        self.in_flight.insert(hash.to_string(), tx.clone());

        DedupAction::Execute(DedupGuard {
            hash: hash.to_string(),
            sender: tx,
            in_flight: self.in_flight.clone(),
            cache: self.cache.clone(),
            cache_ttl: self.cache_ttl,
        })
    }

    /// Remove expired cache entries
    pub fn cleanup(&self) {
        self.cache.retain(|_, v| v.cached_at.elapsed() < self.cache_ttl);
    }

    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

pub enum DedupAction {
    /// Cached result available
    Cached(DedupResult),
    /// Execute the request, then complete the guard
    Execute(DedupGuard),
    /// Wait for an in-flight request's result
    Wait(broadcast::Receiver<DedupResult>),
}

/// Guard that must be completed when the request finishes.
/// Broadcasts the result to all waiting subscribers.
#[allow(dead_code)]
pub struct DedupGuard {
    hash: String,
    sender: broadcast::Sender<DedupResult>,
    in_flight: Arc<DashMap<String, broadcast::Sender<DedupResult>>>,
    cache: Arc<DashMap<String, CachedResult>>,
    cache_ttl: Duration,
}

impl DedupGuard {
    pub fn complete(self, result: DedupResult) {
        // Cache the result
        if !result.is_error {
            self.cache.insert(
                self.hash.clone(),
                CachedResult {
                    result: result.clone(),
                    cached_at: Instant::now(),
                },
            );
        }

        // Remove from in-flight
        self.in_flight.remove(&self.hash);

        // Broadcast to any waiting subscribers (ignore errors — no receivers is fine)
        let _ = self.sender.send(result);
    }
}

impl Drop for DedupGuard {
    fn drop(&mut self) {
        // If the guard is dropped without completing (e.g., panic),
        // clean up the in-flight entry so others aren't stuck waiting
        self.in_flight.remove(&self.hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_deterministic() {
        let h1 = RequestDedup::hash_request(b"hello world");
        let h2 = RequestDedup::hash_request(b"hello world");
        assert_eq!(h1, h2);

        let h3 = RequestDedup::hash_request(b"hello world!");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_cache_hit() {
        let dedup = RequestDedup::new(60);
        let hash = "test_hash";

        // First request — should execute
        match dedup.try_dedup(hash) {
            DedupAction::Execute(guard) => {
                guard.complete(DedupResult {
                    body: "result".into(),
                    is_error: false,
                });
            }
            _ => panic!("Expected Execute"),
        }

        // Second request — should hit cache
        match dedup.try_dedup(hash) {
            DedupAction::Cached(result) => {
                assert_eq!(result.body, "result");
            }
            _ => panic!("Expected Cached"),
        }
    }

    #[test]
    fn test_error_not_cached() {
        let dedup = RequestDedup::new(60);
        let hash = "error_hash";

        match dedup.try_dedup(hash) {
            DedupAction::Execute(guard) => {
                guard.complete(DedupResult {
                    body: "error".into(),
                    is_error: true,
                });
            }
            _ => panic!("Expected Execute"),
        }

        // Error results should not be cached — next request should execute
        match dedup.try_dedup(hash) {
            DedupAction::Execute(_) => {} // Correct
            _ => panic!("Expected Execute after error"),
        }
    }

    #[test]
    fn test_in_flight_dedup() {
        let dedup = RequestDedup::new(60);
        let hash = "inflight_hash";

        // First request acquires the slot
        let guard = match dedup.try_dedup(hash) {
            DedupAction::Execute(g) => g,
            _ => panic!("Expected Execute"),
        };

        // Second request should wait
        match dedup.try_dedup(hash) {
            DedupAction::Wait(_) => {} // Correct
            _ => panic!("Expected Wait"),
        }

        assert_eq!(dedup.in_flight_count(), 1);

        // Complete the first request
        guard.complete(DedupResult {
            body: "done".into(),
            is_error: false,
        });

        assert_eq!(dedup.in_flight_count(), 0);
        assert_eq!(dedup.cache_size(), 1);
    }

    #[test]
    fn test_cleanup() {
        let dedup = RequestDedup::new(0); // 0s TTL — expires immediately

        match dedup.try_dedup("cleanup_hash") {
            DedupAction::Execute(guard) => {
                guard.complete(DedupResult {
                    body: "x".into(),
                    is_error: false,
                });
            }
            _ => panic!("Expected Execute"),
        }

        // Should be cached
        assert_eq!(dedup.cache_size(), 1);

        // Wait a tiny bit and cleanup
        std::thread::sleep(std::time::Duration::from_millis(10));
        dedup.cleanup();
        assert_eq!(dedup.cache_size(), 0);
    }
}
