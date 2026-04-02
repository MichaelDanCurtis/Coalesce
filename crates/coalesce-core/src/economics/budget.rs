use std::sync::atomic::{AtomicU64, Ordering};

/// Budget enforcement — prevents overspending.
/// Tracks spending per provider and overall, enforcing configurable limits.
pub struct BudgetTracker {
    /// Total spending limit (USD * 10000 for atomic precision)
    total_limit_cents: AtomicU64,
    /// Total spent so far
    total_spent_cents: AtomicU64,
    /// Daily spending limit
    daily_limit_cents: AtomicU64,
    /// Spent today
    daily_spent_cents: AtomicU64,
    /// Day boundary (unix timestamp / 86400)
    current_day: AtomicU64,
    /// Bitfield tracking which alert thresholds have already fired (prevents double-fire)
    fired_alerts: AtomicU64,
}

impl BudgetTracker {
    /// Create with limits in USD. 0.0 means no limit.
    pub fn new(total_limit_usd: f64, daily_limit_usd: f64) -> Self {
        Self {
            total_limit_cents: AtomicU64::new(to_cents(total_limit_usd)),
            total_spent_cents: AtomicU64::new(0),
            daily_limit_cents: AtomicU64::new(to_cents(daily_limit_usd)),
            daily_spent_cents: AtomicU64::new(0),
            current_day: AtomicU64::new(today()),
            fired_alerts: AtomicU64::new(0),
        }
    }

    /// No budget limits
    pub fn unlimited() -> Self {
        Self::new(0.0, 0.0)
    }

    /// Check if a request with estimated cost can proceed
    pub fn can_spend(&self, estimated_usd: f64) -> BudgetCheck {
        self.maybe_reset_daily();

        let est_cents = to_cents(estimated_usd);

        // Check total limit
        let total_limit = self.total_limit_cents.load(Ordering::Relaxed);
        if total_limit > 0 {
            let total_spent = self.total_spent_cents.load(Ordering::Relaxed);
            if total_spent + est_cents > total_limit {
                return BudgetCheck::Denied {
                    reason: format!(
                        "Total budget exceeded: ${:.4} spent of ${:.4} limit",
                        from_cents(total_spent),
                        from_cents(total_limit)
                    ),
                };
            }
        }

        // Check daily limit
        let daily_limit = self.daily_limit_cents.load(Ordering::Relaxed);
        if daily_limit > 0 {
            let daily_spent = self.daily_spent_cents.load(Ordering::Relaxed);
            if daily_spent + est_cents > daily_limit {
                return BudgetCheck::Denied {
                    reason: format!(
                        "Daily budget exceeded: ${:.4} spent of ${:.4} limit",
                        from_cents(daily_spent),
                        from_cents(daily_limit)
                    ),
                };
            }
        }

        BudgetCheck::Allowed
    }

    /// Record actual spending
    pub fn record_spending(&self, cost_usd: f64) {
        self.maybe_reset_daily();
        let cents = to_cents(cost_usd);
        self.total_spent_cents.fetch_add(cents, Ordering::Relaxed);
        self.daily_spent_cents.fetch_add(cents, Ordering::Relaxed);
    }

    pub fn total_spent_usd(&self) -> f64 {
        from_cents(self.total_spent_cents.load(Ordering::Relaxed))
    }

    pub fn daily_spent_usd(&self) -> f64 {
        self.maybe_reset_daily();
        from_cents(self.daily_spent_cents.load(Ordering::Relaxed))
    }

    pub fn total_remaining_usd(&self) -> Option<f64> {
        let limit = self.total_limit_cents.load(Ordering::Relaxed);
        if limit == 0 {
            return None;
        }
        let spent = self.total_spent_cents.load(Ordering::Relaxed);
        Some(from_cents(limit.saturating_sub(spent)))
    }

    pub fn daily_remaining_usd(&self) -> Option<f64> {
        self.maybe_reset_daily();
        let limit = self.daily_limit_cents.load(Ordering::Relaxed);
        if limit == 0 {
            return None;
        }
        let spent = self.daily_spent_cents.load(Ordering::Relaxed);
        Some(from_cents(limit.saturating_sub(spent)))
    }

    /// Check which thresholds have been newly crossed. Returns a list of
    /// (threshold_pct, spent_usd, limit_usd) for each newly crossed threshold.
    /// Each threshold fires at most once until `reset_alerts()` is called.
    pub fn check_thresholds(&self, thresholds: &[u32]) -> Vec<BudgetAlert> {
        let total_limit = self.total_limit_cents.load(Ordering::Relaxed);
        if total_limit == 0 {
            return vec![];
        }

        let total_spent = self.total_spent_cents.load(Ordering::Relaxed);
        let spent_pct = (total_spent as f64 / total_limit as f64 * 100.0) as u32;
        let mut alerts = Vec::new();

        let old_fired = self.fired_alerts.load(Ordering::Relaxed);
        let mut new_fired = old_fired;

        for (i, &threshold) in thresholds.iter().enumerate() {
            if i >= 64 {
                break; // max 64 thresholds in a u64 bitfield
            }
            let bit = 1u64 << i;
            if spent_pct >= threshold && (old_fired & bit) == 0 {
                new_fired |= bit;
                alerts.push(BudgetAlert {
                    threshold_pct: threshold,
                    spent_usd: from_cents(total_spent),
                    limit_usd: from_cents(total_limit),
                });
            }
        }

        if new_fired != old_fired {
            self.fired_alerts.store(new_fired, Ordering::Relaxed);
        }

        alerts
    }

    /// Reset fired alerts (e.g., when limits change)
    pub fn reset_alerts(&self) {
        self.fired_alerts.store(0, Ordering::Relaxed);
    }

    fn maybe_reset_daily(&self) {
        let now = today();
        let stored = self.current_day.load(Ordering::Relaxed);
        if now != stored {
            self.current_day.store(now, Ordering::Relaxed);
            self.daily_spent_cents.store(0, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone)]
pub struct BudgetAlert {
    pub threshold_pct: u32,
    pub spent_usd: f64,
    pub limit_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BudgetCheck {
    Allowed,
    Denied { reason: String },
}

impl BudgetCheck {
    pub fn is_allowed(&self) -> bool {
        matches!(self, BudgetCheck::Allowed)
    }
}

fn to_cents(usd: f64) -> u64 {
    (usd * 10000.0) as u64
}

fn from_cents(cents: u64) -> f64 {
    cents as f64 / 10000.0
}

fn today() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unlimited_always_allows() {
        let budget = BudgetTracker::unlimited();
        assert!(budget.can_spend(1000.0).is_allowed());
        budget.record_spending(500.0);
        assert!(budget.can_spend(1000.0).is_allowed());
    }

    #[test]
    fn test_total_limit() {
        let budget = BudgetTracker::new(1.0, 0.0); // $1 total, no daily
        assert!(budget.can_spend(0.50).is_allowed());
        budget.record_spending(0.80);
        assert!(!budget.can_spend(0.50).is_allowed()); // Would exceed $1
        assert!(budget.can_spend(0.10).is_allowed()); // Still under
    }

    #[test]
    fn test_daily_limit() {
        let budget = BudgetTracker::new(0.0, 0.50); // No total, $0.50/day
        assert!(budget.can_spend(0.30).is_allowed());
        budget.record_spending(0.40);
        assert!(!budget.can_spend(0.20).is_allowed());
    }

    #[test]
    fn test_alert_thresholds() {
        let budget = BudgetTracker::new(10.0, 0.0);
        let thresholds = vec![50, 80, 100];

        // No alerts yet at 0%
        assert!(budget.check_thresholds(&thresholds).is_empty());

        // Spend $6 = 60% — should trigger 50% threshold
        budget.record_spending(6.0);
        let alerts = budget.check_thresholds(&thresholds);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].threshold_pct, 50);

        // Check again — same threshold should NOT fire again
        assert!(budget.check_thresholds(&thresholds).is_empty());

        // Spend $3 more = 90% — should trigger 80%
        budget.record_spending(3.0);
        let alerts = budget.check_thresholds(&thresholds);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].threshold_pct, 80);

        // Spend $2 more = 110% — should trigger 100%
        budget.record_spending(2.0);
        let alerts = budget.check_thresholds(&thresholds);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].threshold_pct, 100);
    }

    #[test]
    fn test_tracking() {
        let budget = BudgetTracker::new(10.0, 5.0);
        budget.record_spending(1.50);
        budget.record_spending(0.50);
        assert!((budget.total_spent_usd() - 2.0).abs() < 0.01);
        assert!((budget.daily_spent_usd() - 2.0).abs() < 0.01);
        assert!((budget.total_remaining_usd().unwrap() - 8.0).abs() < 0.01);
        assert!((budget.daily_remaining_usd().unwrap() - 3.0).abs() < 0.01);
    }
}
