# Coalesce

Smart LLM routing proxy that scores requests across 15 dimensions and routes them to the cheapest capable provider -- prioritizing free and included credits before paid options.

<!-- Badges -->
![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)
![License](https://img.shields.io/badge/license-MIT-blue)
![Tests](https://img.shields.io/badge/tests-74%20passing-green)

---

## Features

- **15-dimension request scoring** -- classifies prompts into Simple, Medium, Complex, or Reasoning tiers
- **10 LLM providers** -- GitHub Copilot, OpenRouter, Ollama, GLM/Zhipu, Kimi/Moonshot, Anthropic (via OpenRouter), Google AI, OpenAI, xAI/Grok, DeepSeek
- **Real-time marginal cost engine** -- computes effective cost per request across all available models
- **Free-first priority cascade** -- local/free/quota models before paid, with configurable wait-vs-pay thresholds
- **Circuit breakers** -- automatic provider isolation on repeated failures, with half-open recovery
- **Request deduplication** -- identical in-flight requests share a single upstream call
- **Fallback chains** -- up to 3 automatic retries across different providers
- **SSE streaming** -- full streaming support through the proxy
- **SQLite storage** -- persistent request logging, stats, and cost tracking
- **Budget tracking** -- configurable total and daily spending limits
- **Plugin system** -- trait-based hooks (on_request, on_route, on_response), WASM-ready architecture
- **Prompt injection detection** -- configurable sensitivity (off/low/medium/high)
- **Web dashboard** -- embedded single-page app with routing playground, stats, and model browser
- **gRPC transport** -- protobuf-encoded API on port 8403 for high-frequency agent communication
- **Load testing** -- built-in `bench` command with percentile latency reporting
- **OpenAI-compatible API** -- drop-in replacement at `/v1/chat/completions`

## Quick Start

### Install

```bash
# Clone and build
git clone https://github.com/MichaelDanCurtis/Coalesce.git
cd Coalesce
cargo build --release

# Binary is at target/release/agentpather
```

### Configure

```bash
# Generate a config template
./target/release/agentpather init

# Edit agentpather.toml -- enable providers, add API keys
$EDITOR agentpather.toml
```

### Run

```bash
# Start the proxy (default: http://127.0.0.1:8402)
./target/release/agentpather serve

# Or just run directly (serve is the default command)
./target/release/agentpather
```

### Use

Point any OpenAI-compatible client at the proxy:

```bash
curl http://127.0.0.1:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "auto",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

The response includes routing metadata in the `x_coalesce` field:

```json
{
  "x_coalesce": {
    "tier": "Simple",
    "score": 0.05,
    "provider": "ollama",
    "model": "llama3.2:latest",
    "attempt": 1
  }
}
```

## Architecture

```
                         Clients
                    (curl, SDKs, agents)
                           |
              +------------+------------+
              |                         |
        HTTP :8402                gRPC :8403
              |                         |
    +---------+---------+     +---------+---------+
    |   Axum REST API   |     |   Tonic gRPC API  |
    +-------------------+     +-------------------+
              |                         |
              +------------+------------+
                           |
                   +-------+-------+
                   |    Router     |
                   | 15-dimension  |
                   |   scorer      |
                   +-------+-------+
                           |
                   +-------+-------+
                   |   Economics   |
                   |    Engine     |
                   | marginal cost |
                   |  + quotas     |
                   +-------+-------+
                           |
            +--------------+--------------+
            |              |              |
     +------+------+ +----+----+ +-------+-------+
     | Free/Local  | |  Quota  | |     Paid      |
     | Ollama      | | Copilot | | OpenRouter    |
     |             | | Kimi    | | OpenAI, xAI   |
     |             | | GLM     | | Google, Deep- |
     |             | |         | | Seek          |
     +------+------+ +----+----+ +-------+-------+
            |              |              |
     +------+--------------+--------------+------+
     |         Circuit Breakers + Dedup          |
     +-------------------+----------------------+
                         |
                   +-----+-----+
                   |  SQLite   |
                   |  Storage  |
                   +-----------+
```

## Providers

| Provider       | Env Variable          | Billing Type          | Notes                               |
|----------------|-----------------------|-----------------------|-------------------------------------|
| Ollama         | --                    | `local`               | Auto-detected on localhost:11434    |
| GitHub Copilot | `GITHUB_TOKEN`        | `quota_refreshing`    | Copilot subscription credits        |
| OpenRouter     | `OPENROUTER_API_KEY`  | `per_token`           | Access to 100+ models               |
| GLM / Zhipu    | `GLM_API_KEY`         | `per_token`           | Chinese LLM provider                |
| Kimi / Moonshot| `KIMI_API_KEY`        | `unlimited`           | Long-context specialist             |
| DeepSeek       | `DEEPSEEK_API_KEY`    | `per_token`           | Reasoning and code models           |
| OpenAI         | `OPENAI_API_KEY`      | `per_token`           | GPT-4o, GPT-4, etc.                |
| Google AI      | `GOOGLE_API_KEY`      | `free_credits:N`      | Gemini models                       |
| xAI / Grok     | `XAI_API_KEY`         | `per_token`           | Grok models                         |
| Anthropic      | via OpenRouter        | `per_token`           | Claude models via OpenRouter        |

**Billing types:**
- `local` -- no cost (Ollama, local models)
- `free` -- provider-included free tier
- `unlimited` -- flat subscription, unlimited use
- `per_token` -- pay per input/output token
- `quota_monthly:N` -- N requests per month
- `quota_refreshing:N:secs` -- N requests per time window
- `free_credits:N` -- N USD in free credits before paid

## CLI Usage

```bash
# Start the proxy server
agentpather serve
agentpather serve --port 9000 --host 0.0.0.0

# List all available models across providers
agentpather models

# Health check all configured providers
agentpather doctor

# Generate default config file
agentpather init
agentpather init --output /etc/agentpather/config.toml

# View request statistics from the database
agentpather stats

# Load test the proxy
agentpather bench -n 500 -c 20
agentpather bench --routing-only              # test scoring only (no upstream calls)
agentpather bench --target http://remote:8402

# Global options
agentpather --config /path/to/config.toml serve
```

## Configuration

Coalesce looks for configuration in this order:
1. Path passed via `--config`
2. `./agentpather.toml`
3. `./config.toml`
4. `~/.config/agentpather/config.toml`
5. Built-in defaults (no providers, port 8402)

```toml
[server]
port = 8402
host = "127.0.0.1"

[logging]
level = "info"       # trace, debug, info, warn, error
format = "pretty"    # "pretty" or "json"

# Budget limits (0.0 = unlimited)
[budget]
total_limit_usd = 10.0
daily_limit_usd = 2.0

# Prompt injection detection
[sanitize]
enabled = true
injection_sensitivity = "medium"   # off, low, medium, high
max_request_size_bytes = 1048576   # 1 MB
max_messages = 100

# --- Providers ---

[providers.ollama]
enabled = true
endpoint = "http://localhost:11434"
billing = "local"

[providers.copilot]
enabled = true
api_key_env = "GITHUB_TOKEN"
billing = "quota_refreshing:50:18000"

[providers.openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"
billing = "per_token"

[providers.openai]
enabled = true
api_key_env = "OPENAI_API_KEY"
billing = "per_token"

[providers.google]
enabled = true
api_key_env = "GOOGLE_API_KEY"
billing = "free_credits:50.0"

[providers.deepseek]
enabled = true
api_key_env = "DEEPSEEK_API_KEY"
billing = "per_token"

[providers.xai]
enabled = true
api_key_env = "XAI_API_KEY"
billing = "per_token"

[providers.glm]
enabled = true
api_key_env = "GLM_API_KEY"
billing = "per_token"

[providers.kimi]
enabled = true
api_key_env = "KIMI_API_KEY"
billing = "unlimited"

# --- Routing ---

[routing.weights]
token_count = 0.15
code_presence = 0.12
reasoning_markers = 0.15
technical_terms = 0.08

[routing.thresholds]
simple_max = 0.12
medium_max = 0.25
complex_max = 0.40
```

## Dashboard

<!-- ![Dashboard Screenshot](docs/dashboard.png) -->

The web dashboard is available at `http://127.0.0.1:8402/` when the proxy is running. It is a single-page app embedded directly in the binary (no external assets).

**Dashboard features:**
- **Routing Playground** -- type a prompt and see the 15-dimension scoring breakdown, tier classification, and ranked candidate models with marginal costs
- **Provider Status** -- circuit breaker states, billing types, and model counts per provider
- **Request Stats** -- total requests, success rate, cost tracking, average latency
- **Request Timeline** -- paginated history of routed requests with provider, model, tier, and cost
- **Model Browser** -- all discovered models with pricing, capabilities (reasoning, vision, tools), and availability

Built with Tailwind CSS and vanilla JavaScript. No build step required.

## Desktop App

Coalesce includes a Tauri 2 desktop application (`desktop/src-tauri`) that wraps the proxy with:

- **System tray** -- start/stop the proxy from the menu bar
- **React dashboard** -- full-featured UI built with React
- **Native notifications** -- budget alerts and provider failures
- **Auto-start** -- optional launch at login

Build the desktop app:

```bash
cd desktop
npm install
npm run tauri build
```

## API Transports

Coalesce exposes two parallel API transports that share the same routing engine, economics logic, and provider pool. Choose the one that fits your use case:

### Axum REST API (port 8402) -- Human & Tool Friendly

The REST API is an **OpenAI-compatible HTTP/JSON endpoint**. Any application that can talk to the OpenAI API can point at Coalesce instead -- just change the base URL to `http://localhost:8402`. This makes it a **drop-in replacement** for Claude Code, Cursor, Continue, Open Interpreter, or any OpenAI SDK client.

The REST API also powers the dashboard UI and desktop app with additional endpoints for provider management, stats, and live event streaming.

**Best for:** Developer tools, IDE integrations, the dashboard UI, and any client that already speaks OpenAI's API format.

#### OpenAI-Compatible Endpoints

| Method | Path                     | Description                          |
|--------|--------------------------|--------------------------------------|
| POST   | `/v1/chat/completions`   | Chat completion (streaming + non-streaming) |
| GET    | `/v1/models`             | List all available models with pricing |
| GET    | `/v1/stats`              | Request statistics and quota states  |

#### Dashboard & Management Endpoints

| Method | Path                          | Description                        |
|--------|-------------------------------|------------------------------------|
| GET    | `/health`                     | Health check with circuit breaker states |
| GET    | `/api/v1/providers`           | Provider list with status and billing |
| GET    | `/api/v1/providers/quotas`    | Quota states for all providers     |
| POST   | `/api/v1/routing/playground`  | Dry-run the router on a prompt     |
| GET    | `/api/v1/routing/profiles`    | List routing profiles from config  |
| GET    | `/api/v1/stats/summary`       | Aggregated stats                   |
| GET    | `/api/v1/stats/timeline`      | Paginated request history (`?limit=50&offset=0`) |
| GET    | `/api/events`                 | SSE stream of live routing decisions |

#### Response Headers

Streaming responses include routing metadata:

| Header                   | Description                     |
|--------------------------|---------------------------------|
| `X-Coalesce-Model`       | Selected model ID               |
| `X-Coalesce-Provider`    | Provider name                   |
| `X-Coalesce-Tier`        | Classified tier (Simple/Medium/Complex/Reasoning) |
| `X-Coalesce-Attempt`     | Fallback attempt number (1-3)   |

### Tonic gRPC API (port 8403) -- Agent & Pipeline Friendly

The gRPC API provides the same routing capabilities over **Protocol Buffers** (binary-encoded messages) instead of JSON. It runs automatically on HTTP port + 1.

**Why gRPC?** For high-frequency agent orchestration scenarios -- multi-agent systems dispatching hundreds of routing decisions per minute -- gRPC eliminates JSON serialization overhead and provides ~2-10x lower latency per call. The `.proto` schema also enables auto-generated, strongly-typed client SDKs in any language (Python, Go, TypeScript, Java, etc.).

**Best for:** AI agent pipelines, multi-agent orchestrators, batch processing systems, and any scenario where routing throughput matters.

| RPC Method       | Description                     |
|------------------|---------------------------------|
| `ChatCompletion` | Route and complete a chat request |
| `ListModels`     | List available models           |
| `Health`         | Service health check            |

Proto definition: `proto/agentpather.proto`

### Transport Comparison

| | REST (Axum) | gRPC (Tonic) |
|---|---|---|
| **Port** | 8402 | 8403 |
| **Format** | JSON over HTTP | Protobuf over HTTP/2 |
| **Best for** | Human tools (Cursor, Claude Code, dashboard) | Agent-to-agent, high-frequency automation |
| **Compatibility** | Drop-in OpenAI replacement | Needs generated client stubs |
| **Streaming** | SSE (Server-Sent Events) | gRPC streaming |
| **Overhead** | ~1-5ms JSON parse | ~0.1-0.5ms protobuf decode |

## Plugin System

Plugins use a trait-based interface with three lifecycle hooks:

```
on_request  -->  on_route  -->  on_response
```

Each hook can return one of:
- **Continue(data)** -- pass (possibly modified) data to the next stage
- **Block(message)** -- reject the request with an error
- **Skip** -- no-op, continue without modification

Plugin manifest fields:

| Field         | Type       | Description                      |
|---------------|------------|----------------------------------|
| `name`        | String     | Plugin identifier                |
| `version`     | String     | Semantic version                 |
| `description` | String     | Human-readable description       |
| `hooks`       | Vec        | `on_request`, `on_route`, `on_response` |

The plugin architecture is WASM-ready. Native Rust plugins are loaded at startup; WASM plugin support is planned.

## Security

### Prompt Injection Detection

Coalesce scans incoming messages for prompt injection patterns before routing. Configure sensitivity in `agentpather.toml`:

```toml
[sanitize]
enabled = true
injection_sensitivity = "medium"  # off | low | medium | high
max_request_size_bytes = 1048576
max_messages = 100
```

| Level    | Behavior                                               |
|----------|--------------------------------------------------------|
| `off`    | No scanning                                            |
| `low`    | Flag obvious injection phrases                         |
| `medium` | Flag injection patterns + role confusion attempts      |
| `high`   | Aggressive detection, may flag legitimate edge cases   |

Flagged requests receive a score (0.0 to 1.0) and a list of matched patterns. At higher sensitivity levels, flagged requests are blocked before reaching any provider.

### Request Limits

- Maximum request body size: 1 MB (configurable)
- Maximum messages per request: 100 (configurable)
- Budget enforcement: total and daily USD limits

## Building from Source

### Prerequisites

- Rust 1.75+ (2021 edition)
- Protobuf compiler (`protoc`) for gRPC support
- Node.js 18+ (desktop app only)

### Build

```bash
# All crates (core, proxy, CLI)
cargo build --release

# Run tests
cargo test

# Run with logging
RUST_LOG=agentpather=debug cargo run -- serve
```

### Workspace Crates

| Crate               | Path                         | Description                        |
|----------------------|------------------------------|------------------------------------|
| `agentpather-core`   | `crates/agentpather-core`    | Router, economics, providers, storage, plugins, sanitize |
| `agentpather-proxy`  | `crates/agentpather-proxy`   | HTTP server (Axum), gRPC server (Tonic), web dashboard |
| `agentpather-cli`    | `crates/agentpather-cli`     | CLI binary with serve, models, doctor, stats, bench |
| desktop (Tauri)      | `desktop/src-tauri`          | Tauri 2 desktop app with system tray |

### Key Dependencies

| Dependency | Purpose                |
|------------|------------------------|
| Axum 0.8   | HTTP server            |
| Tonic 0.12 | gRPC server            |
| Tokio 1    | Async runtime          |
| Reqwest 0.12 | HTTP client          |
| rusqlite 0.32 | SQLite storage      |
| DashMap 6  | Concurrent hash maps   |
| Clap 4     | CLI argument parsing   |
| Tauri 2    | Desktop app framework  |

## License

MIT
