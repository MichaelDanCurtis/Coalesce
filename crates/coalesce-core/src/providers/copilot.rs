use super::{ByteStream, Provider};
use crate::error::{CoalesceError, Result};
use crate::types::{ChatRequest, ModelInfo, QualityTier};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

const GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_CHAT_URL: &str = "https://api.githubcopilot.com/chat/completions";
const COPILOT_MODELS_URL: &str = "https://api.githubcopilot.com/models";

/// Token refresh margin — refresh 2 minutes before expiry
const REFRESH_MARGIN_SECS: i64 = 120;

pub struct CopilotProvider {
    client: Client,
    token_state: Arc<RwLock<TokenState>>,
    provider_name: String,
}

#[derive(Debug, Clone)]
struct TokenState {
    /// GitHub OAuth access token (long-lived, from device flow)
    github_token: Option<String>,
    /// Copilot API token (short-lived, ~25 min)
    copilot_token: Option<String>,
    /// When the copilot token expires (unix timestamp)
    copilot_expires_at: i64,
}

impl Default for TokenState {
    fn default() -> Self {
        Self {
            github_token: None,
            copilot_token: None,
            copilot_expires_at: 0,
        }
    }
}

impl CopilotProvider {
    /// Create with an existing GitHub OAuth token (already authenticated).
    pub fn with_token(github_token: String) -> Self {
        Self::with_token_and_name(github_token, "copilot".to_string())
    }

    /// Create with token and custom provider name (for multi-account support).
    pub fn with_token_and_name(github_token: String, name: String) -> Self {
        Self {
            client: Client::new(),
            token_state: Arc::new(RwLock::new(TokenState {
                github_token: Some(github_token),
                copilot_token: None,
                copilot_expires_at: 0,
            })),
            provider_name: name,
        }
    }

    /// Create unauthenticated — must call `start_device_flow()` before use.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            token_state: Arc::new(RwLock::new(TokenState::default())),
            provider_name: "copilot".to_string(),
        }
    }

    /// Start the GitHub device flow. Returns the user code and verification URI
    /// that the user must visit to authorize.
    pub async fn start_device_flow(&self) -> Result<DeviceFlowResponse> {
        let resp = self
            .client
            .post(GITHUB_DEVICE_CODE_URL)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", GITHUB_CLIENT_ID),
                ("scope", ""),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(CoalesceError::Auth {
                message: format!("Device flow initiation failed: {}", resp.status()),
            });
        }

        let flow: DeviceFlowResponse = resp.json().await.map_err(|e| CoalesceError::Auth {
            message: format!("Failed to parse device flow response: {}", e),
        })?;

        Ok(flow)
    }

    /// Poll for the access token after user has authorized via browser.
    pub async fn poll_for_token(&self, device_code: &str, interval: u64) -> Result<String> {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;

            let resp = self
                .client
                .post(GITHUB_TOKEN_URL)
                .header("Accept", "application/json")
                .form(&[
                    ("client_id", GITHUB_CLIENT_ID),
                    ("device_code", device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await?;

            let body: serde_json::Value = resp.json().await?;

            if let Some(token) = body.get("access_token").and_then(|v| v.as_str()) {
                let token = token.to_string();
                let mut state = self.token_state.write().await;
                state.github_token = Some(token.clone());
                info!("GitHub OAuth token obtained successfully");
                return Ok(token);
            }

            match body.get("error").and_then(|v| v.as_str()) {
                Some("authorization_pending") => {
                    debug!("Waiting for user authorization...");
                    continue;
                }
                Some("slow_down") => {
                    debug!("Slowing down polling...");
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
                Some("expired_token") => {
                    return Err(CoalesceError::Auth {
                        message: "Device code expired — please restart the auth flow".into(),
                    });
                }
                Some("access_denied") => {
                    return Err(CoalesceError::Auth {
                        message: "User denied authorization".into(),
                    });
                }
                Some(err) => {
                    return Err(CoalesceError::Auth {
                        message: format!("OAuth error: {}", err),
                    });
                }
                None => {
                    return Err(CoalesceError::Auth {
                        message: "Unexpected OAuth response".into(),
                    });
                }
            }
        }
    }

    /// Get a valid Copilot API token, refreshing if needed.
    async fn get_copilot_token(&self) -> Result<String> {
        // Check if current token is still valid
        {
            let state = self.token_state.read().await;
            let now = chrono::Utc::now().timestamp();
            if let Some(ref token) = state.copilot_token {
                if now < state.copilot_expires_at - REFRESH_MARGIN_SECS {
                    return Ok(token.clone());
                }
            }
        }

        // Need to refresh
        self.refresh_copilot_token().await
    }

    async fn refresh_copilot_token(&self) -> Result<String> {
        let github_token = {
            let state = self.token_state.read().await;
            state.github_token.clone().ok_or_else(|| CoalesceError::Auth {
                message: "No GitHub token — complete device flow first".into(),
            })?
        };

        let resp = self
            .client
            .get(COPILOT_TOKEN_URL)
            .header("Authorization", format!("token {}", github_token))
            .header("Accept", "application/json")
            .header("User-Agent", "Coalesce/0.1.0")
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CoalesceError::Auth {
                message: format!("Copilot token exchange failed ({}): {}", status, body),
            });
        }

        let token_resp: CopilotTokenResponse = resp.json().await.map_err(|e| {
            CoalesceError::Auth {
                message: format!("Failed to parse Copilot token: {}", e),
            }
        })?;

        let mut state = self.token_state.write().await;
        state.copilot_token = Some(token_resp.token.clone());
        state.copilot_expires_at = token_resp.expires_at;

        debug!(
            "Copilot token refreshed, expires in {}s",
            token_resp.expires_at - chrono::Utc::now().timestamp()
        );

        Ok(token_resp.token)
    }

    fn copilot_headers(&self, token: &str) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token).parse().unwrap(),
        );
        headers.insert("editor-version", "vscode/1.85.1".parse().unwrap());
        headers.insert(
            "editor-plugin-version",
            "copilot/1.155.0".parse().unwrap(),
        );
        headers.insert(
            "Copilot-Integration-Id",
            "vscode-chat".parse().unwrap(),
        );
        headers.insert("openai-intent", "conversation-panel".parse().unwrap());
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers
    }

    async fn fetch_models_dynamic(&self, token: &str, prov: &str) -> Result<Vec<ModelInfo>> {
        let resp = self
            .client
            .get(COPILOT_MODELS_URL)
            .headers(self.copilot_headers(token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            debug!("Copilot models API returned {}", status);
            return Err(CoalesceError::Provider {
                provider: prov.to_string(),
                message: format!("Models API failed: {}", status),
                status: Some(status.as_u16()),
            });
        }

        let body: serde_json::Value = resp.json().await?;
        debug!("Copilot models API response keys: {:?}", body.as_object().map(|o| o.keys().collect::<Vec<_>>()));
        if let Some(data) = body.get("data").or_else(|| body.get("models")).and_then(|d| d.as_array()) {
            if let Some(first) = data.first() {
                debug!("Copilot model entry sample: {}", first);
            }
        }
        let mut models = Vec::new();

        if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
            for item in data {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                if id.is_empty() {
                    continue;
                }
                let display_name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(id);

                let model_info = copilot_model_info(id, display_name, prov, item);
                models.push(model_info);
            }
        }
        // Also try top-level "models" array (some API versions)
        if models.is_empty() {
            if let Some(data) = body.get("models").and_then(|d| d.as_array()) {
                for item in data {
                    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                    if id.is_empty() {
                        continue;
                    }
                    let display_name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(id);
                    let model_info = copilot_model_info(id, display_name, prov, item);
                    models.push(model_info);
                }
            }
        }

        Ok(models)
    }

    fn hardcoded_models(prov: &str) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "gpt-4o".into(),
                name: "GPT-4o (Copilot)".into(),
                provider: prov.into(),
                input_price_per_m: 2.50,
                output_price_per_m: 10.0,
                context_window: 128000,
                max_output: Some(16384),
                quality_tier: QualityTier::Complex,
                reasoning: false,
                vision: true,
                tool_calling: true,
            },
            ModelInfo {
                id: "gpt-4o-mini".into(),
                name: "GPT-4o Mini (Copilot)".into(),
                provider: prov.into(),
                input_price_per_m: 0.15,
                output_price_per_m: 0.60,
                context_window: 128000,
                max_output: Some(16384),
                quality_tier: QualityTier::Medium,
                reasoning: false,
                vision: true,
                tool_calling: true,
            },
        ]
    }
}

/// Infer ModelInfo from a Copilot API model entry
fn copilot_model_info(id: &str, display_name: &str, prov: &str, item: &serde_json::Value) -> ModelInfo {
    let lower = id.to_lowercase();
    let lower_name = display_name.to_lowercase();

    // Extract capabilities.supports object from API response
    let supports = item.get("capabilities")
        .and_then(|c| c.get("supports"))
        .and_then(|s| s.as_object());

    // Copilot API uses "adaptive_thinking", "reasoning_effort" for reasoning capability
    let api_has_reasoning = supports
        .map(|o| o.contains_key("adaptive_thinking") || o.contains_key("reasoning_effort")
             || o.contains_key("reasoning") || o.contains_key("thinking"))
        .unwrap_or(false);

    // Infer reasoning from name patterns (o1/o3/o4/thinking/reason) or API capabilities
    let reasoning = lower.contains("o1") || lower.contains("o3") || lower.contains("o4-")
        || lower_name.contains("thinking") || lower_name.contains("reason")
        || api_has_reasoning;

    // Infer quality tier from model name — reasoning models always get Reasoning tier
    let tier = if reasoning {
        QualityTier::Reasoning
    } else if lower.contains("opus") || lower.contains("pro") || (lower.contains("gpt-4o")
        && !lower.contains("mini"))
    {
        QualityTier::Complex
    } else if lower.contains("mini") || lower.contains("flash") || lower_name.contains("medium")
        || lower_name.contains("low")
    {
        QualityTier::Medium
    } else if lower.contains("nano") || lower.contains("haiku") {
        QualityTier::Simple
    } else {
        QualityTier::Complex
    };

    // Extract capabilities from API response
    // Copilot API: capabilities.supports.vision = true (boolean)
    // Also: capabilities.limits.vision = { max_prompt_images, ... } (object, indicates vision support)
    let has_vision = supports
        .and_then(|o| o.get("vision"))
        .and_then(|v| v.as_bool())
        .or_else(|| {
            // If supports.vision isn't a bool, check if limits.vision object exists
            item.get("capabilities")
                .and_then(|c| c.get("limits"))
                .and_then(|l| l.get("vision"))
                .map(|v| v.is_object())
        })
        .unwrap_or(!reasoning && !lower.contains("text-only"));

    // Copilot API: capabilities.supports.tool_calls = true (not "tools")
    let has_tools = supports
        .and_then(|o| {
            o.get("tool_calls").and_then(|v| v.as_bool())
                .or_else(|| o.get("parallel_tool_calls").and_then(|v| v.as_bool()))
                .or_else(|| o.get("tools").and_then(|v| v.as_bool()))
        })
        .unwrap_or(!reasoning);

    // Extract request multiplier from API — this is the key pricing data
    // Try common field names the API might use
    let multiplier = item.get("request_multiplier")
        .or_else(|| item.get("multiplier"))
        .or_else(|| item.get("cost_multiplier"))
        .or_else(|| item.get("capabilities").and_then(|c| c.get("request_multiplier")))
        .or_else(|| item.get("capabilities").and_then(|c| c.get("multiplier")))
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| infer_multiplier_from_name(&lower, &lower_name));

    // Copilot base rate: $0.00001 per token unit
    // For a 1x model, that's $10.00 per 1M tokens (output equivalent)
    // Input is typically 1/4 of output cost, so:
    //   input  = multiplier × $2.50 per 1M
    //   output = multiplier × $10.00 per 1M
    let input_price = multiplier * 2.50;
    let output_price = multiplier * 10.0;

    // Try to extract context window from capabilities
    // Copilot API: capabilities.limits.max_context_window_tokens (total), max_prompt_tokens (input)
    let context_window = item
        .get("capabilities")
        .and_then(|c| c.get("limits"))
        .and_then(|l| l.get("max_context_window_tokens").or(l.get("max_prompt_tokens")))
        .and_then(|v| v.as_u64())
        .or_else(|| item.get("context_window").and_then(|v| v.as_u64()))
        .unwrap_or(if lower.contains("claude") { 200000 } else { 128000 }) as u32;

    let max_output = item
        .get("capabilities")
        .and_then(|c| c.get("limits"))
        .and_then(|l| l.get("max_output_tokens"))
        .and_then(|v| v.as_u64())
        .or_else(|| item.get("max_output_tokens").and_then(|v| v.as_u64()))
        .map(|v| v as u32)
        .or(Some(16384));

    // Build a nice display name
    let name = if display_name != id {
        format!("{} (Copilot)", display_name)
    } else {
        let pretty = id
            .split(&['-', '_', '.'][..])
            .map(|part| {
                let mut c = part.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().to_string() + c.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} (Copilot)", pretty)
    };

    ModelInfo {
        id: id.into(),
        name,
        provider: prov.into(),
        input_price_per_m: input_price,
        output_price_per_m: output_price,
        context_window,
        max_output,
        quality_tier: tier,
        reasoning,
        vision: has_vision,
        tool_calling: has_tools,
    }
}

/// Fallback multiplier inference from model name, matching GitHub Copilot's known multipliers
fn infer_multiplier_from_name(lower_id: &str, lower_name: &str) -> f64 {
    if lower_id.contains("opus") {
        3.0
    } else if lower_id.contains("haiku") {
        0.33
    } else if lower_id.contains("flash") {
        0.33
    } else if lower_id.contains("gpt-4o") && !lower_id.contains("mini") {
        0.0 // GPT-4o is 0x (free/included)
    } else if lower_name.contains("mini") && (lower_id.contains("gpt-5") || lower_id.contains("gpt-4o")) {
        0.0 // GPT-5 mini / GPT-4o mini are 0x
    } else if lower_id.contains("grok") && lower_id.contains("fast") {
        0.25
    } else if lower_id.contains("grok") {
        0.33
    } else {
        1.0 // Default: most models are 1x
    }
}

#[async_trait]
impl Provider for CopilotProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let prov = &self.provider_name;

        // Try dynamic discovery from Copilot models API
        if let Ok(token) = self.get_copilot_token().await {
            if let Ok(models) = self.fetch_models_dynamic(&token, prov).await {
                if !models.is_empty() {
                    debug!("Copilot: discovered {} models dynamically", models.len());
                    return Ok(models);
                }
            }
        }

        debug!("Copilot: falling back to hardcoded model list");
        Ok(Self::hardcoded_models(prov))
    }

    async fn chat(&self, request: &ChatRequest) -> Result<serde_json::Value> {
        let token = self.get_copilot_token().await?;

        let resp = self
            .client
            .post(COPILOT_CHAT_URL)
            .headers(self.copilot_headers(&token))
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
        let token = self.get_copilot_token().await?;

        let mut req = request.clone();
        req.stream = true;

        let resp = self
            .client
            .post(COPILOT_CHAT_URL)
            .headers(self.copilot_headers(&token))
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
        let state = self.token_state.read().await;
        Ok(state.github_token.is_some())
    }
}

// --- API response types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFlowResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state() {
        let provider = CopilotProvider::new();
        // Should be unauthenticated
        let state = provider.token_state.try_read().unwrap();
        assert!(state.github_token.is_none());
        assert!(state.copilot_token.is_none());
    }

    #[test]
    fn test_with_token() {
        let provider = CopilotProvider::with_token("ghu_test123".into());
        let state = provider.token_state.try_read().unwrap();
        assert_eq!(state.github_token.as_deref(), Some("ghu_test123"));
    }

    #[test]
    fn test_known_models() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let provider = CopilotProvider::new();
            let models = provider.list_models().await.unwrap();
            // Without a valid token, falls back to minimal hardcoded list (gpt-4o, gpt-4o-mini)
            assert!(models.len() >= 2);
            assert!(models.iter().all(|m| m.provider == "copilot"));
        });
    }
}
