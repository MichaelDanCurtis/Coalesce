use coalesce_plugin_sdk::*;

struct ProfanityFilter;

const BLOCKED_TERMS: &[&str] = &["hack", "exploit", "inject"];

impl CoalescePlugin for ProfanityFilter {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "profanity-filter".into(),
            version: "1.0.0".into(),
            description: "Blocks requests containing prohibited terms".into(),
            hooks: vec![PluginHook::OnRequest],
        }
    }

    fn on_request(request: &serde_json::Value) -> PluginAction {
        let text = request.to_string().to_lowercase();
        for term in BLOCKED_TERMS {
            if text.contains(term) {
                return PluginAction::Block(format!(
                    "Content blocked: prohibited term '{term}'"
                ));
            }
        }
        PluginAction::Continue(request.clone())
    }
}

coalesce_plugin!(ProfanityFilter);
