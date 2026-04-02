use super::interface::PluginConfig;
use std::path::PathBuf;
use tracing::info;

/// Discover plugin configs from the default plugin directory.
///
/// Scans for `.wasm` files in the plugin directory. Currently returns
/// configs for discovery purposes; actual WASM loading will be added
/// when wasmtime is integrated.
pub fn discover_plugins() -> Vec<PluginConfig> {
    let plugin_dir = default_plugin_dir();
    if !plugin_dir.exists() {
        return Vec::new();
    }

    info!("Scanning for plugins in {}", plugin_dir.display());

    let mut configs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "wasm") {
                info!("Found plugin: {}", path.display());
                configs.push(PluginConfig {
                    enabled: true,
                    path,
                    config: serde_json::Value::Null,
                });
            }
        }
    }

    configs
}

/// Returns the default plugin directory path.
pub fn default_plugin_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("coalesce")
        .join("plugins")
}
