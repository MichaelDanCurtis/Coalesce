use super::interface::{PluginAction, PluginHook, PluginManifest};
use std::sync::Arc;

/// Trait that all plugins implement.
///
/// Currently supports native Rust plugins. The trait-based design allows
/// swapping in WASM-backed implementations later via wasmtime without
/// changing the host or consumer code.
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;

    fn on_request(&self, _request: &serde_json::Value) -> PluginAction {
        PluginAction::Skip
    }

    fn on_route(&self, _decision: &serde_json::Value) -> PluginAction {
        PluginAction::Skip
    }

    fn on_response(&self, _response: &serde_json::Value) -> PluginAction {
        PluginAction::Skip
    }
}

/// Plugin host manages loaded plugins and executes hooks in registration order.
pub struct PluginHost {
    plugins: Vec<Arc<dyn Plugin>>,
}

impl PluginHost {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Arc<dyn Plugin>) {
        self.plugins.push(plugin);
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Run all `on_request` hooks. Returns modified request or Block error.
    pub fn run_on_request(&self, request: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut current = request.clone();
        for plugin in &self.plugins {
            if plugin.manifest().hooks.contains(&PluginHook::OnRequest) {
                match plugin.on_request(&current) {
                    PluginAction::Continue(modified) => current = modified,
                    PluginAction::Block(reason) => {
                        return Err(format!("[{}] {}", plugin.manifest().name, reason));
                    }
                    PluginAction::Skip => {}
                }
            }
        }
        Ok(current)
    }

    /// Run all `on_route` hooks.
    pub fn run_on_route(&self, decision: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut current = decision.clone();
        for plugin in &self.plugins {
            if plugin.manifest().hooks.contains(&PluginHook::OnRoute) {
                match plugin.on_route(&current) {
                    PluginAction::Continue(modified) => current = modified,
                    PluginAction::Block(reason) => {
                        return Err(format!("[{}] {}", plugin.manifest().name, reason));
                    }
                    PluginAction::Skip => {}
                }
            }
        }
        Ok(current)
    }

    /// Run all `on_response` hooks.
    pub fn run_on_response(
        &self,
        response: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let mut current = response.clone();
        for plugin in &self.plugins {
            if plugin.manifest().hooks.contains(&PluginHook::OnResponse) {
                match plugin.on_response(&current) {
                    PluginAction::Continue(modified) => current = modified,
                    PluginAction::Block(reason) => {
                        return Err(format!("[{}] {}", plugin.manifest().name, reason));
                    }
                    PluginAction::Skip => {}
                }
            }
        }
        Ok(current)
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::interface::*;

    struct LogPlugin;
    impl Plugin for LogPlugin {
        fn manifest(&self) -> &PluginManifest {
            Box::leak(Box::new(PluginManifest {
                name: "logger".into(),
                version: "1.0.0".into(),
                description: "Logs requests".into(),
                hooks: vec![PluginHook::OnRequest, PluginHook::OnResponse],
            }))
        }
        fn on_request(&self, req: &serde_json::Value) -> PluginAction {
            PluginAction::Continue(req.clone())
        }
    }

    struct BlockPlugin;
    impl Plugin for BlockPlugin {
        fn manifest(&self) -> &PluginManifest {
            Box::leak(Box::new(PluginManifest {
                name: "blocker".into(),
                version: "1.0.0".into(),
                description: "Blocks everything".into(),
                hooks: vec![PluginHook::OnRequest],
            }))
        }
        fn on_request(&self, _req: &serde_json::Value) -> PluginAction {
            PluginAction::Block("blocked by policy".into())
        }
    }

    struct ModifyPlugin;
    impl Plugin for ModifyPlugin {
        fn manifest(&self) -> &PluginManifest {
            Box::leak(Box::new(PluginManifest {
                name: "modifier".into(),
                version: "1.0.0".into(),
                description: "Modifies requests".into(),
                hooks: vec![PluginHook::OnRequest],
            }))
        }
        fn on_request(&self, req: &serde_json::Value) -> PluginAction {
            let mut modified = req.clone();
            if let Some(obj) = modified.as_object_mut() {
                obj.insert("plugin_modified".into(), serde_json::json!(true));
            }
            PluginAction::Continue(modified)
        }
    }

    #[test]
    fn test_empty_host_passthrough() {
        let host = PluginHost::new();
        let req = serde_json::json!({"model": "test"});
        let result = host.run_on_request(&req).unwrap();
        assert_eq!(result, req);
    }

    #[test]
    fn test_plugin_passthrough() {
        let mut host = PluginHost::new();
        host.register(Arc::new(LogPlugin));
        let req = serde_json::json!({"model": "test"});
        let result = host.run_on_request(&req).unwrap();
        assert_eq!(result, req);
    }

    #[test]
    fn test_plugin_blocks() {
        let mut host = PluginHost::new();
        host.register(Arc::new(BlockPlugin));
        let req = serde_json::json!({"model": "test"});
        let result = host.run_on_request(&req);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked by policy"));
    }

    #[test]
    fn test_plugin_modifies() {
        let mut host = PluginHost::new();
        host.register(Arc::new(ModifyPlugin));
        let req = serde_json::json!({"model": "test"});
        let result = host.run_on_request(&req).unwrap();
        assert_eq!(result["plugin_modified"], true);
    }

    #[test]
    fn test_plugin_chain_order() {
        let mut host = PluginHost::new();
        host.register(Arc::new(ModifyPlugin));
        host.register(Arc::new(LogPlugin));
        let req = serde_json::json!({"model": "test"});
        let result = host.run_on_request(&req).unwrap();
        assert_eq!(result["plugin_modified"], true);
    }

    #[test]
    fn test_block_stops_chain() {
        let mut host = PluginHost::new();
        host.register(Arc::new(BlockPlugin));
        host.register(Arc::new(ModifyPlugin)); // should never run
        let req = serde_json::json!({"model": "test"});
        assert!(host.run_on_request(&req).is_err());
    }

    #[test]
    fn test_plugin_count() {
        let mut host = PluginHost::new();
        assert_eq!(host.plugin_count(), 0);
        host.register(Arc::new(LogPlugin));
        assert_eq!(host.plugin_count(), 1);
    }

    #[test]
    fn test_hooks_only_run_registered() {
        let mut host = PluginHost::new();
        host.register(Arc::new(LogPlugin)); // only has OnRequest + OnResponse
        let route = serde_json::json!({"tier": "SIMPLE"});
        // on_route should pass through unchanged since LogPlugin doesn't register OnRoute
        let result = host.run_on_route(&route).unwrap();
        assert_eq!(result, route);
    }
}
