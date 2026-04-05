use coalesce_plugin_sdk::*;

struct HelloPlugin;

impl CoalescePlugin for HelloPlugin {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "hello-plugin".into(),
            version: "1.0.0".into(),
            description: "Adds a greeting header to all requests".into(),
            hooks: vec![PluginHook::OnRequest],
        }
    }

    fn on_request(request: &serde_json::Value) -> PluginAction {
        let mut modified = request.clone();
        if let Some(obj) = modified.as_object_mut() {
            obj.insert(
                "x-plugin-hello".into(),
                serde_json::json!("Hello from plugin!"),
            );
        }
        PluginAction::Continue(modified)
    }
}

coalesce_plugin!(HelloPlugin);
