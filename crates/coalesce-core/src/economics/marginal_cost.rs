use serde::{Deserialize, Serialize};

/// The real-time marginal cost of a specific request to a specific model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarginalCost {
    /// $0 — provider is free for this request
    Free {
        /// Why it's free (e.g., "local model", "included with subscription", "within quota")
        reason: String,
    },
    /// The model/provider is temporarily depleted but will refresh
    Depleted {
        /// When the quota refreshes (seconds from now)
        refreshes_in_secs: u64,
        /// Human-readable description
        reason: String,
    },
    /// The model/provider is unavailable (quota exhausted, no refresh coming soon)
    Unavailable {
        reason: String,
    },
    /// This request will cost money
    Paid {
        /// Estimated cost in USD for this request
        estimated_usd: f64,
        /// Price per million input tokens
        input_price_per_m: f64,
        /// Price per million output tokens
        output_price_per_m: f64,
    },
}

impl MarginalCost {
    /// Get the USD value for sorting. Free = 0, Depleted = MAX/2, Unavailable = MAX, Paid = actual
    pub fn usd_value(&self) -> f64 {
        match self {
            MarginalCost::Free { .. } => 0.0,
            MarginalCost::Depleted { .. } => f64::MAX / 2.0,
            MarginalCost::Unavailable { .. } => f64::MAX,
            MarginalCost::Paid { estimated_usd, .. } => *estimated_usd,
        }
    }

    /// Is this effectively free right now?
    pub fn is_free(&self) -> bool {
        matches!(self, MarginalCost::Free { .. })
    }

    /// Is this available at all?
    pub fn is_available(&self) -> bool {
        !matches!(self, MarginalCost::Unavailable { .. })
    }
}
