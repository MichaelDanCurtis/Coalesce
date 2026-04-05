use serde::{Deserialize, Serialize};

/// Action returned by plugin hooks.
///
/// Uses serde's default externally-tagged representation to match the host
/// runtime. For example, `Continue(val)` serializes as `{"Continue": val}`,
/// `Block(s)` as `{"Block": "..."}`, and `Skip` as `"Skip"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginAction {
    /// Continue processing with (possibly modified) data.
    Continue(serde_json::Value),
    /// Block the request with an error message.
    Block(String),
    /// Skip -- no modification.
    Skip,
}

/// Plugin metadata returned by the `manifest()` export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin name (e.g. `"my-plugin"`).
    pub name: String,
    /// Semver version string.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Which hooks this plugin implements.
    pub hooks: Vec<PluginHook>,
}

/// Available hook points in the Coalesce proxy pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHook {
    /// Called before a request is routed to a provider.
    OnRequest,
    /// Called after the routing decision is made.
    OnRoute,
    /// Called after a response is received from the provider.
    OnResponse,
}
