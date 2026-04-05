//! # Coalesce Plugin SDK
//!
//! Build WASM plugins for the [Coalesce](https://github.com/MichaelDanCurtis/Coalesce) LLM proxy.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use coalesce_plugin_sdk::*;
//!
//! struct MyPlugin;
//!
//! impl CoalescePlugin for MyPlugin {
//!     fn manifest() -> PluginManifest {
//!         PluginManifest {
//!             name: "my-plugin".into(),
//!             version: "1.0.0".into(),
//!             description: "Does something cool".into(),
//!             hooks: vec![PluginHook::OnRequest],
//!         }
//!     }
//!
//!     fn on_request(request: &serde_json::Value) -> PluginAction {
//!         PluginAction::Continue(request.clone())
//!     }
//! }
//!
//! coalesce_plugin!(MyPlugin);
//! ```
//!
//! ## Building
//!
//! Compile your plugin to a WASM module:
//!
//! ```bash
//! rustup target add wasm32-wasip1
//! cargo build --target wasm32-wasip1 --release
//! ```
//!
//! The resulting `.wasm` file can be loaded by the Coalesce proxy.

mod guest;
mod types;

pub use guest::*;
pub use types::*;

// Re-export serde_json so the macro can reference it without requiring
// plugin authors to add it as a direct dependency.
pub use serde_json;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_serialization() {
        let m = PluginManifest {
            name: "test".into(),
            version: "1.0".into(),
            description: "desc".into(),
            hooks: vec![PluginHook::OnRequest, PluginHook::OnResponse],
        };
        let json = serde_json::to_string(&m).unwrap();
        let m2: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m2.name, "test");
        assert_eq!(m2.hooks.len(), 2);
    }

    #[test]
    fn test_action_serialization_roundtrip() {
        let actions = vec![
            PluginAction::Continue(serde_json::json!({"key": "val"})),
            PluginAction::Block("reason".into()),
            PluginAction::Skip,
        ];
        for action in actions {
            let json = serde_json::to_string(&action).unwrap();
            let parsed: PluginAction = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn test_hook_serde() {
        assert_eq!(
            serde_json::to_string(&PluginHook::OnRequest).unwrap(),
            "\"on_request\""
        );
        assert_eq!(
            serde_json::to_string(&PluginHook::OnRoute).unwrap(),
            "\"on_route\""
        );
        assert_eq!(
            serde_json::to_string(&PluginHook::OnResponse).unwrap(),
            "\"on_response\""
        );
    }

    #[test]
    fn test_action_externally_tagged_format() {
        // Verify the JSON format matches what the host runtime expects
        // (serde default externally-tagged representation).
        let continue_action = PluginAction::Continue(serde_json::json!({"model": "gpt-4"}));
        let json = serde_json::to_string(&continue_action).unwrap();
        assert!(json.starts_with("{\"Continue\":"));

        let block_action = PluginAction::Block("denied".into());
        let json = serde_json::to_string(&block_action).unwrap();
        assert_eq!(json, "{\"Block\":\"denied\"}");

        let skip_action = PluginAction::Skip;
        let json = serde_json::to_string(&skip_action).unwrap();
        assert_eq!(json, "\"Skip\"");
    }

    #[test]
    fn test_to_host_string_nul_terminated() {
        let s = "hello";
        let ptr = to_host_string(s);
        // Read back: 5 bytes of "hello" + 1 NUL
        let slice = unsafe { std::slice::from_raw_parts(ptr, 6) };
        assert_eq!(&slice[..5], b"hello");
        assert_eq!(slice[5], 0);
    }

    #[test]
    fn test_from_host_ptr() {
        let data = b"{\"key\":\"value\"}";
        let val = unsafe { from_host_ptr(data.as_ptr(), data.len()) };
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn test_from_host_ptr_invalid_json() {
        let data = b"not json";
        let val = unsafe { from_host_ptr(data.as_ptr(), data.len()) };
        assert!(val.is_null());
    }
}
