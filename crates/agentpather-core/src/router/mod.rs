pub mod config;
pub mod keywords;
pub mod scorer;
pub mod selector;
pub mod types;

use crate::types::ChatRequest;
use types::ScoringResult;

/// Route a chat request: score across 15 dimensions, determine tier
pub fn route(request: &ChatRequest, config: &config::RoutingConfig) -> ScoringResult {
    // Extract the last user message for scoring
    let text = extract_scoring_text(request);
    scorer::score(&text, config)
}

/// Extract text to score from the request.
/// Uses last user message, falling back to concatenation of recent messages.
fn extract_scoring_text(request: &ChatRequest) -> String {
    // Find the last user message
    for msg in request.messages.iter().rev() {
        if msg.role == "user" {
            if let Some(content) = &msg.content {
                if let Some(text) = content.as_text() {
                    return text.to_string();
                }
            }
        }
    }

    // Fallback: concatenate last 3 messages
    let mut parts = Vec::new();
    for msg in request.messages.iter().rev().take(3) {
        if let Some(content) = &msg.content {
            if let Some(text) = content.as_text() {
                parts.push(text.to_string());
            }
        }
    }
    parts.reverse();
    parts.join("\n")
}
