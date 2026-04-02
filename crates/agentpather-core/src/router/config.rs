use crate::types::QualityTier;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Routing configuration loaded from TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Named routing profiles
    #[serde(default = "default_profiles")]
    pub profiles: HashMap<String, RoutingProfile>,
    /// Dimension weights (can be customized)
    #[serde(default = "DimensionWeights::default")]
    pub weights: DimensionWeights,
    /// Tier thresholds
    #[serde(default = "TierThresholds::default")]
    pub thresholds: TierThresholds,
    /// Model pins per tier — ordered list of preferred model IDs.
    /// Pinned models are tried first (in order) before falling back to cost-based ranking.
    #[serde(default)]
    pub model_pins: HashMap<QualityTier, Vec<ModelPin>>,
    /// Model equivalence map — groups provider-specific model IDs under a canonical name.
    /// Example: `{ "claude-opus-4.6": ["claude-opus-4-6-thinking", "claude-opus-4.6"] }`
    /// The router treats all aliases as the same model for pin matching and cost comparison.
    #[serde(default)]
    pub model_equivalences: HashMap<String, Vec<String>>,
}

/// A pinned model preference for a tier.
/// Each pin represents a model with an ordered list of providers to try.
/// The router tries each provider in order, falling back to the next
/// if the current one is unavailable or over budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPin {
    /// Model ID (e.g. "claude-opus-4-6") — or a common name that maps across providers
    pub model_id: String,
    /// Ordered list of providers to try for this model.
    /// e.g. ["copilot", "google", "openrouter"] — tries copilot first, then google, then openrouter
    pub providers: Vec<String>,
}

impl RoutingConfig {
    /// Resolve a provider-specific model ID to its canonical name.
    /// If the model isn't in any equivalence group, returns the original ID.
    pub fn canonical_model_id(&self, model_id: &str) -> String {
        for (canonical, aliases) in &self.model_equivalences {
            if canonical == model_id || aliases.iter().any(|a| a == model_id) {
                return canonical.clone();
            }
        }
        model_id.to_string()
    }

    /// Check if two model IDs are equivalent (same canonical name).
    pub fn models_equivalent(&self, a: &str, b: &str) -> bool {
        self.canonical_model_id(a) == self.canonical_model_id(b)
    }
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            profiles: default_profiles(),
            weights: DimensionWeights::default(),
            thresholds: TierThresholds::default(),
            model_pins: HashMap::new(),
            model_equivalences: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingProfile {
    pub description: String,
    #[serde(default = "default_strategy")]
    pub strategy: String,
    #[serde(default)]
    pub tiers: HashMap<String, TierConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Minimum quality tier for models in this tier
    #[serde(default = "default_min_quality")]
    pub min_quality: String,
    /// Prefer local models (Ollama) for this tier
    #[serde(default)]
    pub prefer_local: bool,
    /// Allow waiting for quota refresh
    #[serde(default)]
    pub allow_wait: bool,
    /// Max seconds to wait for a free provider to refresh
    #[serde(default)]
    pub max_wait_secs: u64,
}

/// Weights for the 15 scoring dimensions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionWeights {
    pub token_count: f64,
    pub code_presence: f64,
    pub reasoning_markers: f64,
    pub technical_terms: f64,
    pub creative_markers: f64,
    pub simple_indicators: f64,
    pub multi_step: f64,
    pub question_complexity: f64,
    pub imperative_verbs: f64,
    pub constraints: f64,
    pub output_format: f64,
    pub reference_keywords: f64,
    pub negation_keywords: f64,
    pub domain_specific: f64,
    pub agentic_keywords: f64,
}

impl Default for DimensionWeights {
    fn default() -> Self {
        Self {
            token_count: 0.15,
            code_presence: 0.12,
            reasoning_markers: 0.15,
            technical_terms: 0.08,
            creative_markers: 0.06,
            simple_indicators: 0.10,
            multi_step: 0.08,
            question_complexity: 0.04,
            imperative_verbs: 0.05,
            constraints: 0.04,
            output_format: 0.03,
            reference_keywords: 0.02,
            negation_keywords: 0.02,
            domain_specific: 0.03,
            agentic_keywords: 0.03,
        }
    }
}

/// Thresholds for tier classification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierThresholds {
    pub simple_max: f64,
    pub medium_max: f64,
    pub complex_max: f64,
    // Anything above complex_max with reasoning markers → REASONING
}

impl Default for TierThresholds {
    fn default() -> Self {
        Self {
            simple_max: 0.12,
            medium_max: 0.25,
            complex_max: 0.40,
        }
    }
}

fn default_profiles() -> HashMap<String, RoutingProfile> {
    let mut profiles = HashMap::new();

    profiles.insert(
        "auto".to_string(),
        RoutingProfile {
            description: "Balanced cost/quality".to_string(),
            strategy: "cheapest_capable".to_string(),
            tiers: HashMap::new(),
        },
    );

    profiles.insert(
        "eco".to_string(),
        RoutingProfile {
            description: "Maximum savings — free sources only".to_string(),
            strategy: "free_only".to_string(),
            tiers: HashMap::new(),
        },
    );

    profiles.insert(
        "premium".to_string(),
        RoutingProfile {
            description: "Best quality — ignore cost".to_string(),
            strategy: "best_quality".to_string(),
            tiers: HashMap::new(),
        },
    );

    profiles
}

fn default_strategy() -> String {
    "cheapest_capable".to_string()
}

fn default_min_quality() -> String {
    "simple".to_string()
}
