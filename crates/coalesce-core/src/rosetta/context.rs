//! RosettaContext — the integration layer between the proxy request flow and Rosetta.
//!
//! Provides methods that the proxy handler calls at each stage:
//! 1. `normalize_request_tools()` — ingress: parse tools from ChatRequest into canonical form
//! 2. `filter_by_tool_capabilities()` — routing: eliminate providers that can't handle the tools
//! 3. `translate_tools_for_provider()` — egress: convert canonical tools to provider format
//! 4. `normalize_response_thinking()` — egress: extract thinking from provider response
//! 5. `translate_response_for_client()` — egress: convert thinking to client-expected format

use std::collections::HashMap;
use std::sync::Arc;

use super::adapter::{AnthropicToolAdapter, OpenAiCompatToolAdapter, ProviderToolAdapter, TranslationError};
use super::capabilities::{ProviderToolCapabilities, ThinkingToolsSupport};
use super::equivalence::EquivalenceRegistry;
use super::thinking::{CanonicalThinking, ThinkingSplitParser};
use super::types::{CanonicalTool, CanonicalToolChoice};

/// Holds Rosetta state for the proxy lifetime.
pub struct RosettaContext {
    /// Tool equivalence class registry.
    pub equivalence_registry: EquivalenceRegistry,

    /// Per-provider tool adapters.
    adapters: HashMap<String, Arc<dyn ProviderToolAdapter>>,
}

/// Result of normalizing tools from an incoming request.
#[derive(Debug, Clone)]
pub struct NormalizedTools {
    /// Canonical tool definitions.
    pub tools: Vec<CanonicalTool>,
    /// Canonical tool_choice.
    pub tool_choice: Option<CanonicalToolChoice>,
    /// Number of tools that require strict schema enforcement.
    pub strict_count: usize,
    /// Number of server-side tools in the request.
    pub server_side_count: usize,
    /// Whether any tool has an equivalence class.
    pub has_equivalence_tools: bool,
}

/// Result of filtering candidates by tool capabilities.
#[derive(Debug, Clone)]
pub struct ToolFilterResult {
    /// Provider name.
    pub provider: String,
    /// Whether this provider passes all hard gates.
    pub passes: bool,
    /// Reasons for elimination (empty if passes).
    pub elimination_reasons: Vec<String>,
    /// Soft penalty multiplier (1.0 = no penalty, >1.0 = penalized).
    pub cost_multiplier: f64,
}

/// Tool-derived scoring dimensions for capability-aware routing.
/// Computed from normalized tools, used alongside text-based scoring.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ToolScoring {
    /// Number of tools in the request.
    pub tool_count: usize,
    /// Average nesting depth of tool parameter schemas.
    pub schema_complexity: f64,
    /// Number of tools requiring strict schema enforcement.
    pub strict_required: usize,
    /// Whether the request hints at parallel tool calls.
    pub parallel_calls_expected: bool,
    /// Number of server-side tools (provider-executed).
    pub server_tools_count: usize,
    /// Whether thinking + tools are both requested.
    pub thinking_plus_tools: bool,
    /// Fraction of tools that have equivalence class coverage (0.0-1.0).
    pub equivalence_coverage: f64,
    /// Number of proxy-managed tools.
    pub proxy_managed_count: usize,
}

impl ToolScoring {
    /// Compute tool scoring dimensions from normalized tools.
    pub fn from_normalized(normalized: &NormalizedTools, has_thinking: bool) -> Self {
        let tool_count = normalized.tools.len();
        let strict_required = normalized.strict_count;
        let server_tools_count = normalized.server_side_count;
        let thinking_plus_tools = has_thinking && tool_count > 0;

        let equivalence_count = normalized.tools.iter()
            .filter(|t| t.equivalence_class.is_some())
            .count();
        let equivalence_coverage = if tool_count > 0 {
            equivalence_count as f64 / tool_count as f64
        } else {
            0.0
        };

        let proxy_managed_count = normalized.tools.iter()
            .filter(|t| matches!(t.execution, super::types::ToolExecution::ProxyManaged { .. }))
            .count();

        // Schema complexity: average nesting depth across all tool schemas
        let schema_complexity = if tool_count > 0 {
            let total_depth: usize = normalized.tools.iter()
                .map(|t| compute_schema_depth(&t.input_schema))
                .sum();
            total_depth as f64 / tool_count as f64
        } else {
            0.0
        };

        // Parallel calls: hinted by tool_choice=auto + multiple tools
        let parallel_calls_expected = tool_count > 1
            && matches!(
                normalized.tool_choice,
                None | Some(CanonicalToolChoice::Auto)
            );

        Self {
            tool_count,
            schema_complexity,
            strict_required,
            parallel_calls_expected,
            server_tools_count,
            thinking_plus_tools,
            equivalence_coverage,
            proxy_managed_count,
        }
    }

    /// Compute a composite cost multiplier from tool scoring.
    /// Values > 1.0 mean extra cost should be applied for this tool surface.
    pub fn cost_multiplier(&self) -> f64 {
        let mut m = 1.0_f64;
        // Deep schemas are harder to handle correctly
        if self.schema_complexity > 3.0 {
            m *= 1.0 + (self.schema_complexity - 3.0) * 0.1;
        }
        // Low equivalence coverage means fewer fallback options
        if self.tool_count > 0 && self.equivalence_coverage < 0.3 {
            m *= 1.1;
        }
        m
    }
}

/// Compute the maximum nesting depth of a JSON schema.
fn compute_schema_depth(schema: &serde_json::Value) -> usize {
    match schema {
        serde_json::Value::Object(obj) => {
            let mut max_child = 0;
            if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
                for prop in props.values() {
                    max_child = max_child.max(compute_schema_depth(prop));
                }
            }
            if let Some(items) = obj.get("items") {
                max_child = max_child.max(compute_schema_depth(items));
            }
            1 + max_child
        }
        _ => 0,
    }
}

impl RosettaContext {
    pub fn new() -> Self {
        let mut adapters: HashMap<String, Arc<dyn ProviderToolAdapter>> = HashMap::new();

        // Register default adapters for known providers
        adapters.insert(
            "anthropic".into(),
            Arc::new(AnthropicToolAdapter::new()),
        );
        for (name, caps) in [
            ("openai", ProviderToolCapabilities::openai()),
            ("copilot", ProviderToolCapabilities::copilot()),
            ("ollama", ProviderToolCapabilities::ollama()),
            ("openrouter", ProviderToolCapabilities::openrouter()),
            ("kimi", ProviderToolCapabilities::kimi()),
            ("deepseek", ProviderToolCapabilities::deepseek()),
            ("google", ProviderToolCapabilities::google()),
            ("glm", ProviderToolCapabilities::glm()),
        ] {
            adapters.insert(
                name.into(),
                Arc::new(OpenAiCompatToolAdapter::new(name, caps)),
            );
        }

        Self {
            equivalence_registry: EquivalenceRegistry::new(),
            adapters,
        }
    }

    /// Get the adapter for a provider (falls back to a generic OpenAI-compat adapter).
    pub fn adapter_for(&self, provider: &str) -> Arc<dyn ProviderToolAdapter> {
        self.adapters
            .get(provider)
            .cloned()
            .unwrap_or_else(|| {
                Arc::new(OpenAiCompatToolAdapter::new(
                    provider,
                    ProviderToolCapabilities::default(),
                ))
            })
    }

    // -----------------------------------------------------------------------
    // 1. INGRESS: Normalize request tools
    // -----------------------------------------------------------------------

    /// Parse tools from a ChatRequest's JSON tool array into canonical form.
    /// Detects Anthropic vs OpenAI format automatically.
    pub fn normalize_request_tools(
        &self,
        tools_json: &[serde_json::Value],
        tool_choice_json: Option<&serde_json::Value>,
    ) -> NormalizedTools {
        let mut tools = Vec::new();
        for tool_json in tools_json {
            // Try OpenAI format first (has "type": "function" wrapper)
            if let Some(canonical) = CanonicalTool::from_openai(tool_json) {
                tools.push(canonical);
            } else if let Some(canonical) = CanonicalTool::from_anthropic(tool_json) {
                tools.push(canonical);
            }
            // If neither parses, skip (opaque tool — pass through as-is)
        }

        // Detect equivalence class membership
        for tool in &mut tools {
            if let Some(class) = self.equivalence_registry.find_class_for_tool("*", &tool.name) {
                tool.equivalence_class = Some(class.name.clone());
            }
        }

        let strict_count = tools.iter().filter(|t| t.strict).count();
        let server_side_count = tools
            .iter()
            .filter(|t| matches!(t.execution, super::types::ToolExecution::ServerSide { .. }))
            .count();
        let has_equivalence_tools = tools.iter().any(|t| t.equivalence_class.is_some());

        let tool_choice = tool_choice_json.map(|tc| {
            // Try Anthropic format first (has "type" field)
            if tc.get("type").and_then(|t| t.as_str()).is_some()
                && tc.get("type").and_then(|t| t.as_str()) != Some("function")
            {
                CanonicalToolChoice::from_anthropic(tc)
            } else {
                CanonicalToolChoice::from_openai(tc)
            }
        });

        NormalizedTools {
            tools,
            tool_choice,
            strict_count,
            server_side_count,
            has_equivalence_tools,
        }
    }

    // -----------------------------------------------------------------------
    // 2. ROUTING: Filter candidates by tool capabilities
    // -----------------------------------------------------------------------

    /// Check if a provider can handle the given tools. Returns filter result
    /// with pass/fail, elimination reasons, and cost penalty multiplier.
    pub fn filter_by_tool_capabilities(
        &self,
        provider: &str,
        normalized: &NormalizedTools,
        has_thinking: bool,
    ) -> ToolFilterResult {
        let adapter = self.adapter_for(provider);
        let caps = adapter.capabilities();
        let mut reasons = Vec::new();
        let mut cost_multiplier = 1.0_f64;

        // Hard gate: strict mode required but provider doesn't support it
        if normalized.strict_count > 0 && !caps.strict_mode {
            reasons.push(format!(
                "{} tools require strict mode, provider doesn't support it",
                normalized.strict_count
            ));
        }

        // Hard gate: thinking + tools incompatible
        if has_thinking && !normalized.tools.is_empty() {
            if caps.thinking_with_tools == ThinkingToolsSupport::Incompatible {
                reasons.push("Provider can't use thinking with tools simultaneously".into());
            }
        }

        // Hard gate: tool count exceeds hard limit
        if normalized.tools.len() > caps.max_tools_hard_limit {
            reasons.push(format!(
                "{} tools exceeds hard limit of {}",
                normalized.tools.len(),
                caps.max_tools_hard_limit
            ));
        }

        // Soft penalty: tool count above comfortable threshold
        if normalized.tools.len() > caps.max_tools_comfortable {
            let overage = normalized.tools.len() as f64 / caps.max_tools_comfortable as f64;
            cost_multiplier *= 1.0 + (overage - 1.0) * 0.5; // 50% penalty per 100% over
        }

        // Soft penalty: server-side tools without native equivalent
        if normalized.server_side_count > 0 {
            let mut missing = 0;
            for tool in &normalized.tools {
                if let super::types::ToolExecution::ServerSide { ref native_provider } = tool.execution {
                    if native_provider != provider {
                        // Check equivalence class
                        if let Some(ref class_name) = tool.equivalence_class {
                            if self.equivalence_registry.resolve_for_provider(class_name, provider).is_none() {
                                missing += 1;
                            }
                        } else {
                            // No equivalence class — this tool only works on native provider
                            reasons.push(format!(
                                "Tool '{}' is server-side on '{}' with no equivalent",
                                tool.name, native_provider
                            ));
                        }
                    }
                }
            }
            if missing > 0 {
                cost_multiplier *= 1.0 + missing as f64 * 0.3;
            }
        }

        ToolFilterResult {
            provider: provider.to_string(),
            passes: reasons.is_empty(),
            elimination_reasons: reasons,
            cost_multiplier,
        }
    }

    // -----------------------------------------------------------------------
    // 3. EGRESS: Translate tools for selected provider
    // -----------------------------------------------------------------------

    /// Translate canonical tools to provider-native format for dispatch.
    pub fn translate_tools_for_provider(
        &self,
        provider: &str,
        normalized: &NormalizedTools,
    ) -> Result<Vec<serde_json::Value>, TranslationError> {
        let adapter = self.adapter_for(provider);
        adapter.translate_tools(&normalized.tools, &self.equivalence_registry)
    }

    /// Translate canonical tool_choice to provider-native format.
    pub fn translate_tool_choice_for_provider(
        &self,
        provider: &str,
        choice: &CanonicalToolChoice,
    ) -> serde_json::Value {
        let adapter = self.adapter_for(provider);
        adapter.translate_tool_choice(choice)
    }

    // -----------------------------------------------------------------------
    // 4. RESPONSE: Normalize thinking from provider response
    // -----------------------------------------------------------------------

    /// Extract canonical thinking from a provider's response.
    pub fn normalize_response_thinking(
        &self,
        provider: &str,
        raw_response: &serde_json::Value,
    ) -> CanonicalThinking {
        let adapter = self.adapter_for(provider);
        adapter.extract_thinking(raw_response)
    }

    /// Create a streaming think-split parser for a provider.
    /// Returns None if the provider uses native thinking blocks (no parsing needed).
    pub fn create_think_parser(&self, provider: &str) -> Option<ThinkingSplitParser> {
        let adapter = self.adapter_for(provider);
        adapter.create_think_parser()
    }

    /// Get the capabilities for a provider.
    pub fn capabilities_for(&self, provider: &str) -> ProviderToolCapabilities {
        self.adapter_for(provider).capabilities().clone()
    }
}

impl Default for RosettaContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_openai_tools() {
        let ctx = RosettaContext::new();
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather",
                "strict": true,
                "parameters": {"type": "object", "properties": {"loc": {"type": "string"}}}
            }
        })];

        let result = ctx.normalize_request_tools(&tools, None);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "get_weather");
        assert_eq!(result.strict_count, 1);
        assert_eq!(result.server_side_count, 0);
    }

    #[test]
    fn test_normalize_anthropic_tools() {
        let ctx = RosettaContext::new();
        let tools = vec![serde_json::json!({
            "name": "search",
            "description": "Search the web",
            "input_schema": {"type": "object"},
            "cache_control": {"type": "ephemeral"}
        })];

        let result = ctx.normalize_request_tools(&tools, None);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "search");
        assert!(result.tools[0].provider_hints.contains_key("cache_control"));
    }

    #[test]
    fn test_filter_strict_mode() {
        let ctx = RosettaContext::new();
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "test",
                "description": "test",
                "strict": true,
                "parameters": {"type": "object"}
            }
        })];
        let normalized = ctx.normalize_request_tools(&tools, None);

        // OpenAI supports strict — should pass
        let result = ctx.filter_by_tool_capabilities("openai", &normalized, false);
        assert!(result.passes);

        // Ollama doesn't support strict — should fail
        let result = ctx.filter_by_tool_capabilities("ollama", &normalized, false);
        assert!(!result.passes);
        assert!(result.elimination_reasons[0].contains("strict mode"));
    }

    #[test]
    fn test_filter_thinking_plus_tools() {
        let ctx = RosettaContext::new();
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "t", "description": "t", "parameters": {"type": "object"}}
        })];
        let normalized = ctx.normalize_request_tools(&tools, None);

        // Anthropic supports thinking+tools (Full) — should pass
        let result = ctx.filter_by_tool_capabilities("anthropic", &normalized, true);
        assert!(result.passes);
    }

    #[test]
    fn test_filter_tool_count_penalty() {
        let ctx = RosettaContext::new();
        let tools: Vec<serde_json::Value> = (0..25)
            .map(|i| serde_json::json!({
                "type": "function",
                "function": {"name": format!("t{}", i), "description": "t", "parameters": {"type": "object"}}
            }))
            .collect();
        let normalized = ctx.normalize_request_tools(&tools, None);

        // Ollama comfortable with 10, 25 is 2.5x over — should get penalty
        let result = ctx.filter_by_tool_capabilities("ollama", &normalized, false);
        assert!(result.cost_multiplier > 1.0);

        // Anthropic comfortable with 64, 25 is fine — no penalty
        let result = ctx.filter_by_tool_capabilities("anthropic", &normalized, false);
        assert!((result.cost_multiplier - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_translate_for_provider() {
        let ctx = RosettaContext::new();
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "test", "description": "test", "parameters": {"type": "object"}}
        })];
        let normalized = ctx.normalize_request_tools(&tools, None);

        let openai_tools = ctx.translate_tools_for_provider("openai", &normalized).unwrap();
        assert_eq!(openai_tools[0]["type"], "function");

        let anthropic_tools = ctx.translate_tools_for_provider("anthropic", &normalized).unwrap();
        assert_eq!(anthropic_tools[0]["name"], "test");
        assert!(anthropic_tools[0].get("input_schema").is_some());
    }

    #[test]
    fn test_thinking_extraction() {
        let ctx = RosettaContext::new();

        // Anthropic response with thinking blocks
        let resp = serde_json::json!({
            "content": [
                {"type": "thinking", "thinking": "Let me think..."},
                {"type": "text", "text": "Answer"}
            ]
        });
        let thinking = ctx.normalize_response_thinking("anthropic", &resp);
        assert!(thinking.has_content());
        assert_eq!(thinking.blocks[0].text, "Let me think...");

        // Kimi response with reasoning_content
        let resp = serde_json::json!({
            "choices": [{"message": {
                "role": "assistant",
                "content": "Answer",
                "reasoning_content": "My reasoning"
            }}]
        });
        let thinking = ctx.normalize_response_thinking("kimi", &resp);
        assert!(thinking.has_content());
    }

    #[test]
    fn test_create_think_parser() {
        let ctx = RosettaContext::new();

        // DeepSeek uses <think> tags — should return parser
        assert!(ctx.create_think_parser("deepseek").is_some());
        assert!(ctx.create_think_parser("ollama").is_some());

        // Anthropic uses native blocks — no parser needed
        assert!(ctx.create_think_parser("anthropic").is_none());

        // OpenAI hides reasoning — no parser needed
        assert!(ctx.create_think_parser("openai").is_none());
    }

    #[test]
    fn test_tool_scoring_basic() {
        let ctx = RosettaContext::new();
        let tools = vec![
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather",
                    "strict": true,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "location": {"type": "string"},
                            "options": {
                                "type": "object",
                                "properties": {
                                    "unit": {"type": "string"}
                                }
                            }
                        }
                    }
                }
            }),
            serde_json::json!({
                "type": "function",
                "function": {"name": "simple", "description": "Simple", "parameters": {"type": "object"}}
            }),
        ];

        let normalized = ctx.normalize_request_tools(&tools, None);
        let scoring = ToolScoring::from_normalized(&normalized, false);
        assert_eq!(scoring.tool_count, 2);
        assert_eq!(scoring.strict_required, 1);
        assert!(!scoring.thinking_plus_tools);
        assert!(scoring.parallel_calls_expected); // 2 tools, auto choice
        assert!(scoring.schema_complexity > 0.0);
    }

    #[test]
    fn test_tool_scoring_thinking_plus_tools() {
        let ctx = RosettaContext::new();
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "t", "description": "t", "parameters": {"type": "object"}}
        })];
        let normalized = ctx.normalize_request_tools(&tools, None);
        let scoring = ToolScoring::from_normalized(&normalized, true);
        assert!(scoring.thinking_plus_tools);
    }

    #[test]
    fn test_tool_scoring_empty() {
        let normalized = NormalizedTools {
            tools: vec![],
            tool_choice: None,
            strict_count: 0,
            server_side_count: 0,
            has_equivalence_tools: false,
        };
        let scoring = ToolScoring::from_normalized(&normalized, false);
        assert_eq!(scoring.tool_count, 0);
        assert_eq!(scoring.schema_complexity, 0.0);
        assert!((scoring.cost_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_schema_depth_computation() {
        let shallow = serde_json::json!({"type": "object"});
        assert_eq!(compute_schema_depth(&shallow), 1);

        let nested = serde_json::json!({
            "type": "object",
            "properties": {
                "a": {
                    "type": "object",
                    "properties": {
                        "b": {
                            "type": "object",
                            "properties": {
                                "c": {"type": "string"}
                            }
                        }
                    }
                }
            }
        });
        assert_eq!(compute_schema_depth(&nested), 4);
    }
}
