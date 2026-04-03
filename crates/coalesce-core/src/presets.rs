use crate::config::ProviderConfig;
use std::collections::HashMap;

/// Pre-configured provider setups for common providers.
pub struct ProviderPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub config: ProviderConfig,
}

/// Get all available provider presets.
pub fn all_presets() -> Vec<ProviderPreset> {
    vec![
        ProviderPreset {
            name: "ollama",
            description: "Local Ollama instance (free, requires Ollama running)",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: None,
                endpoint: Some("http://localhost:11434".into()),
                billing: Some("local".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "openrouter",
            description: "OpenRouter - 300+ models, per-token pricing",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("OPENROUTER_API_KEY".into()),
                endpoint: None,
                billing: Some("per_token".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "copilot",
            description: "GitHub Copilot - free included models + premium quota",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("GITHUB_TOKEN".into()),
                endpoint: None,
                billing: Some("quota_refreshing:50:18000".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "openai",
            description: "OpenAI direct - GPT-4o, GPT-5, o3",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("OPENAI_API_KEY".into()),
                endpoint: None,
                billing: Some("per_token".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "anthropic",
            description: "Anthropic direct - Claude Sonnet/Opus",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("ANTHROPIC_API_KEY".into()),
                endpoint: None,
                billing: Some("quota_refreshing:50:18000".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "google",
            description: "Google AI Studio - Gemini Pro/Flash with free credits",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("GOOGLE_API_KEY".into()),
                endpoint: None,
                billing: Some("free_credits:50.0".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "deepseek",
            description: "DeepSeek - V3 and R1 models, affordable pricing",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("DEEPSEEK_API_KEY".into()),
                endpoint: None,
                billing: Some("per_token".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "kimi",
            description: "Kimi/Moonshot - quota-only free model",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("KIMI_API_KEY".into()),
                endpoint: None,
                billing: Some("quota_only:50:0".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "glm",
            description: "Zhipu AI/GLM - GLM-4 models, per-token pricing",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("GLM_API_KEY".into()),
                endpoint: None,
                billing: Some("per_token".into()),
                ..Default::default()
            },
        },
        ProviderPreset {
            name: "xai",
            description: "xAI - Grok models",
            config: ProviderConfig {
                enabled: true,
                api_key: None,
                api_key_env: Some("XAI_API_KEY".into()),
                endpoint: None,
                billing: Some("per_token".into()),
                ..Default::default()
            },
        },
    ]
}

/// Get a preset by provider name.
pub fn get_preset(name: &str) -> Option<ProviderPreset> {
    all_presets().into_iter().find(|p| p.name == name)
}

/// Generate a full config with selected presets.
pub fn config_from_presets(names: &[&str]) -> HashMap<String, ProviderConfig> {
    let mut providers = HashMap::new();
    for name in names {
        if let Some(preset) = get_preset(name) {
            providers.insert(preset.name.to_string(), preset.config);
        }
    }
    providers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_presets() {
        let presets = all_presets();
        assert_eq!(presets.len(), 10);
        // All have names
        for p in &presets {
            assert!(!p.name.is_empty());
            assert!(!p.description.is_empty());
        }
    }

    #[test]
    fn test_get_preset() {
        let ollama = get_preset("ollama").unwrap();
        assert_eq!(ollama.config.billing.as_deref(), Some("local"));
        assert!(get_preset("nonexistent").is_none());
    }

    #[test]
    fn test_config_from_presets() {
        let config = config_from_presets(&["ollama", "openrouter"]);
        assert_eq!(config.len(), 2);
        assert!(config.contains_key("ollama"));
        assert!(config.contains_key("openrouter"));
    }
}
