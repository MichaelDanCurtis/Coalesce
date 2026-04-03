use super::{ByteStream, Provider};
use crate::error::{CoalesceError, Result};
use crate::types::{ChatRequest, ModelInfo, QualityTier, derive_canonical_family};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

const BASE_URL: &str = "https://openrouter.ai/api/v1";

pub struct OpenRouterProvider {
    client: Client,
    api_key: String,
}

impl OpenRouterProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.api_key).parse().unwrap(),
        );
        headers.insert(
            "HTTP-Referer",
            "https://coalesce.dev".parse().unwrap(),
        );
        headers.insert(
            "X-Title",
            "Coalesce".parse().unwrap(),
        );
        headers
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let resp = self
            .client
            .get(format!("{}/models", BASE_URL))
            .headers(self.headers())
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(CoalesceError::Provider {
                provider: "openrouter".into(),
                message: format!("Model list failed: {}", resp.status()),
                status: Some(resp.status().as_u16()),
            });
        }

        let body: OpenRouterModelsResponse = resp.json().await?;
        let models = body
            .data
            .into_iter()
            .filter_map(|m| convert_model(m))
            .collect();

        Ok(models)
    }

    async fn chat(&self, request: &ChatRequest) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(format!("{}/chat/completions", BASE_URL))
            .headers(self.headers())
            .json(&request)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Provider {
                provider: "openrouter".into(),
                message: format!("Chat failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        let json: serde_json::Value = resp.json().await?;
        Ok(json)
    }

    async fn chat_stream(&self, request: &ChatRequest) -> Result<ByteStream> {
        // Ensure stream is set
        let mut req = request.clone();
        req.stream = true;

        let resp = self
            .client
            .post(format!("{}/chat/completions", BASE_URL))
            .headers(self.headers())
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Provider {
                provider: "openrouter".into(),
                message: format!("Stream failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        Ok(Box::pin(resp.bytes_stream()))
    }

    async fn health_check(&self) -> Result<bool> {
        let resp = self
            .client
            .get(format!("{}/models", BASE_URL))
            .headers(self.headers())
            .send()
            .await;

        Ok(resp.is_ok_and(|r| r.status().is_success()))
    }
}

// --- OpenRouter API response types ---

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModel {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    pricing: Option<OpenRouterPricing>,
    #[serde(default)]
    top_provider: Option<TopProvider>,
    #[serde(default)]
    architecture: Option<Architecture>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterPricing {
    /// USD per token (string) — NOT per million
    #[serde(default)]
    prompt: Option<String>,
    /// USD per token (string)
    #[serde(default)]
    completion: Option<String>,
    /// USD per token for internal reasoning
    #[serde(default)]
    internal_reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TopProvider {
    #[serde(default)]
    max_completion_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Architecture {
    #[serde(default)]
    modality: Option<String>,
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
}

/// Convert OpenRouter model to our ModelInfo.
/// OpenRouter pricing is per-token (string), we convert to per-million (f64).
fn convert_model(m: OpenRouterModel) -> Option<ModelInfo> {
    let pricing = m.pricing.as_ref();

    // Parse per-token prices → per-million
    let input_price_per_m = pricing
        .and_then(|p| p.prompt.as_deref())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|per_token| per_token * 1_000_000.0)
        .unwrap_or(0.0);

    let output_price_per_m = pricing
        .and_then(|p| p.completion.as_deref())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|per_token| per_token * 1_000_000.0)
        .unwrap_or(0.0);

    // Detect reasoning model: internal_reasoning price > 0
    let reasoning = pricing
        .and_then(|p| p.internal_reasoning.as_deref())
        .and_then(|s| s.parse::<f64>().ok())
        .is_some_and(|v| v > 0.0);

    // Detect vision support from architecture
    let vision = m
        .architecture
        .as_ref()
        .and_then(|a| a.input_modalities.as_ref())
        .is_some_and(|mods| mods.iter().any(|m| m == "image"));

    // Classify quality tier based on pricing
    let quality_tier = if reasoning {
        QualityTier::Reasoning
    } else if input_price_per_m >= 2.0 {
        QualityTier::Complex
    } else if input_price_per_m >= 0.1 {
        QualityTier::Medium
    } else {
        QualityTier::Simple
    };

    let context_window = m.context_length.unwrap_or(4096);
    let max_output = m.top_provider.and_then(|tp| tp.max_completion_tokens);

    let name = if m.name.is_empty() {
        m.id.clone()
    } else {
        m.name
    };

    let family = derive_canonical_family(&m.id);

    Some(ModelInfo {
        id: m.id,
        name,
        provider: "openrouter".to_string(),
        input_price_per_m,
        output_price_per_m,
        context_window,
        max_output,
        quality_tier,
        reasoning,
        vision,
        tool_calling: true, // Most OpenRouter models support tool calling
        canonical_family: Some(family),
        capabilities: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_model_pricing() {
        let m = OpenRouterModel {
            id: "anthropic/claude-sonnet-4".into(),
            name: "Claude Sonnet 4".into(),
            context_length: Some(200000),
            pricing: Some(OpenRouterPricing {
                prompt: Some("0.000003".into()),    // $3/M
                completion: Some("0.000015".into()), // $15/M
                internal_reasoning: None,
            }),
            top_provider: Some(TopProvider {
                max_completion_tokens: Some(8192),
            }),
            architecture: Some(Architecture {
                modality: Some("text+image->text".into()),
                input_modalities: Some(vec!["text".into(), "image".into()]),
            }),
        };

        let info = convert_model(m).unwrap();
        assert!((info.input_price_per_m - 3.0).abs() < 0.01);
        assert!((info.output_price_per_m - 15.0).abs() < 0.01);
        assert_eq!(info.context_window, 200000);
        assert_eq!(info.max_output, Some(8192));
        assert!(info.vision);
        assert!(!info.reasoning);
        assert_eq!(info.quality_tier, QualityTier::Complex);
    }

    #[test]
    fn test_convert_reasoning_model() {
        let m = OpenRouterModel {
            id: "anthropic/claude-opus-4".into(),
            name: "Claude Opus 4".into(),
            context_length: Some(200000),
            pricing: Some(OpenRouterPricing {
                prompt: Some("0.000015".into()),
                completion: Some("0.000075".into()),
                internal_reasoning: Some("0.000015".into()),
            }),
            top_provider: None,
            architecture: None,
        };

        let info = convert_model(m).unwrap();
        assert!(info.reasoning);
        assert_eq!(info.quality_tier, QualityTier::Reasoning);
    }

    #[test]
    fn test_convert_cheap_model() {
        let m = OpenRouterModel {
            id: "meta-llama/llama-3.3-8b-instruct".into(),
            name: "Llama 3.3 8B".into(),
            context_length: Some(131072),
            pricing: Some(OpenRouterPricing {
                prompt: Some("0.00000006".into()), // $0.06/M
                completion: Some("0.00000006".into()),
                internal_reasoning: None,
            }),
            top_provider: None,
            architecture: None,
        };

        let info = convert_model(m).unwrap();
        assert_eq!(info.quality_tier, QualityTier::Simple);
        assert!((info.input_price_per_m - 0.06).abs() < 0.01);
    }
}
