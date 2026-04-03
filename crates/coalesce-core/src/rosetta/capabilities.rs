//! Provider tool capabilities — the source of truth for routing decisions.
//!
//! Each provider declares what it supports for tool calling. The router
//! uses this to eliminate, penalize, or prefer providers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Declares what a provider supports for tool calling.
/// Populated by each provider adapter at registration time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderToolCapabilities {
    /// Maximum number of tools before known degradation.
    pub max_tools_comfortable: usize,

    /// Maximum number of tools the API accepts at all.
    pub max_tools_hard_limit: usize,

    /// Supports `strict: true` schema enforcement.
    pub strict_mode: bool,

    /// Supports parallel tool calls.
    pub parallel_tool_calls: bool,

    /// Can the client control parallel calls (e.g., `parallel_tool_calls: false`)?
    pub parallel_calls_controllable: bool,

    /// Supports tool_search / deferred loading for large tool sets.
    pub tool_search: bool,

    /// Supports thinking/reasoning mode concurrently with tools.
    pub thinking_with_tools: ThinkingToolsSupport,

    /// Built-in tools this provider offers (by equivalence class name).
    #[serde(default)]
    pub builtin_tools: HashMap<String, BuiltinToolInfo>,

    /// Provider-specific streaming behavior for tool calls.
    pub streaming_tool_deltas: StreamingToolBehavior,

    /// Known quirks that affect tool call reliability.
    #[serde(default)]
    pub quirks: Vec<ProviderQuirk>,

    /// Thinking output format this provider uses.
    pub thinking_format: ThinkingFormat,
}

impl Default for ProviderToolCapabilities {
    fn default() -> Self {
        Self {
            max_tools_comfortable: 20,
            max_tools_hard_limit: 128,
            strict_mode: false,
            parallel_tool_calls: true,
            parallel_calls_controllable: false,
            tool_search: false,
            thinking_with_tools: ThinkingToolsSupport::NoThinking,
            builtin_tools: HashMap::new(),
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: Vec::new(),
            thinking_format: ThinkingFormat::None,
        }
    }
}

/// How thinking/reasoning interacts with tools on this provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ThinkingToolsSupport {
    /// No thinking mode available.
    NoThinking,
    /// Thinking works with tools, no restrictions.
    Full,
    /// Thinking works but has constraints.
    Constrained {
        /// e.g., "tool_choice must be auto or none"
        constraints: Vec<String>,
    },
    /// Thinking and tools are mutually exclusive.
    Incompatible,
}

/// How this provider formats thinking/reasoning output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ThinkingFormat {
    /// No thinking output.
    None,
    /// Native thinking content blocks (Anthropic).
    NativeBlocks,
    /// `<think>...</think>` tags in content (DeepSeek, Qwen, Ollama reasoning models).
    ThinkTags,
    /// Separate `reasoning_content` field on message (Kimi/Moonshot).
    ReasoningContentField,
    /// Hidden reasoning, only token count in usage (OpenAI o-series).
    HiddenWithTokenCount,
}

/// Info about a provider's built-in tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinToolInfo {
    /// Provider-specific tool identifier.
    pub tool_id: String,
    /// Whether the provider executes this server-side.
    pub server_side: bool,
    /// Brief description of any behavioral differences from the canonical version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// How tool call deltas arrive in streaming responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StreamingToolBehavior {
    /// `delta.tool_calls[].function.arguments` (chunked JSON string) — OpenAI standard.
    Standard,
    /// `content_block_delta` with `input_json_delta` — Anthropic style.
    AnthropicBlocks,
    /// Tool calls only appear in the final non-streaming response (some providers).
    FinalOnly,
    /// Unknown or untested.
    Unknown,
}

/// Known quirks that affect tool call reliability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProviderQuirk {
    /// Provider sometimes duplicates tool calls when parallel is enabled.
    ParallelCallDuplication,
    /// Provider strips trailing newlines from string parameters.
    StripsTrailingNewlines,
    /// Provider has trouble with deeply nested schemas.
    DeepSchemaDegradation { max_depth: usize },
    /// Provider's tool_call IDs are not globally unique across turns.
    NonUniqueToolCallIds,
    /// Provider requires `additionalProperties: false` for strict mode.
    RequiresAdditionalPropertiesFalse,
    /// Custom quirk with description.
    Other(String),
}

// ---------------------------------------------------------------------------
// Pre-built capability profiles for known providers
// ---------------------------------------------------------------------------

impl ProviderToolCapabilities {
    /// Anthropic (Claude) — native thinking blocks, server-side tools.
    pub fn anthropic() -> Self {
        let mut builtin = HashMap::new();
        builtin.insert(
            "web_search".into(),
            BuiltinToolInfo {
                tool_id: "web_search".into(),
                server_side: true,
                notes: None,
            },
        );
        builtin.insert(
            "code_execution".into(),
            BuiltinToolInfo {
                tool_id: "code_execution".into(),
                server_side: true,
                notes: None,
            },
        );
        builtin.insert(
            "web_fetch".into(),
            BuiltinToolInfo {
                tool_id: "web_fetch".into(),
                server_side: true,
                notes: None,
            },
        );

        Self {
            max_tools_comfortable: 64,
            max_tools_hard_limit: 128,
            strict_mode: true,
            parallel_tool_calls: true,
            parallel_calls_controllable: true,
            tool_search: true,
            thinking_with_tools: ThinkingToolsSupport::Full,
            builtin_tools: builtin,
            streaming_tool_deltas: StreamingToolBehavior::AnthropicBlocks,
            quirks: vec![],
            thinking_format: ThinkingFormat::NativeBlocks,
        }
    }

    /// OpenAI (GPT-4o, GPT-5, o-series) — strict mode, standard streaming.
    pub fn openai() -> Self {
        let mut builtin = HashMap::new();
        builtin.insert(
            "web_search".into(),
            BuiltinToolInfo {
                tool_id: "web_search".into(),
                server_side: true,
                notes: None,
            },
        );
        builtin.insert(
            "code_execution".into(),
            BuiltinToolInfo {
                tool_id: "code_interpreter".into(),
                server_side: true,
                notes: None,
            },
        );

        Self {
            max_tools_comfortable: 30,
            max_tools_hard_limit: 128,
            strict_mode: true,
            parallel_tool_calls: true,
            parallel_calls_controllable: true,
            tool_search: true,
            thinking_with_tools: ThinkingToolsSupport::Constrained {
                constraints: vec!["tool_choice must be auto or none with o-series".into()],
            },
            builtin_tools: builtin,
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: vec![ProviderQuirk::RequiresAdditionalPropertiesFalse],
            thinking_format: ThinkingFormat::HiddenWithTokenCount,
        }
    }

    /// GitHub Copilot — proxied through GitHub, variable model support.
    pub fn copilot() -> Self {
        Self {
            max_tools_comfortable: 20,
            max_tools_hard_limit: 64,
            strict_mode: false,
            parallel_tool_calls: true,
            parallel_calls_controllable: false,
            tool_search: false,
            thinking_with_tools: ThinkingToolsSupport::Constrained {
                constraints: vec!["Depends on underlying model vendor".into()],
            },
            builtin_tools: HashMap::new(),
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: vec![],
            thinking_format: ThinkingFormat::None, // varies by model
        }
    }

    /// Ollama (local models) — varies by model, conservative defaults.
    pub fn ollama() -> Self {
        Self {
            max_tools_comfortable: 10,
            max_tools_hard_limit: 32,
            strict_mode: false,
            parallel_tool_calls: false,
            parallel_calls_controllable: false,
            tool_search: false,
            thinking_with_tools: ThinkingToolsSupport::NoThinking,
            builtin_tools: HashMap::new(),
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: vec![ProviderQuirk::NonUniqueToolCallIds],
            thinking_format: ThinkingFormat::ThinkTags,
        }
    }

    /// OpenRouter — aggregator, capabilities depend on underlying model.
    pub fn openrouter() -> Self {
        Self {
            max_tools_comfortable: 20,
            max_tools_hard_limit: 128,
            strict_mode: false, // varies by model
            parallel_tool_calls: true,
            parallel_calls_controllable: false,
            tool_search: false,
            thinking_with_tools: ThinkingToolsSupport::Constrained {
                constraints: vec!["Depends on underlying model".into()],
            },
            builtin_tools: HashMap::new(),
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: vec![],
            thinking_format: ThinkingFormat::ThinkTags, // most OR models use think tags
        }
    }

    /// Kimi / Moonshot — special web search, reasoning_content field.
    pub fn kimi() -> Self {
        let mut builtin = HashMap::new();
        builtin.insert(
            "web_search".into(),
            BuiltinToolInfo {
                tool_id: "$web_search".into(),
                server_side: true,
                notes: Some("Special echo-back execution model".into()),
            },
        );

        Self {
            max_tools_comfortable: 15,
            max_tools_hard_limit: 32,
            strict_mode: false,
            parallel_tool_calls: true,
            parallel_calls_controllable: false,
            tool_search: false,
            thinking_with_tools: ThinkingToolsSupport::Constrained {
                constraints: vec![
                    "tool_choice restricted to auto/none with thinking".into(),
                    "Must preserve reasoning_content in follow-up".into(),
                ],
            },
            builtin_tools: builtin,
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: vec![],
            thinking_format: ThinkingFormat::ReasoningContentField,
        }
    }

    /// DeepSeek — think tags, good tool support.
    pub fn deepseek() -> Self {
        Self {
            max_tools_comfortable: 20,
            max_tools_hard_limit: 64,
            strict_mode: false,
            parallel_tool_calls: true,
            parallel_calls_controllable: false,
            tool_search: false,
            thinking_with_tools: ThinkingToolsSupport::Full,
            builtin_tools: HashMap::new(),
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: vec![],
            thinking_format: ThinkingFormat::ThinkTags,
        }
    }

    /// Google (Gemini) — decent tool support, no native built-ins exposed.
    pub fn google() -> Self {
        Self {
            max_tools_comfortable: 20,
            max_tools_hard_limit: 64,
            strict_mode: false,
            parallel_tool_calls: true,
            parallel_calls_controllable: false,
            tool_search: false,
            thinking_with_tools: ThinkingToolsSupport::Full,
            builtin_tools: HashMap::new(),
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: vec![],
            thinking_format: ThinkingFormat::ThinkTags,
        }
    }

    /// GLM / Zhipu — OpenAI-compatible, limited tool support.
    pub fn glm() -> Self {
        Self {
            max_tools_comfortable: 10,
            max_tools_hard_limit: 32,
            strict_mode: false,
            parallel_tool_calls: true,
            parallel_calls_controllable: false,
            tool_search: false,
            thinking_with_tools: ThinkingToolsSupport::NoThinking,
            builtin_tools: HashMap::new(),
            streaming_tool_deltas: StreamingToolBehavior::Standard,
            quirks: vec![],
            thinking_format: ThinkingFormat::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_capabilities() {
        let caps = ProviderToolCapabilities::anthropic();
        assert!(caps.strict_mode);
        assert!(caps.tool_search);
        assert_eq!(caps.thinking_with_tools, ThinkingToolsSupport::Full);
        assert_eq!(caps.thinking_format, ThinkingFormat::NativeBlocks);
        assert!(caps.builtin_tools.contains_key("web_search"));
        assert!(caps.builtin_tools.contains_key("code_execution"));
        assert!(caps.builtin_tools.contains_key("web_fetch"));
    }

    #[test]
    fn test_openai_capabilities() {
        let caps = ProviderToolCapabilities::openai();
        assert!(caps.strict_mode);
        assert_eq!(caps.thinking_format, ThinkingFormat::HiddenWithTokenCount);
        assert!(caps.quirks.contains(&ProviderQuirk::RequiresAdditionalPropertiesFalse));
    }

    #[test]
    fn test_kimi_capabilities() {
        let caps = ProviderToolCapabilities::kimi();
        assert_eq!(caps.thinking_format, ThinkingFormat::ReasoningContentField);
        assert!(caps.builtin_tools.contains_key("web_search"));
        assert_eq!(caps.builtin_tools["web_search"].tool_id, "$web_search");
    }

    #[test]
    fn test_ollama_capabilities() {
        let caps = ProviderToolCapabilities::ollama();
        assert!(!caps.parallel_tool_calls);
        assert_eq!(caps.thinking_format, ThinkingFormat::ThinkTags);
        assert!(caps.quirks.contains(&ProviderQuirk::NonUniqueToolCallIds));
    }
}
