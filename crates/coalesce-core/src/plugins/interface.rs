use serde::{Deserialize, Serialize};

/// Result from a plugin hook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginAction {
    /// Continue with (possibly modified) data
    Continue(serde_json::Value),
    /// Block the request with an error message
    Block(String),
    /// Skip this plugin (no-op)
    Skip,
}

/// Plugin metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    /// Which hooks this plugin implements
    pub hooks: Vec<PluginHook>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHook {
    OnRequest,
    OnRoute,
    OnResponse,
}

/// Plugin configuration from TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    pub enabled: bool,
    pub path: std::path::PathBuf,
    #[serde(default)]
    pub config: serde_json::Value,
}
