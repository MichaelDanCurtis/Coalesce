//! Stateful driver that wraps the stateless Rosetta `ProviderStreamAdapter`s
//! and fills in the gaps each adapter cannot handle on its own:
//!
//! * **OpenAI-compat:** synthesize `ToolCallEnd` when `finish_reason ==
//!   ToolCalls`, using the ids we've seen on `ToolCallStart` / `ToolCallDelta`.
//! * **Anthropic:** track `content_block_start` index → (kind, id, name), so
//!   that (a) `input_json_delta` events (which the adapter emits with an empty
//!   id because the event only carries an index) can have their id back-filled,
//!   and (b) `content_block_stop` events on `tool_use` blocks can be turned
//!   into `ToolCallEnd`.
//! * **Google:** the adapter emits a placeholder `gemini_<name>` id for
//!   full `ToolCall` blocks; we replace that with a stable counter-based id
//!   so downstream consumers get a unique identifier per call.
//!
//! This module is self-contained and has no dependency on the proxy's SSE
//! forwarder. Ticket B2 will wire it in; this ticket (B1) ships only the
//! driver plus unit tests.

use coalesce_core::rosetta::{
    AnthropicAdapter, CanonicalBlock, CanonicalStreamDelta, FinishReason, GoogleAdapter,
    OpenAiCompatAdapter, ParseError, ProviderStreamAdapter,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Which on-wire format the provider is streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFormat {
    /// OpenAI-shaped `choices[].delta` chunks. Used by OpenAI, GLM/z.ai,
    /// DeepSeek, Copilot, OpenRouter, Ollama, Mistral, and the LocalLLM
    /// passthroughs. Also used by Google today because the
    /// `GoogleCloudCodeProvider` translates native Gemini → OpenAI-compat
    /// on the wire before the stream reaches the proxy forwarder.
    OpenAiCompat,
    /// Anthropic native events (`type` field on each JSON payload).
    AnthropicNative,
    /// Native Gemini chunks (`candidates[].content.parts[...]`). Not used
    /// by the current proxy forwarder but kept for completeness — if a
    /// future provider starts streaming raw Gemini this is the format to
    /// pick. Ticket B2 should prefer `OpenAiCompat` for `google` today.
    GoogleNative,
}

/// Pick the streaming format for a provider identified by its `Provider::name()`.
///
/// The mapping matches the convention used throughout the proxy: providers
/// are keyed by string name (see `crates/coalesce-core/src/providers/`). We
/// default unknown providers to `OpenAiCompat` because every provider we
/// currently ship either speaks OpenAI-compat natively or translates to it
/// before emitting bytes.
pub fn format_for_provider(provider_name: &str) -> StreamFormat {
    match provider_name {
        "anthropic" => StreamFormat::AnthropicNative,
        // "google" intentionally maps to OpenAiCompat: GoogleCloudCodeProvider
        // rewrites native Gemini chunks into OpenAI-compat shape in
        // `chat_stream` before returning the ByteStream.
        _ => StreamFormat::OpenAiCompat,
    }
}

/// Per-index state tracked for an Anthropic stream so we can back-fill tool
/// call delta ids and synthesize `ToolCallEnd` on `content_block_stop`.
#[derive(Debug, Clone)]
struct AnthropicBlockState {
    /// "text" | "thinking" | "tool_use"
    kind: String,
    /// Tool use id, if this block is a tool_use.
    id: Option<String>,
    /// Tool name, mostly for debugging.
    #[allow(dead_code)]
    name: Option<String>,
}

/// Stateful per-stream driver. One `CanonicalDriver` is created per
/// upstream request and lives for the duration of the stream.
pub struct CanonicalDriver {
    adapter: Arc<dyn ProviderStreamAdapter>,
    format: StreamFormat,

    // OpenAI: we don't track per-index ids because OpenAI tool-call chunks
    // always carry the id on the first sight (`ToolCallStart`) and subsequent
    // deltas reference the same id. We keep a small vec of live ids so that
    // when `finish_reason == ToolCalls` arrives we can synthesize one
    // `ToolCallEnd` per live id in insertion order.
    openai_live_tool_ids: Vec<String>,

    // Anthropic: content_block index → (kind, id, name).
    anthropic_blocks: HashMap<u32, AnthropicBlockState>,
    // Anthropic: most-recently-started tool_use index (for back-filling
    // `input_json_delta` ids where the adapter leaves id empty).
    anthropic_last_tool_index: Option<u32>,

    // Google: counter for stable synthetic tool-call ids, replacing the
    // adapter's `gemini_<name>` placeholder.
    google_counter: u64,
}

impl CanonicalDriver {
    /// Create a new driver for the given stream format.
    pub fn new(format: StreamFormat) -> Self {
        let adapter: Arc<dyn ProviderStreamAdapter> = match format {
            StreamFormat::OpenAiCompat => Arc::new(OpenAiCompatAdapter::new()),
            StreamFormat::AnthropicNative => Arc::new(AnthropicAdapter::new()),
            StreamFormat::GoogleNative => Arc::new(GoogleAdapter::new()),
        };
        Self {
            adapter,
            format,
            openai_live_tool_ids: Vec::new(),
            anthropic_blocks: HashMap::new(),
            anthropic_last_tool_index: None,
            google_counter: 0,
        }
    }

    /// Convenience accessor — useful for tests and for B2 wiring.
    pub fn format(&self) -> StreamFormat {
        self.format
    }

    /// Process a single raw JSON payload (already stripped of its
    /// `data: ` prefix and not `[DONE]`) and return zero or more canonical
    /// stream deltas. Multiple deltas can come back when the driver
    /// synthesizes a follow-up event (e.g. a `ToolCallEnd` derived from a
    /// stateless `finish_reason == tool_calls` signal, or a `ToolCallEnd`
    /// derived from an Anthropic `content_block_stop` that the adapter
    /// itself returned as `Ok(None)`).
    pub fn process_chunk(
        &mut self,
        raw_json: &str,
    ) -> Result<Vec<CanonicalStreamDelta>, ParseError> {
        // Anthropic `content_block_stop` is the one event where we must
        // peek at the raw JSON ourselves — the stateless adapter returns
        // `Ok(None)` because the event only carries an index. Handle it
        // before delegating so the driver can synthesize a `ToolCallEnd`
        // when the stopped block was a tool_use.
        if matches!(self.format, StreamFormat::AnthropicNative) {
            // Parse once; propagate errors so malformed JSON still surfaces as
            // ParseError::Json rather than being silently swallowed.
            let v: serde_json::Value = serde_json::from_str(raw_json)?;
            {
                if v.get("type").and_then(|t| t.as_str()) == Some("content_block_stop") {
                    if let Some(index) = v.get("index").and_then(|i| i.as_u64()) {
                        let idx = index as u32;
                        if let Some(state) = self.anthropic_blocks.remove(&idx) {
                            if state.kind == "tool_use" {
                                if let Some(id) = state.id {
                                    if self.anthropic_last_tool_index == Some(idx) {
                                        self.anthropic_last_tool_index = None;
                                    }
                                    let mut delta = CanonicalStreamDelta::default();
                                    delta.blocks.push(CanonicalBlock::ToolCallEnd { id });
                                    return Ok(vec![delta]);
                                }
                            }
                        }
                    }
                    // Not a tool_use stop (or unknown index) — nothing to emit.
                    return Ok(vec![]);
                }
                let _ = v;
            }
        }

        let parsed = self.adapter.parse_chunk(raw_json)?;
        let Some(mut delta) = parsed else {
            return Ok(vec![]);
        };

        match self.format {
            StreamFormat::OpenAiCompat => {
                // Record tool-call ids seen on Start, rewrite any Delta with an
                // empty id to the most-recent live id.
                for block in &mut delta.blocks {
                    match block {
                        CanonicalBlock::ToolCallStart { id, .. } if !id.is_empty() => {
                            if !self.openai_live_tool_ids.contains(id) {
                                self.openai_live_tool_ids.push(id.clone());
                            }
                        }
                        CanonicalBlock::ToolCallDelta { id, .. } if id.is_empty() => {
                            if let Some(last) = self.openai_live_tool_ids.last() {
                                *id = last.clone();
                            }
                        }
                        _ => {}
                    }
                }

                // If finish_reason == ToolCalls, synthesize ToolCallEnd for
                // every live id (preserving insertion order) and return the
                // original delta + a follow-up synthetic delta.
                if delta.finish_reason == Some(FinishReason::ToolCalls)
                    && !self.openai_live_tool_ids.is_empty()
                {
                    let mut synth = CanonicalStreamDelta::default();
                    for id in self.openai_live_tool_ids.drain(..) {
                        synth.blocks.push(CanonicalBlock::ToolCallEnd { id });
                    }
                    // Emit both: the original (with its finish_reason) and the
                    // synthetic tool-call-end follow-up.
                    return Ok(vec![delta, synth]);
                }
            }

            StreamFormat::AnthropicNative => {
                // Inspect the raw JSON once to learn the content_block_start
                // index (the adapter drops it). We need it to keep
                // anthropic_blocks and anthropic_last_tool_index in sync.
                let raw: Option<serde_json::Value> = serde_json::from_str(raw_json).ok();
                let event_type = raw
                    .as_ref()
                    .and_then(|v| v.get("type"))
                    .and_then(|t| t.as_str());
                let raw_index = raw
                    .as_ref()
                    .and_then(|v| v.get("index"))
                    .and_then(|i| i.as_u64())
                    .map(|n| n as u32);

                if event_type == Some("content_block_start") {
                    if let Some(idx) = raw_index {
                        let cb = raw.as_ref().and_then(|v| v.get("content_block"));
                        let kind = cb
                            .and_then(|c| c.get("type"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        let id = cb
                            .and_then(|c| c.get("id"))
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        let name = cb
                            .and_then(|c| c.get("name"))
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        if kind == "tool_use" {
                            self.anthropic_last_tool_index = Some(idx);
                        }
                        self.anthropic_blocks.insert(
                            idx,
                            AnthropicBlockState {
                                kind,
                                id,
                                name,
                            },
                        );
                    }
                }

                // Back-fill empty-id ToolCallDelta blocks from the most
                // recent tool_use start we've seen.
                for block in &mut delta.blocks {
                    if let CanonicalBlock::ToolCallDelta { id, .. } = block {
                        if id.is_empty() {
                            if let Some(live_idx) = self.anthropic_last_tool_index {
                                if let Some(state) = self.anthropic_blocks.get(&live_idx) {
                                    if let Some(state_id) = &state.id {
                                        *id = state_id.clone();
                                    }
                                }
                            }
                        }
                    }
                }
            }

            StreamFormat::GoogleNative => {
                // Replace `gemini_<name>` placeholder ids with stable counter ids.
                for block in &mut delta.blocks {
                    if let CanonicalBlock::ToolCall { id, .. } = block {
                        if id.starts_with("gemini_") {
                            self.google_counter += 1;
                            *id = format!("call_{}", self.google_counter);
                        }
                    }
                }
            }
        }

        if delta.is_empty() {
            Ok(vec![])
        } else {
            Ok(vec![delta])
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_blocks(deltas: &[CanonicalStreamDelta]) -> Vec<CanonicalBlock> {
        deltas.iter().flat_map(|d| d.blocks.clone()).collect()
    }

    // ── format_for_provider ────────────────────────────────────────────────

    #[test]
    fn provider_mapping_matches_expected() {
        assert_eq!(format_for_provider("anthropic"), StreamFormat::AnthropicNative);
        assert_eq!(format_for_provider("google"), StreamFormat::OpenAiCompat);
        assert_eq!(format_for_provider("openai"), StreamFormat::OpenAiCompat);
        assert_eq!(format_for_provider("glm"), StreamFormat::OpenAiCompat);
        assert_eq!(format_for_provider("copilot"), StreamFormat::OpenAiCompat);
        assert_eq!(format_for_provider("deepseek"), StreamFormat::OpenAiCompat);
        assert_eq!(format_for_provider("openrouter"), StreamFormat::OpenAiCompat);
        assert_eq!(format_for_provider("ollama"), StreamFormat::OpenAiCompat);
        assert_eq!(format_for_provider("something_new"), StreamFormat::OpenAiCompat);
    }

    // ── OpenAI-compat ──────────────────────────────────────────────────────

    #[test]
    fn openai_plain_text_flows_through_unchanged() {
        let mut d = CanonicalDriver::new(StreamFormat::OpenAiCompat);
        let out = d
            .process_chunk(r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#)
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].blocks,
            vec![CanonicalBlock::Text { text: "hi".into() }]
        );
    }

    #[test]
    fn openai_tool_call_end_synthesized_on_finish_reason() {
        let mut d = CanonicalDriver::new(StreamFormat::OpenAiCompat);

        // Start + first args
        let out = d
            .process_chunk(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"foo","arguments":"{\"x\":"}}]}}]}"#,
            )
            .unwrap();
        assert_eq!(out.len(), 1);
        let blocks = flat_blocks(&out);
        assert!(matches!(blocks[0], CanonicalBlock::ToolCallStart { .. }));
        assert!(matches!(blocks[1], CanonicalBlock::ToolCallDelta { .. }));

        // Second args chunk — no id on the wire; driver must back-fill.
        let out = d
            .process_chunk(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}"#,
            )
            .unwrap();
        let blocks = flat_blocks(&out);
        match &blocks[0] {
            CanonicalBlock::ToolCallDelta { id, arguments_delta } => {
                assert_eq!(id, "call_abc", "delta id should be back-filled");
                assert_eq!(arguments_delta, "1}");
            }
            other => panic!("unexpected {:?}", other),
        }

        // finish_reason arrives → driver synthesizes ToolCallEnd.
        let out = d
            .process_chunk(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        assert_eq!(out.len(), 2, "expect original delta + synthetic end delta");
        assert_eq!(out[0].finish_reason, Some(FinishReason::ToolCalls));
        assert_eq!(
            out[1].blocks,
            vec![CanonicalBlock::ToolCallEnd {
                id: "call_abc".into()
            }]
        );

        // Live ids are drained; a second finish_reason wouldn't re-emit.
        assert!(d.openai_live_tool_ids.is_empty());
    }

    #[test]
    fn openai_plain_finish_reason_does_not_synthesize_end() {
        let mut d = CanonicalDriver::new(StreamFormat::OpenAiCompat);
        let _ = d
            .process_chunk(r#"{"choices":[{"delta":{"content":"done"}}]}"#)
            .unwrap();
        let out = d
            .process_chunk(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#)
            .unwrap();
        // One delta, no synthetic follow-up, no tool_call_end blocks.
        assert_eq!(out.len(), 1);
        assert!(flat_blocks(&out)
            .iter()
            .all(|b| !matches!(b, CanonicalBlock::ToolCallEnd { .. })));
    }

    // ── Anthropic ──────────────────────────────────────────────────────────

    #[test]
    fn anthropic_tool_use_roundtrip_backfills_and_synthesizes_end() {
        let mut d = CanonicalDriver::new(StreamFormat::AnthropicNative);

        // content_block_start for a tool_use at index 1
        let out = d
            .process_chunk(
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"get_weather","input":{}}}"#,
            )
            .unwrap();
        let blocks = flat_blocks(&out);
        assert_eq!(
            blocks[0],
            CanonicalBlock::ToolCallStart {
                id: "toolu_01".into(),
                name: "get_weather".into()
            }
        );

        // input_json_delta — adapter emits empty id, driver back-fills.
        let out = d
            .process_chunk(
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"loc\":\"NYC\"}"}}"#,
            )
            .unwrap();
        let blocks = flat_blocks(&out);
        match &blocks[0] {
            CanonicalBlock::ToolCallDelta { id, arguments_delta } => {
                assert_eq!(id, "toolu_01");
                assert_eq!(arguments_delta, "{\"loc\":\"NYC\"}");
            }
            other => panic!("unexpected {:?}", other),
        }

        // content_block_stop — adapter returns None; driver synthesizes End.
        let out = d
            .process_chunk(r#"{"type":"content_block_stop","index":1}"#)
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].blocks,
            vec![CanonicalBlock::ToolCallEnd {
                id: "toolu_01".into()
            }]
        );
    }

    #[test]
    fn anthropic_text_block_stop_emits_nothing_synthetic() {
        let mut d = CanonicalDriver::new(StreamFormat::AnthropicNative);

        let _ = d
            .process_chunk(
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            )
            .unwrap();
        let out = d
            .process_chunk(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            )
            .unwrap();
        assert_eq!(
            out[0].blocks,
            vec![CanonicalBlock::Text { text: "Hello".into() }]
        );
        // Stop on a text block → no synthetic tool_call_end.
        let out = d
            .process_chunk(r#"{"type":"content_block_stop","index":0}"#)
            .unwrap();
        assert!(out.is_empty(), "text block stop should be silent");
    }

    // ── Google ─────────────────────────────────────────────────────────────

    #[test]
    fn google_tool_call_ids_are_stable_and_counter_based() {
        let mut d = CanonicalDriver::new(StreamFormat::GoogleNative);
        let out = d
            .process_chunk(
                r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"loc":"NYC"}}}]}}]}"#,
            )
            .unwrap();
        let blocks = flat_blocks(&out);
        let id1 = match &blocks[0] {
            CanonicalBlock::ToolCall { id, .. } => id.clone(),
            other => panic!("unexpected {:?}", other),
        };
        assert_eq!(id1, "call_1");

        // A second tool call in a later chunk gets a fresh id.
        let out = d
            .process_chunk(
                r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"loc":"SF"}}}]}}]}"#,
            )
            .unwrap();
        let id2 = match &flat_blocks(&out)[0] {
            CanonicalBlock::ToolCall { id, .. } => id.clone(),
            other => panic!("unexpected {:?}", other),
        };
        assert_eq!(id2, "call_2");
        assert_ne!(id1, id2);
    }

    // ── Error propagation ──────────────────────────────────────────────────

    #[test]
    fn malformed_json_errors_on_openai() {
        let mut d = CanonicalDriver::new(StreamFormat::OpenAiCompat);
        let err = d.process_chunk("{not json").unwrap_err();
        assert!(matches!(err, ParseError::Json(_)));
    }

    #[test]
    fn malformed_json_errors_on_anthropic() {
        let mut d = CanonicalDriver::new(StreamFormat::AnthropicNative);
        // Malformed JSON must surface as ParseError::Json even on the
        // Anthropic path, which has its own up-front parse for the
        // content_block_stop fast-path.
        let err = d.process_chunk("{not json").unwrap_err();
        assert!(matches!(err, ParseError::Json(_)));
    }
}
