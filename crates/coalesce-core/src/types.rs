use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// OpenAI-compatible chat completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopSequence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
    /// Catch-all for provider-specific fields
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopSequence {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s),
            MessageContent::Parts(parts) => parts.iter().find_map(|p| {
                if let ContentPart::Text { text } = p {
                    Some(text.as_str())
                } else {
                    None
                }
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Default for ChatRequest {
    fn default() -> Self {
        Self {
            model: String::new(),
            messages: Vec::new(),
            stream: false,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            extra: Default::default(),
        }
    }
}

/// OpenAI-compatible chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Streaming chunk (SSE data)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

/// Provider-specific capability details beyond simple boolean flags
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Supported reasoning effort levels (e.g. ["low","medium","high","xhigh"])
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_levels: Option<Vec<String>>,
    /// Supports adaptive thinking (extended thinking with budget)
    #[serde(default)]
    pub adaptive_thinking: bool,
    /// Min/max thinking budget tokens (min, max)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<(u32, u32)>,
    /// Supported API endpoints (e.g. ["/v1/messages", "/chat/completions"])
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_endpoints: Option<Vec<String>>,
    /// Actual model vendor (e.g. "Anthropic", "OpenAI", "Google")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Model picker category from Copilot API (e.g. "powerful", "versatile")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_picker_category: Option<String>,
}

/// Model info as understood by Coalesce
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Full model ID (e.g., "openai/gpt-5.2" or "anthropic/claude-sonnet-4.6")
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Which provider serves this model
    pub provider: String,
    /// Input price per million tokens (USD)
    pub input_price_per_m: f64,
    /// Output price per million tokens (USD)
    pub output_price_per_m: f64,
    /// Context window size in tokens
    pub context_window: u32,
    /// Max output tokens
    pub max_output: Option<u32>,
    /// Quality tier this model is capable of
    pub quality_tier: QualityTier,
    /// Supports reasoning/thinking
    pub reasoning: bool,
    /// Supports vision (image input)
    pub vision: bool,
    /// Supports tool/function calling
    pub tool_calling: bool,
    /// Canonical model family (e.g. "claude-opus-4.6", "gpt-4o", "gemini-2.5-pro")
    /// Groups the same model across different providers for cost comparison
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_family: Option<String>,
    /// Extended capability details from provider API
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
}

/// Quality tiers — what complexity level a model can handle
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityTier {
    Simple,
    Medium,
    Complex,
    Reasoning,
}

impl QualityTier {
    /// A model rated at this tier can handle this tier and all lower tiers
    pub fn can_handle(&self, requested: &QualityTier) -> bool {
        *self >= *requested
    }
}

/// Derive a canonical family name from a raw model ID.
/// Strips provider prefixes, suffixes, date stamps, and normalizes separators.
/// Examples:
///   "claude-opus-4-6-thinking" → "claude-opus-4.6"
///   "anthropic/claude-sonnet-4.6-20250514" → "claude-sonnet-4.6"
///   "gpt-4o-mini-2024-07-18" → "gpt-4o-mini"
///   "gemini-2.5-pro-preview" → "gemini-2.5-pro"
///   "glm-4.6v-flash" → "glm-4.6v-flash"
pub fn derive_canonical_family(model_id: &str) -> String {
    let mut s = model_id.to_lowercase();

    // Strip known provider prefixes
    for prefix in &["anthropic/", "google/", "openai/", "meta-llama/", "meta/", "mistralai/", "cohere/", "deepseek/"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }

    // Strip date stamps: -2024-07-18 or -20250514
    if s.len() > 11 {
        let tail = &s[s.len() - 11..];
        if tail.as_bytes()[0] == b'-'
            && tail[1..5].chars().all(|c| c.is_ascii_digit())
            && tail.as_bytes()[5] == b'-'
            && tail[6..8].chars().all(|c| c.is_ascii_digit())
            && tail.as_bytes()[8] == b'-'
            && tail[9..11].chars().all(|c| c.is_ascii_digit())
        {
            s.truncate(s.len() - 11);
        }
    }
    if s.len() > 9 {
        let tail = &s[s.len() - 9..];
        if tail.as_bytes()[0] == b'-' && tail[1..].chars().all(|c| c.is_ascii_digit()) {
            s.truncate(s.len() - 9);
        }
    }

    // Strip common suffixes (loop to catch multiple, e.g. "-preview-latest")
    loop {
        let before = s.len();
        for suffix in &["-latest", "-preview", "-exp", "-free", "-thinking"] {
            if let Some(rest) = s.strip_suffix(suffix) {
                s = rest.to_string();
            }
        }
        if s.len() == before { break; }
    }

    // Normalize version separators: "4-6" → "4.6" (single digit-dash-single digit)
    // Only when both sides are single digits, to avoid mangling sizes like "1-70b"
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    for i in 0..bytes.len() {
        if bytes[i] == b'-'
            && i > 0 && bytes[i - 1].is_ascii_digit()
            && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit()
            // Check the char before the first digit is NOT a digit (single digit before dash)
            && (i < 2 || !bytes[i - 2].is_ascii_digit())
            // Check the char after the second digit is NOT a digit (single digit after dash)
            && (i + 2 >= bytes.len() || !bytes[i + 2].is_ascii_digit())
        {
            out.push(b'.');
        } else {
            out.push(bytes[i]);
        }
    }
    s = String::from_utf8(out).unwrap_or(s);

    s
}

impl std::fmt::Display for QualityTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QualityTier::Simple => write!(f, "SIMPLE"),
            QualityTier::Medium => write!(f, "MEDIUM"),
            QualityTier::Complex => write!(f, "COMPLEX"),
            QualityTier::Reasoning => write!(f, "REASONING"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_canonical_family() {
        // Strip provider prefix and date stamp
        assert_eq!(derive_canonical_family("anthropic/claude-sonnet-4-20250514"), "claude-sonnet-4");
        assert_eq!(derive_canonical_family("claude-opus-4-20250514"), "claude-opus-4");

        // Normalize version separators (digit-dash-digit → dot)
        assert_eq!(derive_canonical_family("claude-opus-4-6-thinking"), "claude-opus-4.6");
        assert_eq!(derive_canonical_family("claude-sonnet-4-6"), "claude-sonnet-4.6");

        // Strip suffixes
        assert_eq!(derive_canonical_family("gemini-2.5-pro-preview"), "gemini-2.5-pro");
        assert_eq!(derive_canonical_family("gpt-4o-mini-2024-07-18"), "gpt-4o-mini");
        assert_eq!(derive_canonical_family("gemini-3-flash-latest"), "gemini-3-flash");

        // Preserve meaningful parts
        assert_eq!(derive_canonical_family("gpt-4o"), "gpt-4o");
        assert_eq!(derive_canonical_family("gpt-4o-mini"), "gpt-4o-mini");
        assert_eq!(derive_canonical_family("o3-mini"), "o3-mini");
        assert_eq!(derive_canonical_family("grok-3"), "grok-3");

        // OpenRouter prefixed models
        assert_eq!(derive_canonical_family("openai/gpt-4o"), "gpt-4o");
        assert_eq!(derive_canonical_family("google/gemini-2.5-pro"), "gemini-2.5-pro");
        assert_eq!(derive_canonical_family("meta-llama/llama-3.1-70b"), "llama-3.1-70b");

        // GLM models
        assert_eq!(derive_canonical_family("glm-4.6v-flash"), "glm-4.6v-flash");
    }

    #[test]
    fn test_quality_tier_ordering() {
        assert!(QualityTier::Reasoning > QualityTier::Complex);
        assert!(QualityTier::Complex > QualityTier::Medium);
        assert!(QualityTier::Medium > QualityTier::Simple);
        assert!(QualityTier::Reasoning.can_handle(&QualityTier::Simple));
        assert!(!QualityTier::Simple.can_handle(&QualityTier::Reasoning));
    }
}
