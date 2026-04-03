use super::{ByteStream, Provider};
use crate::error::{CoalesceError, Result};
use crate::types::{ChatRequest, ModelInfo, derive_canonical_family};
use async_trait::async_trait;
use reqwest::Client;

/// Generic OpenAI-compatible provider.
/// Works for: OpenAI, GLM/Zhipu, Kimi/Moonshot, DeepSeek, xAI, and any
/// other provider that speaks the OpenAI chat completions API.
pub struct OpenAICompatProvider {
    client: Client,
    provider_name: String,
    base_url: String,
    api_key: String,
    /// Static model list (for providers without a /models endpoint)
    known_models: Vec<ModelInfo>,
    /// Whether to try GET /models for discovery
    supports_model_list: bool,
}

impl OpenAICompatProvider {
    pub fn new(
        provider_name: String,
        base_url: String,
        api_key: String,
        known_models: Vec<ModelInfo>,
        supports_model_list: bool,
    ) -> Self {
        Self {
            client: Client::new(),
            provider_name,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            known_models,
            supports_model_list,
        }
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }

    /// Build a ModelInfo from a raw model ID (e.g. "gemini-3.1-pro-latest").
    /// Produces a human-readable name and infers quality tier from the ID.
    fn model_info_from_id(id: &str, provider: &str) -> ModelInfo {
        // Strip common suffixes like "-latest", "-preview", date stamps
        let base = id
            .trim_end_matches("-latest")
            .trim_end_matches("-preview");

        // Title-case: "gemini-3.1-pro" -> "Gemini 3.1 Pro"
        let name: String = base
            .split('-')
            .map(|seg| {
                // Keep version numbers as-is, capitalize words
                if seg.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                    seg.to_string()
                } else {
                    let mut chars = seg.chars();
                    match chars.next() {
                        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                        None => String::new(),
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        let id_lower = id.to_lowercase();
        let is_pro = id_lower.contains("pro");
        let is_flash = id_lower.contains("flash");
        let is_nano = id_lower.contains("nano") || id_lower.contains("lite");
        let is_plus = id_lower.contains("plus");
        let is_turbo = id_lower.contains("turbo");

        let is_thinking = id_lower.contains("thinking") || id_lower.contains("reason");
        let is_complex = is_plus || id_lower.contains("opus") || id_lower.contains("ultra");

        // GLM-specific: models with "v" suffix (glm-4.6v, glm-5v-turbo) have vision
        // GLM 4.5+ series support reasoning (thinking param)
        let is_glm = id_lower.starts_with("glm");
        let glm_has_vision = is_glm && id_lower.contains("v") && {
            // Check it's actually a vision indicator (e.g. "4.6v", "5v-") not just "v" in the name
            id_lower.contains("4v") || id_lower.contains("5v") || id_lower.contains("6v")
        };
        let glm_has_reasoning = is_glm && !is_flash && {
            // GLM 4.5 and above support thinking param
            id_lower.contains("glm-4.5") || id_lower.contains("glm-4.6")
                || id_lower.contains("glm-4.7") || id_lower.contains("glm-5")
        };

        let is_reasoning = is_pro || is_thinking || glm_has_reasoning;
        let quality_tier = if is_reasoning {
            crate::types::QualityTier::Reasoning
        } else if is_complex {
            crate::types::QualityTier::Complex
        } else if is_nano {
            crate::types::QualityTier::Simple
        } else if is_flash || is_turbo {
            crate::types::QualityTier::Medium
        } else {
            crate::types::QualityTier::Medium
        };

        // Vision: GLM needs explicit "V" models; Gemini models generally support vision
        let has_vision = if is_glm { glm_has_vision } else { true };

        // Infer pricing from model name (defaults for unknown providers)
        let (input_price, output_price) = if is_nano {
            (0.10, 0.40)
        } else if is_flash && is_thinking {
            (0.10, 0.40)
        } else if is_flash {
            (0.075, 0.30)
        } else if is_pro && is_thinking {
            (1.25, 10.0)
        } else if is_pro {
            (1.25, 5.0)
        } else {
            (0.50, 2.0)
        };

        ModelInfo {
            id: id.to_string(),
            name,
            provider: provider.to_string(),
            input_price_per_m: input_price,
            output_price_per_m: output_price,
            context_window: if is_flash || is_pro { 1048576 } else { 131072 },
            max_output: Some(if is_glm && !is_flash { 128000 } else { 65536 }),
            quality_tier,
            reasoning: is_reasoning,
            vision: has_vision,
            tool_calling: true,
            canonical_family: Some(derive_canonical_family(id)),
            capabilities: None,
        }
    }
}

#[async_trait]
impl Provider for OpenAICompatProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        if !self.supports_model_list || self.api_key.is_empty() {
            return Ok(self.known_models.clone());
        }

        let resp = self
            .client
            .get(format!("{}/models", self.base_url))
            .header("Authorization", self.auth_header())
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                // Try to parse standard OpenAI /models response
                let body: serde_json::Value = r.json().await?;
                if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
                    let mut models: Vec<ModelInfo> = data
                        .iter()
                        .filter_map(|m| {
                            let id = m.get("id")?.as_str()?.to_string();
                            // Find matching known model or create entry from API data
                            self.known_models
                                .iter()
                                .find(|km| km.id == id)
                                .cloned()
                                .or_else(|| {
                                    Some(Self::model_info_from_id(
                                        &id,
                                        &self.provider_name,
                                    ))
                                })
                        })
                        .collect();

                    // Add any known models not returned by API
                    for km in &self.known_models {
                        if !models.iter().any(|m| m.id == km.id) {
                            models.push(km.clone());
                        }
                    }

                    Ok(models)
                } else {
                    Ok(self.known_models.clone())
                }
            }
            _ => Ok(self.known_models.clone()),
        }
    }

    async fn chat(&self, request: &ChatRequest) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", self.auth_header())
            .json(&request)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Provider {
                provider: self.provider_name.clone(),
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
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", self.auth_header())
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Provider {
                provider: self.provider_name.clone(),
                message: format!("Stream failed ({}): {}", status, body),
                status: Some(status.as_u16()),
            });
        }

        Ok(Box::pin(resp.bytes_stream()))
    }

    async fn health_check(&self) -> Result<bool> {
        if self.api_key.is_empty() {
            return Ok(false);
        }

        // Google/Antigravity uses OAuth tokens that don't work with the OpenAI-compat
        // /models endpoint — token validity is confirmed via loadCodeAssist on startup
        if self.provider_name == "google" {
            return Ok(true);
        }

        let resp = self
            .client
            .get(format!("{}/models", self.base_url))
            .header("Authorization", self.auth_header())
            .send()
            .await;

        Ok(resp.is_ok_and(|r| r.status().is_success()))
    }
}

// --- Factory functions for specific providers ---

pub mod factories {
    use super::*;
    use crate::types::{QualityTier, derive_canonical_family};

    /// GLM/Zhipu AI — api.z.ai coding plan (international)
    /// Uses the coding plan endpoint by default.
    /// Models discovered dynamically via the /models API.
    pub fn glm(api_key: String) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "glm".into(),
            "https://api.z.ai/api/coding/paas/v4".into(),
            api_key,
            vec![],  // No hardcoded models — fully dynamic
            true,
        )
    }

    /// GLM/Zhipu AI — api.z.ai general (non-coding) endpoint
    pub fn glm_general(api_key: String) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "glm".into(),
            "https://api.z.ai/api/paas/v4".into(),
            api_key,
            vec![],
            true,
        )
    }

    /// GLM/Zhipu AI — open.bigmodel.cn (China domestic endpoint)
    /// Use this if your API key is from the bigmodel.cn platform.
    pub fn glm_cn(api_key: String) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "glm".into(),
            "https://open.bigmodel.cn/api/paas/v4".into(),
            api_key,
            vec![],
            true,
        )
    }

    /// Kimi — api.kimi.com/coding/v1 (sk-kimi- keys)
    pub fn kimi(api_key: String) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "kimi".into(),
            "https://api.kimi.com/coding/v1".into(),
            api_key,
            vec![
                ModelInfo {
                    id: "kimi-for-coding".into(),
                    name: "Kimi For Coding".into(),
                    provider: "kimi".into(),
                    input_price_per_m: 0.0,
                    output_price_per_m: 0.0,
                    context_window: 262144,
                    max_output: Some(8192),
                    quality_tier: QualityTier::Reasoning,
                    reasoning: true,
                    vision: true,
                    tool_calling: true,
                    canonical_family: Some(derive_canonical_family("kimi-for-coding")),
                    capabilities: None,
                },
            ],
            true,
        )
    }

    /// DeepSeek — api.deepseek.com
    pub fn deepseek(api_key: String) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "deepseek".into(),
            "https://api.deepseek.com/v1".into(),
            api_key,
            vec![
                ModelInfo {
                    id: "deepseek-chat".into(),
                    name: "DeepSeek V3".into(),
                    provider: "deepseek".into(),
                    input_price_per_m: 0.27,
                    output_price_per_m: 1.10,
                    context_window: 64000,
                    max_output: Some(8192),
                    quality_tier: QualityTier::Complex,
                    reasoning: false,
                    vision: false,
                    tool_calling: true,
                    canonical_family: Some(derive_canonical_family("deepseek-chat")),
                    capabilities: None,
                },
                ModelInfo {
                    id: "deepseek-reasoner".into(),
                    name: "DeepSeek R1".into(),
                    provider: "deepseek".into(),
                    input_price_per_m: 0.55,
                    output_price_per_m: 2.19,
                    context_window: 64000,
                    max_output: Some(8192),
                    quality_tier: QualityTier::Reasoning,
                    reasoning: true,
                    vision: false,
                    tool_calling: false,
                    canonical_family: Some(derive_canonical_family("deepseek-reasoner")),
                    capabilities: None,
                },
            ],
            false,
        )
    }

    /// OpenAI direct — api.openai.com
    pub fn openai(api_key: String) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "openai".into(),
            "https://api.openai.com/v1".into(),
            api_key,
            vec![
                ModelInfo {
                    id: "gpt-4.1".into(),
                    name: "GPT-4.1".into(),
                    provider: "openai".into(),
                    input_price_per_m: 2.0,
                    output_price_per_m: 8.0,
                    context_window: 1047576,
                    max_output: Some(32768),
                    quality_tier: QualityTier::Complex,
                    reasoning: false,
                    vision: true,
                    tool_calling: true,
                    canonical_family: Some(derive_canonical_family("gpt-4.1")),
                    capabilities: None,
                },
                ModelInfo {
                    id: "gpt-4.1-mini".into(),
                    name: "GPT-4.1 Mini".into(),
                    provider: "openai".into(),
                    input_price_per_m: 0.4,
                    output_price_per_m: 1.6,
                    context_window: 1047576,
                    max_output: Some(32768),
                    quality_tier: QualityTier::Medium,
                    reasoning: false,
                    vision: true,
                    tool_calling: true,
                    canonical_family: Some(derive_canonical_family("gpt-4.1-mini")),
                    capabilities: None,
                },
                ModelInfo {
                    id: "o3-mini".into(),
                    name: "o3 Mini".into(),
                    provider: "openai".into(),
                    input_price_per_m: 1.1,
                    output_price_per_m: 4.4,
                    context_window: 200000,
                    max_output: Some(100000),
                    quality_tier: QualityTier::Reasoning,
                    reasoning: true,
                    vision: false,
                    tool_calling: true,
                    canonical_family: Some(derive_canonical_family("o3-mini")),
                    capabilities: None,
                },
            ],
            true,
        )
    }

    /// xAI/Grok — api.x.ai
    pub fn xai(api_key: String) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "xai".into(),
            "https://api.x.ai/v1".into(),
            api_key,
            vec![ModelInfo {
                id: "grok-3".into(),
                name: "Grok 3".into(),
                provider: "xai".into(),
                input_price_per_m: 3.0,
                output_price_per_m: 15.0,
                context_window: 131072,
                max_output: Some(131072),
                quality_tier: QualityTier::Complex,
                reasoning: false,
                vision: false,
                tool_calling: true,
                canonical_family: Some(derive_canonical_family("grok-3")),
                capabilities: None,
            }],
            true,
        )
    }

    /// Google Gemini — generativelanguage.googleapis.com (OpenAI-compatible endpoint)
    /// Models are discovered dynamically via the /models API.
    pub fn google(api_key: String) -> OpenAICompatProvider {
        OpenAICompatProvider::new(
            "google".into(),
            "https://generativelanguage.googleapis.com/v1beta/openai".into(),
            api_key,
            vec![],  // No hardcoded models — fully dynamic
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glm_factory() {
        let provider = factories::glm("test-key".into());
        assert_eq!(provider.name(), "glm");
        assert_eq!(provider.base_url, "https://api.z.ai/api/coding/paas/v4");
        assert!(provider.known_models.is_empty()); // fully dynamic
    }

    #[test]
    fn test_glm_cn_factory() {
        let provider = factories::glm_cn("test-key".into());
        assert_eq!(provider.name(), "glm");
        assert_eq!(provider.base_url, "https://open.bigmodel.cn/api/paas/v4");
    }

    #[test]
    fn test_kimi_factory() {
        let provider = factories::kimi("test-key".into());
        assert_eq!(provider.name(), "kimi");
        assert!(provider.known_models.iter().any(|m| m.id == "kimi-for-coding"));
    }

    #[test]
    fn test_deepseek_factory() {
        let provider = factories::deepseek("test-key".into());
        assert_eq!(provider.name(), "deepseek");
        let reasoner = provider.known_models.iter().find(|m| m.id == "deepseek-reasoner").unwrap();
        assert!(reasoner.reasoning);
        assert_eq!(reasoner.quality_tier, crate::types::QualityTier::Reasoning);
    }

    #[test]
    fn test_url_trailing_slash_stripped() {
        let provider = OpenAICompatProvider::new(
            "test".into(),
            "https://example.com/v1/".into(),
            "key".into(),
            vec![],
            false,
        );
        assert_eq!(provider.base_url, "https://example.com/v1");
    }
}
