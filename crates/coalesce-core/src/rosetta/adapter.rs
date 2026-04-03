//! Provider Tool Adapter — trait for bidirectional tool translation.
//!
//! Each provider implements this to translate between canonical form
//! and its native API format.

use super::capabilities::ProviderToolCapabilities;
use super::equivalence::EquivalenceRegistry;
use super::thinking::{CanonicalThinking, ThinkingSplitParser};
use super::types::{CanonicalTool, CanonicalToolCall, CanonicalToolChoice};

/// Trait implemented by each provider's tool adapter.
///
/// Handles schema translation, built-in tool substitution, result
/// normalization, and quirk mitigation.
pub trait ProviderToolAdapter: Send + Sync {
    /// Provider name (e.g., "anthropic", "openai", "kimi").
    fn provider_name(&self) -> &str;

    /// Translate canonical tool definitions to provider-native JSON format.
    fn translate_tools(
        &self,
        tools: &[CanonicalTool],
        registry: &EquivalenceRegistry,
    ) -> Result<Vec<serde_json::Value>, TranslationError>;

    /// Translate a provider-native tool call response back to canonical form.
    fn normalize_tool_calls(
        &self,
        raw_response: &serde_json::Value,
    ) -> Result<Vec<CanonicalToolCall>, TranslationError>;

    /// Translate canonical tool_choice to provider-native format.
    fn translate_tool_choice(
        &self,
        choice: &CanonicalToolChoice,
    ) -> serde_json::Value;

    /// Extract thinking content from a provider response.
    fn extract_thinking(
        &self,
        raw_response: &serde_json::Value,
    ) -> CanonicalThinking;

    /// Create a streaming think-split parser configured for this provider.
    /// Returns None if this provider uses native thinking blocks (no parsing needed).
    fn create_think_parser(&self) -> Option<ThinkingSplitParser>;

    /// Declare this provider's tool capabilities.
    fn capabilities(&self) -> &ProviderToolCapabilities;
}

/// Error during tool translation.
#[derive(Debug, thiserror::Error)]
pub enum TranslationError {
    #[error("Tool '{name}' has no equivalent on provider '{provider}'")]
    NoEquivalent { name: String, provider: String },

    #[error("Schema translation failed for tool '{name}': {reason}")]
    SchemaError { name: String, reason: String },

    #[error("Provider response malformed: {reason}")]
    MalformedResponse { reason: String },

    #[error("Tool count {count} exceeds provider limit {limit}")]
    TooManyTools { count: usize, limit: usize },
}

// ---------------------------------------------------------------------------
// Default implementations for OpenAI-compatible providers
// ---------------------------------------------------------------------------

/// Default adapter for OpenAI-compatible providers.
/// Works for OpenAI, Copilot, OpenRouter, DeepSeek, GLM, Google, etc.
pub struct OpenAiCompatToolAdapter {
    name: String,
    capabilities: ProviderToolCapabilities,
}

impl OpenAiCompatToolAdapter {
    pub fn new(name: impl Into<String>, capabilities: ProviderToolCapabilities) -> Self {
        Self {
            name: name.into(),
            capabilities,
        }
    }
}

impl ProviderToolAdapter for OpenAiCompatToolAdapter {
    fn provider_name(&self) -> &str {
        &self.name
    }

    fn translate_tools(
        &self,
        tools: &[CanonicalTool],
        _registry: &EquivalenceRegistry,
    ) -> Result<Vec<serde_json::Value>, TranslationError> {
        if tools.len() > self.capabilities.max_tools_hard_limit {
            return Err(TranslationError::TooManyTools {
                count: tools.len(),
                limit: self.capabilities.max_tools_hard_limit,
            });
        }
        Ok(tools.iter().map(|t| t.to_openai()).collect())
    }

    fn normalize_tool_calls(
        &self,
        raw_response: &serde_json::Value,
    ) -> Result<Vec<CanonicalToolCall>, TranslationError> {
        let mut calls = Vec::new();

        // Navigate to choices[0].message.tool_calls
        if let Some(choices) = raw_response.get("choices").and_then(|c| c.as_array()) {
            if let Some(choice) = choices.first() {
                if let Some(tool_calls) = choice
                    .get("message")
                    .and_then(|m| m.get("tool_calls"))
                    .and_then(|tc| tc.as_array())
                {
                    for tc in tool_calls {
                        if let Some(call) = CanonicalToolCall::from_openai(tc) {
                            calls.push(call);
                        }
                    }
                }
            }
        }

        Ok(calls)
    }

    fn translate_tool_choice(&self, choice: &CanonicalToolChoice) -> serde_json::Value {
        choice.to_openai()
    }

    fn extract_thinking(&self, raw_response: &serde_json::Value) -> CanonicalThinking {
        let mut thinking = CanonicalThinking::default();

        // Check for reasoning_content field (Kimi/Moonshot style)
        if let Some(choices) = raw_response.get("choices").and_then(|c| c.as_array()) {
            if let Some(choice) = choices.first() {
                if let Some(msg) = choice.get("message") {
                    let kimi_thinking = CanonicalThinking::from_kimi_response(msg);
                    if kimi_thinking.has_content() {
                        return kimi_thinking;
                    }
                }
            }
        }

        // Check for reasoning_tokens in usage (OpenAI o-series)
        if let Some(usage) = raw_response.get("usage") {
            if let Some(reasoning) = usage
                .get("completion_tokens_details")
                .and_then(|d| d.get("reasoning_tokens"))
                .and_then(|t| t.as_u64())
            {
                thinking = thinking.with_reasoning_tokens(reasoning as u32);
            }
        }

        thinking
    }

    fn create_think_parser(&self) -> Option<ThinkingSplitParser> {
        use super::capabilities::ThinkingFormat;
        match self.capabilities.thinking_format {
            ThinkingFormat::ThinkTags => Some(ThinkingSplitParser::new()),
            _ => None,
        }
    }

    fn capabilities(&self) -> &ProviderToolCapabilities {
        &self.capabilities
    }
}

/// Adapter for Anthropic's Messages API.
pub struct AnthropicToolAdapter {
    capabilities: ProviderToolCapabilities,
}

impl AnthropicToolAdapter {
    pub fn new() -> Self {
        Self {
            capabilities: ProviderToolCapabilities::anthropic(),
        }
    }
}

impl Default for AnthropicToolAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderToolAdapter for AnthropicToolAdapter {
    fn provider_name(&self) -> &str {
        "anthropic"
    }

    fn translate_tools(
        &self,
        tools: &[CanonicalTool],
        _registry: &EquivalenceRegistry,
    ) -> Result<Vec<serde_json::Value>, TranslationError> {
        if tools.len() > self.capabilities.max_tools_hard_limit {
            return Err(TranslationError::TooManyTools {
                count: tools.len(),
                limit: self.capabilities.max_tools_hard_limit,
            });
        }
        Ok(tools.iter().map(|t| t.to_anthropic()).collect())
    }

    fn normalize_tool_calls(
        &self,
        raw_response: &serde_json::Value,
    ) -> Result<Vec<CanonicalToolCall>, TranslationError> {
        let mut calls = Vec::new();

        // Anthropic: content[] blocks with type: "tool_use"
        if let Some(content) = raw_response.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let Some(call) = CanonicalToolCall::from_anthropic(block) {
                        calls.push(call);
                    }
                }
            }
        }

        Ok(calls)
    }

    fn translate_tool_choice(&self, choice: &CanonicalToolChoice) -> serde_json::Value {
        choice.to_anthropic()
    }

    fn extract_thinking(&self, raw_response: &serde_json::Value) -> CanonicalThinking {
        let mut thinking = CanonicalThinking::default();

        if let Some(content) = raw_response.get("content").and_then(|c| c.as_array()) {
            thinking = CanonicalThinking::from_anthropic_response(content);
        }

        // Anthropic reports reasoning tokens in usage
        if let Some(usage) = raw_response.get("usage") {
            if let Some(tokens) = usage
                .get("cache_creation_input_tokens")
                .or_else(|| usage.get("reasoning_tokens"))
                .and_then(|t| t.as_u64())
            {
                thinking = thinking.with_reasoning_tokens(tokens as u32);
            }
        }

        thinking
    }

    fn create_think_parser(&self) -> Option<ThinkingSplitParser> {
        // Anthropic uses native thinking blocks, no tag parsing needed
        None
    }

    fn capabilities(&self) -> &ProviderToolCapabilities {
        &self.capabilities
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_compat_translate_tools() {
        let adapter = OpenAiCompatToolAdapter::new("openai", ProviderToolCapabilities::openai());
        let registry = EquivalenceRegistry::new();

        let tools = vec![CanonicalTool {
            name: "get_weather".into(),
            description: "Get weather".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"location": {"type": "string"}}}),
            strict: true,
            execution: super::super::types::ToolExecution::ClientSide,
            equivalence_class: None,
            provider_hints: Default::default(),
            defer_loading: false,
        }];

        let result = adapter.translate_tools(&tools, &registry).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn test_openai_compat_normalize_tool_calls() {
        let adapter = OpenAiCompatToolAdapter::new("openai", ProviderToolCapabilities::openai());

        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\":\"NYC\"}"
                        }
                    }]
                }
            }]
        });

        let calls = adapter.normalize_tool_calls(&response).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments["location"], "NYC");
    }

    #[test]
    fn test_anthropic_normalize_tool_calls() {
        let adapter = AnthropicToolAdapter::new();

        let response = serde_json::json!({
            "content": [
                {"type": "text", "text": "Let me check the weather."},
                {
                    "type": "tool_use",
                    "id": "toolu_01ABC",
                    "name": "get_weather",
                    "input": {"location": "NYC"}
                }
            ]
        });

        let calls = adapter.normalize_tool_calls(&response).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_01ABC");
        assert_eq!(calls[0].name, "get_weather");
    }

    #[test]
    fn test_anthropic_extract_thinking() {
        let adapter = AnthropicToolAdapter::new();

        let response = serde_json::json!({
            "content": [
                {"type": "thinking", "thinking": "Let me reason about this...", "signature": "sig123"},
                {"type": "text", "text": "The answer is 42."}
            ]
        });

        let thinking = adapter.extract_thinking(&response);
        assert!(thinking.has_content());
        assert_eq!(thinking.blocks[0].text, "Let me reason about this...");
    }

    #[test]
    fn test_openai_extract_reasoning_tokens() {
        let adapter = OpenAiCompatToolAdapter::new("openai", ProviderToolCapabilities::openai());

        let response = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "Answer"}}],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "completion_tokens_details": {
                    "reasoning_tokens": 30
                }
            }
        });

        let thinking = adapter.extract_thinking(&response);
        assert_eq!(thinking.reasoning_tokens, Some(30));
        assert!(!thinking.has_content()); // OpenAI hides the actual thinking
    }

    #[test]
    fn test_kimi_extract_thinking() {
        let adapter = OpenAiCompatToolAdapter::new("kimi", ProviderToolCapabilities::kimi());

        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "The answer is 42.",
                    "reasoning_content": "I need to think about this..."
                }
            }]
        });

        let thinking = adapter.extract_thinking(&response);
        assert!(thinking.has_content());
        assert_eq!(thinking.blocks[0].text, "I need to think about this...");
    }

    #[test]
    fn test_too_many_tools_error() {
        let adapter = OpenAiCompatToolAdapter::new("ollama", ProviderToolCapabilities::ollama());
        let registry = EquivalenceRegistry::new();

        // Create 33 tools (exceeds ollama's 32 hard limit)
        let tools: Vec<CanonicalTool> = (0..33)
            .map(|i| CanonicalTool {
                name: format!("tool_{}", i),
                description: "test".into(),
                input_schema: serde_json::json!({"type": "object"}),
                strict: false,
                execution: super::super::types::ToolExecution::ClientSide,
                equivalence_class: None,
                provider_hints: Default::default(),
                defer_loading: false,
            })
            .collect();

        let result = adapter.translate_tools(&tools, &registry);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TranslationError::TooManyTools { count: 33, limit: 32 }
        ));
    }
}
