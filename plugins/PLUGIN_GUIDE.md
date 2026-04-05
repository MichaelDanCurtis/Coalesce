# Coalesce Plugin Development Guide

Build WASM plugins that hook into the Coalesce LLM proxy pipeline.

## Overview

Coalesce plugins are WebAssembly modules that intercept requests, routing decisions, and responses as they flow through the proxy. Plugins run in a sandboxed wasmtime runtime — they can inspect and modify data but cannot access the filesystem, network, or host memory directly.

### Hook Points

| Hook | When it runs | Input | Typical use |
|------|-------------|-------|-------------|
| `on_request` | Before routing | Chat completion request JSON | Validation, filtering, injection |
| `on_route` | After routing decision | Routing decision JSON | Override provider/model selection |
| `on_response` | After provider responds | Provider response JSON | Transform, log, or block responses |

### Plugin Actions

Each hook returns a `PluginAction`:

- **`Continue(value)`** — pass the (possibly modified) data to the next plugin in the chain
- **`Block(reason)`** — reject the request with an error message
- **`Skip`** — do nothing, pass data through unchanged

Plugins execute in registration order. If any plugin returns `Block`, the chain stops immediately.

## Quick Start

### Prerequisites

```bash
# Install the WASM target
rustup target add wasm32-wasip1
```

### 1. Create a new plugin

```bash
cargo new --lib my-plugin
cd my-plugin
```

### 2. Configure Cargo.toml

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]   # Required: produces a .wasm file

[dependencies]
coalesce-plugin-sdk = { git = "https://github.com/MichaelDanCurtis/Coalesce", path = "crates/coalesce-plugin-sdk" }
serde_json = "1"           # Optional: only if you need json! macro
```

> **Note:** `crate-type = ["cdylib"]` is required. Without it, Cargo won't produce a `.wasm` file.

### 3. Implement the plugin

```rust
// src/lib.rs
use coalesce_plugin_sdk::*;

struct MyPlugin;

impl CoalescePlugin for MyPlugin {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "my-plugin".into(),
            version: "0.1.0".into(),
            description: "Adds custom metadata to requests".into(),
            hooks: vec![PluginHook::OnRequest],
        }
    }

    fn on_request(request: &serde_json::Value) -> PluginAction {
        let mut req = request.clone();
        if let Some(obj) = req.as_object_mut() {
            obj.insert("x-plugin".into(), serde_json::json!("my-plugin"));
        }
        PluginAction::Continue(req)
    }
}

// This macro generates all the WASM exports the host expects
coalesce_plugin!(MyPlugin);
```

### 4. Build

```bash
cargo build --target wasm32-wasip1 --release
```

Your plugin is at `target/wasm32-wasip1/release/my_plugin.wasm`.

### 5. Install

Copy the `.wasm` file to the Coalesce plugins directory:

```bash
# macOS
cp target/wasm32-wasip1/release/my_plugin.wasm \
   ~/Library/Application\ Support/coalesce/plugins/

# Linux
cp target/wasm32-wasip1/release/my_plugin.wasm \
   ~/.config/coalesce/plugins/
```

Then scan for plugins in the Coalesce dashboard (Plugins tab → Scan) or restart the proxy.

## API Reference

### `CoalescePlugin` trait

```rust
pub trait CoalescePlugin {
    /// Required: return plugin metadata.
    fn manifest() -> PluginManifest;

    /// Optional: intercept requests before routing.
    fn on_request(_request: &serde_json::Value) -> PluginAction {
        PluginAction::Skip  // default: pass through
    }

    /// Optional: intercept routing decisions.
    fn on_route(_decision: &serde_json::Value) -> PluginAction {
        PluginAction::Skip
    }

    /// Optional: intercept provider responses.
    fn on_response(_response: &serde_json::Value) -> PluginAction {
        PluginAction::Skip
    }
}
```

Only override the hooks you declared in your manifest. Hooks not listed in `manifest().hooks` are never called by the host.

### `PluginManifest`

```rust
pub struct PluginManifest {
    pub name: String,        // Unique identifier (e.g. "my-plugin")
    pub version: String,     // Semver (e.g. "1.0.0")
    pub description: String, // Human-readable summary
    pub hooks: Vec<PluginHook>,
}
```

### `PluginHook`

```rust
pub enum PluginHook {
    OnRequest,   // Serializes as "on_request"
    OnRoute,     // Serializes as "on_route"
    OnResponse,  // Serializes as "on_response"
}
```

### `PluginAction`

```rust
pub enum PluginAction {
    Continue(serde_json::Value),  // JSON: {"Continue": <value>}
    Block(String),                // JSON: {"Block": "reason"}
    Skip,                         // JSON: "Skip"
}
```

### `coalesce_plugin!` macro

```rust
coalesce_plugin!(MyPlugin);
```

Generates the WASM exports the host runtime expects: `manifest`, `on_request`, `on_route`, `on_response`, `alloc`, and `dealloc`. Call this exactly once per plugin crate, at the top level of `lib.rs`.

## Request/Response JSON Shapes

### `on_request` input

The request JSON follows the OpenAI chat completion format:

```json
{
  "model": "gpt-4",
  "messages": [
    {"role": "system", "content": "You are helpful."},
    {"role": "user", "content": "Hello!"}
  ],
  "temperature": 0.7,
  "stream": false
}
```

### `on_route` input

The routing decision:

```json
{
  "tier": "COMPLEX",
  "provider": "openai",
  "model": "gpt-4",
  "score": 0.87,
  "cost_usd": 0.003
}
```

### `on_response` input

The provider response (OpenAI format):

```json
{
  "id": "chatcmpl-abc123",
  "model": "gpt-4",
  "choices": [{
    "index": 0,
    "message": {"role": "assistant", "content": "Hi there!"},
    "finish_reason": "stop"
  }],
  "usage": {
    "prompt_tokens": 12,
    "completion_tokens": 4,
    "total_tokens": 16
  }
}
```

## Examples

### Request validator

Block requests that exceed a token estimate:

```rust
use coalesce_plugin_sdk::*;

struct TokenGuard;

impl CoalescePlugin for TokenGuard {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "token-guard".into(),
            version: "1.0.0".into(),
            description: "Blocks requests with too many messages".into(),
            hooks: vec![PluginHook::OnRequest],
        }
    }

    fn on_request(request: &serde_json::Value) -> PluginAction {
        let msg_count = request["messages"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);

        if msg_count > 100 {
            return PluginAction::Block(
                format!("Too many messages: {} (max 100)", msg_count)
            );
        }
        PluginAction::Skip
    }
}

coalesce_plugin!(TokenGuard);
```

### Response logger

Log response metadata (passthrough):

```rust
use coalesce_plugin_sdk::*;

struct ResponseLogger;

impl CoalescePlugin for ResponseLogger {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "response-logger".into(),
            version: "1.0.0".into(),
            description: "Injects logging metadata into responses".into(),
            hooks: vec![PluginHook::OnResponse],
        }
    }

    fn on_response(response: &serde_json::Value) -> PluginAction {
        let mut resp = response.clone();
        if let Some(obj) = resp.as_object_mut() {
            obj.insert("x-logged".into(), serde_json::json!(true));
            obj.insert("x-logged-at".into(), serde_json::json!("plugin:response-logger"));
        }
        PluginAction::Continue(resp)
    }
}

coalesce_plugin!(ResponseLogger);
```

### Content filter

Block requests containing prohibited content:

```rust
use coalesce_plugin_sdk::*;

struct ContentFilter;

const BLOCKED: &[&str] = &["forbidden-topic", "restricted-query"];

impl CoalescePlugin for ContentFilter {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "content-filter".into(),
            version: "1.0.0".into(),
            description: "Blocks requests with prohibited content".into(),
            hooks: vec![PluginHook::OnRequest],
        }
    }

    fn on_request(request: &serde_json::Value) -> PluginAction {
        let text = request.to_string().to_lowercase();
        for term in BLOCKED {
            if text.contains(term) {
                return PluginAction::Block(
                    format!("Blocked: contains '{}'", term)
                );
            }
        }
        PluginAction::Skip
    }
}

coalesce_plugin!(ContentFilter);
```

## Managing Plugins

### Dashboard UI

The **Plugins** tab in the Coalesce dashboard shows all discovered plugins. You can:
- **Scan** for new plugins in the plugin directory
- **Toggle** plugins on/off without removing files
- See plugin file paths and types (WASM/Native)

### REST API

```bash
# List plugins
curl http://localhost:8402/api/v1/plugins

# Re-scan plugin directory
curl -X POST http://localhost:8402/api/v1/plugins/scan

# Toggle a plugin
curl -X POST http://localhost:8402/api/v1/plugins/my-plugin/toggle
```

### Plugin directory

| OS | Path |
|----|------|
| macOS | `~/Library/Application Support/coalesce/plugins/` |
| Linux | `~/.config/coalesce/plugins/` |
| Windows | `%APPDATA%\coalesce\plugins\` |

## Troubleshooting

**Plugin not detected after copying .wasm file**
- Click "Scan" in the Plugins tab or call `POST /api/v1/plugins/scan`
- Verify the file has a `.wasm` extension

**Plugin loads but hooks don't fire**
- Check that your `manifest().hooks` includes the hook you implemented
- Hooks not listed in the manifest are never called

**Build fails with "error: cannot find macro `coalesce_plugin`"**
- Ensure `coalesce-plugin-sdk` is in your `[dependencies]`
- The macro is `#[macro_export]` so it should be available at crate root

**WASM file too large**
- Build with `--release` (debug builds are much larger)
- Add to your Cargo.toml:
  ```toml
  [profile.release]
  opt-level = "s"    # optimize for size
  lto = true
  strip = true
  ```

**Plugin panics at runtime**
- The host catches panics and treats them as `PluginAction::Skip`
- Check that your JSON parsing handles unexpected input gracefully
- Use `serde_json::from_str(...).unwrap_or(Value::Null)` for defensive parsing

## Architecture

```
Request → [on_request hooks] → Router → [on_route hooks] → Provider → [on_response hooks] → Client
              │                              │                              │
         Plugin 1                       Plugin 1                       Plugin 1
         Plugin 2                       Plugin 2                       Plugin 2
         ...                            ...                            ...
```

Plugins run synchronously in the order they were registered. The host serializes request data to JSON, writes it into WASM linear memory via `alloc`, calls the hook export, reads the returned JSON, and passes it to the next plugin. This is repeated for each hook point.

The WASM sandbox ensures plugins cannot:
- Access the host filesystem or network
- Read other plugins' memory
- Crash the proxy (panics are caught)
- Execute indefinitely (host enforces timeouts)
