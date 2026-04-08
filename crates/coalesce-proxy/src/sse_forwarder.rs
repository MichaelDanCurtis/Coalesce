//! SSE forwarder that drives a `CanonicalDriver` over a provider byte stream
//! and injects a per-chunk `x_coalesce` canonical delta into each JSON payload.
//!
//! The provider's raw byte stream is parsed with `eventsource_stream` (already
//! in the proxy's dependency set — no new crate), events are routed through
//! `CanonicalDriver::process_chunk`, and the results are merged back into the
//! original JSON under the key `x_coalesce`. OpenAI SDKs and other consumers
//! that don't know about `x_coalesce` will ignore the unknown field.
//!
//! Head-of-stream routing metadata (the small synthetic `data: {"x_coalesce":
//! {tier, provider, model, ...}}` event prepended in `lib.rs` before this
//! function sees bytes) is NOT produced here; the caller prepends it with
//! `futures::stream::chain`. Ticket B3 will rename that head key to
//! `x_coalesce_route` to disambiguate it from this per-chunk field.
//!
//! Ownership: the `CanonicalDriver` is held `&mut` across awaits inside the
//! `async_stream::stream!` closure, which is the cleanest way to keep a
//! non-`Clone`, non-`Sync` piece of state alive across a streaming pipeline
//! without resorting to `Arc<Mutex<_>>`.

use crate::canonical_driver::{format_for_provider, CanonicalDriver};
use bytes::Bytes;
use coalesce_core::providers::ByteStream;
use coalesce_core::rosetta::{CanonicalStreamDelta, CanonicalBlock};
use eventsource_stream::Eventsource;
use futures::Stream;
use futures::StreamExt;
use std::io;

/// Transform a provider byte stream into an `io::Result<Bytes>` stream suitable
/// for `axum::body::Body::from_stream`, attaching a `x_coalesce` field to each
/// JSON data event.
///
/// Non-JSON events (`[DONE]`, heartbeats, unknown payloads) are passed through
/// unchanged. Parse errors on the canonical path are logged at `debug!` and
/// the original event is re-emitted unchanged — we never drop provider output
/// because of a canonical-side problem.
pub fn transform_stream(
    byte_stream: ByteStream,
    provider_name: &str,
) -> impl Stream<Item = io::Result<Bytes>> + Send {
    let format = format_for_provider(provider_name);
    let mut driver = CanonicalDriver::new(format);
    // Parse bytes into SSE events. eventsource_stream handles multi-line
    // `data:` concatenation, CRLF, event-type headers, and keep-alive
    // comments (which it silently drops).
    let mut events = byte_stream.eventsource();

    async_stream::stream! {
        while let Some(event_result) = events.next().await {
            match event_result {
                Err(e) => {
                    yield Err(io::Error::new(io::ErrorKind::Other, e.to_string()));
                }
                Ok(event) => {
                    // `[DONE]` sentinel — pass through untouched.
                    if event.data == "[DONE]" {
                        yield Ok(render_sse(&event.event, "[DONE]"));
                        continue;
                    }

                    // Empty data — rare but possible; pass through.
                    if event.data.is_empty() {
                        yield Ok(render_sse(&event.event, ""));
                        continue;
                    }

                    // Hand the payload to the canonical driver.
                    let deltas = match driver.process_chunk(&event.data) {
                        Ok(d) => d,
                        Err(err) => {
                            tracing::debug!(
                                error = %err,
                                "canonical driver parse error; re-emitting original chunk"
                            );
                            yield Ok(render_sse(&event.event, &event.data));
                            continue;
                        }
                    };

                    // Merge all returned deltas into one: concatenate blocks,
                    // keep the last non-None finish_reason and usage.
                    let merged = merge_deltas(deltas);

                    if merged.is_empty() {
                        // Driver had nothing useful to add (e.g. ping on the
                        // Anthropic fast-path that swallowed a content_block_stop
                        // non-tool event). Forward the original.
                        yield Ok(render_sse(&event.event, &event.data));
                        continue;
                    }

                    // Parse the original JSON so we can splice `x_coalesce` in
                    // at the top level. If it isn't a JSON object, fall back
                    // to emitting a standalone synthetic event carrying only
                    // the canonical delta.
                    match serde_json::from_str::<serde_json::Value>(&event.data) {
                        Ok(serde_json::Value::Object(mut map)) => {
                            match serde_json::to_value(&merged) {
                                Ok(v) => { map.insert("x_coalesce".into(), v); }
                                Err(e) => {
                                    tracing::debug!(
                                        error = %e,
                                        "failed to serialize canonical delta; dropping injection"
                                    );
                                }
                            }
                            let out = serde_json::Value::Object(map).to_string();
                            yield Ok(render_sse(&event.event, &out));
                        }
                        _ => {
                            // Non-object or non-JSON — emit the original and a
                            // separate synthetic canonical chunk so the client
                            // still sees the blocks.
                            yield Ok(render_sse(&event.event, &event.data));
                            let synth = serde_json::json!({ "x_coalesce": merged });
                            yield Ok(render_sse("", &synth.to_string()));
                        }
                    }
                }
            }
        }
    }
}

/// Combine a vector of canonical deltas into a single one, preserving block
/// order and taking the last non-None `finish_reason` / `usage`.
fn merge_deltas(deltas: Vec<CanonicalStreamDelta>) -> CanonicalStreamDelta {
    let mut out = CanonicalStreamDelta::default();
    for d in deltas {
        out.blocks.extend(d.blocks);
        if d.finish_reason.is_some() {
            out.finish_reason = d.finish_reason;
        }
        if d.usage.is_some() {
            out.usage = d.usage;
        }
    }
    out
}

/// Render one SSE event frame. Empty `event_type` omits the `event:` header,
/// which is what the OpenAI wire format uses.
fn render_sse(event_type: &str, data: &str) -> Bytes {
    let mut buf = String::with_capacity(data.len() + 16);
    if !event_type.is_empty() {
        buf.push_str("event: ");
        buf.push_str(event_type);
        buf.push('\n');
    }
    buf.push_str("data: ");
    buf.push_str(data);
    buf.push_str("\n\n");
    Bytes::from(buf)
}

// Silence dead-code warnings if CanonicalBlock ends up unused after edits.
#[allow(dead_code)]
fn _touch_canonical_block(_: &CanonicalBlock) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    /// Build a fake `ByteStream` from a slice of &str chunks.
    fn fake_stream(chunks: &[&str]) -> ByteStream {
        let owned: Vec<Bytes> = chunks.iter().map(|s| Bytes::from(s.to_string())).collect();
        Box::pin(stream::iter(owned.into_iter().map(Ok::<_, reqwest::Error>)))
    }

    /// Drain a transformed stream into a single String for assertion.
    async fn drain(s: impl Stream<Item = io::Result<Bytes>> + Send) -> String {
        let mut out = String::new();
        futures::pin_mut!(s);
        while let Some(chunk) = s.next().await {
            let b = chunk.unwrap();
            out.push_str(std::str::from_utf8(&b).unwrap());
        }
        out
    }

    #[tokio::test]
    async fn openai_text_chunk_gets_x_coalesce_injected() {
        let raw = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n";
        let bs = fake_stream(&[raw]);
        let s = transform_stream(bs, "openai");
        let out = drain(s).await;

        // Find the data line.
        let line = out.lines().find(|l| l.starts_with("data: ")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&line[6..]).unwrap();
        // Original fields preserved.
        assert_eq!(
            json["choices"][0]["delta"]["content"], "hi",
            "original content preserved"
        );
        // Injection present.
        let xc = &json["x_coalesce"];
        assert_eq!(xc["blocks"][0]["kind"], "text");
        assert_eq!(xc["blocks"][0]["text"], "hi");
    }

    #[tokio::test]
    async fn openai_done_sentinel_passes_through() {
        let bs = fake_stream(&["data: [DONE]\n\n"]);
        let s = transform_stream(bs, "openai");
        let out = drain(s).await;
        assert!(out.contains("data: [DONE]"), "got: {}", out);
        assert!(!out.contains("x_coalesce"));
    }

    #[tokio::test]
    async fn openai_tool_call_finish_reason_synthesizes_end_block() {
        // Three SSE events: start+args, args-only, finish.
        let start = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_abc\",\"function\":{\"name\":\"foo\",\"arguments\":\"{\\\"x\\\":\"}}]}}]}\n\n";
        let mid = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]}}]}\n\n";
        let fin = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n";
        let bs = fake_stream(&[start, mid, fin]);
        let s = transform_stream(bs, "openai");
        let out = drain(s).await;

        // Collect every data: JSON line.
        let data_lines: Vec<serde_json::Value> = out
            .lines()
            .filter(|l| l.starts_with("data: "))
            .map(|l| serde_json::from_str(&l[6..]).unwrap())
            .collect();

        // Expect 3 lines total — the finish event merges its synthesized
        // ToolCallEnd into its own x_coalesce blocks array.
        assert_eq!(data_lines.len(), 3, "got {} data lines: {}", data_lines.len(), out);

        // Final line should have finish_reason + a synthesized tool_call_end block.
        let last_xc = &data_lines[2]["x_coalesce"];
        assert_eq!(last_xc["finish_reason"], "tool_calls");
        let blocks = last_xc["blocks"].as_array().unwrap();
        assert!(
            blocks.iter().any(|b| b["kind"] == "tool_call_end" && b["id"] == "call_abc"),
            "expected tool_call_end for call_abc, got {:?}",
            blocks
        );
    }

    #[tokio::test]
    async fn anthropic_content_block_stop_injects_end() {
        // content_block_start for a tool_use at index 1, then content_block_stop.
        // The stop event has no useful original JSON from the adapter's POV —
        // the driver synthesizes a ToolCallEnd. transform_stream injects it
        // into the stop event's JSON as x_coalesce.
        let start = "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_01\",\"name\":\"get_weather\",\"input\":{}}}\n\n";
        let stop = "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n";
        let bs = fake_stream(&[start, stop]);
        let s = transform_stream(bs, "anthropic");
        let out = drain(s).await;

        let data_lines: Vec<&str> = out.lines().filter(|l| l.starts_with("data: ")).collect();
        assert_eq!(data_lines.len(), 2, "got: {}", out);

        // First data line: original content_block_start JSON + x_coalesce with ToolCallStart.
        let start_json: serde_json::Value = serde_json::from_str(&data_lines[0][6..]).unwrap();
        assert_eq!(start_json["type"], "content_block_start");
        assert_eq!(
            start_json["x_coalesce"]["blocks"][0]["kind"], "tool_call_start"
        );
        assert_eq!(
            start_json["x_coalesce"]["blocks"][0]["id"], "toolu_01"
        );

        // Second data line: the stop event now carries a synthetic tool_call_end.
        let stop_json: serde_json::Value = serde_json::from_str(&data_lines[1][6..]).unwrap();
        assert_eq!(stop_json["type"], "content_block_stop");
        let blocks = stop_json["x_coalesce"]["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["kind"], "tool_call_end");
        assert_eq!(blocks[0]["id"], "toolu_01");
    }

    #[tokio::test]
    async fn parse_error_re_emits_original_chunk() {
        // Not-JSON payload — driver returns Err, forwarder must fall back
        // to passing the original through.
        let raw = "data: not-json-at-all\n\n";
        let bs = fake_stream(&[raw]);
        let s = transform_stream(bs, "openai");
        let out = drain(s).await;
        assert!(out.contains("not-json-at-all"));
        assert!(!out.contains("x_coalesce"));
    }

    #[tokio::test]
    async fn empty_delta_still_emits_original_chunk() {
        // OpenAI chunk whose delta is {} — driver returns an empty canonical
        // delta, forwarder should fall back to the original payload.
        let raw = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":null}]}\n\n";
        let bs = fake_stream(&[raw]);
        let s = transform_stream(bs, "openai");
        let out = drain(s).await;
        // Should emit exactly one data line, and x_coalesce should NOT be
        // present because the delta had nothing to contribute.
        let data_lines: Vec<&str> = out.lines().filter(|l| l.starts_with("data: ")).collect();
        assert_eq!(data_lines.len(), 1);
        assert!(!out.contains("x_coalesce"), "got: {}", out);
    }

    #[tokio::test]
    async fn merge_deltas_concatenates_blocks_and_keeps_last_finish() {
        let a = CanonicalStreamDelta {
            blocks: vec![CanonicalBlock::Text { text: "a".into() }],
            finish_reason: None,
            usage: None,
        };
        let b = CanonicalStreamDelta {
            blocks: vec![CanonicalBlock::Text { text: "b".into() }],
            finish_reason: Some(coalesce_core::rosetta::FinishReason::Stop),
            usage: None,
        };
        let m = merge_deltas(vec![a, b]);
        assert_eq!(m.blocks.len(), 2);
        assert_eq!(
            m.finish_reason,
            Some(coalesce_core::rosetta::FinishReason::Stop)
        );
    }
}
