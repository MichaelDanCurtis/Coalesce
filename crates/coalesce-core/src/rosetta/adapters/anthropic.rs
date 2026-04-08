//! Anthropic native streaming event adapter.
//!
//! Anthropic's stream emits events like `message_start`, `content_block_start`,
//! `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`,
//! and `ping`. Each SSE `data:` line is a JSON object whose `type` field
//! identifies the event, so this adapter inspects that field and ignores the
//! separate `event:` header — keeping the trait signature uniform with other
//! adapters.
//!
//! TODO(ticket-b): Anthropic citations (`content_block_delta` with
//! `delta.type == "citations_delta"`) are skipped here pending a concrete
//! shape decision — the Rosetta types don't yet have a hint for per-block
//! citation accumulation, and we don't want to guess.

use crate::rosetta::response::{
    CanonicalBlock, CanonicalStreamDelta, CanonicalUsage, FinishReason,
};
use crate::rosetta::stream_adapter::{ParseError, ProviderStreamAdapter};

/// Adapter for Anthropic-native streaming events.
#[derive(Debug, Default, Clone, Copy)]
pub struct AnthropicAdapter;

impl AnthropicAdapter {
    pub fn new() -> Self {
        Self
    }
}

fn map_stop_reason(s: &str) -> Option<FinishReason> {
    match s {
        "end_turn" | "stop_sequence" => Some(FinishReason::Stop),
        "max_tokens" => Some(FinishReason::Length),
        "tool_use" => Some(FinishReason::ToolCalls),
        "content_filter" | "refusal" => Some(FinishReason::ContentFilter),
        _ => None,
    }
}

impl ProviderStreamAdapter for AnthropicAdapter {
    fn parse_chunk(&self, raw_json: &str) -> Result<Option<CanonicalStreamDelta>, ParseError> {
        let v: serde_json::Value = serde_json::from_str(raw_json)?;
        let event_type = v
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| ParseError::Shape("missing `type` field".into()))?;

        let mut out = CanonicalStreamDelta::default();

        match event_type {
            "ping" | "message_start" | "message_stop" => return Ok(None),

            "content_block_start" => {
                if let Some(cb) = v.get("content_block") {
                    match cb.get("type").and_then(|t| t.as_str()) {
                        Some("tool_use") => {
                            let id = cb
                                .get("id")
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = cb
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            out.blocks.push(CanonicalBlock::ToolCallStart { id, name });
                        }
                        // text / thinking block starts carry no incremental payload
                        _ => return Ok(None),
                    }
                }
            }

            "content_block_delta" => {
                if let Some(delta) = v.get("delta") {
                    match delta.get("type").and_then(|t| t.as_str()) {
                        Some("text_delta") => {
                            let text = delta
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string();
                            out.blocks.push(CanonicalBlock::Text { text });
                        }
                        Some("thinking_delta") => {
                            let text = delta
                                .get("thinking")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string();
                            out.blocks.push(CanonicalBlock::Thinking {
                                text,
                                signature: None,
                            });
                        }
                        Some("signature_delta") => {
                            let sig = delta
                                .get("signature")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string();
                            out.blocks.push(CanonicalBlock::Thinking {
                                text: String::new(),
                                signature: Some(sig),
                            });
                        }
                        Some("input_json_delta") => {
                            // Anthropic doesn't include the tool id on the delta event;
                            // the caller correlates by index from the most recent
                            // content_block_start. We pass an empty id and let the
                            // driver fill it in. TODO(ticket-b): consider threading
                            // index through the trait.
                            let partial = delta
                                .get("partial_json")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string();
                            out.blocks.push(CanonicalBlock::ToolCallDelta {
                                id: String::new(),
                                arguments_delta: partial,
                            });
                        }
                        _ => return Ok(None),
                    }
                }
            }

            "content_block_stop" => {
                // We don't know the block type from the stop event alone (only an
                // index). The driver in ticket B will track state and emit
                // `ToolCallEnd` when the stopped block was a tool_use. For now,
                // we return an empty delta rather than fabricating a block.
                return Ok(None);
            }

            "message_delta" => {
                if let Some(delta) = v.get("delta") {
                    if let Some(sr) = delta.get("stop_reason").and_then(|s| s.as_str()) {
                        out.finish_reason = map_stop_reason(sr);
                    }
                }
                if let Some(usage) = v.get("usage") {
                    out.usage = Some(CanonicalUsage {
                        prompt_tokens: usage
                            .get("input_tokens")
                            .and_then(|n| n.as_u64())
                            .map(|n| n as u32),
                        completion_tokens: usage
                            .get("output_tokens")
                            .and_then(|n| n.as_u64())
                            .map(|n| n as u32),
                        cache_read_input_tokens: usage
                            .get("cache_read_input_tokens")
                            .and_then(|n| n.as_u64())
                            .map(|n| n as u32),
                        cache_creation_input_tokens: usage
                            .get("cache_creation_input_tokens")
                            .and_then(|n| n.as_u64())
                            .map(|n| n as u32),
                    });
                }
            }

            _ => return Ok(None),
        }

        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Option<CanonicalStreamDelta> {
        AnthropicAdapter::new().parse_chunk(s).unwrap()
    }

    #[test]
    fn ping_returns_none() {
        assert!(parse(r#"{"type":"ping"}"#).is_none());
    }

    #[test]
    fn message_start_returns_none() {
        assert!(parse(r#"{"type":"message_start","message":{}}"#).is_none());
    }

    #[test]
    fn text_delta_emits_text_block() {
        let d = parse(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        )
        .unwrap();
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::Text {
                text: "Hello".into()
            }
        );
    }

    #[test]
    fn thinking_delta_emits_thinking_block() {
        let d = parse(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
        )
        .unwrap();
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::Thinking {
                text: "hmm".into(),
                signature: None
            }
        );
    }

    #[test]
    fn signature_delta_attaches_signature() {
        let d = parse(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig_xyz"}}"#,
        )
        .unwrap();
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::Thinking {
                text: String::new(),
                signature: Some("sig_xyz".into())
            }
        );
    }

    #[test]
    fn tool_use_start_emits_tool_call_start() {
        let d = parse(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"get_weather","input":{}}}"#,
        )
        .unwrap();
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::ToolCallStart {
                id: "toolu_01".into(),
                name: "get_weather".into(),
            }
        );
    }

    #[test]
    fn input_json_delta_emits_tool_call_delta() {
        let d = parse(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"loc\":"}}"#,
        )
        .unwrap();
        match &d.blocks[0] {
            CanonicalBlock::ToolCallDelta {
                arguments_delta, ..
            } => assert_eq!(arguments_delta, "{\"loc\":"),
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn message_delta_maps_stop_and_usage() {
        let d = parse(
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"input_tokens":12,"output_tokens":3,"cache_read_input_tokens":2}}"#,
        )
        .unwrap();
        assert_eq!(d.finish_reason, Some(FinishReason::ToolCalls));
        let u = d.usage.unwrap();
        assert_eq!(u.prompt_tokens, Some(12));
        assert_eq!(u.completion_tokens, Some(3));
        assert_eq!(u.cache_read_input_tokens, Some(2));
    }

    #[test]
    fn missing_type_errors() {
        let err = AnthropicAdapter::new()
            .parse_chunk(r#"{"foo":1}"#)
            .unwrap_err();
        assert!(matches!(err, ParseError::Shape(_)));
    }
}
