use crate::types::QualityTier;
use serde::{Deserialize, Serialize};

/// Result of scoring a request across all 15 dimensions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringResult {
    /// The determined complexity tier
    pub tier: QualityTier,
    /// Raw weighted score (0.0 - 1.0+)
    pub score: f64,
    /// Confidence after sigmoid calibration (0.0 - 1.0)
    pub confidence: f64,
    /// Individual dimension scores
    pub dimensions: DimensionScores,
    /// Which routing method was used
    pub method: String,
    /// Human-readable reasoning
    pub reasoning: String,
}

/// Scores for each of the 15 dimensions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DimensionScores {
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
