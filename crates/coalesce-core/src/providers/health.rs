use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Circuit breaker states: Closed (healthy) → Open (failing) → HalfOpen (testing)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// Circuit breaker per provider with time-based recovery.
///
/// After `threshold` consecutive failures, the circuit opens.
/// After `recovery_secs`, it transitions to half-open (allows one probe request).
/// A success in half-open closes the circuit; a failure re-opens it.
pub struct CircuitBreaker {
    failures: AtomicU32,
    threshold: u32,
    /// Unix timestamp when the circuit was last opened
    opened_at: AtomicU64,
    /// Seconds to wait before half-open probe
    recovery_secs: u64,
    /// Total requests routed through this breaker
    total_requests: AtomicU64,
    /// Total failures (not reset on success, for stats)
    total_failures: AtomicU64,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, recovery_secs: u64) -> Self {
        Self {
            failures: AtomicU32::new(0),
            threshold,
            opened_at: AtomicU64::new(0),
            recovery_secs,
            total_requests: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
        }
    }

    /// Default circuit breaker: opens after 3 failures, recovers after 30s
    pub fn default_provider() -> Self {
        Self::new(3, 30)
    }

    pub fn record_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
        self.opened_at.store(0, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        let prev = self.failures.fetch_add(1, Ordering::Relaxed);
        self.total_failures.fetch_add(1, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        // If we just hit the threshold, record when we opened
        if prev + 1 >= self.threshold {
            self.opened_at.store(now_secs(), Ordering::Relaxed);
        }
    }

    pub fn state(&self) -> CircuitState {
        let failures = self.failures.load(Ordering::Relaxed);
        if failures < self.threshold {
            return CircuitState::Closed;
        }

        let opened = self.opened_at.load(Ordering::Relaxed);
        if opened > 0 && now_secs() - opened >= self.recovery_secs {
            CircuitState::HalfOpen
        } else {
            CircuitState::Open
        }
    }

    pub fn is_available(&self) -> bool {
        self.state() != CircuitState::Open
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.failures.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> CircuitStats {
        CircuitStats {
            state: self.state(),
            consecutive_failures: self.failures.load(Ordering::Relaxed),
            total_requests: self.total_requests.load(Ordering::Relaxed),
            total_failures: self.total_failures.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CircuitStats {
    pub state: CircuitState,
    pub consecutive_failures: u32,
    pub total_requests: u64,
    pub total_failures: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starts_closed() {
        let cb = CircuitBreaker::new(3, 10);
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_available());
    }

    #[test]
    fn test_opens_after_threshold() {
        let cb = CircuitBreaker::new(3, 10);
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.is_available());
    }

    #[test]
    fn test_success_resets() {
        let cb = CircuitBreaker::new(3, 10);
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_half_open_after_recovery() {
        let cb = CircuitBreaker::new(2, 0); // 0s recovery for test
        cb.record_failure();
        cb.record_failure();
        // With 0s recovery, it should immediately be half-open
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        assert!(cb.is_available());
    }

    #[test]
    fn test_stats_tracking() {
        let cb = CircuitBreaker::new(3, 30);
        cb.record_success();
        cb.record_failure();
        cb.record_success();

        let stats = cb.stats();
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.total_failures, 1);
        assert_eq!(stats.consecutive_failures, 0);
    }
}
