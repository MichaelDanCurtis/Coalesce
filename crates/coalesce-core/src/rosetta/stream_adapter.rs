//! Provider stream adapter trait — the parse-side interface used by the proxy
//! to turn raw provider SSE payloads into canonical deltas.

use super::response::CanonicalStreamDelta;

/// Parses a single provider-shaped SSE data line (already stripped of the
/// `data: ` prefix and `[DONE]` sentinels) into zero or one canonical deltas.
///
/// * `Ok(Some(delta))` — successfully parsed.
/// * `Ok(None)` — chunk was a heartbeat / ping / non-emitting event.
/// * `Err(_)` — malformed chunk; caller should log + skip.
///
/// Adapters are expected to be stateless (or to hold only cheaply-clonable
/// state); per-stream state (e.g. tool-call id tracking for OpenAI) should
/// live in the caller that drives the adapter.
pub trait ProviderStreamAdapter: Send + Sync {
    fn parse_chunk(&self, raw_json: &str) -> Result<Option<CanonicalStreamDelta>, ParseError>;
}

/// Errors raised by stream adapters when a chunk cannot be parsed.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unexpected chunk shape: {0}")]
    Shape(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_from_serde_json() {
        let err: ParseError = serde_json::from_str::<serde_json::Value>("not json")
            .unwrap_err()
            .into();
        assert!(matches!(err, ParseError::Json(_)));
        assert!(err.to_string().starts_with("invalid JSON"));
    }

    #[test]
    fn parse_error_shape_display() {
        let err = ParseError::Shape("missing choices".into());
        assert_eq!(err.to_string(), "unexpected chunk shape: missing choices");
    }
}
