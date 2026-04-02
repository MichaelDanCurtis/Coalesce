use super::billing::BillingType;
use super::marginal_cost::MarginalCost;
use super::quota_tracker::QuotaState;
use crate::types::ModelInfo;
use dashmap::DashMap;
use std::sync::Arc;

/// The Provider Economics Engine.
/// Computes real-time marginal cost for every model based on subscription state.
pub struct EconomicsEngine {
    /// Current quota states keyed by "provider" or "provider:model"
    quotas: Arc<DashMap<String, QuotaState>>,
}

impl EconomicsEngine {
    pub fn new() -> Self {
        Self {
            quotas: Arc::new(DashMap::new()),
        }
    }

    /// Register a provider's billing model.
    pub fn register(&self, provider: &str, model: Option<&str>, billing: BillingType) {
        let key = quota_key(provider, model);
        self.quotas.insert(
            key,
            QuotaState::new(provider.to_string(), model.map(|s| s.to_string()), billing),
        );
    }

    /// Compute the marginal cost of a request to a specific model.
    pub fn marginal_cost(
        &self,
        model: &ModelInfo,
        estimated_input_tokens: u32,
        estimated_output_tokens: u32,
    ) -> MarginalCost {
        // Check model-level quota first, then provider-level
        let model_key = quota_key(&model.provider, Some(&model.id));
        let provider_key = quota_key(&model.provider, None::<&str>);

        let quota = self
            .quotas
            .get(&model_key)
            .or_else(|| self.quotas.get(&provider_key));

        match quota {
            Some(ref q) => self.compute_cost(q.value(), model, estimated_input_tokens, estimated_output_tokens),
            None => {
                // No quota tracking — assume per-token
                let cost = estimate_token_cost(
                    model,
                    estimated_input_tokens,
                    estimated_output_tokens,
                );
                if cost == 0.0 {
                    MarginalCost::Free {
                        reason: "no pricing data".to_string(),
                    }
                } else {
                    MarginalCost::Paid {
                        estimated_usd: cost,
                        input_price_per_m: model.input_price_per_m,
                        output_price_per_m: model.output_price_per_m,
                    }
                }
            }
        }
    }

    fn compute_cost(
        &self,
        quota: &QuotaState,
        model: &ModelInfo,
        estimated_input_tokens: u32,
        estimated_output_tokens: u32,
    ) -> MarginalCost {
        match &quota.billing {
            BillingType::Local => MarginalCost::Free {
                reason: "local model".to_string(),
            },
            BillingType::UnlimitedSubscription => MarginalCost::Free {
                reason: "unlimited subscription".to_string(),
            },
            BillingType::FreeIncluded => MarginalCost::Free {
                reason: "included with plan".to_string(),
            },
            BillingType::QuotaMonthly { .. } => {
                if quota.is_depleted() {
                    MarginalCost::Unavailable {
                        reason: format!(
                            "monthly quota exhausted ({} used)",
                            quota.used
                        ),
                    }
                } else {
                    MarginalCost::Free {
                        reason: format!(
                            "within monthly quota ({} remaining)",
                            quota.remaining().unwrap_or(0)
                        ),
                    }
                }
            }
            BillingType::QuotaRefreshing { .. } => {
                if quota.is_depleted() {
                    let secs = quota.seconds_until_refresh().unwrap_or(0);
                    MarginalCost::Depleted {
                        refreshes_in_secs: secs,
                        reason: format!(
                            "quota depleted, refreshes in {}m {}s",
                            secs / 60,
                            secs % 60
                        ),
                    }
                } else {
                    MarginalCost::Free {
                        reason: format!(
                            "within refresh window ({} remaining)",
                            quota.remaining().unwrap_or(0)
                        ),
                    }
                }
            }
            BillingType::FreeTierCredits { .. } => {
                let remaining = quota.remaining_credits_usd().unwrap_or(0.0);
                let est_cost = estimate_token_cost(
                    model,
                    estimated_input_tokens,
                    estimated_output_tokens,
                );
                if remaining >= est_cost {
                    MarginalCost::Free {
                        reason: format!("within free credits (${:.2} remaining)", remaining),
                    }
                } else {
                    MarginalCost::Paid {
                        estimated_usd: est_cost,
                        input_price_per_m: model.input_price_per_m,
                        output_price_per_m: model.output_price_per_m,
                    }
                }
            }
            BillingType::PerToken => {
                let cost = estimate_token_cost(
                    model,
                    estimated_input_tokens,
                    estimated_output_tokens,
                );
                MarginalCost::Paid {
                    estimated_usd: cost,
                    input_price_per_m: model.input_price_per_m,
                    output_price_per_m: model.output_price_per_m,
                }
            }
            BillingType::QuotaOnly { .. } => {
                // Quota-only: free while quota lasts, UNAVAILABLE when depleted
                if quota.is_depleted() {
                    let secs = quota.seconds_until_refresh().unwrap_or(0);
                    if secs > 0 {
                        MarginalCost::Depleted {
                            refreshes_in_secs: secs,
                            reason: format!(
                                "quota exhausted, refreshes in {}m {}s",
                                secs / 60,
                                secs % 60
                            ),
                        }
                    } else {
                        MarginalCost::Unavailable {
                            reason: "quota exhausted (subscription limit reached)".to_string(),
                        }
                    }
                } else {
                    MarginalCost::Free {
                        reason: format!(
                            "within subscription quota ({} remaining)",
                            quota.remaining().unwrap_or(0)
                        ),
                    }
                }
            }
            BillingType::QuotaMetered { .. } => {
                // Quota + metered: free while quota lasts, PAID when depleted
                if quota.is_depleted() {
                    let cost = estimate_token_cost(
                        model,
                        estimated_input_tokens,
                        estimated_output_tokens,
                    );
                    MarginalCost::Paid {
                        estimated_usd: cost,
                        input_price_per_m: model.input_price_per_m,
                        output_price_per_m: model.output_price_per_m,
                    }
                } else {
                    MarginalCost::Free {
                        reason: format!(
                            "within quota ({} remaining, then per-token)",
                            quota.remaining().unwrap_or(0)
                        ),
                    }
                }
            }
        }
    }

    /// Record that a request was completed, updating quota tracking.
    pub fn record_usage(&self, provider: &str, model_id: &str, cost_usd: f64) {
        let model_key = quota_key(provider, Some(model_id));
        let provider_key = quota_key(provider, None::<&str>);

        if let Some(mut q) = self.quotas.get_mut(&model_key) {
            q.record_usage(cost_usd);
        } else if let Some(mut q) = self.quotas.get_mut(&provider_key) {
            q.record_usage(cost_usd);
        }
    }

    /// Get all provider quota states for dashboard display.
    pub fn all_quotas(&self) -> Vec<QuotaState> {
        self.quotas.iter().map(|entry| entry.value().clone()).collect()
    }
}

impl Default for EconomicsEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn quota_key(provider: &str, model: Option<impl AsRef<str>>) -> String {
    match model {
        Some(m) => format!("{}:{}", provider, m.as_ref()),
        None => provider.to_string(),
    }
}

fn estimate_token_cost(
    model: &ModelInfo,
    input_tokens: u32,
    output_tokens: u32,
) -> f64 {
    let input_cost = (input_tokens as f64 / 1_000_000.0) * model.input_price_per_m;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * model.output_price_per_m;
    input_cost + output_cost
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::QualityTier;

    fn test_model(provider: &str, id: &str, input_price: f64, output_price: f64) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            provider: provider.to_string(),
            input_price_per_m: input_price,
            output_price_per_m: output_price,
            context_window: 128000,
            max_output: Some(4096),
            quality_tier: QualityTier::Complex,
            reasoning: false,
            vision: false,
            tool_calling: true,
        }
    }

    #[test]
    fn test_free_providers_are_free() {
        let engine = EconomicsEngine::new();
        engine.register("ollama", None::<&str>, BillingType::Local);
        engine.register("kimi", None::<&str>, BillingType::UnlimitedSubscription);
        engine.register("copilot", Some("gpt-4.1"), BillingType::FreeIncluded);

        let ollama_model = test_model("ollama", "llama3", 0.0, 0.0);
        let kimi_model = test_model("kimi", "kimi-k2.5", 0.0, 0.0);
        let copilot_model = test_model("copilot", "gpt-4.1", 0.0, 0.0);

        assert!(engine.marginal_cost(&ollama_model, 1000, 500).is_free());
        assert!(engine.marginal_cost(&kimi_model, 1000, 500).is_free());
        assert!(engine.marginal_cost(&copilot_model, 1000, 500).is_free());
    }

    #[test]
    fn test_per_token_provider() {
        let engine = EconomicsEngine::new();
        engine.register("openrouter", None::<&str>, BillingType::PerToken);

        let model = test_model("openrouter", "anthropic/claude-sonnet-4", 3.0, 15.0);
        let cost = engine.marginal_cost(&model, 1000, 500);
        assert!(!cost.is_free());
        match cost {
            MarginalCost::Paid { estimated_usd, .. } => {
                // 1000 input @ $3/M + 500 output @ $15/M = $0.003 + $0.0075 = $0.0105
                assert!((estimated_usd - 0.0105).abs() < 0.001);
            }
            _ => panic!("Expected Paid cost"),
        }
    }

    #[test]
    fn test_quota_depletion() {
        let engine = EconomicsEngine::new();
        engine.register(
            "copilot",
            Some("claude-sonnet-4.6"),
            BillingType::QuotaMonthly { quota_total: 3 },
        );

        let model = test_model("copilot", "claude-sonnet-4.6", 0.0, 0.0);

        // First 3 requests are free
        assert!(engine.marginal_cost(&model, 1000, 500).is_free());
        engine.record_usage("copilot", "claude-sonnet-4.6", 0.0);
        engine.record_usage("copilot", "claude-sonnet-4.6", 0.0);
        engine.record_usage("copilot", "claude-sonnet-4.6", 0.0);

        // 4th request should be unavailable
        let cost = engine.marginal_cost(&model, 1000, 500);
        assert!(!cost.is_available());
    }

    #[test]
    fn test_priority_cascade() {
        let engine = EconomicsEngine::new();
        engine.register("ollama", None::<&str>, BillingType::Local);
        engine.register("kimi", None::<&str>, BillingType::UnlimitedSubscription);
        engine.register("openrouter", None::<&str>, BillingType::PerToken);

        let models = vec![
            test_model("openrouter", "gpt-5", 5.0, 15.0),
            test_model("ollama", "llama3", 0.0, 0.0),
            test_model("kimi", "kimi-k2.5", 0.0, 0.0),
        ];

        let costs: Vec<_> = models
            .iter()
            .map(|m| engine.marginal_cost(m, 1000, 500))
            .collect();

        // Ollama and Kimi should be free, OpenRouter should be paid
        assert!(costs[0].usd_value() > 0.0, "OpenRouter should cost money");
        assert_eq!(costs[1].usd_value(), 0.0, "Ollama should be free");
        assert_eq!(costs[2].usd_value(), 0.0, "Kimi should be free");
    }
}
