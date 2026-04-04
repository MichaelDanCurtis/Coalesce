pub mod anthropic;
pub mod copilot;
pub mod google_cloudcode;
pub mod health;
pub mod mock;
pub mod ollama;
pub mod openai_compat;
pub mod openrouter;

use crate::error::Result;
use crate::types::{ChatRequest, ModelInfo};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;

// Re-export async_trait for providers
pub use async_trait::async_trait as provider_trait;

/// Stream of SSE bytes from a provider
pub type ByteStream =
    Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>;

/// Trait that all LLM providers must implement
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider name (e.g., "openrouter", "copilot", "ollama")
    fn name(&self) -> &str;

    /// List available models from this provider
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;

    /// Send a chat completion request (non-streaming)
    async fn chat(&self, request: &ChatRequest) -> Result<serde_json::Value>;

    /// Send a streaming chat completion request
    async fn chat_stream(&self, request: &ChatRequest) -> Result<ByteStream>;

    /// Health check
    async fn health_check(&self) -> Result<bool> {
        Ok(true)
    }
}
