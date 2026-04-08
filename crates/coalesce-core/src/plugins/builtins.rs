//! Built-in reference plugins that ship with Coalesce.
//!
//! These demonstrate the plugin API and provide useful functionality
//! without requiring external WASM modules.

use super::host::Plugin;
use super::interface::{PluginAction, PluginHook, PluginManifest};
use dashmap::DashMap;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Rate Limiter
// ---------------------------------------------------------------------------

/// Blocks requests that exceed a configurable per-model rate limit.
///
/// Tracks request timestamps per model and rejects requests that arrive
/// faster than `max_per_minute` within any rolling 60-second window.
pub struct RateLimiterPlugin {
    manifest: PluginManifest,
    max_per_minute: u32,
    /// model -> list of request timestamps
    requests: DashMap<String, Vec<Instant>>,
}

impl RateLimiterPlugin {
    /// Create a new rate limiter that allows at most `max_per_minute`
    /// requests per model per rolling 60-second window.
    pub fn new(max_per_minute: u32) -> Self {
        Self {
            manifest: PluginManifest {
                name: "rate_limiter".into(),
                version: "1.0.0".into(),
                description: "Blocks requests exceeding N per minute per model".into(),
                hooks: vec![PluginHook::OnRequest],
            },
            max_per_minute,
            requests: DashMap::new(),
        }
    }
}

impl Plugin for RateLimiterPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn on_request(&self, request: &serde_json::Value) -> PluginAction {
        let model = request
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        let mut entry = self.requests.entry(model.clone()).or_default();
        // Evict timestamps older than the window.
        entry.retain(|&t| now.duration_since(t) < window);

        if entry.len() >= self.max_per_minute as usize {
            return PluginAction::Block(format!(
                "Rate limit exceeded for model {model}: {}/{} per minute",
                entry.len(),
                self.max_per_minute,
            ));
        }

        entry.push(now);
        PluginAction::Continue(request.clone())
    }
}

// ---------------------------------------------------------------------------
// Request Logger
// ---------------------------------------------------------------------------

/// Logs every request that passes through and continues without modification.
///
/// This is a passthrough plugin useful for debugging and auditing.
pub struct RequestLoggerPlugin {
    manifest: PluginManifest,
}

impl RequestLoggerPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                name: "request_logger".into(),
                version: "1.0.0".into(),
                description: "Logs all requests (passthrough)".into(),
                hooks: vec![PluginHook::OnRequest, PluginHook::OnResponse],
            },
        }
    }
}

impl Default for RequestLoggerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for RequestLoggerPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn on_request(&self, request: &serde_json::Value) -> PluginAction {
        tracing::info!(
            plugin = "request_logger",
            model = request.get("model").and_then(|v| v.as_str()).unwrap_or("?"),
            "Plugin: logging request"
        );
        PluginAction::Continue(request.clone())
    }

    fn on_response(&self, response: &serde_json::Value) -> PluginAction {
        tracing::info!(plugin = "request_logger", "Plugin: logging response");
        PluginAction::Continue(response.clone())
    }
}

// ---------------------------------------------------------------------------
// Content Filter
// ---------------------------------------------------------------------------

/// Blocks requests whose message content contains any of the configured
/// blocked terms (case-insensitive substring match).
pub struct ContentFilterPlugin {
    manifest: PluginManifest,
    blocked_terms: Vec<String>,
}

impl ContentFilterPlugin {
    /// Create a content filter that blocks requests containing any of the
    /// given terms (matched case-insensitively).
    pub fn new(blocked_terms: Vec<String>) -> Self {
        Self {
            manifest: PluginManifest {
                name: "content_filter".into(),
                version: "1.0.0".into(),
                description: "Blocks requests with configurable blocked terms".into(),
                hooks: vec![PluginHook::OnRequest],
            },
            blocked_terms: blocked_terms
                .into_iter()
                .map(|t| t.to_lowercase())
                .collect(),
        }
    }

    /// Extract all text content from a chat request JSON value.
    fn extract_text(request: &serde_json::Value) -> String {
        let mut text = String::new();
        if let Some(messages) = request.get("messages").and_then(|v| v.as_array()) {
            for msg in messages {
                if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
                    text.push(' ');
                    text.push_str(content);
                }
            }
        }
        text
    }
}

impl Plugin for ContentFilterPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn on_request(&self, request: &serde_json::Value) -> PluginAction {
        let text = Self::extract_text(request).to_lowercase();
        for term in &self.blocked_terms {
            if text.contains(term.as_str()) {
                return PluginAction::Block(format!("Content blocked: matched term \"{term}\""));
            }
        }
        PluginAction::Continue(request.clone())
    }
}

// ---------------------------------------------------------------------------
// Respond With Choices
// ---------------------------------------------------------------------------

/// Injects a `respond_with_choices` tool definition into chat requests so
/// models can formally return a question + list of options instead of
/// prose. The Chat UI (Phase 19.3) detects a tool call to this name and
/// renders polished choice buttons.
///
/// Off by default — callers must explicitly register this plugin. Intended
/// to be gated behind a UI/config toggle in the desktop app.
pub struct RespondWithChoicesPlugin {
    manifest: PluginManifest,
}

impl RespondWithChoicesPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                name: "respond_with_choices".into(),
                version: "1.0.0".into(),
                description:
                    "Injects a respond_with_choices tool so models return structured options"
                        .into(),
                hooks: vec![PluginHook::OnRequest],
            },
        }
    }

    /// The canonical tool definition (OpenAI chat-completions shape).
    fn tool_definition() -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "respond_with_choices",
                "description": "Present the user with a question and a short list of clickable options instead of free-form prose. Use this whenever you would otherwise ask the user to pick between discrete alternatives.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string",
                            "description": "The question or prompt shown above the options."
                        },
                        "options": {
                            "type": "array",
                            "description": "2-6 short option labels the user can click.",
                            "items": { "type": "string" },
                            "minItems": 2,
                            "maxItems": 6
                        }
                    },
                    "required": ["question", "options"]
                }
            }
        })
    }
}

impl Default for RespondWithChoicesPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for RespondWithChoicesPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn on_request(&self, request: &serde_json::Value) -> PluginAction {
        let mut req = request.clone();
        let tool_def = Self::tool_definition();

        // Append to existing tools array, or create one. Preserve any
        // user-provided tools; don't clobber.
        let obj = match req.as_object_mut() {
            Some(o) => o,
            None => return PluginAction::Continue(request.clone()),
        };

        match obj.get_mut("tools") {
            Some(serde_json::Value::Array(arr)) => {
                // Idempotent: don't double-inject.
                let already = arr.iter().any(|t| {
                    t.get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                        == Some("respond_with_choices")
                });
                if !already {
                    arr.push(tool_def);
                }
            }
            _ => {
                obj.insert("tools".to_string(), serde_json::Value::Array(vec![tool_def]));
            }
        }

        PluginAction::Continue(req)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::host::PluginHost;
    use std::sync::Arc;

    fn chat_request(model: &str, content: &str) -> serde_json::Value {
        serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": content}]
        })
    }

    // -- RateLimiterPlugin --

    #[test]
    fn test_rate_limiter_allows_under_limit() {
        let plugin = RateLimiterPlugin::new(5);
        let req = chat_request("gpt-4", "hello");
        for _ in 0..5 {
            match plugin.on_request(&req) {
                PluginAction::Continue(_) => {}
                other => panic!("expected Continue, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let plugin = RateLimiterPlugin::new(3);
        let req = chat_request("gpt-4", "hello");
        for _ in 0..3 {
            assert!(matches!(plugin.on_request(&req), PluginAction::Continue(_)));
        }
        // 4th request should be blocked.
        assert!(matches!(plugin.on_request(&req), PluginAction::Block(_)));
    }

    #[test]
    fn test_rate_limiter_per_model() {
        let plugin = RateLimiterPlugin::new(2);
        let req_a = chat_request("model-a", "hello");
        let req_b = chat_request("model-b", "hello");
        assert!(matches!(plugin.on_request(&req_a), PluginAction::Continue(_)));
        assert!(matches!(plugin.on_request(&req_a), PluginAction::Continue(_)));
        // model-a is at limit, but model-b should still be allowed.
        assert!(matches!(plugin.on_request(&req_a), PluginAction::Block(_)));
        assert!(matches!(plugin.on_request(&req_b), PluginAction::Continue(_)));
    }

    // -- RequestLoggerPlugin --

    #[test]
    fn test_logger_passthrough() {
        let plugin = RequestLoggerPlugin::new();
        let req = chat_request("gpt-4", "hello");
        match plugin.on_request(&req) {
            PluginAction::Continue(v) => assert_eq!(v, req),
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn test_logger_response_passthrough() {
        let plugin = RequestLoggerPlugin::new();
        let resp = serde_json::json!({"choices": []});
        match plugin.on_response(&resp) {
            PluginAction::Continue(v) => assert_eq!(v, resp),
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    // -- ContentFilterPlugin --

    #[test]
    fn test_content_filter_allows_clean() {
        let plugin = ContentFilterPlugin::new(vec!["banned".into()]);
        let req = chat_request("gpt-4", "this is a clean request");
        assert!(matches!(plugin.on_request(&req), PluginAction::Continue(_)));
    }

    #[test]
    fn test_content_filter_blocks_bad_content() {
        let plugin = ContentFilterPlugin::new(vec!["banned".into(), "forbidden".into()]);
        let req = chat_request("gpt-4", "this contains the BANNED word");
        match plugin.on_request(&req) {
            PluginAction::Block(reason) => {
                assert!(reason.contains("banned"), "reason: {reason}");
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn test_content_filter_case_insensitive() {
        let plugin = ContentFilterPlugin::new(vec!["secret".into()]);
        let req = chat_request("gpt-4", "Tell me the SECRET please");
        assert!(matches!(plugin.on_request(&req), PluginAction::Block(_)));
    }

    // -- RespondWithChoicesPlugin --

    #[test]
    fn test_respond_with_choices_injects_tool() {
        let plugin = RespondWithChoicesPlugin::new();
        let req = chat_request("gpt-4", "what next?");
        match plugin.on_request(&req) {
            PluginAction::Continue(v) => {
                let tools = v.get("tools").and_then(|t| t.as_array()).expect("tools");
                assert_eq!(tools.len(), 1);
                assert_eq!(
                    tools[0]["function"]["name"].as_str(),
                    Some("respond_with_choices")
                );
            }
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn test_respond_with_choices_preserves_existing_tools() {
        let plugin = RespondWithChoicesPlugin::new();
        let mut req = chat_request("gpt-4", "hi");
        req["tools"] = serde_json::json!([
            {"type": "function", "function": {"name": "get_weather"}}
        ]);
        match plugin.on_request(&req) {
            PluginAction::Continue(v) => {
                let tools = v.get("tools").and_then(|t| t.as_array()).unwrap();
                assert_eq!(tools.len(), 2);
                let names: Vec<&str> = tools
                    .iter()
                    .filter_map(|t| t["function"]["name"].as_str())
                    .collect();
                assert!(names.contains(&"get_weather"));
                assert!(names.contains(&"respond_with_choices"));
            }
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn test_respond_with_choices_idempotent() {
        let plugin = RespondWithChoicesPlugin::new();
        let req = chat_request("gpt-4", "hi");
        let first = match plugin.on_request(&req) {
            PluginAction::Continue(v) => v,
            _ => panic!(),
        };
        let second = match plugin.on_request(&first) {
            PluginAction::Continue(v) => v,
            _ => panic!(),
        };
        assert_eq!(
            second.get("tools").and_then(|t| t.as_array()).unwrap().len(),
            1
        );
    }

    // -- Integration with PluginHost --

    #[test]
    fn test_builtins_in_plugin_host() {
        let mut host = PluginHost::new();
        host.register(Arc::new(RequestLoggerPlugin::new()));
        host.register(Arc::new(ContentFilterPlugin::new(vec!["blocked".into()])));
        host.register(Arc::new(RateLimiterPlugin::new(100)));

        assert_eq!(host.plugin_count(), 3);

        // Clean request goes through all 3 plugins.
        let req = chat_request("gpt-4", "hello world");
        let result = host.run_on_request(&req);
        assert!(result.is_ok());

        // Bad content is caught by the content filter.
        let bad = chat_request("gpt-4", "this is blocked content");
        let result = host.run_on_request(&bad);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("content_filter"));
    }

    #[test]
    fn test_host_chain_rate_then_filter() {
        let mut host = PluginHost::new();
        host.register(Arc::new(RateLimiterPlugin::new(2)));
        host.register(Arc::new(ContentFilterPlugin::new(vec!["bad".into()])));

        let req = chat_request("gpt-4", "hello");
        assert!(host.run_on_request(&req).is_ok());
        assert!(host.run_on_request(&req).is_ok());
        // Third request blocked by rate limiter before reaching filter.
        let result = host.run_on_request(&req);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("rate_limiter"));
    }
}
