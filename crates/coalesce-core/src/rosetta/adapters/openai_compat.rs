//! OpenAI-compatible streaming chunk adapter.
//!
//! Handles the shape used by OpenAI, GLM/z.ai, DeepSeek, Copilot, OpenRouter,
//! Ollama, and Google's OpenAI-compat endpoint.
//!
//! Example chunk shape:
//! ```json
//! {"choices":[{"delta":{"content":"hello","reasoning_content":"...",
//!   "tool_calls":[{"index":0,"id":"call_abc",
//!     "function":{"name":"foo","arguments":"{\"x\":"}}]},
//!   "finish_reason":null}],"usage":null}
//! ```
//!
//! Note: OpenAI does not emit an explicit tool-call-end event. We therefore
//! do NOT synthesize `ToolCallEnd` blocks here; callers that need them can
//! derive them from `finish_reason == ToolCalls`. See TODO(ticket-b).

use crate::rosetta::response::{
    CanonicalBlock, CanonicalStreamDelta, CanonicalUsage, FinishReason,
};
use crate::rosetta::stream_adapter::{ParseError, ProviderStreamAdapter};

/// Adapter for OpenAI-shaped streaming chunks.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAiCompatAdapter;

impl OpenAiCompatAdapter {
    pub fn new() -> Self {
        Self
    }
}

fn map_finish(s: &str) -> Option<FinishReason> {
    match s {
        "stop" | "end_turn" => Some(FinishReason::Stop),
        "length" | "max_tokens" => Some(FinishReason::Length),
        "tool_calls" | "function_call" => Some(FinishReason::ToolCalls),
        "content_filter" => Some(FinishReason::ContentFilter),
        "error" => Some(FinishReason::Error),
        _ => None,
    }
}

impl ProviderStreamAdapter for OpenAiCompatAdapter {
    fn parse_chunk(&self, raw_json: &str) -> Result<Option<CanonicalStreamDelta>, ParseError> {
        let v: serde_json::Value = serde_json::from_str(raw_json)?;

        let mut out = CanonicalStreamDelta::default();

        if let Some(choice) = v.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first())
        {
            if let Some(delta) = choice.get("delta") {
                // Text content
                if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                    if !text.is_empty() {
                        out.blocks.push(CanonicalBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }

                // Reasoning / thinking content (DeepSeek, GLM, some OpenRouter)
                if let Some(rtext) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                    if !rtext.is_empty() {
                        out.blocks.push(CanonicalBlock::Thinking {
                            text: rtext.to_string(),
                            signature: None,
                        });
                    }
                }

                // Tool calls
                if let Some(calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                    for call in calls {
                        let id = call.get("id").and_then(|i| i.as_str());
                        let func = call.get("function");
                        let name = func
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str());
                        let args = func
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str());

                        // First sight of this call (has an id + name) → Start.
                        // Subsequent delta chunks typically only have arguments.
                        match (id, name) {
                            (Some(id), Some(name)) if !name.is_empty() => {
                                out.blocks.push(CanonicalBlock::ToolCallStart {
                                    id: id.to_string(),
                                    name: name.to_string(),
                                });
                            }
                            _ => {}
                        }

                        if let Some(args) = args {
                            if !args.is_empty() {
                                // Prefer the explicit id; fall back to empty — the
                                // caller correlates by stream position if needed.
                                let id = id.unwrap_or("").to_string();
                                out.blocks.push(CanonicalBlock::ToolCallDelta {
                                    id,
                                    arguments_delta: args.to_string(),
                                });
                            }
                        }
                    }
                }
            }

            if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                out.finish_reason = map_finish(fr);
            }
        }

        if let Some(usage) = v.get("usage") {
            if usage.is_object() {
                out.usage = Some(CanonicalUsage {
                    prompt_tokens: usage
                        .get("prompt_tokens")
                        .and_then(|n| n.as_u64())
                        .map(|n| n as u32),
                    completion_tokens: usage
                        .get("completion_tokens")
                        .and_then(|n| n.as_u64())
                        .map(|n| n as u32),
                    cache_read_input_tokens: usage
                        .get("prompt_tokens_details")
                        .and_then(|d| d.get("cached_tokens"))
                        .and_then(|n| n.as_u64())
                        .map(|n| n as u32),
                    cache_creation_input_tokens: None,
                });
            }
        }

        // Empty deltas are returned as Ok(Some(empty_delta)) rather than None;
        // the caller decides whether to forward or drop them.
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> CanonicalStreamDelta {
        OpenAiCompatAdapter::new()
            .parse_chunk(s)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn plain_text_delta() {
        let d = parse(r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#);
        assert_eq!(d.blocks.len(), 1);
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::Text {
                text: "hello".into()
            }
        );
        assert!(d.finish_reason.is_none());
    }

    #[test]
    fn reasoning_content_becomes_thinking() {
        let d = parse(
            r#"{"choices":[{"delta":{"reasoning_content":"because..."},"finish_reason":null}]}"#,
        );
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::Thinking {
                text: "because...".into(),
                signature: None
            }
        );
    }

    #[test]
    fn tool_call_start_with_name_and_first_args() {
        let d = parse(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"foo","arguments":"{\"x\":"}}]}}]}"#,
        );
        assert_eq!(d.blocks.len(), 2);
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::ToolCallStart {
                id: "call_abc".into(),
                name: "foo".into(),
            }
        );
        assert_eq!(
            d.blocks[1],
            CanonicalBlock::ToolCallDelta {
                id: "call_abc".into(),
                arguments_delta: "{\"x\":".into(),
            }
        );
    }

    #[test]
    fn tool_call_argument_delta_only() {
        let d = parse(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}"#,
        );
        assert_eq!(d.blocks.len(), 1);
        match &d.blocks[0] {
            CanonicalBlock::ToolCallDelta { arguments_delta, .. } => {
                assert_eq!(arguments_delta, "1}");
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn finish_reason_maps() {
        let d = parse(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#);
        assert_eq!(d.finish_reason, Some(FinishReason::Stop));

        let d = parse(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#);
        assert_eq!(d.finish_reason, Some(FinishReason::ToolCalls));

        let d = parse(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#);
        assert_eq!(d.finish_reason, Some(FinishReason::Length));
    }

    #[test]
    fn usage_extraction() {
        let d = parse(
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":5}}}"#,
        );
        let u = d.usage.unwrap();
        assert_eq!(u.prompt_tokens, Some(10));
        assert_eq!(u.completion_tokens, Some(4));
        assert_eq!(u.cache_read_input_tokens, Some(5));
    }

    #[test]
    fn empty_delta_is_some_empty() {
        let d = parse(r#"{"choices":[{"delta":{},"finish_reason":null}]}"#);
        assert!(d.blocks.is_empty());
        assert!(d.finish_reason.is_none());
        assert!(d.usage.is_none());
        assert!(d.is_empty());
    }

    #[test]
    fn malformed_json_errors() {
        let err = OpenAiCompatAdapter::new().parse_chunk("{not json").unwrap_err();
        assert!(matches!(err, ParseError::Json(_)));
    }
}
