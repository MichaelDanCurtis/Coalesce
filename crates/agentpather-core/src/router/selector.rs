use crate::economics::MarginalCost;
use crate::types::{ModelInfo, QualityTier};

/// Select the best model for a given tier from available models,
/// using the economics engine to determine real-time cost.
pub fn select_model<'a>(
    tier: QualityTier,
    strategy: &str,
    models: &'a [ModelInfo],
    costs: &[MarginalCost],
) -> Option<(&'a ModelInfo, String)> {
    // Filter to models that can handle this tier
    let mut candidates: Vec<(usize, &ModelInfo)> = models
        .iter()
        .enumerate()
        .filter(|(_, m)| m.quality_tier.can_handle(&tier))
        .collect();

    if candidates.is_empty() {
        return None;
    }

    match strategy {
        "best_quality" => {
            // Sort by quality tier (highest first), then by name for stability
            candidates.sort_by(|(_, a), (_, b)| {
                b.quality_tier
                    .cmp(&a.quality_tier)
                    .then_with(|| a.name.cmp(&b.name))
            });
            let (idx, model) = candidates[0];
            Some((model, reason_for(idx, costs).to_string()))
        }
        "free_only" => {
            // Only pick models with $0 marginal cost
            let free: Vec<_> = candidates
                .iter()
                .filter(|(idx, _)| {
                    costs
                        .get(*idx)
                        .is_some_and(|c| matches!(c, MarginalCost::Free { .. }))
                })
                .collect();

            if let Some(&&(idx, model)) = free.first() {
                Some((model, reason_for(idx, costs).to_string()))
            } else {
                None
            }
        }
        _ => {
            // "cheapest_capable" (default): sort by marginal cost ascending
            candidates.sort_by(|(idx_a, _), (idx_b, _)| {
                let cost_a = costs.get(*idx_a).map(|c| c.usd_value()).unwrap_or(f64::MAX);
                let cost_b = costs.get(*idx_b).map(|c| c.usd_value()).unwrap_or(f64::MAX);
                cost_a
                    .partial_cmp(&cost_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let (idx, model) = candidates[0];
            Some((model, reason_for(idx, costs).to_string()))
        }
    }
}

fn reason_for<'a>(_idx: usize, _costs: &'a [MarginalCost]) -> &'a str {
    // In a real implementation, this would return the reason from the cost entry
    "cheapest capable model"
}
