//! Google Gemini streaming chunk adapter.
//!
//! Example shape:
//! ```json
//! {"candidates":[{"content":{"parts":[{"text":"hello"}]},"finishReason":"STOP"}],
//!  "usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":4}}
//! ```
//!
//! Gemini streams text via repeated `parts[].text` fragments, and emits tool
//! calls as fully-formed `functionCall` parts (not incremental) — so those
//! map to the `CanonicalBlock::ToolCall` variant rather than Start/Delta/End.
//!
//! TODO(ticket-b): The "thought summaries" API (parts with `thought: true`)
//! is handled as a `Thinking` block; verify against real payloads once the
//! streaming path is wired in ticket B.

use crate::rosetta::response::{
    CanonicalBlock, CanonicalStreamDelta, CanonicalUsage, FinishReason,
};
use crate::rosetta::stream_adapter::{ParseError, ProviderStreamAdapter};

/// Adapter for Google Gemini streaming chunks.
#[derive(Debug, Default, Clone, Copy)]
pub struct GoogleAdapter;

impl GoogleAdapter {
    pub fn new() -> Self {
        Self
    }
}

fn map_finish(s: &str) -> Option<FinishReason> {
    match s {
        "STOP" => Some(FinishReason::Stop),
        "MAX_TOKENS" => Some(FinishReason::Length),
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            Some(FinishReason::ContentFilter)
        }
        "OTHER" => Some(FinishReason::Error),
        _ => None,
    }
}

impl ProviderStreamAdapter for GoogleAdapter {
    fn parse_chunk(&self, raw_json: &str) -> Result<Option<CanonicalStreamDelta>, ParseError> {
        let v: serde_json::Value = serde_json::from_str(raw_json)?;
        let mut out = CanonicalStreamDelta::default();

        if let Some(cand) = v
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        {
            if let Some(parts) = cand
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    let is_thought = part
                        .get("thought")
                        .and_then(|t| t.as_bool())
                        .unwrap_or(false);

                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            if is_thought {
                                out.blocks.push(CanonicalBlock::Thinking {
                                    text: text.to_string(),
                                    signature: None,
                                });
                            } else {
                                out.blocks.push(CanonicalBlock::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                    } else if let Some(fc) = part.get("functionCall") {
                        let name = fc
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = fc
                            .get("args")
                            .cloned()
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        // Gemini doesn't supply a call id; synthesize one from the name.
                        // Ticket B's driver may override this with a UUID.
                        let id = format!("gemini_{}", name);
                        out.blocks.push(CanonicalBlock::ToolCall { id, name, arguments: args });
                    }
                }
            }

            if let Some(fr) = cand.get("finishReason").and_then(|f| f.as_str()) {
                out.finish_reason = map_finish(fr);
            }
        }

        if let Some(usage) = v.get("usageMetadata") {
            out.usage = Some(CanonicalUsage {
                prompt_tokens: usage
                    .get("promptTokenCount")
                    .and_then(|n| n.as_u64())
                    .map(|n| n as u32),
                completion_tokens: usage
                    .get("candidatesTokenCount")
                    .and_then(|n| n.as_u64())
                    .map(|n| n as u32),
                cache_read_input_tokens: usage
                    .get("cachedContentTokenCount")
                    .and_then(|n| n.as_u64())
                    .map(|n| n as u32),
                cache_creation_input_tokens: None,
            });
        }

        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> CanonicalStreamDelta {
        GoogleAdapter::new().parse_chunk(s).unwrap().unwrap()
    }

    #[test]
    fn text_part_emits_text() {
        let d = parse(
            r#"{"candidates":[{"content":{"parts":[{"text":"hello"}]},"finishReason":null}]}"#,
        );
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::Text {
                text: "hello".into()
            }
        );
    }

    #[test]
    fn function_call_emits_full_tool_call() {
        let d = parse(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"location":"NYC"}}}]}}]}"#,
        );
        match &d.blocks[0] {
            CanonicalBlock::ToolCall { name, arguments, .. } => {
                assert_eq!(name, "get_weather");
                assert_eq!(arguments["location"], "NYC");
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn thought_part_emits_thinking() {
        let d = parse(
            r#"{"candidates":[{"content":{"parts":[{"text":"reasoning","thought":true}]}}]}"#,
        );
        assert_eq!(
            d.blocks[0],
            CanonicalBlock::Thinking {
                text: "reasoning".into(),
                signature: None
            }
        );
    }

    #[test]
    fn finish_reason_maps() {
        let d = parse(
            r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}"#,
        );
        assert_eq!(d.finish_reason, Some(FinishReason::Stop));

        let d = parse(
            r#"{"candidates":[{"content":{"parts":[]},"finishReason":"MAX_TOKENS"}]}"#,
        );
        assert_eq!(d.finish_reason, Some(FinishReason::Length));

        let d = parse(
            r#"{"candidates":[{"content":{"parts":[]},"finishReason":"SAFETY"}]}"#,
        );
        assert_eq!(d.finish_reason, Some(FinishReason::ContentFilter));
    }

    #[test]
    fn usage_extraction() {
        let d = parse(
            r#"{"candidates":[{"content":{"parts":[]}}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":4,"cachedContentTokenCount":3}}"#,
        );
        let u = d.usage.unwrap();
        assert_eq!(u.prompt_tokens, Some(12));
        assert_eq!(u.completion_tokens, Some(4));
        assert_eq!(u.cache_read_input_tokens, Some(3));
    }
}
