//! WASM plugin runtime backed by wasmtime.
//!
//! A WASM module must export the following functions to be loaded as a plugin:
//!
//! - `manifest() -> i32` - returns a pointer to a NUL-terminated UTF-8 JSON string
//!   describing the plugin (deserialized as [`PluginManifest`]).
//! - `alloc(len: i32) -> i32` - allocate `len` bytes in the WASM linear memory.
//! - `dealloc(ptr: i32, len: i32)` - free a previous allocation.
//!
//! Optional hook exports (missing hooks default to [`PluginAction::Skip`]):
//!
//! - `on_request(ptr: i32, len: i32) -> i32` - receive a JSON request, return
//!   a pointer to a NUL-terminated JSON [`PluginAction`].
//! - `on_route(ptr: i32, len: i32) -> i32`
//! - `on_response(ptr: i32, len: i32) -> i32`

use super::host::Plugin;
use super::interface::{PluginAction, PluginHook, PluginManifest};
use std::sync::Mutex;
use wasmtime::*;

/// A WASM-backed plugin loaded via wasmtime.
#[allow(missing_debug_implementations)]
pub struct WasmPlugin {
    store: Mutex<Store<()>>,
    instance: Instance,
    manifest_cache: PluginManifest,
}

// Safety: Store is guarded by Mutex so only one thread accesses it at a time.
// Instance is safe to share across threads when Store access is serialized.
unsafe impl Send for WasmPlugin {}
unsafe impl Sync for WasmPlugin {}

impl WasmPlugin {
    /// Load and instantiate a WASM plugin from the given file path.
    ///
    /// The module is compiled, instantiated, and its `manifest()` export is
    /// called immediately to cache the plugin metadata.
    pub fn load(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let engine = Engine::default();
        let module = Module::from_file(&engine, path)?;
        let mut store = Store::new(&engine, ());
        let linker = Linker::new(&engine);
        let instance = linker.instantiate(&mut store, &module)?;

        // Read the manifest to cache it.
        let manifest_cache = Self::call_manifest(&mut store, &instance)?;

        Ok(Self {
            store: Mutex::new(store),
            instance,
            manifest_cache,
        })
    }

    /// Call the `manifest()` export and parse the returned JSON.
    fn call_manifest(
        store: &mut Store<()>,
        instance: &Instance,
    ) -> Result<PluginManifest, Box<dyn std::error::Error>> {
        let manifest_fn = instance
            .get_typed_func::<(), i32>(&mut *store, "manifest")
            .map_err(|e| format!("WASM module missing `manifest` export: {e}"))?;

        let ptr = manifest_fn.call(&mut *store, ())?;

        let memory = instance
            .get_memory(&mut *store, "memory")
            .ok_or("WASM module missing `memory` export")?;

        let json_str = read_cstr(memory, &*store, ptr as usize)?;
        let manifest: PluginManifest = serde_json::from_str(&json_str)?;
        Ok(manifest)
    }

    /// Call a hook function, passing JSON data in and reading JSON data out.
    /// Returns `None` if the export does not exist.
    fn call_hook(&self, hook_name: &str, data: &serde_json::Value) -> Option<PluginAction> {
        let mut store = self.store.lock().ok()?;

        let hook_fn = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut *store, hook_name)
            .ok()?;

        let alloc_fn = self
            .instance
            .get_typed_func::<i32, i32>(&mut *store, "alloc")
            .ok()?;

        let memory = self.instance.get_memory(&mut *store, "memory")?;

        let json_bytes = serde_json::to_vec(data).ok()?;
        let len = json_bytes.len() as i32;

        // Allocate space in WASM memory and write the JSON bytes.
        let wasm_ptr = alloc_fn.call(&mut *store, len).ok()?;
        memory
            .write(&mut *store, wasm_ptr as usize, &json_bytes)
            .ok()?;

        // Call the hook.
        let result_ptr = hook_fn.call(&mut *store, (wasm_ptr, len)).ok()?;

        // Read result as NUL-terminated string.
        let result_str = read_cstr(memory, &*store, result_ptr as usize).ok()?;

        // Try to deallocate the input buffer.
        if let Ok(dealloc_fn) = self
            .instance
            .get_typed_func::<(i32, i32), ()>(&mut *store, "dealloc")
        {
            let _ = dealloc_fn.call(&mut *store, (wasm_ptr, len));
        }

        serde_json::from_str(&result_str).ok()
    }
}

impl Plugin for WasmPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest_cache
    }

    fn on_request(&self, request: &serde_json::Value) -> PluginAction {
        self.call_hook("on_request", request)
            .unwrap_or(PluginAction::Skip)
    }

    fn on_route(&self, decision: &serde_json::Value) -> PluginAction {
        self.call_hook("on_route", decision)
            .unwrap_or(PluginAction::Skip)
    }

    fn on_response(&self, response: &serde_json::Value) -> PluginAction {
        self.call_hook("on_response", response)
            .unwrap_or(PluginAction::Skip)
    }
}

/// Read a NUL-terminated UTF-8 string from WASM linear memory starting at `ptr`.
fn read_cstr(
    memory: Memory,
    store: impl AsContext,
    ptr: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let data = memory.data(&store);
    if ptr >= data.len() {
        return Err("pointer out of bounds".into());
    }
    let slice = &data[ptr..];
    let end = slice
        .iter()
        .position(|&b| b == 0)
        .ok_or("no NUL terminator found")?;
    let s = std::str::from_utf8(&slice[..end])?;
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_load_nonexistent_file() {
        let result = WasmPlugin::load(&PathBuf::from("/tmp/nonexistent_plugin.wasm"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_invalid_wasm() {
        // Write a file that is not valid WASM.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.wasm");
        std::fs::write(&path, b"this is not wasm").unwrap();
        let result = WasmPlugin::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_valid_wasm_missing_manifest() {
        // Minimal valid WASM module (empty, no exports).
        // WAT: (module)
        let wasm_bytes: &[u8] = &[
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wasm");
        std::fs::write(&path, wasm_bytes).unwrap();
        let result = WasmPlugin::load(&path);
        // Should fail because there's no `manifest` export.
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().unwrap());
        assert!(
            err_msg.contains("manifest"),
            "Error should mention missing manifest export, got: {err_msg}"
        );
    }
}
