//! Google Cloud Code Assist provider.
//!
//! Uses the internal Cloud Code API (`cloudcode-pa.googleapis.com/v1internal`)
//! with OAuth tokens from Antigravity/Google Cloud Code IDE extension.
//! This is NOT the public Gemini API — it uses a proprietary wrapper format.

use super::{ByteStream, Provider};
use crate::error::{CoalesceError, Result};
use crate::types::{ChatRequest, MessageContent, ContentPart};
use async_trait::async_trait;
use reqwest::Client;

const CLOUDCODE_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";

pub struct GoogleCloudCodeProvider {
    client: Client,
    token: String,
    project_id: String,
}

impl GoogleCloudCodeProvider {
    pub fn new(token: String, project_id: String) -> Self {
        Self {
            client: Client::new(),
            token,
            project_id,
        }
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Authorization", format!("Bearer {}", self.token).parse().unwrap());
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("X-Client-Name", "antigravity".parse().unwrap());
        headers.insert("X-Client-Version", "1.107.0".parse().unwrap());
        headers.insert("x-goog-api-client", "gl-node/18.18.2 fire/0.8.6 grpc/1.10.x".parse().unwrap());
        headers
    }

    /// Convert OpenAI-format messages to Google Generative AI contents format
    fn convert_messages(request: &ChatRequest) -> (Vec<serde_json::Value>, Option<serde_json::Value>) {
        let mut contents = Vec::new();
        let mut system_instruction = None;

        for msg in &request.messages {
            let role = match msg.role.as_str() {
                "assistant" => "model",
                "system" => {
                    // System messages go to systemInstruction
                    let text = match &msg.content {
                        Some(MessageContent::Text(t)) => t.clone(),
                        Some(MessageContent::Parts(parts)) => {
                            parts.iter().filter_map(|p| match p {
                                ContentPart::Text { text } => Some(text.as_str()),
                                _ => None,
                            }).collect::<Vec<_>>().join("\n")
                        }
                        None => String::new(),
                    };
                    if !text.is_empty() {
                        system_instruction = Some(serde_json::json!({
                            "role": "user",
                            "parts": [{"text": text}]
                        }));
                    }
                    continue;
                }
                _ => "user",
            };

            let parts = match &msg.content {
                Some(MessageContent::Text(t)) => vec![serde_json::json!({"text": t})],
                Some(MessageContent::Parts(parts)) => {
                    parts.iter().map(|p| match p {
                        ContentPart::Text { text } => serde_json::json!({"text": text}),
                        ContentPart::ImageUrl { image_url } => {
                            // Extract base64 data from data URI
                            if let Some(data) = image_url.url.strip_prefix("data:") {
                                if let Some((mime, b64)) = data.split_once(";base64,") {
                                    return serde_json::json!({
                                        "inlineData": {
                                            "mimeType": mime,
                                            "data": b64
                                        }
                                    });
                                }
                            }
                            serde_json::json!({"text": "[image]"})
                        }
                    }).collect()
                }
                None => vec![serde_json::json!({"text": ""})],
            };

            contents.push(serde_json::json!({
                "role": role,
                "parts": parts
            }));
        }

        (contents, system_instruction)
    }

    /// Check if model is a "thinking" model
    fn is_thinking_model(model: &str) -> bool {
        model.contains("thinking")
    }

    /// Build the Cloud Code wrapped request
    fn build_request(request: &ChatRequest) -> serde_json::Value {
        let (contents, system_instruction) = Self::convert_messages(request);
        let is_thinking = Self::is_thinking_model(&request.model);

        let mut generation_config = serde_json::json!({});
        if let Some(max_tokens) = request.max_tokens {
            generation_config["maxOutputTokens"] = serde_json::json!(max_tokens);
        }
        if let Some(temp) = request.temperature {
            generation_config["temperature"] = serde_json::json!(temp);
        }
        if let Some(top_p) = request.top_p {
            generation_config["topP"] = serde_json::json!(top_p);
        }

        let mut inner_request = serde_json::json!({
            "contents": contents,
            "generationConfig": generation_config,
        });

        if let Some(si) = system_instruction {
            inner_request["systemInstruction"] = si;
        }

        if is_thinking {
            inner_request["generationConfig"]["thinkingConfig"] = serde_json::json!({
                "includeThoughts": true,
                "thinkingBudget": 32000
            });
        }

        // Convert tools if present
        if let Some(tools) = &request.tools {
            let function_declarations: Vec<serde_json::Value> = tools.iter().map(|t| {
                let mut decl = serde_json::json!({
                    "name": t.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("unknown"),
                    "description": t.get("function").and_then(|f| f.get("description")).and_then(|d| d.as_str()).unwrap_or(""),
                });
                if let Some(params) = t.get("function").and_then(|f| f.get("parameters")) {
                    decl["parameters"] = params.clone();
                }
                decl
            }).collect();
            if !function_declarations.is_empty() {
                inner_request["tools"] = serde_json::json!([{
                    "functionDeclarations": function_declarations
                }]);
            }
        }

        serde_json::json!({
            "project": "", // filled in by caller
            "model": request.model,
            "request": inner_request,
            "userAgent": "antigravity",
            "requestType": "agent",
            "requestId": format!("agent-{}", uuid::Uuid::new_v4()),
        })
    }

    /// Parse Google Generative AI response to OpenAI format
    fn parse_response(model: &str, body: &serde_json::Value) -> serde_json::Value {
        let candidates = body.get("candidates").and_then(|c| c.as_array());
        let mut content = String::new();
        let mut reasoning_content = String::new();

        if let Some(candidates) = candidates {
            if let Some(candidate) = candidates.first() {
                if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                    for part in parts {
                        if part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false) {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                reasoning_content.push_str(text);
                            }
                        } else if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            content.push_str(text);
                        }
                    }
                }
            }
        }

        let usage_metadata = body.get("usageMetadata");
        let prompt_tokens = usage_metadata.and_then(|u| u.get("promptTokenCount")).and_then(|v| v.as_u64()).unwrap_or(0);
        let completion_tokens = usage_metadata.and_then(|u| u.get("candidatesTokenCount")).and_then(|v| v.as_u64()).unwrap_or(0);
        let total_tokens = usage_metadata.and_then(|u| u.get("totalTokenCount")).and_then(|v| v.as_u64()).unwrap_or(prompt_tokens + completion_tokens);

        let mut message = serde_json::json!({
            "role": "assistant",
            "content": content,
        });
        if !reasoning_content.is_empty() {
            message["reasoning_content"] = serde_json::json!(reasoning_content);
        }

        serde_json::json!({
            "id": format!("cloudcode-{}", uuid::Uuid::new_v4()),
            "object": "chat.completion",
            "model": model,
            "choices": [{
                "index": 0,
                "message": message,
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": total_tokens,
            }
        })
    }

    /// Parse SSE streaming response from Cloud Code into OpenAI SSE chunks
    fn parse_sse_chunk(model: &str, data: &str) -> Option<String> {
        let body: serde_json::Value = serde_json::from_str(data).ok()?;
        let candidates = body.get("candidates")?.as_array()?;
        let candidate = candidates.first()?;
        let parts = candidate.get("content")?.get("parts")?.as_array()?;

        let mut chunks = Vec::new();
        for part in parts {
            let is_thought = part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                let mut delta = serde_json::json!({"role": "assistant"});
                if is_thought {
                    delta["reasoning_content"] = serde_json::json!(text);
                } else {
                    delta["content"] = serde_json::json!(text);
                }
                let chunk = serde_json::json!({
                    "id": format!("cloudcode-{}", uuid::Uuid::new_v4()),
                    "object": "chat.completion.chunk",
                    "model": model,
                    "choices": [{"index": 0, "delta": delta}]
                });
                chunks.push(format!("data: {}\n\n", chunk));
            }
        }

        if chunks.is_empty() {
            // Check for usage metadata in final chunk
            if let Some(usage) = body.get("usageMetadata") {
                let chunk = serde_json::json!({
                    "id": format!("cloudcode-{}", uuid::Uuid::new_v4()),
                    "object": "chat.completion.chunk",
                    "model": model,
                    "choices": [{"index": 0, "delta": {}}],
                    "usage": {
                        "prompt_tokens": usage.get("promptTokenCount").and_then(|v| v.as_u64()).unwrap_or(0),
                        "completion_tokens": usage.get("candidatesTokenCount").and_then(|v| v.as_u64()).unwrap_or(0),
                        "total_tokens": usage.get("totalTokenCount").and_then(|v| v.as_u64()).unwrap_or(0),
                    }
                });
                return Some(format!("data: {}\n\n", chunk));
            }
            None
        } else {
            Some(chunks.join(""))
        }
    }
}

#[async_trait]
impl Provider for GoogleCloudCodeProvider {
    fn name(&self) -> &str {
        "google"
    }

    async fn list_models(&self) -> Result<Vec<crate::types::ModelInfo>> {
        // Models are discovered via loadCodeAssist in the proxy init, not here
        Ok(vec![])
    }

    async fn chat(&self, request: &ChatRequest) -> Result<serde_json::Value> {
        let mut wrapped = Self::build_request(request);
        wrapped["project"] = serde_json::json!(self.project_id);

        let url = format!("{}/v1internal:generateContent", CLOUDCODE_ENDPOINT);

        let resp = self.client
            .post(&url)
            .headers(self.headers())
            .json(&wrapped)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Provider {
                provider: "google".into(),
                message: format!("Chat failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        let body: serde_json::Value = resp.json().await?;
        Ok(Self::parse_response(&request.model, &body))
    }

    async fn chat_stream(&self, request: &ChatRequest) -> Result<ByteStream> {
        let mut wrapped = Self::build_request(request);
        wrapped["project"] = serde_json::json!(self.project_id);

        let url = format!("{}/v1internal:streamGenerateContent?alt=sse", CLOUDCODE_ENDPOINT);

        let mut headers = self.headers();
        headers.insert("Accept", "text/event-stream".parse().unwrap());

        if Self::is_thinking_model(&request.model) {
            headers.insert("anthropic-beta", "interleaved-thinking-2025-05-14".parse().unwrap());
        }

        let resp = self.client
            .post(&url)
            .headers(headers)
            .json(&wrapped)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Provider {
                provider: "google".into(),
                message: format!("Stream failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        let model = request.model.clone();
        let stream = resp.bytes_stream();

        // Transform Google SSE to OpenAI SSE format
        use futures::StreamExt;
        let transformed = stream.map(move |result| {
            match result {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let mut output = String::new();
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Some(chunk) = Self::parse_sse_chunk(&model, data) {
                                output.push_str(&chunk);
                            }
                        }
                    }
                    if output.is_empty() {
                        Ok(bytes::Bytes::new())
                    } else {
                        Ok(bytes::Bytes::from(output))
                    }
                }
                Err(e) => Err(e),
            }
        });

        Ok(Box::pin(transformed))
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(!self.token.is_empty() && !self.project_id.is_empty())
    }
}
