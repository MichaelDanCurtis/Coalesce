use super::billing::BillingType;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Tracks the current quota state for a provider/model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaState {
    pub provider: String,
    pub model: Option<String>,
    pub billing: BillingType,
    pub used: u32,
    pub last_reset: DateTime<Utc>,
    pub credits_used_usd: f64,
}

impl QuotaState {
    pub fn new(provider: String, model: Option<String>, billing: BillingType) -> Self {
        Self {
            provider,
            model,
            billing,
            used: 0,
            last_reset: Utc::now(),
            credits_used_usd: 0.0,
        }
    }

    /// Record that a request was made.
    pub fn record_usage(&mut self, cost_usd: f64) {
        self.used += 1;
        self.credits_used_usd += cost_usd;
    }

    /// Get remaining quota, if applicable.
    pub fn remaining(&self) -> Option<u32> {
        match &self.billing {
            BillingType::QuotaMonthly { quota_total } => {
                Some(quota_total.saturating_sub(self.used))
            }
            BillingType::QuotaRefreshing {
                quota_per_window,
                refresh_interval_secs,
            }
            | BillingType::QuotaOnly {
                quota_per_window,
                refresh_interval_secs,
            }
            | BillingType::QuotaMetered {
                quota_per_window,
                refresh_interval_secs,
            } => {
                let elapsed = Utc::now()
                    .signed_duration_since(self.last_reset)
                    .num_seconds() as u64;
                if *refresh_interval_secs > 0 && elapsed >= *refresh_interval_secs {
                    Some(*quota_per_window)
                } else {
                    Some(quota_per_window.saturating_sub(self.used))
                }
            }
            _ => None,
        }
    }

    /// Check if the quota has refreshed and reset if so.
    pub fn check_refresh(&mut self) -> bool {
        let refresh_secs = match &self.billing {
            BillingType::QuotaRefreshing { refresh_interval_secs, .. }
            | BillingType::QuotaOnly { refresh_interval_secs, .. }
            | BillingType::QuotaMetered { refresh_interval_secs, .. } => {
                if *refresh_interval_secs == 0 { return false; }
                *refresh_interval_secs
            }
            _ => return false,
        };
        let elapsed = Utc::now()
            .signed_duration_since(self.last_reset)
            .num_seconds() as u64;
        if elapsed >= refresh_secs {
            self.used = 0;
            self.last_reset = Utc::now();
            return true;
        }
        false
    }

    /// Seconds until the next quota refresh, if applicable.
    pub fn seconds_until_refresh(&self) -> Option<u64> {
        let refresh_secs = match &self.billing {
            BillingType::QuotaRefreshing { refresh_interval_secs, .. }
            | BillingType::QuotaOnly { refresh_interval_secs, .. }
            | BillingType::QuotaMetered { refresh_interval_secs, .. } => {
                if *refresh_interval_secs == 0 { return None; }
                *refresh_interval_secs
            }
            _ => return None,
        };
        let elapsed = Utc::now()
            .signed_duration_since(self.last_reset)
            .num_seconds() as u64;
        if elapsed >= refresh_secs { Some(0) } else { Some(refresh_secs - elapsed) }
    }

    /// Remaining free credits in USD, if applicable.
    pub fn remaining_credits_usd(&self) -> Option<f64> {
        if let BillingType::FreeTierCredits { free_credits_usd } = &self.billing {
            Some((free_credits_usd - self.credits_used_usd).max(0.0))
        } else {
            None
        }
    }

    /// Is quota depleted?
    pub fn is_depleted(&self) -> bool {
        match &self.billing {
            BillingType::QuotaMonthly { .. }
            | BillingType::QuotaRefreshing { .. }
            | BillingType::QuotaOnly { .. }
            | BillingType::QuotaMetered { .. } => {
                self.remaining().is_some_and(|r| r == 0)
            }
            BillingType::FreeTierCredits { .. } => {
                self.remaining_credits_usd().is_some_and(|r| r <= 0.0)
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monthly_quota() {
        let mut state = QuotaState::new(
            "copilot".into(),
            Some("claude-sonnet-4.6".into()),
            BillingType::QuotaMonthly { quota_total: 300 },
        );
        assert_eq!(state.remaining(), Some(300));
        assert!(!state.is_depleted());

        for _ in 0..300 {
            state.record_usage(0.0);
        }
        assert_eq!(state.remaining(), Some(0));
        assert!(state.is_depleted());
    }

    #[test]
    fn test_free_tier_credits() {
        let mut state = QuotaState::new(
            "google".into(),
            None,
            BillingType::FreeTierCredits {
                free_credits_usd: 10.0,
            },
        );
        assert_eq!(state.remaining_credits_usd(), Some(10.0));
        assert!(!state.is_depleted());

        state.record_usage(7.50);
        assert_eq!(state.remaining_credits_usd(), Some(2.50));
        assert!(!state.is_depleted());

        state.record_usage(3.00);
        assert!(state.is_depleted());
    }

    #[test]
    fn test_unlimited_never_depleted() {
        let state = QuotaState::new(
            "kimi".into(),
            None,
            BillingType::UnlimitedSubscription,
        );
        assert!(!state.is_depleted());
        assert_eq!(state.remaining(), None);
    }
}
