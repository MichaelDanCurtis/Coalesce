//! Canonical thinking/reasoning representation and streaming think-split parser.
//!
//! Normalizes thinking output across providers:
//! - Anthropic: native `thinking` content blocks
//! - DeepSeek/Qwen/Ollama: `<think>...</think>` tags in content
//! - Kimi/Moonshot: `reasoning_content` field on message
//! - OpenAI o-series: hidden reasoning, token count only

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Canonical Thinking Types
// ---------------------------------------------------------------------------

/// Normalized thinking/reasoning content from any provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CanonicalThinking {
    /// Thinking content blocks (may be multiple if interleaved with tool use).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<ThinkingBlock>,

    /// Total reasoning tokens consumed (even if content is hidden, like OpenAI o-series).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
}

/// A single block of thinking content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBlock {
    /// The thinking text.
    pub text: String,

    /// Optional signature for Anthropic's extended thinking verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl CanonicalThinking {
    /// Whether any thinking content exists.
    pub fn has_content(&self) -> bool {
        !self.blocks.is_empty()
    }

    /// Get all thinking text concatenated.
    pub fn full_text(&self) -> String {
        self.blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Extract thinking from an Anthropic Messages response.
    /// Looks for content blocks with `type: "thinking"`.
    pub fn from_anthropic_response(content: &[serde_json::Value]) -> Self {
        let mut blocks = Vec::new();
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                if let Some(text) = block.get("thinking").and_then(|t| t.as_str()) {
                    blocks.push(ThinkingBlock {
                        text: text.to_string(),
                        signature: block
                            .get("signature")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string()),
                    });
                }
            }
        }
        Self {
            blocks,
            reasoning_tokens: None,
        }
    }

    /// Extract thinking from a Kimi/Moonshot response.
    /// Looks for `reasoning_content` field on the message.
    pub fn from_kimi_response(message: &serde_json::Value) -> Self {
        let mut blocks = Vec::new();
        if let Some(reasoning) = message.get("reasoning_content").and_then(|r| r.as_str()) {
            if !reasoning.is_empty() {
                blocks.push(ThinkingBlock {
                    text: reasoning.to_string(),
                    signature: None,
                });
            }
        }
        Self {
            blocks,
            reasoning_tokens: None,
        }
    }

    /// Convert to Anthropic thinking content blocks for egress.
    pub fn to_anthropic_blocks(&self) -> Vec<serde_json::Value> {
        self.blocks
            .iter()
            .map(|b| {
                let mut block = serde_json::json!({
                    "type": "thinking",
                    "thinking": b.text,
                });
                if let Some(sig) = &b.signature {
                    block["signature"] = serde_json::json!(sig);
                }
                block
            })
            .collect()
    }

    /// Merge thinking from OpenAI usage stats (hidden reasoning).
    pub fn with_reasoning_tokens(mut self, tokens: u32) -> Self {
        self.reasoning_tokens = Some(tokens);
        self
    }
}

// ---------------------------------------------------------------------------
// Streaming Think-Split Parser
// ---------------------------------------------------------------------------

/// Stateful parser that extracts `<think>...</think>` tags from streaming content.
///
/// Handles partial tags across SSE chunks:
/// - Buffers content inside `<think>` tags
/// - Emits regular content outside tags
/// - Handles malformed/unclosed tags gracefully
#[derive(Debug, Clone)]
pub struct ThinkingSplitParser {
    /// Current parser state.
    state: ParseState,
    /// Buffer for thinking content being accumulated.
    thinking_buffer: String,
    /// Buffer for partial tag detection (e.g., received "<thi" waiting for "nk>").
    tag_buffer: String,
    /// Completed thinking blocks.
    completed_blocks: Vec<ThinkingBlock>,
}

#[derive(Debug, Clone, PartialEq)]
enum ParseState {
    /// Outside any think tag — content is regular output.
    Normal,
    /// Inside `<think>` — content is thinking, buffer it.
    InThinking,
}

/// Result of processing a chunk through the think-split parser.
#[derive(Debug, Clone)]
pub struct SplitResult {
    /// Regular content to pass through to the client.
    pub content: String,
    /// Thinking content completed in this chunk (may be empty).
    pub thinking_completed: Vec<ThinkingBlock>,
    /// Whether we're currently inside a thinking block (for streaming state).
    pub in_thinking: bool,
}

impl ThinkingSplitParser {
    pub fn new() -> Self {
        Self {
            state: ParseState::Normal,
            thinking_buffer: String::new(),
            tag_buffer: String::new(),
            completed_blocks: Vec::new(),
        }
    }

    /// Process a chunk of streaming content.
    /// Returns the split result with regular content and any completed thinking blocks.
    pub fn process_chunk(&mut self, chunk: &str) -> SplitResult {
        let mut content = String::new();
        let mut newly_completed = Vec::new();

        // Prepend any partial tag buffer from previous chunk
        let input = if self.tag_buffer.is_empty() {
            chunk.to_string()
        } else {
            let combined = format!("{}{}", self.tag_buffer, chunk);
            self.tag_buffer.clear();
            combined
        };

        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match self.state {
                ParseState::Normal => {
                    if c == '<' {
                        // Could be start of <think> tag — accumulate
                        let mut potential_tag = String::from('<');
                        let mut buffered = false;

                        for expected in "think>".chars() {
                            match chars.peek() {
                                Some(&next) if next == expected => {
                                    potential_tag.push(chars.next().unwrap());
                                }
                                Some(_) => {
                                    // Not a <think> tag, emit as regular content
                                    content.push_str(&potential_tag);
                                    break;
                                }
                                None => {
                                    // Ran out of input — buffer for next chunk
                                    self.tag_buffer = potential_tag.clone();
                                    buffered = true;
                                    break;
                                }
                            }
                        }
                        if !buffered && potential_tag == "<think>" {
                            self.state = ParseState::InThinking;
                            self.thinking_buffer.clear();
                        }
                    } else {
                        content.push(c);
                    }
                }
                ParseState::InThinking => {
                    if c == '<' {
                        // Could be </think> closing tag
                        let mut potential_tag = String::from('<');
                        let mut buffered = false;

                        for expected in "/think>".chars() {
                            match chars.peek() {
                                Some(&next) if next == expected => {
                                    potential_tag.push(chars.next().unwrap());
                                }
                                Some(_) => {
                                    // Not a </think> tag — it's part of thinking content
                                    self.thinking_buffer.push_str(&potential_tag);
                                    break;
                                }
                                None => {
                                    // Ran out of input mid-tag
                                    self.tag_buffer = potential_tag.clone();
                                    buffered = true;
                                    break;
                                }
                            }
                        }
                        if !buffered && potential_tag == "</think>" {
                            let block = ThinkingBlock {
                                text: self.thinking_buffer.drain(..).collect(),
                                signature: None,
                            };
                            newly_completed.push(block.clone());
                            self.completed_blocks.push(block);
                            self.state = ParseState::Normal;
                        }
                    } else {
                        self.thinking_buffer.push(c);
                    }
                }
            }
        }

        SplitResult {
            content,
            thinking_completed: newly_completed,
            in_thinking: self.state == ParseState::InThinking,
        }
    }

    /// Finalize the parser. If there's an unclosed thinking block, return it.
    pub fn finalize(mut self) -> (Vec<ThinkingBlock>, Option<String>) {
        let remainder = if !self.tag_buffer.is_empty() {
            Some(self.tag_buffer)
        } else if self.state == ParseState::InThinking && !self.thinking_buffer.is_empty() {
            // Unclosed <think> — treat accumulated content as thinking
            self.completed_blocks.push(ThinkingBlock {
                text: self.thinking_buffer,
                signature: None,
            });
            None
        } else {
            None
        };
        (self.completed_blocks, remainder)
    }

    /// Get all completed thinking blocks so far.
    pub fn completed_blocks(&self) -> &[ThinkingBlock] {
        &self.completed_blocks
    }
}

impl Default for ThinkingSplitParser {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_think_split() {
        let mut parser = ThinkingSplitParser::new();
        let result = parser.process_chunk("<think>Let me reason about this</think>The answer is 42.");

        assert_eq!(result.content, "The answer is 42.");
        assert_eq!(result.thinking_completed.len(), 1);
        assert_eq!(
            result.thinking_completed[0].text,
            "Let me reason about this"
        );
        assert!(!result.in_thinking);
    }

    #[test]
    fn test_streaming_chunks() {
        let mut parser = ThinkingSplitParser::new();

        // Chunk 1: start of thinking
        let r1 = parser.process_chunk("<think>I need to");
        assert_eq!(r1.content, "");
        assert!(r1.in_thinking);
        assert!(r1.thinking_completed.is_empty());

        // Chunk 2: more thinking
        let r2 = parser.process_chunk(" consider this carefully");
        assert_eq!(r2.content, "");
        assert!(r2.in_thinking);

        // Chunk 3: end of thinking + regular content
        let r3 = parser.process_chunk("</think>Here is my answer");
        assert_eq!(r3.content, "Here is my answer");
        assert_eq!(r3.thinking_completed.len(), 1);
        assert_eq!(
            r3.thinking_completed[0].text,
            "I need to consider this carefully"
        );
        assert!(!r3.in_thinking);
    }

    #[test]
    fn test_partial_tag_across_chunks() {
        let mut parser = ThinkingSplitParser::new();

        // Tag split across chunks: "<thi" | "nk>reasoning</think>answer"
        let r1 = parser.process_chunk("prefix<thi");
        assert_eq!(r1.content, "prefix");

        let r2 = parser.process_chunk("nk>reasoning</think>answer");
        assert_eq!(r2.content, "answer");
        assert_eq!(r2.thinking_completed.len(), 1);
        assert_eq!(r2.thinking_completed[0].text, "reasoning");
    }

    #[test]
    fn test_no_think_tags() {
        let mut parser = ThinkingSplitParser::new();
        let result = parser.process_chunk("Just regular content with <b>html</b> tags");
        assert_eq!(
            result.content,
            "Just regular content with <b>html</b> tags"
        );
        assert!(result.thinking_completed.is_empty());
        assert!(!result.in_thinking);
    }

    #[test]
    fn test_unclosed_thinking_finalize() {
        let mut parser = ThinkingSplitParser::new();
        parser.process_chunk("<think>reasoning without close");
        let (blocks, _) = parser.finalize();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "reasoning without close");
    }

    #[test]
    fn test_anthropic_thinking_extraction() {
        let content = vec![
            serde_json::json!({"type": "thinking", "thinking": "Let me reason...", "signature": "sig123"}),
            serde_json::json!({"type": "text", "text": "The answer is 42."}),
        ];
        let thinking = CanonicalThinking::from_anthropic_response(&content);
        assert!(thinking.has_content());
        assert_eq!(thinking.blocks.len(), 1);
        assert_eq!(thinking.blocks[0].text, "Let me reason...");
        assert_eq!(thinking.blocks[0].signature.as_deref(), Some("sig123"));
    }

    #[test]
    fn test_kimi_reasoning_extraction() {
        let message = serde_json::json!({
            "role": "assistant",
            "content": "The answer is 42.",
            "reasoning_content": "I need to think about this..."
        });
        let thinking = CanonicalThinking::from_kimi_response(&message);
        assert!(thinking.has_content());
        assert_eq!(thinking.blocks[0].text, "I need to think about this...");
    }

    #[test]
    fn test_thinking_to_anthropic_blocks() {
        let thinking = CanonicalThinking {
            blocks: vec![ThinkingBlock {
                text: "My reasoning".into(),
                signature: Some("sig456".into()),
            }],
            reasoning_tokens: Some(150),
        };
        let blocks = thinking.to_anthropic_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "My reasoning");
        assert_eq!(blocks[0]["signature"], "sig456");
    }

    #[test]
    fn test_multiple_think_blocks() {
        let mut parser = ThinkingSplitParser::new();
        let result = parser.process_chunk(
            "<think>first thought</think>middle text<think>second thought</think>final text",
        );
        assert_eq!(result.content, "middle textfinal text");
        assert_eq!(result.thinking_completed.len(), 2);
        assert_eq!(result.thinking_completed[0].text, "first thought");
        assert_eq!(result.thinking_completed[1].text, "second thought");
    }
}
