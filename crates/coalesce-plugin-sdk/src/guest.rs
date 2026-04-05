//! Guest-side helpers for WASM plugin development.
//!
//! This module provides the [`CoalescePlugin`] trait that plugin authors
//! implement, memory management exports required by the host runtime, and
//! the [`coalesce_plugin!`] macro that wires everything together.

use crate::types::*;

/// Trait that plugin authors implement.
///
/// Only [`CoalescePlugin::manifest`] is required. The hook methods default to
/// [`PluginAction::Skip`] so you only need to override the ones declared in
/// your manifest.
pub trait CoalescePlugin {
    /// Return plugin metadata. Called once when the plugin is loaded.
    fn manifest() -> PluginManifest;

    /// Called before a request is routed. Return [`PluginAction::Continue`]
    /// with a modified request, [`PluginAction::Block`] to reject, or
    /// [`PluginAction::Skip`] to pass through unchanged.
    fn on_request(_request: &serde_json::Value) -> PluginAction {
        PluginAction::Skip
    }

    /// Called after the routing decision is made.
    fn on_route(_decision: &serde_json::Value) -> PluginAction {
        PluginAction::Skip
    }

    /// Called after a response is received from the provider.
    fn on_response(_response: &serde_json::Value) -> PluginAction {
        PluginAction::Skip
    }
}

/// Allocate `len` bytes in WASM linear memory. Called by the host to write
/// data into the guest before invoking a hook.
///
/// # Safety
///
/// This function is called from the host runtime via the WASM ABI.
#[no_mangle]
pub unsafe extern "C" fn alloc(len: usize) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(len, 1).unwrap();
    std::alloc::alloc(layout)
}

/// Free `len` bytes at `ptr`. Called by the host to reclaim input buffers.
///
/// # Safety
///
/// `ptr` must have been allocated by [`alloc`] with the same `len`.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    let layout = std::alloc::Layout::from_size_align(len, 1).unwrap();
    std::alloc::dealloc(ptr, layout);
}

/// Serialize a string to a NUL-terminated byte sequence in WASM memory and
/// return its pointer. The host reads until the NUL terminator.
pub fn to_host_string(s: &str) -> *const u8 {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0); // NUL terminator
    let ptr = bytes.as_ptr();
    std::mem::forget(bytes); // host will call dealloc
    ptr
}

/// Read a JSON value from a host-provided `(ptr, len)` pair.
///
/// # Safety
///
/// `ptr` must point to valid UTF-8 of exactly `len` bytes.
pub unsafe fn from_host_ptr(ptr: *const u8, len: usize) -> serde_json::Value {
    let slice = std::slice::from_raw_parts(ptr, len);
    let s = std::str::from_utf8_unchecked(slice);
    serde_json::from_str(s).unwrap_or(serde_json::Value::Null)
}

/// Generate all required WASM exports for a plugin.
///
/// Usage:
/// ```rust,ignore
/// coalesce_plugin!(MyPlugin);
/// ```
///
/// This generates the `manifest`, `on_request`, `on_route`, and `on_response`
/// exports that the Coalesce host runtime expects.
#[macro_export]
macro_rules! coalesce_plugin {
    ($plugin_type:ty) => {
        #[no_mangle]
        pub extern "C" fn manifest() -> *const u8 {
            let m = <$plugin_type as $crate::CoalescePlugin>::manifest();
            let json = $crate::serde_json::to_string(&m).unwrap();
            $crate::to_host_string(&json)
        }

        #[no_mangle]
        pub unsafe extern "C" fn on_request(ptr: *const u8, len: usize) -> *const u8 {
            let input = $crate::from_host_ptr(ptr, len);
            let action = <$plugin_type as $crate::CoalescePlugin>::on_request(&input);
            let json = $crate::serde_json::to_string(&action).unwrap();
            $crate::to_host_string(&json)
        }

        #[no_mangle]
        pub unsafe extern "C" fn on_route(ptr: *const u8, len: usize) -> *const u8 {
            let input = $crate::from_host_ptr(ptr, len);
            let action = <$plugin_type as $crate::CoalescePlugin>::on_route(&input);
            let json = $crate::serde_json::to_string(&action).unwrap();
            $crate::to_host_string(&json)
        }

        #[no_mangle]
        pub unsafe extern "C" fn on_response(ptr: *const u8, len: usize) -> *const u8 {
            let input = $crate::from_host_ptr(ptr, len);
            let action = <$plugin_type as $crate::CoalescePlugin>::on_response(&input);
            let json = $crate::serde_json::to_string(&action).unwrap();
            $crate::to_host_string(&json)
        }
    };
}
