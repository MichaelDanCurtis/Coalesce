use super::host::Plugin;
use super::interface::PluginConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// Discover plugin configs from the default plugin directory.
///
/// Scans for `.wasm` files in the plugin directory. Returns configs for
/// discovery purposes; actual WASM loading happens via [`load_all_plugins`].
pub fn discover_plugins() -> Vec<PluginConfig> {
    discover_plugins_in(&default_plugin_dir())
}

/// Discover plugin configs from a specific directory.
pub fn discover_plugins_in(plugin_dir: &Path) -> Vec<PluginConfig> {
    if !plugin_dir.exists() {
        return Vec::new();
    }

    info!("Scanning for plugins in {}", plugin_dir.display());

    let mut configs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(plugin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "wasm") {
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

/// Load a WASM plugin from the given path.
///
/// Only available when the `wasm` feature is enabled.
#[cfg(feature = "wasm")]
pub fn load_wasm_plugin(
    path: &Path,
) -> Result<super::wasm_runtime::WasmPlugin, Box<dyn std::error::Error>> {
    super::wasm_runtime::WasmPlugin::load(path)
}

/// Discover and load all plugins from a directory.
///
/// This loads both built-in plugins and any discovered WASM plugins.
/// Built-in plugins are always registered first in a fixed order:
/// 1. `RequestLoggerPlugin`
/// 2. `ContentFilterPlugin` (no terms by default)
/// 3. `RateLimiterPlugin` (60 req/min default)
///
/// WASM plugins are loaded next. Any WASM file that fails to load is
/// logged as a warning and skipped.
pub fn load_all_plugins(plugin_dir: &Path) -> Vec<Arc<dyn Plugin>> {
    let mut plugins: Vec<Arc<dyn Plugin>> = Vec::new();

    // Register built-in plugins.
    plugins.push(Arc::new(
        super::builtins::RequestLoggerPlugin::new(),
    ));
    plugins.push(Arc::new(
        super::builtins::ContentFilterPlugin::new(Vec::new()),
    ));
    plugins.push(Arc::new(super::builtins::RateLimiterPlugin::new(60)));

    // Discover and load WASM plugins.
    let configs = discover_plugins_in(plugin_dir);
    for config in &configs {
        if !config.enabled {
            continue;
        }
        info!("Loading WASM plugin: {}", config.path.display());
        #[cfg(feature = "wasm")]
        {
            match load_wasm_plugin(&config.path) {
                Ok(p) => plugins.push(Arc::new(p)),
                Err(e) => warn!(
                    "Failed to load WASM plugin {}: {e}",
                    config.path.display()
                ),
            }
        }
        #[cfg(not(feature = "wasm"))]
        {
            warn!(
                "WASM plugin {} found but `wasm` feature not enabled, skipping",
                config.path.display()
            );
        }
    }

    plugins
}

/// Returns the default plugin directory path.
pub fn default_plugin_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("coalesce")
        .join("plugins")
}
