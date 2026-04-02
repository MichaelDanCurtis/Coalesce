use super::{ByteStream, Provider};
use crate::error::{AgentPatherError, Result};
use crate::types::{ChatRequest, MessageContent, ModelInfo, QualityTier};
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }
}

// --- Request translation: OpenAI -> Anthropic ---

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicBlock>),
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum AnthropicBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        source: ImageSource,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

fn translate_request(request: &ChatRequest) -> AnthropicRequest {
    let mut system_text = String::new();
    let mut messages = Vec::new();

    for msg in &request.messages {
        // Extract system messages into the system field
        if msg.role == "system" {
            if let Some(ref content) = msg.content {
                let text = match content {
                    MessageContent::Text(t) => t.clone(),
                    MessageContent::Parts(parts) => parts
                        .iter()
                        .filter_map(|p| match p {
                            crate::types::ContentPart::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                if !system_text.is_empty() {
                    system_text.push('\n');
                }
                system_text.push_str(&text);
            }
            continue;
        }

        // Handle tool_call_id messages -> tool_result
        if msg.role == "tool" {
            if let Some(ref tool_call_id) = msg.tool_call_id {
                let text = msg
                    .content
                    .as_ref()
                    .map(|c| match c {
                        MessageContent::Text(t) => t.clone(),
                        MessageContent::Parts(parts) => parts
                            .iter()
                            .filter_map(|p| match p {
                                crate::types::ContentPart::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    })
                    .unwrap_or_default();

                messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(vec![AnthropicBlock::ToolResult {
                        tool_use_id: tool_call_id.clone(),
                        content: text,
                    }]),
                });
                continue;
            }
        }

        // Handle assistant messages with tool_calls
        if msg.role == "assistant" {
            if let Some(ref tool_calls) = msg.tool_calls {
                let mut blocks: Vec<AnthropicBlock> = Vec::new();

                // Add text content if present
                if let Some(ref content) = msg.content {
                    let text = match content {
                        MessageContent::Text(t) => t.clone(),
                        MessageContent::Parts(_) => String::new(),
                    };
                    if !text.is_empty() {
                        blocks.push(AnthropicBlock::Text { text });
                    }
                }

                // Add tool_use blocks
                for tc in tool_calls {
                    if let (Some(id), Some(func)) = (
                        tc.get("id").and_then(|v| v.as_str()),
                        tc.get("function"),
                    ) {
                        let name = func
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let input: serde_json::Value = func
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        blocks.push(AnthropicBlock::ToolUse {
                            id: id.to_string(),
                            name,
                            input,
                        });
                    }
                }

                messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: AnthropicContent::Blocks(blocks),
                });
                continue;
            }
        }

        // Regular user/assistant messages
        let role = if msg.role == "user" || msg.role == "assistant" {
            msg.role.clone()
        } else {
            "user".to_string()
        };

        let content = msg
            .content
            .as_ref()
            .map(|c| match c {
                MessageContent::Text(t) => AnthropicContent::Text(t.clone()),
                MessageContent::Parts(parts) => {
                    let blocks: Vec<AnthropicBlock> = parts
                        .iter()
                        .map(|p| match p {
                            crate::types::ContentPart::Text { text } => {
                                AnthropicBlock::Text { text: text.clone() }
                            }
                            crate::types::ContentPart::ImageUrl { image_url } => {
                                // Try to extract base64 data URL
                                if image_url.url.starts_with("data:") {
                                    let parts: Vec<&str> =
                                        image_url.url.splitn(2, ',').collect();
                                    let media_type = parts
                                        .first()
                                        .unwrap_or(&"")
                                        .trim_start_matches("data:")
                                        .trim_end_matches(";base64")
                                        .to_string();
                                    let data =
                                        parts.get(1).unwrap_or(&"").to_string();
                                    AnthropicBlock::Image {
                                        source: ImageSource {
                                            source_type: "base64".to_string(),
                                            media_type,
                                            data,
                                        },
                                    }
                                } else {
                                    // URL-based image - Anthropic requires base64, fallback to text
                                    AnthropicBlock::Text {
                                        text: format!("[Image: {}]", image_url.url),
                                    }
                                }
                            }
                        })
                        .collect();
                    AnthropicContent::Blocks(blocks)
                }
            })
            .unwrap_or_else(|| AnthropicContent::Text(String::new()));

        messages.push(AnthropicMessage { role, content });
    }

    // Translate tools to Anthropic format
    let tools = request.tools.as_ref().map(|tools| {
        tools
            .iter()
            .filter_map(|t| {
                let func = t.get("function")?;
                Some(serde_json::json!({
                    "name": func.get("name")?,
                    "description": func.get("description").unwrap_or(&serde_json::Value::Null),
                    "input_schema": func.get("parameters").unwrap_or(&serde_json::json!({"type": "object"})),
                }))
            })
            .collect()
    });

    AnthropicRequest {
        model: request.model.clone(),
        max_tokens: request.max_tokens.unwrap_or(4096),
        system: if system_text.is_empty() {
            None
        } else {
            Some(system_text)
        },
        messages,
        stream: if request.stream { Some(true) } else { None },
        temperature: request.temperature,
        top_p: request.top_p,
        tools,
    }
}

// --- Response translation: Anthropic -> OpenAI ---

#[derive(Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

fn translate_response(resp: AnthropicResponse) -> serde_json::Value {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in &resp.content {
        match block.block_type.as_str() {
            "text" => {
                if let Some(ref text) = block.text {
                    text_parts.push(text.clone());
                }
            }
            "tool_use" => {
                if let (Some(id), Some(name), Some(input)) =
                    (&block.id, &block.name, &block.input)
                {
                    tool_calls.push(serde_json::json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": serde_json::to_string(input).unwrap_or_default(),
                        }
                    }));
                }
            }
            _ => {}
        }
    }

    let finish_reason = match resp.stop_reason.as_deref() {
        Some("end_turn") => "stop",
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        Some("stop_sequence") => "stop",
        _ => "stop",
    };

    let content_text = text_parts.join("");

    let mut message = serde_json::json!({
        "role": "assistant",
        "content": if content_text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(content_text) },
    });

    if !tool_calls.is_empty() {
        message
            .as_object_mut()
            .unwrap()
            .insert("tool_calls".to_string(), serde_json::json!(tool_calls));
    }

    let mut result = serde_json::json!({
        "id": format!("chatcmpl-{}", resp.id),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": resp.model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
    });

    if let Some(usage) = resp.usage {
        result.as_object_mut().unwrap().insert(
            "usage".to_string(),
            serde_json::json!({
                "prompt_tokens": usage.input_tokens,
                "completion_tokens": usage.output_tokens,
                "total_tokens": usage.input_tokens + usage.output_tokens,
            }),
        );
    }

    result
}

// --- Streaming translation ---

/// Translate Anthropic SSE stream to OpenAI-compatible SSE stream
fn translate_stream(input: impl futures::Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send + 'static) -> ByteStream {
    let stream = input.map(|result| {
        result.map(|bytes| {
            let text = String::from_utf8_lossy(&bytes);
            let mut output = String::new();

            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        output.push_str("data: [DONE]\n\n");
                        continue;
                    }

                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        let event_type = event
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        match event_type {
                            "content_block_delta" => {
                                let delta = event.get("delta").unwrap_or(&serde_json::Value::Null);
                                let delta_type = delta
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");

                                match delta_type {
                                    "text_delta" => {
                                        if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                            let chunk = serde_json::json!({
                                                "id": "chatcmpl-stream",
                                                "object": "chat.completion.chunk",
                                                "created": chrono::Utc::now().timestamp(),
                                                "model": "",
                                                "choices": [{
                                                    "index": 0,
                                                    "delta": { "content": text },
                                                    "finish_reason": null,
                                                }]
                                            });
                                            output.push_str(&format!(
                                                "data: {}\n\n",
                                                serde_json::to_string(&chunk).unwrap_or_default()
                                            ));
                                        }
                                    }
                                    "input_json_delta" => {
                                        // Tool call argument streaming - pass through as content for now
                                        if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                                            let chunk = serde_json::json!({
                                                "id": "chatcmpl-stream",
                                                "object": "chat.completion.chunk",
                                                "created": chrono::Utc::now().timestamp(),
                                                "model": "",
                                                "choices": [{
                                                    "index": 0,
                                                    "delta": { "content": partial },
                                                    "finish_reason": null,
                                                }]
                                            });
                                            output.push_str(&format!(
                                                "data: {}\n\n",
                                                serde_json::to_string(&chunk).unwrap_or_default()
                                            ));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            "message_stop" => {
                                let chunk = serde_json::json!({
                                    "id": "chatcmpl-stream",
                                    "object": "chat.completion.chunk",
                                    "created": chrono::Utc::now().timestamp(),
                                    "model": "",
                                    "choices": [{
                                        "index": 0,
                                        "delta": {},
                                        "finish_reason": "stop",
                                    }]
                                });
                                output.push_str(&format!(
                                    "data: {}\n\n",
                                    serde_json::to_string(&chunk).unwrap_or_default()
                                ));
                                output.push_str("data: [DONE]\n\n");
                            }
                            "message_delta" => {
                                // Contains stop_reason and usage
                                let stop = event
                                    .get("delta")
                                    .and_then(|d| d.get("stop_reason"))
                                    .and_then(|v| v.as_str());
                                if stop.is_some() {
                                    let finish = match stop {
                                        Some("end_turn") => "stop",
                                        Some("max_tokens") => "length",
                                        Some("tool_use") => "tool_calls",
                                        _ => "stop",
                                    };
                                    let chunk = serde_json::json!({
                                        "id": "chatcmpl-stream",
                                        "object": "chat.completion.chunk",
                                        "created": chrono::Utc::now().timestamp(),
                                        "model": "",
                                        "choices": [{
                                            "index": 0,
                                            "delta": {},
                                            "finish_reason": finish,
                                        }]
                                    });
                                    output.push_str(&format!(
                                        "data: {}\n\n",
                                        serde_json::to_string(&chunk).unwrap_or_default()
                                    ));
                                }
                            }
                            _ => {} // message_start, content_block_start, content_block_stop, ping
                        }
                    }
                }
            }

            if output.is_empty() {
                Bytes::new()
            } else {
                Bytes::from(output)
            }
        })
    });

    Box::pin(stream)
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo {
                id: "claude-sonnet-4-20250514".into(),
                name: "Claude Sonnet 4".into(),
                provider: "anthropic".into(),
                input_price_per_m: 3.0,
                output_price_per_m: 15.0,
                context_window: 200000,
                max_output: Some(16000),
                quality_tier: QualityTier::Complex,
                reasoning: false,
                vision: true,
                tool_calling: true,
            },
            ModelInfo {
                id: "claude-haiku-3-5-20241022".into(),
                name: "Claude 3.5 Haiku".into(),
                provider: "anthropic".into(),
                input_price_per_m: 0.80,
                output_price_per_m: 4.0,
                context_window: 200000,
                max_output: Some(8192),
                quality_tier: QualityTier::Medium,
                reasoning: false,
                vision: true,
                tool_calling: true,
            },
            ModelInfo {
                id: "claude-opus-4-20250514".into(),
                name: "Claude Opus 4".into(),
                provider: "anthropic".into(),
                input_price_per_m: 15.0,
                output_price_per_m: 75.0,
                context_window: 200000,
                max_output: Some(32000),
                quality_tier: QualityTier::Reasoning,
                reasoning: true,
                vision: true,
                tool_calling: true,
            },
        ])
    }

    async fn chat(&self, request: &ChatRequest) -> Result<serde_json::Value> {
        let anthropic_req = translate_request(request);

        let resp = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&anthropic_req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentPatherError::Provider {
                provider: "anthropic".into(),
                message: format!("Chat failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        let anthropic_resp: AnthropicResponse = resp.json().await.map_err(|e| {
            AgentPatherError::Provider {
                provider: "anthropic".into(),
                message: format!("Failed to parse response: {}", e),
                status: None,
            }
        })?;

        Ok(translate_response(anthropic_resp))
    }

    async fn chat_stream(&self, request: &ChatRequest) -> Result<ByteStream> {
        let mut anthropic_req = translate_request(request);
        anthropic_req.stream = Some(true);

        let resp = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&anthropic_req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentPatherError::Provider {
                provider: "anthropic".into(),
                message: format!("Stream failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        Ok(translate_stream(resp.bytes_stream()))
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(!self.api_key.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatRequest, Message, MessageContent};
    use std::collections::HashMap;

    #[test]
    fn test_translate_request_basic() {
        let request = ChatRequest {
            model: "claude-sonnet-4-20250514".into(),
            messages: vec![
                Message {
                    role: "system".into(),
                    content: Some(MessageContent::Text("You are helpful.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: HashMap::new(),
                },
                Message {
                    role: "user".into(),
                    content: Some(MessageContent::Text("Hello".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: HashMap::new(),
                },
            ],
            stream: false,
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            extra: HashMap::new(),
        };

        let translated = translate_request(&request);
        assert_eq!(translated.system.as_deref(), Some("You are helpful."));
        assert_eq!(translated.messages.len(), 1);
        assert_eq!(translated.messages[0].role, "user");
        assert_eq!(translated.max_tokens, 1024);
        assert_eq!(translated.temperature, Some(0.7));
    }

    #[test]
    fn test_translate_response_text() {
        let resp = AnthropicResponse {
            id: "msg_123".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![ContentBlock {
                block_type: "text".into(),
                text: Some("Hello! How can I help?".into()),
                id: None,
                name: None,
                input: None,
            }],
            stop_reason: Some("end_turn".into()),
            usage: Some(AnthropicUsage {
                input_tokens: 10,
                output_tokens: 8,
            }),
        };

        let result = translate_response(resp);
        assert_eq!(result["choices"][0]["finish_reason"], "stop");
        assert_eq!(
            result["choices"][0]["message"]["content"],
            "Hello! How can I help?"
        );
        assert_eq!(result["usage"]["prompt_tokens"], 10);
        assert_eq!(result["usage"]["completion_tokens"], 8);
    }

    #[test]
    fn test_translate_response_tool_use() {
        let resp = AnthropicResponse {
            id: "msg_456".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![
                ContentBlock {
                    block_type: "text".into(),
                    text: Some("Let me search for that.".into()),
                    id: None,
                    name: None,
                    input: None,
                },
                ContentBlock {
                    block_type: "tool_use".into(),
                    text: None,
                    id: Some("toolu_1".into()),
                    name: Some("search".into()),
                    input: Some(serde_json::json!({"query": "rust async"})),
                },
            ],
            stop_reason: Some("tool_use".into()),
            usage: Some(AnthropicUsage {
                input_tokens: 20,
                output_tokens: 15,
            }),
        };

        let result = translate_response(resp);
        assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
        let tool_calls = &result["choices"][0]["message"]["tool_calls"];
        assert_eq!(tool_calls[0]["id"], "toolu_1");
        assert_eq!(tool_calls[0]["function"]["name"], "search");
    }

    #[test]
    fn test_known_models() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let provider = AnthropicProvider::new("test-key".into());
            let models = provider.list_models().await.unwrap();
            assert_eq!(models.len(), 3);
            assert!(models.iter().all(|m| m.provider == "anthropic"));
            assert!(models.iter().any(|m| m.reasoning)); // opus
            assert!(models.iter().all(|m| m.vision));
        });
    }
}
