use super::{ByteStream, Provider};
use crate::error::{CoalesceError, Result};
use crate::types::{ChatRequest, ModelInfo, QualityTier, derive_canonical_family};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

const DEFAULT_URL: &str = "http://localhost:11434";

pub struct OllamaProvider {
    client: Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_URL.to_string()),
        }
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        // Ollama uses /api/tags for model listing
        let resp = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(CoalesceError::Provider {
                provider: "ollama".into(),
                message: format!("Model list failed: {}", resp.status()),
                status: Some(resp.status().as_u16()),
            });
        }

        let body: OllamaTagsResponse = resp.json().await?;
        let models = body
            .models
            .into_iter()
            .map(|m| convert_ollama_model(m))
            .collect();

        Ok(models)
    }

    async fn chat(&self, request: &ChatRequest) -> Result<serde_json::Value> {
        // Ollama supports OpenAI-compatible API at /v1/chat/completions
        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&request)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Provider {
                provider: "ollama".into(),
                message: format!("Chat failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        let json: serde_json::Value = resp.json().await?;
        Ok(json)
    }

    async fn chat_stream(&self, request: &ChatRequest) -> Result<ByteStream> {
        let mut req = request.clone();
        req.stream = true;

        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Provider {
                provider: "ollama".into(),
                message: format!("Stream failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        Ok(Box::pin(resp.bytes_stream()))
    }

    async fn health_check(&self) -> Result<bool> {
        let resp = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await;

        Ok(resp.is_ok_and(|r| r.status().is_success()))
    }
}

// --- Ollama API types ---

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OllamaModel {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    details: Option<OllamaModelDetails>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OllamaModelDetails {
    #[serde(default)]
    parameter_size: Option<String>,
    #[serde(default)]
    family: Option<String>,
}

fn convert_ollama_model(m: OllamaModel) -> ModelInfo {
    // Estimate quality tier from parameter size
    let param_billions = m
        .details
        .as_ref()
        .and_then(|d| d.parameter_size.as_deref())
        .and_then(|s| {
            // Parse "7B", "70B", "3.8B" etc
            s.trim_end_matches('B')
                .trim_end_matches('b')
                .parse::<f64>()
                .ok()
        })
        .unwrap_or(7.0); // Default guess

    let quality_tier = if param_billions >= 70.0 {
        QualityTier::Complex
    } else if param_billions >= 13.0 {
        QualityTier::Medium
    } else {
        QualityTier::Simple
    };

    // Estimate context window from model family
    let context_window = if m.name.contains("llama") || m.name.contains("qwen") {
        131072
    } else {
        8192
    };

    // Strip tag suffix for display name
    let display_name = m.name.split(':').next().unwrap_or(&m.name).to_string();

    ModelInfo {
        id: m.name.clone(),
        name: display_name,
        provider: "ollama".to_string(),
        input_price_per_m: 0.0,
        output_price_per_m: 0.0,
        context_window,
        max_output: Some(4096),
        quality_tier,
        reasoning: false,
        vision: m.name.contains("vision") || m.name.contains("llava"),
        tool_calling: m.name.contains("llama") || m.name.contains("qwen") || m.name.contains("mistral"),
        canonical_family: Some(derive_canonical_family(&m.name)),
        capabilities: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_ollama_model() {
        let m = OllamaModel {
            name: "llama3.3:70b".into(),
            size: 40_000_000_000,
            details: Some(OllamaModelDetails {
                parameter_size: Some("70B".into()),
                family: Some("llama".into()),
            }),
        };

        let info = convert_ollama_model(m);
        assert_eq!(info.provider, "ollama");
        assert_eq!(info.quality_tier, QualityTier::Complex);
        assert_eq!(info.input_price_per_m, 0.0);
        assert_eq!(info.name, "llama3.3");
        assert!(info.tool_calling);
    }

    #[test]
    fn test_small_model_tier() {
        let m = OllamaModel {
            name: "phi3:3.8b".into(),
            size: 2_000_000_000,
            details: Some(OllamaModelDetails {
                parameter_size: Some("3.8B".into()),
                family: Some("phi".into()),
            }),
        };

        let info = convert_ollama_model(m);
        assert_eq!(info.quality_tier, QualityTier::Simple);
    }
}
