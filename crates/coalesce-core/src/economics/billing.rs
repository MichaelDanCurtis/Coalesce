use serde::{Deserialize, Serialize};

/// The billing model for a provider or specific model within a provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BillingType {
    /// Always free, runs on local hardware (e.g., Ollama)
    Local,
    /// Free unlimited access, included with subscription (e.g., Copilot GPT-4.1)
    FreeIncluded,
    /// Monthly quota of free requests, unavailable when exhausted (e.g., Copilot premium)
    QuotaMonthly {
        /// Total requests per month
        quota_total: u32,
    },
    /// Quota that refreshes on a time interval (e.g., Anthropic 5h window)
    QuotaRefreshing {
        /// Requests per refresh window
        quota_per_window: u32,
        /// Refresh interval in seconds
        refresh_interval_secs: u64,
    },
    /// Flat subscription fee, effectively unlimited (e.g., Kimi K2)
    UnlimitedSubscription,
    /// Free credits included with subscription, then per-token (e.g., Google AI Pro)
    FreeTierCredits {
        /// Monthly free credit amount in USD
        free_credits_usd: f64,
    },
    /// Pay per token, always costs money (e.g., OpenRouter, direct APIs)
    PerToken,
    /// Quota-only: free while quota lasts, then STOPS (provider becomes unavailable).
    /// Use for subscription providers like Google/Antigravity where exceeding quota
    /// means you're cut off, not charged more.
    QuotaOnly {
        /// Requests per refresh window
        quota_per_window: u32,
        /// Refresh interval in seconds (0 = monthly, never auto-refreshes)
        refresh_interval_secs: u64,
    },
    /// Quota + metered: free while quota lasts, then switches to per-token pricing.
    /// Use for providers like Copilot where included requests are free but you can
    /// keep going at a per-token rate after quota is exhausted.
    QuotaMetered {
        /// Requests per refresh window
        quota_per_window: u32,
        /// Refresh interval in seconds
        refresh_interval_secs: u64,
    },
}

impl std::fmt::Display for BillingType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BillingType::Local => write!(f, "local"),
            BillingType::FreeIncluded => write!(f, "free"),
            BillingType::QuotaMonthly { quota_total } => write!(f, "quota_monthly:{}", quota_total),
            BillingType::QuotaRefreshing { quota_per_window, refresh_interval_secs } => write!(f, "quota_refreshing:{}:{}", quota_per_window, refresh_interval_secs),
            BillingType::UnlimitedSubscription => write!(f, "unlimited"),
            BillingType::FreeTierCredits { free_credits_usd } => write!(f, "free_credits:{}", free_credits_usd),
            BillingType::PerToken => write!(f, "per_token"),
            BillingType::QuotaOnly { quota_per_window, refresh_interval_secs } => write!(f, "quota_only:{}:{}", quota_per_window, refresh_interval_secs),
            BillingType::QuotaMetered { quota_per_window, refresh_interval_secs } => write!(f, "quota_metered:{}:{}", quota_per_window, refresh_interval_secs),
        }
    }
}

impl BillingType {
    /// Priority band for routing (lower = preferred).
    /// Used in the cascade: free(0) → wait(1) → paid(2)
    pub fn priority_band(&self) -> u8 {
        match self {
            BillingType::Local => 0,
            BillingType::UnlimitedSubscription => 0,
            BillingType::FreeIncluded => 0,
            BillingType::QuotaMonthly { .. } => 0, // $0 when quota available
            BillingType::QuotaRefreshing { .. } => 0, // $0 when quota available
            BillingType::FreeTierCredits { .. } => 0, // $0 when credits available
            BillingType::QuotaOnly { .. } => 0,     // $0 while quota lasts, then unavailable
            BillingType::QuotaMetered { .. } => 0,  // $0 while quota lasts, then per-token
            BillingType::PerToken => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_billing_priority() {
        assert_eq!(BillingType::Local.priority_band(), 0);
        assert_eq!(BillingType::UnlimitedSubscription.priority_band(), 0);
        assert_eq!(BillingType::PerToken.priority_band(), 2);
    }

    #[test]
    fn test_billing_serde() {
        let bt = BillingType::QuotaRefreshing {
            quota_per_window: 45,
            refresh_interval_secs: 18000,
        };
        let json = serde_json::to_string(&bt).unwrap();
        let parsed: BillingType = serde_json::from_str(&json).unwrap();
        assert_eq!(bt, parsed);
    }
}
