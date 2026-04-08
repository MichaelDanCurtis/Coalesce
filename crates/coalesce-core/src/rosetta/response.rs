//! Canonical response blocks — the unified assistant-output wire format.
//!
//! Every provider's streaming output is normalized into `CanonicalStreamDelta`
//! values containing `CanonicalBlock`s. This is the foundation for Ticket B
//! (SSE wiring) and downstream consumers like the desktop Chat, plugins,
//! failover comparison, and quality scoring.
//!
//! Note: `CanonicalBlock::Thinking` is the *wire-level* representation of a
//! thinking delta on the streaming path. It is distinct from
//! `super::thinking::CanonicalThinking`, which is the higher-level aggregated
//! representation used by the thinking optimizer.

use serde::{Deserialize, Serialize};

/// A single normalized output block from an assistant response.
///
/// Streaming providers emit sequences of these; non-streaming providers emit
/// fully-formed variants (e.g. `ToolCall` rather than `ToolCallStart`+`ToolCallDelta`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalBlock {
    /// Plain text output (incremental or full).
    Text { text: String },

    /// Thinking / reasoning text (incremental or full).
    Thinking {
        text: String,
        /// Anthropic-style signature for extended-thinking replay.
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Marks the start of a streamed tool call so the client can render a
    /// pending card before arguments arrive.
    ToolCallStart { id: String, name: String },

    /// Incremental JSON-string delta of a streamed tool call's arguments.
    ToolCallDelta { id: String, arguments_delta: String },

    /// Marks the end of a streamed tool call's arguments.
    ToolCallEnd { id: String },

    /// A fully-formed tool call (non-streaming providers, e.g. Google Gemini).
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// A tool execution result on the return trip.
    ToolResult {
        tool_call_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    /// A citation / source reference.
    Citation {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        start: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        end: Option<usize>,
    },

    /// An image block. Either `data` (base64) or `url` will be populated.
    Image {
        media_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
}

/// Canonical finish reason.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
}

/// Canonical token usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CanonicalUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
}

/// One normalized streaming delta from a provider.
///
/// A single provider chunk may contain multiple blocks (e.g. tool call start
/// plus its first arguments delta), plus optional `finish_reason` / `usage`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CanonicalStreamDelta {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<CanonicalBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<CanonicalUsage>,
}

impl CanonicalStreamDelta {
    /// Returns true if this delta has no blocks, finish reason, or usage.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.finish_reason.is_none() && self.usage.is_none()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip(block: CanonicalBlock) -> serde_json::Value {
        let v = serde_json::to_value(&block).unwrap();
        let back: CanonicalBlock = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(back, block);
        v
    }

    #[test]
    fn text_block_roundtrip() {
        let v = roundtrip(CanonicalBlock::Text { text: "hi".into() });
        assert_eq!(v, json!({"kind": "text", "text": "hi"}));
    }

    #[test]
    fn thinking_block_skips_none_signature() {
        let v = roundtrip(CanonicalBlock::Thinking {
            text: "hmm".into(),
            signature: None,
        });
        assert_eq!(v, json!({"kind": "thinking", "text": "hmm"}));
        assert!(v.get("signature").is_none());
    }

    #[test]
    fn thinking_block_with_signature() {
        let v = roundtrip(CanonicalBlock::Thinking {
            text: "".into(),
            signature: Some("sig_abc".into()),
        });
        assert_eq!(v["signature"], "sig_abc");
    }

    #[test]
    fn tool_call_start_and_delta_roundtrip() {
        let v = roundtrip(CanonicalBlock::ToolCallStart {
            id: "call_1".into(),
            name: "foo".into(),
        });
        assert_eq!(v["kind"], "tool_call_start");
        assert_eq!(v["id"], "call_1");

        let v = roundtrip(CanonicalBlock::ToolCallDelta {
            id: "call_1".into(),
            arguments_delta: "{\"x\":".into(),
        });
        assert_eq!(v["kind"], "tool_call_delta");
        assert_eq!(v["arguments_delta"], "{\"x\":");
    }

    #[test]
    fn tool_call_end_and_full_tool_call() {
        let v = roundtrip(CanonicalBlock::ToolCallEnd { id: "c".into() });
        assert_eq!(v, json!({"kind": "tool_call_end", "id": "c"}));

        let v = roundtrip(CanonicalBlock::ToolCall {
            id: "c".into(),
            name: "foo".into(),
            arguments: json!({"x": 1}),
        });
        assert_eq!(v["kind"], "tool_call");
        assert_eq!(v["arguments"]["x"], 1);
    }

    #[test]
    fn tool_result_skips_none_is_error() {
        let v = roundtrip(CanonicalBlock::ToolResult {
            tool_call_id: "c".into(),
            content: "ok".into(),
            is_error: None,
        });
        assert!(v.get("is_error").is_none());

        let v = roundtrip(CanonicalBlock::ToolResult {
            tool_call_id: "c".into(),
            content: "bad".into(),
            is_error: Some(true),
        });
        assert_eq!(v["is_error"], true);
    }

    #[test]
    fn citation_drops_none_fields() {
        let v = roundtrip(CanonicalBlock::Citation {
            source: "https://example.com".into(),
            text: None,
            start: None,
            end: None,
        });
        assert_eq!(v["kind"], "citation");
        assert!(v.get("text").is_none());
        assert!(v.get("start").is_none());
        assert!(v.get("end").is_none());
    }

    #[test]
    fn image_block_drops_none() {
        let v = roundtrip(CanonicalBlock::Image {
            media_type: "image/png".into(),
            data: Some("base64data".into()),
            url: None,
        });
        assert_eq!(v["data"], "base64data");
        assert!(v.get("url").is_none());
    }

    #[test]
    fn finish_reason_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(FinishReason::ToolCalls).unwrap(),
            json!("tool_calls")
        );
        assert_eq!(
            serde_json::to_value(FinishReason::ContentFilter).unwrap(),
            json!("content_filter")
        );
    }

    #[test]
    fn canonical_usage_skips_nones() {
        let u = CanonicalUsage {
            prompt_tokens: Some(10),
            ..Default::default()
        };
        let v = serde_json::to_value(&u).unwrap();
        assert_eq!(v, json!({"prompt_tokens": 10}));
    }

    #[test]
    fn stream_delta_empty_default() {
        let d = CanonicalStreamDelta::default();
        assert!(d.is_empty());
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v, json!({}));
    }

    #[test]
    fn stream_delta_full_roundtrip() {
        let d = CanonicalStreamDelta {
            blocks: vec![
                CanonicalBlock::Text { text: "hi".into() },
                CanonicalBlock::ToolCallStart {
                    id: "c1".into(),
                    name: "f".into(),
                },
            ],
            finish_reason: Some(FinishReason::Stop),
            usage: Some(CanonicalUsage {
                prompt_tokens: Some(3),
                completion_tokens: Some(7),
                ..Default::default()
            }),
        };
        let v = serde_json::to_value(&d).unwrap();
        let back: CanonicalStreamDelta = serde_json::from_value(v).unwrap();
        assert_eq!(back, d);
    }
}
