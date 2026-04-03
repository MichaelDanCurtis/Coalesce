# Coalesce

Smart LLM routing proxy that scores requests across 15 dimensions and routes them to the cheapest capable provider -- prioritizing free and included credits before paid options.

<!-- Badges -->
![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)
![License](https://img.shields.io/badge/license-MIT-blue)
![Tests](https://img.shields.io/badge/tests-172%20passing-green)

---

## Features

- **15-dimension request scoring** -- classifies prompts into Simple, Medium, Complex, or Reasoning tiers
- **10+ LLM providers** -- GitHub Copilot (multi-account), OpenRouter, Ollama, GLM/Zhipu, Kimi/Moonshot, Anthropic, Google Cloud Code, OpenAI, xAI/Grok, DeepSeek
- **Real-time marginal cost engine** -- computes effective cost per request across all available models with 8 billing types
- **Free-first priority cascade** -- local/free/quota models before paid, with configurable wait-vs-pay thresholds
- **Canonical model families** -- groups equivalent models across providers (e.g. `claude-sonnet-4` from Copilot and Anthropic) for family-aware routing
- **Per-model overrides** -- customize quality tier, pricing, capabilities, and family for any model via the UI
- **Rosetta tool layer** -- canonical tool types, equivalence classes, capability-aware routing, and automatic tool substitution across providers
- **Anthropic API support** -- native `/v1/messages` endpoint with thinking blocks, tool use, and streaming
- **Circuit breakers** -- automatic provider isolation on repeated failures, with half-open recovery
- **Request deduplication** -- identical in-flight requests share a single upstream call
- **Fallback chains** -- automatic retries across different providers with auth-skip logic
- **SSE streaming** -- full streaming support through the proxy with thinking/reasoning token passthrough
- **SQLite storage** -- persistent request logging, stats, cost tracking, and provider config
- **Budget tracking** -- configurable total and daily spending limits with threshold alerts
- **Auto-failover rules** -- configurable rules engine for automatic provider failover
- **Plugin system** -- trait-based hooks (on_request, on_route, on_response), WASM-ready architecture
- **Prompt injection detection** -- configurable sensitivity (off/low/medium/high)
- **Tauri 2 desktop app** -- system tray, React dashboard, native notifications
- **Web dashboard** -- full-featured UI with chat, routing playground, cost analytics, provider config, and live events
- **gRPC transport** -- protobuf-encoded API on port 8403 for high-frequency agent communication
- **Configuration profiles** -- save/restore/import full provider and routing configurations
- **Load testing** -- built-in `bench` command with percentile latency reporting
- **OpenAI-compatible API** -- drop-in replacement at `/v1/chat/completions`

## Quick Start

### Install

```bash
# Clone and build
git clone https://github.com/MichaelDanCurtis/Coalesce.git
cd Coalesce
cargo build --release

# Binary is at target/release/coalesce
```

### Configure

```bash
# Generate a config template
./target/release/coalesce init

# Edit coalesce.toml -- enable providers, add API keys
$EDITOR coalesce.toml
```

### Run

```bash
# Start the proxy (default: http://127.0.0.1:8402)
./target/release/coalesce serve

# Or just run directly (serve is the default command)
./target/release/coalesce
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
                    (curl, SDKs, agents,
                     Claude Code, Cursor)
                           |
              +------------+------------+
              |            |            |
        HTTP :8402   Anthropic :8402  gRPC :8403
        /v1/chat/*   /v1/messages     protobuf
              |            |            |
    +---------+------------+------------+---------+
    |              Unified Request Handler         |
    +----------------------------------------------+
                           |
                   +-------+-------+
                   |    Router     |
                   | 15-dimension  |
                   |   scorer      |
                   | tier: Simple  |
                   | Medium/Complex|
                   | /Reasoning    |
                   +-------+-------+
                           |
                   +-------+-------+
                   |   Economics   |
                   |    Engine     |
                   | marginal cost |
                   | 8 billing     |
                   | types + quota |
                   | tracking      |
                   +-------+-------+
                           |
                   +-------+-------+
                   |   Rosetta     |
                   | canonical     |
                   | tools, model  |
                   | families,     |
                   | equivalence   |
                   | substitution  |
                   +-------+-------+
                           |
         +---------+-------+-------+---------+
         |         |       |       |         |
    +----+---+ +---+---+ +-+---+ +-+------+ +--+------+
    | Free/  | | Quota | | Paid| | Quota  | | Quota   |
    | Local  | | Only  | |     | | Metered| | Refresh |
    | Ollama | | GLM   | | OR  | | Copilot| | Copilot |
    |        | | Kimi  | | OAI | | (work) | | (pers.) |
    |        | |Google | | xAI | |        | |         |
    |        | |       | | DS  | |        | |         |
    +----+---+ +---+---+ +-+---+ +-+------+ +--+------+
         |         |       |       |         |
    +----+---------+-------+-------+---------+----+
    |    Circuit Breakers + Dedup + Failover       |
    |    Auth-skip | Priority routing | Rules      |
    +----------------------+-----------------------+
                           |
                   +-------+-------+
                   |    SQLite     |
                   | requests, stats|
                   | quotas, config |
                   | provider state |
                   +---------------+
```

### How a Request Flows

1. **Ingress** -- Request arrives via OpenAI-compat (`/v1/chat/completions`), Anthropic (`/v1/messages`), or gRPC
2. **Scoring** -- The 15-dimension scorer analyzes the prompt (token count, code presence, reasoning markers, technical terms, etc.) and assigns a quality tier: Simple, Medium, Complex, or Reasoning
3. **Candidate Selection** -- All models matching the tier are gathered. Models are filtered by capabilities (vision, tools, reasoning) if the request requires them
4. **Economics Ranking** -- The marginal cost engine computes the effective cost for each candidate based on its billing type and current quota state. Free/local models rank first, then quota-available, then paid
5. **Priority + Family Routing** -- Provider priorities and canonical model families influence ordering. If provider A has priority 10 and provider B has priority 50, A's models are tried first at equal cost
6. **Rosetta Translation** -- If the request includes tools, the Rosetta layer translates between provider-specific tool formats and substitutes equivalent tools from the canonical registry
7. **Dispatch** -- The request is sent to the top-ranked candidate. Circuit breakers gate access (open = skip, half-open = test one request)
8. **Fallback** -- If the provider fails (timeout, 5xx, auth error), the router moves to the next candidate. Auth-failed providers are skipped without burning attempt slots. Up to 3 attempts
9. **Response** -- The response streams back through the proxy with routing metadata in headers and the `x_coalesce` field. Thinking/reasoning tokens are passed through for models that support them
10. **Accounting** -- Cost is recorded, quota state updated, circuit breaker notified of success/failure, and the request is logged to SQLite

### Billing Types

The economics engine supports 8 billing models that determine routing priority:

| Type | Config String | Behavior | Priority |
|------|--------------|----------|----------|
| Local | `local` | Always free, local hardware | 0 (free) |
| Free Included | `free` | Free with subscription | 0 (free) |
| Unlimited | `unlimited` | Flat subscription, unlimited use | 0 (free) |
| Quota Monthly | `quota_monthly:N` | N requests/month, stops when exhausted | 0 (free) |
| Quota Refreshing | `quota_refreshing:N:secs` | N requests per time window, refreshes | 0 (free) |
| Quota Only | `quota_only:N:secs` | Free while quota lasts, then unavailable | 0 (free) |
| Quota + Metered | `quota_metered:N:secs` | Free while quota lasts, then per-token | 0→2 |
| Per Token | `per_token` | Pay per token always | 2 (paid) |
| Free Credits | `free_credits:N` | N USD free credits, then per-token | 0→2 |

The router always prefers priority band 0 (free) over band 2 (paid). Within the same band, lower provider priority number wins.

## Providers

| Provider       | Auth Type | Default Billing | Notes |
|----------------|-----------|-----------------|-------|
| Ollama         | None      | `local` | Auto-detected on localhost:11434 |
| GitHub Copilot | OAuth     | `quota_refreshing:50:18000` | Multi-account support, 5h refresh windows |
| OpenRouter     | API Key   | `per_token` | 300+ models, single key |
| GLM / Zhipu    | API Key   | `quota_only:50:0` | z.ai coding endpoint |
| Kimi / Moonshot| API Key   | `quota_only:50:0` | Long-context specialist |
| Anthropic      | API Key   | `per_token` | Direct Claude API + `/v1/messages` |
| Google         | OAuth     | `quota_only:50:0` | Cloud Code Assist (Antigravity) |
| OpenAI         | API Key   | `per_token` | GPT-4o, o1, o3 |
| xAI / Grok     | API Key   | `per_token` | Grok models |
| DeepSeek       | API Key   | `per_token` | Reasoning and code models |

All billing types are configurable per-provider via the UI or API. Changes persist across restarts.

## Dashboard & Desktop App

The web dashboard runs at `http://127.0.0.1:8402/` and is also available as a Tauri 2 desktop app with system tray integration.

**Tabs:**
- **Chat** -- Test conversations with any model through the proxy, with streaming and thinking token display
- **Overview** -- Total requests, success rate, cost savings, latency, provider status cards
- **Providers** -- Model browser with pricing, capabilities, quality tier, marginal cost, enable/disable toggles, and per-model overrides
- **Provider Config** -- Add/remove providers, set billing type and priority, Copilot account ordering, Ollama model management
- **Local LLM** -- Ollama model management (pull, delete, preload)
- **Cost Analytics** -- Savings waterfall, budget tracking, daily cost trends
- **Usage Metrics** -- Per-provider token usage, quota burndown, latency stats
- **Routing Playground** -- Dry-run the router: see 15-dimension scores, tier classification, ranked candidates with costs
- **Request Timeline** -- Paginated history with search, export to CSV
- **System Prompts** -- Manage system prompts for different use cases
- **Live Events** -- Real-time SSE stream of routing decisions
- **Settings** -- Theme, proxy config, notifications

Build the desktop app:

```bash
cd desktop
npm install
npm run tauri build
```

## API

### OpenAI-Compatible Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/chat/completions` | Chat completion (streaming + non-streaming) |
| GET | `/v1/models` | List all models with pricing and capabilities |
| GET | `/v1/stats` | Request statistics and quota states |

### Anthropic-Compatible Endpoint

| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/messages` | Anthropic Messages API with thinking blocks and tool use |

### Management API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check with circuit breaker states |
| GET | `/api/v1/providers` | Provider list with billing, status, stats |
| GET | `/api/v1/providers/quotas` | Quota states for all providers |
| PUT | `/api/v1/providers/{name}/billing` | Update provider billing type |
| GET/PUT | `/api/v1/providers/priorities` | Provider priority and pricing mode |
| POST | `/api/v1/providers/manage` | Add a new provider |
| PUT | `/api/v1/providers/manage/{name}` | Update provider config |
| POST | `/api/v1/routing/playground` | Dry-run the router on a prompt |
| GET | `/api/v1/stats/summary` | Aggregated stats |
| GET | `/api/v1/stats/timeline` | Paginated request history |
| GET | `/api/events` | SSE stream of live routing events |

### gRPC API (port 8403)

| RPC Method | Description |
|------------|-------------|
| `ChatCompletion` | Route and complete a chat request |
| `ListModels` | List available models |
| `Health` | Service health check |

Proto definition: `proto/coalesce.proto`

## CLI Usage

```bash
# Start the proxy server
coalesce serve
coalesce serve --port 9000 --host 0.0.0.0

# List all available models across providers
coalesce models

# Health check all configured providers
coalesce doctor

# Generate default config file
coalesce init

# View request statistics
coalesce stats

# Load test the proxy
coalesce bench -n 500 -c 20
coalesce bench --routing-only

# Global options
coalesce --config /path/to/config.toml serve
```

## Configuration

Coalesce looks for configuration in this order:
1. Path passed via `--config`
2. `./coalesce.toml`
3. `./config.toml`
4. `~/.config/coalesce/config.toml`
5. Built-in defaults (no providers, port 8402)

```toml
[server]
port = 8402
host = "127.0.0.1"

[logging]
level = "info"       # trace, debug, info, warn, error
format = "pretty"    # "pretty" or "json"

[budget]
total_limit_usd = 10.0
daily_limit_usd = 2.0

[sanitize]
enabled = true
injection_sensitivity = "medium"   # off, low, medium, high

[providers.ollama]
enabled = true
endpoint = "http://localhost:11434"
billing = "local"

[providers.copilot]
enabled = true
billing = "quota_metered:50:18000"
priority = 50          # lower = tried first
pricing_mode = "subscription"  # "subscription" or "metered"

[providers.openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"
billing = "per_token"
```

## Workspace Crates

| Crate | Path | Description |
|-------|------|-------------|
| `coalesce-core` | `crates/coalesce-core` | Router, economics, providers, storage, plugins, Rosetta |
| `coalesce-proxy` | `crates/coalesce-proxy` | HTTP server (Axum), gRPC (Tonic), dashboard, API handlers |
| `coalesce-cli` | `crates/coalesce-cli` | CLI binary: serve, models, doctor, stats, bench |
| desktop | `desktop/` | Tauri 2 desktop app + React UI |

## License

MIT
