use super::config::RoutingConfig;
use super::keywords::*;
use super::types::{DimensionScores, ScoringResult};
use crate::types::QualityTier;

/// Score text across 15 dimensions and classify into a tier.
pub fn score(text: &str, config: &RoutingConfig) -> ScoringResult {
    let w = &config.weights;
    let t = &config.thresholds;

    let dims = score_dimensions(text);

    // Compute weighted sum
    let weighted_sum = dims.token_count * w.token_count
        + dims.code_presence * w.code_presence
        + dims.reasoning_markers * w.reasoning_markers
        + dims.technical_terms * w.technical_terms
        + dims.creative_markers * w.creative_markers
        + dims.simple_indicators * w.simple_indicators
        + dims.multi_step * w.multi_step
        + dims.question_complexity * w.question_complexity
        + dims.imperative_verbs * w.imperative_verbs
        + dims.constraints * w.constraints
        + dims.output_format * w.output_format
        + dims.reference_keywords * w.reference_keywords
        + dims.negation_keywords * w.negation_keywords
        + dims.domain_specific * w.domain_specific
        + dims.agentic_keywords * w.agentic_keywords;

    // Sigmoid calibration for confidence
    let confidence = sigmoid(weighted_sum, 5.0, 0.5);

    // Determine tier. Reasoning markers can promote directly regardless of score.
    let has_reasoning = dims.reasoning_markers > 0.3;
    let has_strong_simple = dims.simple_indicators > 0.6
        && dims.code_presence < 0.1
        && dims.technical_terms < 0.1
        && !has_reasoning;

    let tier = if has_strong_simple && weighted_sum < t.simple_max + 0.1 {
        QualityTier::Simple
    } else if has_reasoning && (weighted_sum >= t.simple_max || dims.reasoning_markers > 0.6) {
        QualityTier::Reasoning
    } else if weighted_sum >= t.complex_max {
        QualityTier::Complex
    } else if weighted_sum >= t.simple_max {
        QualityTier::Medium
    } else {
        QualityTier::Simple
    };

    // Build reasoning string
    let mut reasons = Vec::new();
    if dims.code_presence > 0.3 {
        reasons.push("contains code");
    }
    if has_reasoning {
        reasons.push("reasoning requested");
    }
    if dims.technical_terms > 0.3 {
        reasons.push("technical content");
    }
    if dims.simple_indicators > 0.5 {
        reasons.push("simple query");
    }
    if dims.multi_step > 0.3 {
        reasons.push("multi-step task");
    }
    if dims.agentic_keywords > 0.3 {
        reasons.push("agentic workflow");
    }
    if reasons.is_empty() {
        reasons.push("general request");
    }

    ScoringResult {
        tier,
        score: weighted_sum,
        confidence,
        dimensions: dims,
        method: "rules".to_string(),
        reasoning: reasons.join(", "),
    }
}

/// Score each of the 15 dimensions independently.
/// Each dimension returns a value between 0.0 and 1.0.
fn score_dimensions(text: &str) -> DimensionScores {
    let estimated_tokens = estimate_tokens(text);

    DimensionScores {
        // 1. Token count — longer prompts are more complex
        token_count: normalize_token_count(estimated_tokens),

        // 2. Code presence — code blocks or code keywords
        code_presence: score_code_presence(text),

        // 3. Reasoning markers
        reasoning_markers: normalize_keyword_count(count_matches(text, REASONING_KEYWORDS), 3),

        // 4. Technical terms
        technical_terms: normalize_keyword_count(count_matches(text, TECHNICAL_KEYWORDS), 4),

        // 5. Creative markers
        creative_markers: normalize_keyword_count(count_matches(text, CREATIVE_KEYWORDS), 3),

        // 6. Simple indicators (inversely correlated with complexity)
        simple_indicators: normalize_keyword_count(count_matches(text, SIMPLE_KEYWORDS), 2),

        // 7. Multi-step patterns
        multi_step: normalize_keyword_count(count_matches(text, MULTI_STEP_KEYWORDS), 3),

        // 8. Question complexity (count of "?")
        question_complexity: {
            let q_count = text.matches('?').count();
            if q_count > 3 {
                1.0
            } else {
                q_count as f64 / 3.0
            }
        },

        // 9. Imperative verbs
        imperative_verbs: normalize_keyword_count(count_matches(text, IMPERATIVE_KEYWORDS), 3),

        // 10. Constraint indicators
        constraints: normalize_keyword_count(count_matches(text, CONSTRAINT_KEYWORDS), 3),

        // 11. Output format keywords
        output_format: normalize_keyword_count(count_matches(text, OUTPUT_FORMAT_KEYWORDS), 2),

        // 12. Reference keywords
        reference_keywords: normalize_keyword_count(count_matches(text, REFERENCE_KEYWORDS), 2),

        // 13. Negation keywords
        negation_keywords: normalize_keyword_count(count_matches(text, NEGATION_KEYWORDS), 3),

        // 14. Domain-specific keywords
        domain_specific: normalize_keyword_count(count_matches(text, DOMAIN_SPECIFIC_KEYWORDS), 3),

        // 15. Agentic keywords
        agentic_keywords: normalize_keyword_count(count_matches(text, AGENTIC_KEYWORDS), 3),
    }
}

/// Normalize token count to 0.0-1.0 range.
/// <100 tokens → 0.0, >2000 tokens → 1.0
fn normalize_token_count(tokens: usize) -> f64 {
    if tokens < 100 {
        0.0
    } else if tokens > 2000 {
        1.0
    } else {
        (tokens as f64 - 100.0) / 1900.0
    }
}

/// Score code presence: code blocks + code keywords.
fn score_code_presence(text: &str) -> f64 {
    let mut score = 0.0;
    if has_code_blocks(text) {
        score += 0.6;
    }
    let kw_count = count_matches(text, CODE_KEYWORDS);
    score += (kw_count as f64 / 5.0).min(0.4);
    score.min(1.0)
}

/// Normalize keyword count to 0.0-1.0. `saturate_at` matches → 1.0.
fn normalize_keyword_count(count: usize, saturate_at: usize) -> f64 {
    if saturate_at == 0 {
        return 0.0;
    }
    (count as f64 / saturate_at as f64).min(1.0)
}

/// Sigmoid function for confidence calibration
fn sigmoid(x: f64, steepness: f64, midpoint: f64) -> f64 {
    1.0 / (1.0 + (-steepness * (x - midpoint)).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> RoutingConfig {
        RoutingConfig::default()
    }

    #[test]
    fn test_simple_query() {
        let result = score("What is the capital of France?", &default_config());
        assert_eq!(result.tier, QualityTier::Simple);
        assert!(result.score < 0.3, "Score {} should be < 0.3", result.score);
    }

    #[test]
    fn test_medium_query() {
        let result = score(
            "Explain the difference between a hash map and a binary tree. \
             Compare their time complexity for insertion, deletion, and lookup operations.",
            &default_config(),
        );
        assert!(
            result.tier == QualityTier::Medium || result.tier == QualityTier::Complex,
            "Tier {:?} should be Medium or Complex",
            result.tier
        );
    }

    #[test]
    fn test_complex_code_query() {
        let result = score(
            "Build a distributed consensus algorithm in Rust that handles \
             network partitions. The implementation must use async/await with tokio, \
             implement the Raft protocol, and include proper error handling. \
             ```rust\nuse tokio::sync::Mutex;\n```",
            &default_config(),
        );
        assert!(
            result.tier >= QualityTier::Medium,
            "Tier {:?} should be at least Medium (score: {})",
            result.tier, result.score
        );
    }

    #[test]
    fn test_reasoning_query() {
        let result = score(
            "Think step by step and analyze the following proof. Consider each premise \
             carefully, evaluate the logical chain, and derive the conclusion. \
             Prove that P implies Q given the following axioms. \
             Reason through each step and examine whether the argument is valid.",
            &default_config(),
        );
        assert_eq!(
            result.tier,
            QualityTier::Reasoning,
            "Score: {}, reasoning_markers: {}",
            result.score,
            result.dimensions.reasoning_markers
        );
    }

    #[test]
    fn test_code_detection() {
        let result = score(
            "Here's my function:\n```python\ndef hello():\n    return 'world'\n```\nFix the bug.",
            &default_config(),
        );
        assert!(
            result.dimensions.code_presence > 0.5,
            "Code presence {} should be > 0.5",
            result.dimensions.code_presence
        );
    }

    #[test]
    fn test_agentic_query() {
        let result = score(
            "Create an autonomous agent that can iterate on its own output, \
             use tool calls to gather information, and self-correct based on \
             observations. The agent should plan, execute, and reflect in a loop.",
            &default_config(),
        );
        assert!(
            result.dimensions.agentic_keywords > 0.3,
            "Agentic score {} should be > 0.3",
            result.dimensions.agentic_keywords
        );
    }

    #[test]
    fn test_dimension_scores_bounded() {
        let result = score(
            "This is a test with many keywords: algorithm, function, class, \
             think, analyze, prove, build, implement, must, required, json, \
             cite, not, avoid, diagnosis, iterate, refine",
            &default_config(),
        );
        let d = &result.dimensions;
        assert!(d.token_count >= 0.0 && d.token_count <= 1.0);
        assert!(d.code_presence >= 0.0 && d.code_presence <= 1.0);
        assert!(d.reasoning_markers >= 0.0 && d.reasoning_markers <= 1.0);
        assert!(d.simple_indicators >= 0.0 && d.simple_indicators <= 1.0);
    }

    #[test]
    fn test_sigmoid_calibration() {
        let result = score("What is 2+2?", &default_config());
        assert!(
            result.confidence >= 0.0 && result.confidence <= 1.0,
            "Confidence {} should be in [0, 1]",
            result.confidence
        );
    }

    #[test]
    fn test_chinese_query() {
        let result = score("请分析这个算法的时间复杂度，逐步推理", &default_config());
        // Should detect reasoning markers (推理) and technical terms (算法, 复杂度)
        assert!(
            result.dimensions.reasoning_markers > 0.0 || result.dimensions.technical_terms > 0.0,
            "Should detect Chinese keywords"
        );
    }
}
