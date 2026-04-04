use crate::error::Result;
use crate::types::{ChatRequest, Message, MessageContent, ModelInfo, QualityTier};
use crate::providers::{Provider, ByteStream};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Mock provider for testing without burning real API credits.
/// Generates synthetic responses with configurable latency and behavior.
pub struct MockProvider {
    name: String,
    config: MockConfig,
    request_count: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct MockConfig {
    /// Simulated latency per response
    pub latency_ms: u64,
    /// Simulated tokens in response
    pub response_tokens: u32,
    /// Whether to simulate failures (every Nth request)
    pub fail_every_n: Option<u64>,
    /// Custom response prefix
    pub response_prefix: String,
    /// Model name to report
    pub model_name: String,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            latency_ms: 100,
            response_tokens: 50,
            fail_every_n: None,
            response_prefix: "This is a mock response from Coalesce test provider.".into(),
            model_name: "mock-model".into(),
        }
    }
}

impl MockProvider {
    pub fn new(name: impl Into<String>, config: MockConfig) -> Self {
        Self {
            name: name.into(),
            config,
            request_count: AtomicU64::new(0),
        }
    }

    pub fn default_provider() -> Self {
        Self::new("mock", MockConfig::default())
    }

    fn should_fail(&self) -> bool {
        if let Some(n) = self.config.fail_every_n {
            let count = self.request_count.load(Ordering::Relaxed);
            count > 0 && count % n == 0
        } else {
            false
        }
    }

    fn generate_response(&self, request: &ChatRequest) -> serde_json::Value {
        let user_msg = request.messages.last()
            .and_then(|m| m.content.as_ref())
            .and_then(|c| c.as_text())
            .unwrap_or("(empty)");

        let response_text = format!(
            "{}\n\nYou said: \"{}\"\n\n[Mock provider: {}, model: {}, request #{}]",
            self.config.response_prefix,
            &user_msg[..user_msg.len().min(200)],
            self.name,
            self.config.model_name,
            self.request_count.load(Ordering::Relaxed),
        );

        serde_json::json!({
            "id": format!("mock-{}", uuid_simple()),
            "object": "chat.completion",
            "created": chrono_timestamp(),
            "model": self.config.model_name,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": response_text,
                },
                "finish_reason": "stop",
            }],
            "usage": {
                "prompt_tokens": request.messages.len() as u32 * 10,
                "completion_tokens": self.config.response_tokens,
                "total_tokens": request.messages.len() as u32 * 10 + self.config.response_tokens,
            }
        })
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: self.config.model_name.clone(),
            name: format!("Mock Model ({})", self.name),
            provider: self.name.clone(),
            input_price_per_m: 0.0,
            output_price_per_m: 0.0,
            context_window: 128000,
            max_output: Some(4096),
            quality_tier: QualityTier::Simple,
            reasoning: false,
            vision: false,
            tool_calling: false,
            canonical_family: None,
            capabilities: None,
        }])
    }

    async fn chat(&self, request: &ChatRequest) -> Result<serde_json::Value> {
        self.request_count.fetch_add(1, Ordering::Relaxed);

        if self.config.latency_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.config.latency_ms)).await;
        }

        if self.should_fail() {
            return Err(crate::error::CoalesceError::Provider {
                provider: self.name.clone(),
                message: "Simulated failure from mock provider".into(),
                status: Some(500),
            });
        }

        Ok(self.generate_response(request))
    }

    async fn chat_stream(&self, request: &ChatRequest) -> Result<ByteStream> {
        self.request_count.fetch_add(1, Ordering::Relaxed);

        if self.config.latency_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.config.latency_ms / 2)).await;
        }

        if self.should_fail() {
            return Err(crate::error::CoalesceError::Provider {
                provider: self.name.clone(),
                message: "Simulated failure from mock provider".into(),
                status: Some(500),
            });
        }

        let resp = self.generate_response(request);
        let content = resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let model = self.config.model_name.clone();
        let usage = resp["usage"].clone();

        // Split content into chunks and emit as SSE
        let chunk_size = 20;
        let chunks: Vec<String> = content
            .chars()
            .collect::<Vec<_>>()
            .chunks(chunk_size)
            .map(|c| c.iter().collect::<String>())
            .collect();

        let id = format!("mock-{}", uuid_simple());
        let mut events: Vec<std::result::Result<Bytes, reqwest::Error>> = Vec::new();

        for (i, chunk_text) in chunks.iter().enumerate() {
            let chunk = serde_json::json!({
                "id": &id,
                "object": "chat.completion.chunk",
                "created": chrono_timestamp(),
                "model": &model,
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": if i == 0 { "assistant" } else { "" },
                        "content": chunk_text,
                    },
                    "finish_reason": serde_json::Value::Null,
                }],
            });
            events.push(Ok(Bytes::from(format!("data: {}\n\n", chunk))));
        }

        // Final chunk with usage
        let final_chunk = serde_json::json!({
            "id": &id,
            "object": "chat.completion.chunk",
            "model": &model,
            "choices": [{
                "index": 0,
                "delta": {"content": serde_json::Value::Null},
                "finish_reason": "stop",
            }],
            "usage": usage,
        });
        events.push(Ok(Bytes::from(format!("data: {}\n\n", final_chunk))));
        events.push(Ok(Bytes::from("data: [DONE]\n\n")));

        Ok(Box::pin(stream::iter(events)))
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(true)
    }
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{:x}{:x}", t.as_secs(), t.subsec_nanos())
}

fn chrono_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_msg(content: &str) -> Message {
        Message {
            role: "user".into(),
            content: Some(MessageContent::Text(content.into())),
            name: None, tool_calls: None, tool_call_id: None,
            extra: Default::default(),
        }
    }

    #[tokio::test]
    async fn test_mock_chat() {
        let provider = MockProvider::default_provider();
        let request = ChatRequest {
            model: "mock-model".into(),
            messages: vec![test_msg("Hello")],
            stream: false,
            ..Default::default()
        };

        let resp = provider.chat(&request).await.unwrap();
        assert!(resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .contains("mock response"));
        assert_eq!(resp["model"], "mock-model");
    }

    #[tokio::test]
    async fn test_mock_stream() {
        use futures::StreamExt;

        let provider = MockProvider::new("test-mock", MockConfig {
            latency_ms: 0,
            ..Default::default()
        });
        let request = ChatRequest {
            model: "mock-model".into(),
            messages: vec![test_msg("Hi")],
            stream: true,
            ..Default::default()
        };

        let stream = provider.chat_stream(&request).await.unwrap();
        let chunks: Vec<_> = stream.collect().await;
        assert!(chunks.len() >= 2); // at least content + [DONE]
        assert!(chunks.iter().all(|c| c.is_ok()));
    }

    #[tokio::test]
    async fn test_mock_failure() {
        let request = ChatRequest {
            model: "mock-model".into(),
            messages: vec![test_msg("Hi")],
            stream: false,
            ..Default::default()
        };

        // fail_every_n=1 means every request fails (count%1==0 always true when count>0)
        let provider = MockProvider::new("fail-mock", MockConfig {
            fail_every_n: Some(1),
            latency_ms: 0,
            ..Default::default()
        });
        let result = provider.chat(&request).await;
        assert!(result.is_err()); // count=1, 1%1==0 → fail

        // With fail_every_n=2, it should fail on every 2nd request
        let provider2 = MockProvider::new("fail2", MockConfig {
            fail_every_n: Some(2),
            latency_ms: 0,
            ..Default::default()
        });
        let _ = provider2.chat(&request).await; // count=1, ok
        let result = provider2.chat(&request).await; // count=2, fail
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_list_models() {
        let provider = MockProvider::default_provider();
        let models = provider.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "mock-model");
    }
}
