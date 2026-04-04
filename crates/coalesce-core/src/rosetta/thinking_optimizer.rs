use crate::types::QualityTier;

/// Thinking optimizer — auto-configures extended thinking based on model,
/// complexity score, and budget constraints.
pub struct ThinkingOptimizer {
    pub config: ThinkingOptimizerConfig,
}

#[derive(Debug, Clone)]
pub struct ThinkingOptimizerConfig {
    pub enabled: bool,
    /// Minimum complexity score to enable thinking (0.0–1.0)
    pub min_complexity_for_thinking: f64,
    /// Maximum budget_tokens for thinking (provider-specific caps apply)
    pub max_budget_tokens: u32,
    /// Default budget_tokens when thinking is enabled
    pub default_budget_tokens: u32,
}

impl Default for ThinkingOptimizerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_complexity_for_thinking: 0.3,
            max_budget_tokens: 32768,
            default_budget_tokens: 8192,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThinkingDecision {
    /// Whether to enable thinking for this request
    pub enable_thinking: bool,
    /// Budget tokens to allocate (None = provider default)
    pub budget_tokens: Option<u32>,
    /// Reason for the decision
    pub reason: &'static str,
}

/// Model thinking capability profile
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThinkingCapability {
    /// No thinking support (most models)
    None,
    /// Native extended thinking (Claude 3.5+, o1/o3)
    Native,
    /// Reasoning via thinking param (GLM 4.5+, DeepSeek-R1)
    ThinkingParam,
    /// Reasoning via <think> tags in output (Qwen, some Ollama)
    ThinkTags,
}

impl ThinkingOptimizer {
    pub fn new(config: ThinkingOptimizerConfig) -> Self {
        Self { config }
    }

    /// Decide whether to enable thinking for a given request context.
    pub fn decide(
        &self,
        model_id: &str,
        provider: &str,
        complexity_score: f64,
        tier: &QualityTier,
        has_tools: bool,
    ) -> ThinkingDecision {
        if !self.config.enabled {
            return ThinkingDecision {
                enable_thinking: false,
                budget_tokens: None,
                reason: "optimizer disabled",
            };
        }

        let capability = Self::model_capability(model_id, provider);

        // No thinking support → skip
        if capability == ThinkingCapability::None {
            return ThinkingDecision {
                enable_thinking: false,
                budget_tokens: None,
                reason: "model does not support thinking",
            };
        }

        // Simple tier → skip thinking to save tokens
        if *tier == QualityTier::Simple {
            return ThinkingDecision {
                enable_thinking: false,
                budget_tokens: None,
                reason: "simple tier — thinking skipped",
            };
        }

        // Low complexity → skip thinking
        if complexity_score < self.config.min_complexity_for_thinking {
            return ThinkingDecision {
                enable_thinking: false,
                budget_tokens: None,
                reason: "complexity below threshold",
            };
        }

        // Tools + thinking can be problematic on some providers
        if has_tools && capability == ThinkingCapability::ThinkTags {
            return ThinkingDecision {
                enable_thinking: false,
                budget_tokens: None,
                reason: "think-tags incompatible with tools",
            };
        }

        // Scale budget by complexity
        let budget = if complexity_score > 0.8 {
            self.config.max_budget_tokens
        } else if complexity_score > 0.5 {
            self.config.default_budget_tokens
        } else {
            self.config.default_budget_tokens / 2
        };

        ThinkingDecision {
            enable_thinking: true,
            budget_tokens: Some(budget),
            reason: "thinking enabled based on complexity",
        }
    }

    /// Determine thinking capability for a model.
    pub fn model_capability(model_id: &str, provider: &str) -> ThinkingCapability {
        let id = model_id.to_lowercase();

        // Anthropic — Claude 3.5 Sonnet+, Claude 4+
        if provider == "anthropic" || id.contains("claude") {
            if id.contains("haiku") {
                return ThinkingCapability::None; // Haiku: skip thinking
            }
            if id.contains("opus") || id.contains("sonnet") {
                return ThinkingCapability::Native;
            }
        }

        // OpenAI — o1, o3 series
        if id.starts_with("o1") || id.starts_with("o3") || id.starts_with("o4") {
            return ThinkingCapability::Native;
        }

        // GLM 4.5+ — thinking param
        if id.starts_with("glm-") {
            let parts: Vec<&str> = id.strip_prefix("glm-").unwrap_or("").split('.').collect();
            if let Some(major) = parts.first().and_then(|p| p.parse::<u32>().ok()) {
                if major >= 4 {
                    if let Some(minor) = parts.get(1).and_then(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u32>().ok()) {
                        if major > 4 || minor >= 5 {
                            return ThinkingCapability::ThinkingParam;
                        }
                    }
                    if major >= 5 {
                        return ThinkingCapability::ThinkingParam;
                    }
                }
            }
        }

        // DeepSeek R1
        if id.contains("deepseek") && (id.contains("r1") || id.contains("reasoner")) {
            return ThinkingCapability::ThinkingParam;
        }

        // Kimi — has reasoning_content field
        if id.contains("kimi") {
            return ThinkingCapability::ThinkingParam;
        }

        // Qwen with thinking
        if id.contains("qwen") && (id.contains("think") || id.contains("reason")) {
            return ThinkingCapability::ThinkTags;
        }

        ThinkingCapability::None
    }

    /// Rectify malformed thinking blocks from a response.
    /// Returns cleaned content if changes were made.
    pub fn rectify_thinking(content: &str) -> Option<String> {
        // Check for unclosed <think> tags
        let open_count = content.matches("<think>").count();
        let close_count = content.matches("</think>").count();

        if open_count == close_count {
            return None; // No issues
        }

        let mut result = content.to_string();

        // Close unclosed <think> tags
        if open_count > close_count {
            for _ in 0..(open_count - close_count) {
                result.push_str("</think>");
            }
            return Some(result);
        }

        // Remove orphaned </think> tags
        if close_count > open_count {
            for _ in 0..(close_count - open_count) {
                if let Some(pos) = result.find("</think>") {
                    result = format!("{}{}", &result[..pos], &result[pos + 8..]);
                }
            }
            return Some(result);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_detection() {
        assert_eq!(ThinkingOptimizer::model_capability("claude-opus-4.6", "copilot"), ThinkingCapability::Native);
        assert_eq!(ThinkingOptimizer::model_capability("claude-haiku-3.5", "copilot"), ThinkingCapability::None);
        assert_eq!(ThinkingOptimizer::model_capability("glm-4.5", "glm"), ThinkingCapability::ThinkingParam);
        assert_eq!(ThinkingOptimizer::model_capability("glm-4.0", "glm"), ThinkingCapability::None);
        assert_eq!(ThinkingOptimizer::model_capability("glm-5", "glm"), ThinkingCapability::ThinkingParam);
        assert_eq!(ThinkingOptimizer::model_capability("o1-preview", "openai"), ThinkingCapability::Native);
        assert_eq!(ThinkingOptimizer::model_capability("gpt-4o", "openai"), ThinkingCapability::None);
        assert_eq!(ThinkingOptimizer::model_capability("kimi-for-coding", "kimi"), ThinkingCapability::ThinkingParam);
        assert_eq!(ThinkingOptimizer::model_capability("deepseek-r1", "deepseek"), ThinkingCapability::ThinkingParam);
    }

    #[test]
    fn test_decision_simple_tier() {
        let opt = ThinkingOptimizer::new(Default::default());
        let d = opt.decide("claude-opus-4.6", "copilot", 0.9, &QualityTier::Simple, false);
        assert!(!d.enable_thinking);
        assert_eq!(d.reason, "simple tier — thinking skipped");
    }

    #[test]
    fn test_decision_low_complexity() {
        let opt = ThinkingOptimizer::new(Default::default());
        let d = opt.decide("claude-opus-4.6", "copilot", 0.1, &QualityTier::Complex, false);
        assert!(!d.enable_thinking);
        assert_eq!(d.reason, "complexity below threshold");
    }

    #[test]
    fn test_decision_high_complexity() {
        let opt = ThinkingOptimizer::new(Default::default());
        let d = opt.decide("claude-opus-4.6", "copilot", 0.9, &QualityTier::Reasoning, false);
        assert!(d.enable_thinking);
        assert_eq!(d.budget_tokens, Some(32768));
    }

    #[test]
    fn test_decision_medium_complexity() {
        let opt = ThinkingOptimizer::new(Default::default());
        let d = opt.decide("glm-4.5", "glm", 0.6, &QualityTier::Complex, false);
        assert!(d.enable_thinking);
        assert_eq!(d.budget_tokens, Some(8192));
    }

    #[test]
    fn test_decision_no_thinking_model() {
        let opt = ThinkingOptimizer::new(Default::default());
        let d = opt.decide("gpt-4o", "openai", 0.9, &QualityTier::Reasoning, false);
        assert!(!d.enable_thinking);
    }

    #[test]
    fn test_rectify_unclosed_think() {
        let content = "Hello <think>some reasoning";
        let fixed = ThinkingOptimizer::rectify_thinking(content);
        assert!(fixed.is_some());
        assert!(fixed.unwrap().contains("</think>"));
    }

    #[test]
    fn test_rectify_valid() {
        let content = "Hello <think>reasoning</think> answer";
        assert!(ThinkingOptimizer::rectify_thinking(content).is_none());
    }

    #[test]
    fn test_disabled_optimizer() {
        let opt = ThinkingOptimizer::new(ThinkingOptimizerConfig {
            enabled: false,
            ..Default::default()
        });
        let d = opt.decide("claude-opus-4.6", "copilot", 0.9, &QualityTier::Reasoning, false);
        assert!(!d.enable_thinking);
    }
}
