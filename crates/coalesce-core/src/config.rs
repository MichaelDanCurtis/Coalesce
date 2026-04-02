use crate::router::config::RoutingConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_server")]
    pub server: ServerConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub budget: BudgetConfig,
    #[serde(default)]
    pub sanitize: SanitizeConfig,
    #[serde(default)]
    pub multi_tenant: MultiTenantConfig,
    #[serde(default)]
    pub semantic_cache: SemanticCacheConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: default_server(),
            providers: HashMap::new(),
            routing: RoutingConfig::default(),
            logging: LoggingConfig::default(),
            budget: BudgetConfig::default(),
            sanitize: SanitizeConfig::default(),
            multi_tenant: MultiTenantConfig::default(),
            semantic_cache: SemanticCacheConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load config from TOML file. Falls back to defaults if file doesn't exist.
    pub fn load(path: Option<&PathBuf>) -> anyhow::Result<Self> {
        // Try explicit path first
        if let Some(p) = path {
            if p.exists() {
                let contents = std::fs::read_to_string(p)?;
                let config: AppConfig = toml::from_str(&contents)?;
                return Ok(config);
            } else {
                anyhow::bail!("Config file not found: {}", p.display());
            }
        }

        // Search default locations
        let search_paths = vec![
            PathBuf::from("coalesce.toml"),
            PathBuf::from("config.toml"),
            dirs::config_dir()
                .unwrap_or_default()
                .join("coalesce")
                .join("config.toml"),
        ];

        for p in &search_paths {
            if p.exists() {
                let contents = std::fs::read_to_string(p)?;
                let config: AppConfig = toml::from_str(&contents)?;
                tracing::info!("Loaded config from {}", p.display());
                return Ok(config);
            }
        }

        // No config file found — use defaults
        tracing::info!("No config file found, using defaults");
        Ok(AppConfig::default())
    }

    /// Save the current config to a TOML file
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    /// Write a default config file as a template
    pub fn write_default(path: &PathBuf) -> anyhow::Result<()> {
        let template = r#"# Coalesce Configuration

[server]
port = 8402
host = "127.0.0.1"

[logging]
level = "info"
format = "pretty"  # "pretty" or "json"

# --- Providers ---
# Uncomment and configure the providers you want to use.
# Billing types: local, free, unlimited, per_token,
#   quota_monthly:N, quota_refreshing:N:secs, free_credits:N

# [providers.ollama]
# enabled = true
# endpoint = "http://localhost:11434"
# billing = "local"

# [providers.openrouter]
# enabled = true
# api_key_env = "OPENROUTER_API_KEY"
# billing = "per_token"

# [providers.copilot]
# enabled = true
# api_key_env = "GITHUB_TOKEN"
# billing = "quota_refreshing:50:18000"

# [providers.kimi]
# enabled = true
# api_key_env = "KIMI_API_KEY"
# billing = "unlimited"

# [providers.glm]
# enabled = true
# api_key_env = "GLM_API_KEY"
# billing = "per_token"

# [providers.deepseek]
# enabled = true
# api_key_env = "DEEPSEEK_API_KEY"
# billing = "per_token"

# [providers.openai]
# enabled = true
# api_key_env = "OPENAI_API_KEY"
# billing = "per_token"

# [providers.google]
# enabled = true
# api_key_env = "GOOGLE_API_KEY"
# billing = "free_credits:50.0"

# [providers.xai]
# enabled = true
# api_key_env = "XAI_API_KEY"
# billing = "per_token"

# --- Routing ---
# [routing.weights]
# token_count = 0.15
# code_presence = 0.12
# reasoning_markers = 0.15
# technical_terms = 0.08

# [routing.thresholds]
# simple_max = 0.12
# medium_max = 0.25
# complex_max = 0.40
"#;
        std::fs::write(path, template)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_host")]
    pub host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub endpoint: Option<String>,
    pub billing: Option<String>,
    /// Provider priority for routing (lower = tried first). Default 50.
    /// Use 1-10 for "use first", 50 for default, 90+ for "last resort".
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// Pricing mode: "subscription" = treat as $0/token (use until quota exhausted),
    /// "metered" = use actual per-token pricing for cost comparison.
    /// Subscription providers are always preferred over metered ones at the same priority.
    #[serde(default = "default_pricing_mode")]
    pub pricing_mode: String,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: None,
            api_key_env: None,
            endpoint: None,
            billing: None,
            priority: 50,
            pricing_mode: "metered".to_string(),
        }
    }
}

fn default_priority() -> u32 {
    50
}

fn default_pricing_mode() -> String {
    "metered".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Total spending limit in USD (0.0 = unlimited)
    #[serde(default)]
    pub total_limit_usd: f64,
    /// Daily spending limit in USD (0.0 = unlimited)
    #[serde(default)]
    pub daily_limit_usd: f64,
    /// Alert thresholds as percentages (e.g., [50, 80, 90, 100])
    #[serde(default = "default_alert_thresholds")]
    pub alert_thresholds: Vec<u32>,
    /// Webhook URL to POST alerts to
    #[serde(default)]
    pub alert_webhook: Option<String>,
    /// Shell command to run on alert (receives JSON on stdin)
    #[serde(default)]
    pub alert_command: Option<String>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            total_limit_usd: 0.0,
            daily_limit_usd: 0.0,
            alert_thresholds: default_alert_thresholds(),
            alert_webhook: None,
            alert_command: None,
        }
    }
}

fn default_alert_thresholds() -> Vec<u32> {
    vec![50, 80, 90, 100]
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
        }
    }
}

fn default_server() -> ServerConfig {
    ServerConfig {
        port: 8402,
        host: "127.0.0.1".to_string(),
    }
}
fn default_port() -> u16 {
    8402
}
fn default_host() -> String {
    "127.0.0.1".to_string()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanitizeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_injection_sensitivity")]
    pub injection_sensitivity: String,
    #[serde(default = "default_max_request_size")]
    pub max_request_size_bytes: usize,
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,
}

impl Default for SanitizeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            injection_sensitivity: default_injection_sensitivity(),
            max_request_size_bytes: default_max_request_size(),
            max_messages: default_max_messages(),
        }
    }
}

fn default_injection_sensitivity() -> String {
    "medium".to_string()
}
fn default_max_request_size() -> usize {
    1_048_576
}
fn default_max_messages() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTenantConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_tenant")]
    pub default_tenant: String,
    #[serde(default)]
    pub tenants: HashMap<String, TenantConfig>,
}

impl Default for MultiTenantConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_tenant: default_tenant(),
            tenants: HashMap::new(),
        }
    }
}

fn default_tenant() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantConfig {
    /// Override budget limits for this tenant
    #[serde(default)]
    pub budget: Option<BudgetConfig>,
    /// Override allowed providers for this tenant
    #[serde(default)]
    pub allowed_providers: Option<Vec<String>>,
    /// Override routing profile for this tenant
    #[serde(default)]
    pub routing_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,
    #[serde(default = "default_max_cache_entries")]
    pub max_entries: usize,
    #[serde(default = "default_cache_ttl")]
    pub ttl_secs: u64,
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            similarity_threshold: default_similarity_threshold(),
            max_entries: default_max_cache_entries(),
            ttl_secs: default_cache_ttl(),
        }
    }
}

fn default_similarity_threshold() -> f64 {
    0.95
}
fn default_max_cache_entries() -> usize {
    1000
}
fn default_cache_ttl() -> u64 {
    3600
}

fn default_true() -> bool {
    true
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_format() -> String {
    "pretty".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.server.port, 8402);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
            [server]
            port = 9000
            host = "0.0.0.0"

            [providers.openrouter]
            enabled = true
            api_key = "sk-test"
            billing = "per_token"

            [providers.kimi]
            enabled = true
            api_key_env = "KIMI_KEY"
            billing = "unlimited"

            [logging]
            level = "debug"
            format = "json"
        "#;

        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.providers.len(), 2);
        assert_eq!(
            config.providers["openrouter"].api_key.as_deref(),
            Some("sk-test")
        );
        assert_eq!(
            config.providers["kimi"].billing.as_deref(),
            Some("unlimited")
        );
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.format, "json");
    }
}
