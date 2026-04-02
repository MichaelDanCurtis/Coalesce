# Coalesce — Product Requirements Document

## Overview

Coalesce is a smart LLM routing system with two components:

1. **Coalesce Core** — A high-performance Rust proxy that routes LLM requests across providers using a dynamic cost optimization engine
2. **Coalesce Desktop** — A Tauri 2 cross-platform desktop app (Windows, macOS, Linux) with system tray integration for quick provider switching, real-time monitoring, and master configuration

It is inspired by [ClawRouter](https://github.com/BlockRunAI/ClawRouter) (smart routing engine) and [CC Switch](https://github.com/farion1231/cc-switch) (desktop UX / system tray management), but is a ground-up Rust implementation with key differences:

- **Removed**: x402 payment protocol, blockchain wallets, USDC micropayments, all crypto dependencies
- **Added**: Dynamic provider economics engine, GitHub Copilot OAuth, OpenRouter, GLM, local model support
- **Added**: Tauri 2 desktop app with system tray, web dashboard, cross-platform support
- **Rewritten**: Everything in Rust (backend) + React/TypeScript (frontend), renamed to Coalesce

### What It Does

Coalesce sits between AI agents/clients and LLM providers as a local HTTP proxy. It:
- Analyzes each request across 15 weighted dimensions and classifies into complexity tiers
- Computes the **real-time marginal cost** of every candidate model across all providers
- Routes to the cheapest capable model — prioritizing free/included credits before paid options
- Provides a system tray app for quick switching, monitoring, and provider management
- Serves a web dashboard for detailed analytics, routing playground, and master configuration

### Providers

| Provider | Auth Method | Billing Type | Models |
|----------|------------|-------------|--------|
| **GitHub Copilot** | OAuth Device Flow | Free included + quota premium | GPT-4.1, GPT-5 mini, Claude Sonnet/Opus, Gemini |
| **OpenRouter** | API Key (Bearer) | Per-token (300+ models) | Everything — pricing auto-fetched |
| **Ollama** (local) | None (localhost) | Free (local) | Any locally running model |
| **GLM / Zhipu AI** | API Key | Per-token / subscription | GLM-4, GLM-4-Plus, GLM-4V, CogView |
| **Kimi / Moonshot** | API Key | Unlimited subscription | Kimi K2.5 |
| **Anthropic** (direct) | API Key | Quota refreshing (5h window) | Claude Sonnet/Opus |
| **Google AI** (direct) | API Key | Free tier credits + per-token | Gemini 2.5/3.1 Pro/Flash |
| **OpenAI** (direct) | API Key | Per-token | GPT-5.x, o3 |
| **xAI** (direct) | API Key | Per-token | Grok 4 |
| **DeepSeek** (direct) | API Key | Per-token | DeepSeek V3, R1 |

---

## Architecture

```
 ┌─────────────────────────────────────────────────────────────────────┐
 │                     Coalesce Desktop (Tauri 2)                   │
 │  ┌───────────────┐  ┌─────────────────────────────────────────┐    │
 │  │  System Tray   │  │  Main Window (React + TailwindCSS)     │    │
 │  │  ─────────────│  │  ┌──────────────────────────────────┐   │    │
 │  │  ① Ollama  $0 │  │  │  Provider Dashboard              │   │    │
 │  │  ② Kimi    $0 │  │  │  Economics Visualization          │   │    │
 │  │  ③ Copilot $0 │  │  │  Routing Playground               │   │    │
 │  │  ④ GLM     $$ │  │  │  Request Timeline                 │   │    │
 │  │  ⑤ OpenRtr $$ │  │  │  Provider Config (Master)         │   │    │
 │  │  ──────────── │  │  │  Quota Burndown Charts            │   │    │
 │  │  Profile: auto│  │  └──────────────────────────────────┘   │    │
 │  │  Saved: $47   │  │                                          │    │
 │  └───────────────┘  └─────────────────────────────────────────┘    │
 └───────────────────────────────────┬─────────────────────────────────┘
                              Tauri IPC │
 ┌───────────────────────────────────▼─────────────────────────────────┐
 │                     Coalesce Core (Rust)                         │
 │                                                                     │
 │  ┌──────────┐   ┌───────────────────────┐   ┌──────────────────┐   │
 │  │  Dedup   │──►│  15-Dimension Router  │──►│ Economics Engine │   │
 │  │  Cache   │   └───────────────────────┘   │ Quota Tracker    │   │
 │  └──────────┘                               │ Cost Comparator  │   │
 │                                             └────────┬─────────┘   │
 │       ┌──────────┬──────────┬──────────┬─────────┬───┘             │
 │       ▼          ▼          ▼          ▼         ▼                 │
 │  ┌────────┐ ┌────────┐ ┌───────┐ ┌────────┐ ┌────────┐           │
 │  │Copilot │ │  Kimi  │ │  GLM  │ │ Google │ │OpenRtr │           │
 │  │ OAuth  │ │ Unlim  │ │ Zhipu │ │AI Pro  │ │per-tok │           │
 │  └────────┘ └────────┘ └───────┘ └────────┘ └────────┘           │
 │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐                     │
 │  │Anthropic│ │ Ollama │ │ OpenAI │ │DeepSeek│                     │
 │  │5h quota│ │ Local  │ │per-tok │ │per-tok │                     │
 │  └────────┘ └────────┘ └────────┘ └────────┘                     │
 │                                                                     │
 │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────────────┐  │
 │  │  SQLite  │ │Prometheus│ │Web Dash  │ │  WASM Plugin System  │  │
 │  │ DB+Quota │ │ /metrics │ │/dashboard│ └──────────────────────┘  │
 │  └──────────┘ └──────────┘ └──────────┘                            │
 │                                                                     │
 │  HTTP Proxy: localhost:8402  │  gRPC: localhost:8403                │
 └─────────────────────────────────────────────────────────────────────┘

 Clients (Claude Code, Cursor, agents, etc.) ──► localhost:8402/v1/chat/completions
 Browser ──► localhost:8402/dashboard (web UI, also accessible without desktop app)
```

### Core Components

| Component | Rust Crate | Purpose |
|-----------|-----------|---------|
| HTTP Server | `axum` + `tokio` | Async HTTP proxy with streaming |
| HTTP Client | `reqwest` | Connection-pooled upstream requests |
| SSE Streaming | `axum` SSE + `eventsource-stream` | Server-Sent Events parsing & forwarding |
| Config | `serde` + `toml` | TOML-based routing profiles |
| Database | `rusqlite` | Usage history, analytics |
| Metrics | `metrics` + `metrics-exporter-prometheus` | Prometheus `/metrics` endpoint |
| gRPC | `tonic` | Alternative agent-to-router protocol |
| CLI | `clap` | Command-line interface |
| Tracing | `tracing` + `tracing-subscriber` | Structured logging |
| OAuth | `reqwest` (manual) | GitHub device flow (simple enough to hand-roll) |
| WASM Plugins | `wasmtime` | Language-agnostic plugin system |
| Input Validation | `validator` + custom | Request sanitization |

---

## Feature Specification

### PHASE 1 — Core Proxy & Routing Engine

**Goal**: Minimal working proxy that routes requests to OpenRouter.

#### 1.1 Zero-Copy Request Parsing
- Parse incoming OpenAI-compatible requests without intermediate string allocations
- Use `serde_json` with borrowed string deserialization where possible
- Benchmark: parse + route decision in <100μs

#### 1.2 15-Dimension Weighted Router
Port the exact scoring system from ClawRouter:

| # | Dimension | Weight | Signal |
|---|-----------|--------|--------|
| 1 | Token Count | 0.15 | Estimated total tokens |
| 2 | Code Presence | 0.12 | Code blocks, keywords (function, class, SELECT) |
| 3 | Reasoning Markers | 0.15 | "think", "reason", "analyze", "prove" |
| 4 | Technical Terms | 0.08 | algorithm, architecture, cryptography |
| 5 | Creative Markers | 0.06 | "write", "create", "generate", "poetry" |
| 6 | Simple Indicators | 0.10 | "summary", "explain", "what is" |
| 7 | Multi-Step Patterns | 0.08 | "first...then", "step 1", numbered lists |
| 8 | Question Complexity | 0.04 | Count of "?" marks (>3 = complex) |
| 9 | Imperative Verbs | 0.05 | "build", "implement", "design", "refactor" |
| 10 | Constraint Indicators | 0.04 | "must", "required", "constraints" |
| 11 | Output Format | 0.03 | "json", "yaml", "xml", "table" |
| 12 | Reference Keywords | 0.02 | "cite", "reference", "source" |
| 13 | Negation Keywords | 0.02 | "not", "avoid", "don't", "without" |
| 14 | Domain-Specific | 0.03 | medical, legal, financial terms |
| 15 | Agentic Keywords | 0.03 | "iterate", "refine", "loop", "autonomous" |

Tier thresholds (sigmoid-calibrated):
- **SIMPLE**: score < 0.3
- **MEDIUM**: 0.3 ≤ score < 0.6
- **COMPLEX**: 0.6 ≤ score < 0.8
- **REASONING**: score ≥ 0.8 + reasoning marker present

Multilingual keyword support: English, Chinese, Japanese, Russian, German, Spanish, Portuguese, Korean, Arabic.

#### 1.3 Provider Economics Engine (Dynamic Cost Optimization)

**This is the core differentiator from ClawRouter.** Instead of static tier-to-model mappings, Coalesce maintains a real-time understanding of every provider's billing model, subscription state, and marginal cost — then routes to minimize actual spend.

##### Subscription Models

Each provider has a billing type that determines its real-time marginal cost:

| Billing Type | Example | Behavior |
|-------------|---------|----------|
| `free_included` | Copilot GPT-4.1, GPT-5 mini | Always $0, unlimited (on paid plan) |
| `quota_monthly` | Copilot premium models (50-300/mo) | $0 until quota exhausted, then unavailable |
| `quota_refreshing` | Anthropic API (resets every 5 hours) | $0 during window, unavailable when depleted, auto-resets |
| `unlimited_subscription` | Kimi K2 subscription | Always $0 (flat monthly fee already paid) |
| `free_tier_credits` | Google AI Pro included credits | $0 until credits gone, then per-token |
| `per_token` | OpenRouter, direct OpenAI API | Always costs $/token |
| `local` | Ollama | Always $0, but limited quality/speed |

##### Real-Time Marginal Cost Calculator

For every candidate model, the router computes the **actual marginal cost** of this specific request:

```
marginal_cost(model, request) =
    if provider.billing == local:                     → $0.00
    if provider.billing == unlimited_subscription:    → $0.00
    if provider.billing == free_included:             → $0.00
    if provider.billing == quota_monthly:
        if provider.quota_remaining > 0:              → $0.00
        else:                                         → UNAVAILABLE
    if provider.billing == quota_refreshing:
        if provider.quota_remaining > 0:              → $0.00
        if provider.next_refresh < 30min:             → WAIT (queue)
        else:                                         → UNAVAILABLE
    if provider.billing == free_tier_credits:
        if provider.credits_remaining > estimated_cost: → $0.00
        else:                                         → per_token_rate
    if provider.billing == per_token:
        → input_tokens * input_price + output_tokens * output_price
```

##### Routing Priority Cascade

When a request is classified into a tier, the router picks from all capable models using this priority:

```
1. FREE sources (marginal cost = $0)
   ├── Local models (Ollama) — if quality sufficient for tier
   ├── Unlimited subscriptions (Kimi K2) — always available
   ├── Free included models (Copilot GPT-4.1) — always available
   ├── Quota models with remaining credits (Copilot premium, Anthropic)
   └── Free tier credits (Google AI Pro)

2. WAIT candidates (marginal cost = $0 but temporarily depleted)
   └── Quota-refreshing providers resetting within configurable threshold

3. PAID sources (marginal cost > $0), sorted by cost
   ├── Cheapest per-token option (often OpenRouter)
   └── Direct provider APIs
```

Within each priority band, models are ranked by **quality score** for the given tier (from the 15-dimension analysis + adaptive quality tracking).

##### Provider Config (TOML)

```toml
# ~/.coalesce/providers.toml

[providers.copilot]
enabled = true
billing = "mixed"  # some models free, some quota

[providers.copilot.models.gpt-4_1]
billing = "free_included"
quality_tier = "medium"  # capable of SIMPLE + MEDIUM

[providers.copilot.models.claude-sonnet-4_6]
billing = "quota_monthly"
quota_total = 300         # premium requests per month
quality_tier = "complex"

[providers.kimi]
enabled = true
billing = "unlimited_subscription"
endpoint = "https://api.moonshot.cn/v1/chat/completions"
api_key_env = "KIMI_API_KEY"
quality_tier = "complex"    # capable of SIMPLE through COMPLEX
models = ["kimi-k2.5"]

[providers.anthropic_direct]
enabled = true
billing = "quota_refreshing"
refresh_interval = "5h"
quota_per_window = 45      # ~45 messages per 5h window (estimated)
endpoint = "https://api.anthropic.com/v1/messages"
api_key_env = "ANTHROPIC_API_KEY"
format = "anthropic"       # needs message format translation
models = ["claude-sonnet-4.6", "claude-opus-4.6"]
quality_tier = "reasoning"

[providers.google_ai]
enabled = true
billing = "free_tier_credits"
free_credits_usd = 10.00   # monthly included credits
endpoint = "https://generativelanguage.googleapis.com/v1beta"
api_key_env = "GOOGLE_AI_KEY"
models = ["gemini-2.5-pro", "gemini-3.1-pro"]
quality_tier = "reasoning"

[providers.openrouter]
enabled = true
billing = "per_token"
# Pricing auto-fetched from GET /api/v1/models
api_key_env = "OPENROUTER_API_KEY"
# All 300+ models available, pricing is dynamic

[providers.ollama]
enabled = true
billing = "local"
endpoint = "http://localhost:11434"
# Models auto-discovered from GET /api/tags
quality_tier = "simple"    # default, overridable per model
```

##### Quota Tracking

The economics engine tracks quota state in SQLite:

```sql
CREATE TABLE provider_quotas (
    provider TEXT NOT NULL,
    model TEXT,                    -- NULL = provider-level quota
    billing_type TEXT NOT NULL,
    quota_total INTEGER,
    quota_used INTEGER DEFAULT 0,
    quota_period TEXT,             -- 'monthly', 'refreshing', etc.
    refresh_interval_secs INTEGER, -- for quota_refreshing
    last_refresh TEXT,
    next_refresh TEXT,
    free_credits_total_usd REAL,
    free_credits_used_usd REAL,
    updated_at TEXT DEFAULT (datetime('now'))
);
```

Quota tracking methods:
- **Copilot**: Track premium request count locally (increment on each premium model call)
- **Anthropic**: Track calls per window; detect 429/rate-limit as "quota depleted"; reset timer on next success after refresh
- **Google AI**: Track estimated spend against free credit balance
- **OpenRouter**: Query `GET /api/v1/key` for credit balance
- **Kimi/unlimited**: No tracking needed

##### Dashboard Integration

The economics engine feeds the dashboard:
- Real-time marginal cost per provider (color-coded: green=$0, yellow=low, red=expensive)
- Quota burn rate and estimated depletion time
- "Money saved" counter (actual cost vs if everything went to OpenRouter/direct)
- Provider recommendation for next request

#### 1.4 Configurable Routing Profiles (TOML)

Profiles now define **quality preferences**, not specific models. The economics engine handles model selection:

```toml
# ~/.coalesce/routing.toml

[profiles.auto]
description = "Balanced cost/quality"
strategy = "cheapest_capable"  # default: cheapest model that meets tier quality

[profiles.auto.tiers.simple]
min_quality = "simple"     # any model rated simple+ is acceptable
prefer_local = true        # prefer Ollama if available

[profiles.auto.tiers.medium]
min_quality = "medium"

[profiles.auto.tiers.complex]
min_quality = "complex"

[profiles.auto.tiers.reasoning]
min_quality = "reasoning"
allow_wait = true          # willing to wait for quota refresh (e.g., Anthropic)
max_wait_secs = 300        # wait up to 5 min for free provider to refresh

[profiles.eco]
description = "Maximum savings — free sources only"
strategy = "free_only"     # never use per-token providers
fallback = "queue"         # queue requests if all free sources depleted

[profiles.premium]
description = "Best quality — ignore cost"
strategy = "best_quality"  # always pick highest quality model regardless of cost

[profiles.budget]
description = "Stay within daily budget"
strategy = "cheapest_capable"
daily_limit_usd = 5.00
```

#### 1.4 Connection Pooling
- Per-provider `reqwest::Client` with persistent connection pools
- Configurable max connections per provider (default: 10)
- Per-provider rate limiting (token bucket)

#### 1.5 Startup & Binary
- Single static binary: `coalesce`
- Startup time target: <10ms
- Memory footprint target: <10MB idle
- Default port: 8402 (same as ClawRouter for compatibility)

---

### PHASE 2 — Provider Integrations

#### 2.1 OpenRouter Provider
- Auth: `Authorization: Bearer sk-or-v1-xxx` (from config or env `OPENROUTER_API_KEY`)
- Endpoint: `https://openrouter.ai/api/v1/chat/completions`
- Model discovery: `GET /api/v1/models` on startup, refresh every 15 minutes
- Pricing auto-sync: Parse `pricing.prompt` / `pricing.completion` (strings, USD per token)
- Detect reasoning models: `pricing.internal_reasoning` > 0
- Use `models` array in request body for built-in fallback
- Headers: `HTTP-Referer`, `X-OpenRouter-Title` for attribution
- Streaming: SSE with `: OPENROUTER PROCESSING` keepalive comments

#### 2.2 GitHub Copilot OAuth Provider
**Device Flow (one-time setup via `coalesce auth copilot`):**

1. `POST https://github.com/login/device/code`
   - `client_id`: `Iv1.b507a08c87ecfe98`
   - `scope`: `read:user`
   - Returns: `device_code`, `user_code`, `verification_uri`
2. Display user_code, open browser to `https://github.com/login/device`
3. Poll `POST https://github.com/login/oauth/access_token` every 5s
   - Returns: `access_token` (ghu_xxx) — persist to `~/.coalesce/copilot_token`

**Token refresh (automatic, every ~25 min):**
1. `GET https://api.github.com/copilot_internal/v2/token`
   - `Authorization: token <access_token>`
   - Returns: `copilot_token` (short-lived) + `expires_at`
2. Cache in memory, refresh when `expires_at` approaches

**API calls:**
- Endpoint: `POST https://api.githubcopilot.com/chat/completions`
- Required headers:
  ```
  Authorization: Bearer <copilot_token>
  Copilot-Integration-Id: vscode-chat
  editor-version: vscode/1.85.1
  editor-plugin-version: copilot/1.155.0
  user-agent: GithubCopilot/1.155.0
  ```
- Model discovery: `GET https://api.githubcopilot.com/models`
- Available: GPT-4.1, GPT-5 mini (included), Claude Sonnet/Opus, Gemini (premium)

#### 2.3 Ollama (Local) Provider
- Endpoint: `http://localhost:11434/v1/chat/completions` (OpenAI-compatible mode)
- No auth required
- Model discovery: `GET http://localhost:11434/api/tags`
- Cost: $0.00 (always cheapest option)
- Route SIMPLE tier here when available
- Auto-detect: check if Ollama is running on startup

#### 2.4 GLM / Zhipu AI Provider
- Endpoint: `https://open.bigmodel.cn/api/paas/v4/chat/completions` (OpenAI-compatible)
- Auth: `Authorization: Bearer <api_key>` (from `ZHIPU_API_KEY` env or config)
- Models: `glm-4`, `glm-4-plus`, `glm-4-long`, `glm-4v` (vision), `glm-4-flash` (cheap/fast)
- Model discovery: Hardcoded model list with pricing (no dynamic endpoint)
- Pricing: glm-4-flash is very cheap (¥0.1/M tokens), glm-4-plus is mid-range
- Streaming: Standard SSE, OpenAI-compatible
- Special: `glm-4v` supports vision (image URLs in messages)
- CogView image generation: `POST /api/paas/v4/images/generations`
- Quality tier mapping: glm-4-flash → SIMPLE, glm-4 → MEDIUM, glm-4-plus → COMPLEX

#### 2.5 Direct Provider Support
- OpenAI: `https://api.openai.com/v1/chat/completions`
- Anthropic: `https://api.anthropic.com/v1/messages` (needs format translation)
- Google: Vertex AI or AI Studio endpoints
- xAI: `https://api.x.ai/v1/chat/completions`
- DeepSeek: `https://api.deepseek.com/v1/chat/completions` (OpenAI-compatible)
- Auth: API keys from config or env vars

---

### PHASE 3 — Streaming & Reliability

#### 3.1 Streaming-First Architecture
- Use `axum`'s `Sse` extractor for server-side streaming
- Parse upstream SSE with `eventsource-stream`
- Proper backpressure via tokio channels (bounded)
- No heartbeat hack needed — Rust's async handles this natively
- Convert non-streaming JSON responses to SSE format for clients requesting `stream: true`

#### 3.2 Fallback Chain
- On provider error (5xx, 429, timeout), try next model in tier
- Configurable fallback depth (default: 3)
- Per-provider error cooldown (429 → 60s, 503 → 15s)
- Circuit breaker per provider (open after 5 consecutive failures, half-open after 30s)

#### 3.3 Request Deduplication
- SHA-256 hash of request body (strip timestamps/request IDs)
- 30s TTL cache (dashmap for lock-free concurrent access)
- In-flight dedup: concurrent identical requests share one upstream call
- Cached response replay for completed requests

#### 3.4 Session Persistence
- Pin model selection within a conversation (by session ID header or message history hash)
- 30-minute timeout
- Store in-memory (dashmap)

---

### PHASE 4 — Observability & Storage

#### 4.1 Prometheus Metrics
Built-in `/metrics` endpoint exposing:

```
# Request metrics
coalesce_requests_total{provider, model, tier, status}
coalesce_request_duration_seconds{provider, model, tier}
coalesce_request_tokens_input{provider, model}
coalesce_request_tokens_output{provider, model}

# Routing metrics
coalesce_routing_decisions_total{tier, profile}
coalesce_routing_duration_seconds

# Provider health
coalesce_provider_errors_total{provider, error_type}
coalesce_provider_circuit_state{provider}  # 0=closed, 1=open, 2=half-open

# Cost metrics
coalesce_cost_dollars_total{provider, model}
coalesce_cost_savings_dollars_total  # vs baseline (Claude Opus)
```

#### 4.2 SQLite Usage Database
Replace JSON-lines logging with queryable SQLite:

```sql
CREATE TABLE requests (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    session_id TEXT,
    tier TEXT NOT NULL,  -- SIMPLE/MEDIUM/COMPLEX/REASONING
    profile TEXT NOT NULL,  -- auto/eco/premium
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cost_usd REAL,
    baseline_cost_usd REAL,
    savings_usd REAL,
    latency_ms INTEGER,
    status TEXT,  -- ok/error/fallback
    error_type TEXT,
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE provider_health (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    status TEXT,  -- ok/error/rate_limited/timeout
    latency_ms INTEGER,
    error_message TEXT
);
```

CLI commands:
- `coalesce stats` — cost savings summary
- `coalesce stats --daily` — daily breakdown
- `coalesce stats --by-model` — per-model usage
- `coalesce history` — recent request log

#### 4.3 Structured Logging
- `tracing` crate with JSON output
- Log levels: ERROR (failures), WARN (fallbacks, rate limits), INFO (requests), DEBUG (routing decisions), TRACE (full request/response)
- Log file: `~/.coalesce/logs/coalesce.log` (rotated daily)

---

### PHASE 5 — Advanced Features

#### 5.1 Multi-Tenant Mode
- Support multiple named profiles with separate provider credentials
- `X-Coalesce-Profile: <name>` header to select profile
- Independent routing configs, budgets, and stats per profile
- Use case: agent fleets with different cost/quality tradeoffs

#### 5.2 Request Priority Queues
- REASONING tier gets priority in provider rate limit windows
- SIMPLE tier can be delayed/batched during high load
- Priority header: `X-Coalesce-Priority: high|normal|low`
- Configurable queue depth and timeout

#### 5.3 Cost Budgeting with Alerts
```toml
# ~/.coalesce/config.toml
[budget]
daily_limit_usd = 10.00
monthly_limit_usd = 200.00
alert_threshold = 0.80  # Alert at 80% of limit

[budget.alerts]
webhook_url = "https://hooks.slack.com/..."
# or
command = "notify-send 'Coalesce: budget alert'"
```

When limit hit: fall back to free/local models only.

#### 5.4 Adaptive Routing (Response Quality Scoring)
- Track response quality signals per model:
  - Completion rate (did the model finish without error?)
  - Tool call success rate
  - Response length vs expected
  - User feedback signal (optional `X-Coalesce-Feedback: good|bad` header on follow-up)
- Exponential moving average per model
- Adjust tier assignments based on observed quality
- Store quality scores in SQLite

#### 5.5 Semantic Similarity Cache
- On request, compute embedding of the prompt (using a cheap/local model)
- Check cache for semantically similar previous requests (cosine similarity > 0.95)
- Return cached response if match found
- Storage: SQLite with embedding vectors
- Opt-in: disabled by default (`[cache] semantic = true` in config)

#### 5.6 gRPC Support
- `tonic`-based gRPC server alongside HTTP
- Proto definition for chat completions (more efficient for high-frequency agents)
- Same routing engine, just different transport
- Default port: 8403

---

### PHASE 6 — Plugin System & CLI

#### 6.1 WASM Plugin System
- Host WASM plugins via `wasmtime`
- Plugin interface:
  ```rust
  // Plugin can implement any of these hooks:
  trait CoalescePlugin {
      fn on_request(&self, req: &Request) -> PluginResult<Request>;  // modify/filter
      fn on_route(&self, decision: &RouteDecision) -> PluginResult<RouteDecision>;  // override routing
      fn on_response(&self, resp: &Response) -> PluginResult<Response>;  // modify/log
  }
  ```
- Plugins loaded from `~/.coalesce/plugins/*.wasm`
- Plugin config in TOML
- Use case: custom routing logic, request transformation, logging to external services

#### 6.2 CLI Commands
```
coalesce                     # Start proxy (default)
coalesce serve               # Start proxy (explicit)
coalesce serve --port 8402   # Custom port

# Authentication
coalesce auth copilot        # GitHub Copilot OAuth device flow
coalesce auth openrouter     # Set OpenRouter API key
coalesce auth status         # Show auth status for all providers

# Stats & History
coalesce stats               # Cost savings summary
coalesce stats --daily       # Daily breakdown
coalesce history             # Recent requests
coalesce history --model gpt-5  # Filter by model

# Config
coalesce config show         # Show current config
coalesce config edit         # Open config in $EDITOR
coalesce config routing      # Show routing profiles
coalesce config providers    # Show provider status

# Diagnostics
coalesce doctor              # Health check all providers
coalesce bench               # Load test routing throughput

# Models
coalesce models              # List available models across all providers
coalesce models --provider openrouter  # Filter by provider
coalesce models --tier reasoning       # Filter by tier
coalesce exclude add <model>           # Block a model
coalesce exclude remove <model>
coalesce exclude list
```

#### 6.3 Request Sandboxing (Input Validation)
- Validate all incoming requests against OpenAI schema
- Reject malformed requests with clear error messages
- Prompt injection detection:
  - Scan for known injection patterns ("ignore previous instructions", "system prompt override")
  - Configurable sensitivity (off/low/medium/high)
  - Log flagged requests
- Max request size limit (default: 1MB)
- Max message count limit (default: 100)
- Rate limit per client IP (configurable)

---

### PHASE 7 — Desktop App & System Tray (Tauri 2)

Inspired by [CC Switch](https://github.com/farion1231/cc-switch). Cross-platform desktop app that wraps the core proxy and provides system tray quick-access.

#### 7.1 System Tray

**Always-visible tray icon** with dynamic menu generated from live provider state:

```
┌─────────────────────────────────┐
│  Coalesce                    │
│  ─────────────────────────────  │
│  Profile: ● Auto (balanced)    │
│           ○ Eco (free only)    │
│           ○ Premium (best)     │
│           ○ Budget ($5/day)    │
│  ─────────────────────────────  │
│  Providers:                     │
│  ✅ Ollama         local  $0   │
│  ✅ Kimi K2        unlim  $0   │
│  ✅ Copilot        free   $0   │
│  ⚠️  Copilot Prem  47/300 $0   │
│  ❌ Anthropic      reset 1h23  │
│  ✅ GLM-4-flash    ¥0.1/M     │
│  ✅ Google AI      $7.86 left  │
│  ✅ OpenRouter     $0.003/1K   │
│  ─────────────────────────────  │
│  💰 Saved $47.23 today         │
│  📊 142 requests | 0 errors    │
│  ─────────────────────────────  │
│  Open Dashboard...              │
│  Settings...                    │
│  ─────────────────────────────  │
│  Quit                           │
└─────────────────────────────────┘
```

**Tray behavior:**
- Left-click: Open menu (macOS/Linux) or toggle dashboard window (Windows)
- Right-click: Context menu
- Icon changes color: green (all good), yellow (some providers depleted), red (errors/all depleted)
- macOS: Template icon adapts to light/dark mode
- Menu regenerated dynamically after every state change

**Quick actions from tray:**
- Switch routing profile with one click
- Enable/disable individual providers
- See live quota status at a glance
- Open dashboard window

#### 7.2 Desktop Dashboard Window

The same UI serves both the desktop app (via Tauri webview) and the browser (`localhost:8402/dashboard`). Built with React + TailwindCSS, shared codebase.

**Tech stack (frontend):**
- React 18 + TypeScript
- TailwindCSS (styling)
- Radix UI / shadcn/ui (component library)
- Recharts (charts/visualizations)
- TanStack Query (data fetching, auto-refresh)
- react-hook-form + Zod (form validation)
- Framer Motion (animations)
- Lucide React (icons)
- react-i18next (i18n: English, Chinese, Japanese)

**Views:**

##### Overview (Landing)
- **Provider Status Cards** — each provider shows:
  - Name, billing type, connection status (green/yellow/red)
  - Marginal cost right now (color-coded: green=$0, yellow=cheap, red=expensive)
  - Quota bar (e.g., "47/300 premium requests used" or "Refreshes in 2h 14m")
  - Requests served today / errors
- **Routing Cascade Visualization** — live display of priority ordering:
  ```
  ① Ollama (local, $0) ✓ available
  ② Kimi K2 (unlimited, $0) ✓ available
  ③ Copilot GPT-4.1 (free included, $0) ✓ available
  ④ Copilot Claude Sonnet (quota, $0) ⚠ 12/300 remaining
  ⑤ Anthropic (refreshing, $0) ✗ depleted, resets in 1h 23m
  ⑥ GLM-4-flash (per-token, ¥0.1/M) ✓ available
  ⑦ Google AI Pro ($0 credits) ⚠ $2.14 of $10 remaining
  ⑧ OpenRouter ($0.003/1K) ✓ available (paid fallback)
  ```
- **Money Saved Counter** — real-time ticker: "Saved $47.23 today ($312.87 this month)"
- **Request Rate** — live requests/min sparkline

##### Request Timeline
- Live-updating feed of recent requests
- Each entry shows: timestamp, tier, provider chosen, model, tokens, cost, latency
- Color-coded by cost ($0 = green, paid = orange)
- Click to expand: full routing decision (all 15 dimension scores, candidate models considered, why the winner was chosen)

##### Provider Economics
- **Cost Comparison Table** — for each tier, show all candidate models with:
  - Provider, model name, marginal cost right now, quality score, latency p50
  - Highlight the model that would be chosen right now
- **Quota Burndown Chart** — time series showing quota depletion per provider
- **Credit Balance Over Time** — for per-token providers (OpenRouter, etc.)
- **Savings Waterfall** — bar chart: baseline cost → free providers savings → cheap provider savings → actual spend

##### Provider Config (Master Configuration)
- Add/edit/remove providers with visual form (not TOML editing)
- Drag-and-drop provider priority ordering
- Billing type picker with smart defaults
- Quota configuration (total, refresh interval, etc.)
- Test provider connectivity ("ping" button)
- One-click OAuth flow (Copilot)
- API key entry with validation
- Provider presets: one-click import for common providers (like CC Switch's 50+ presets)
- Import/Export configuration

##### Routing Playground
- Paste a prompt, see how the router would classify it
- Shows all 15 dimension scores, tier classification, candidate models, and final selection
- Interactive: drag sliders to adjust dimension weights, see how routing changes
- Useful for tuning routing profiles

##### Settings
- Routing profile management (create/edit/delete)
- Budget configuration (daily/monthly limits, alerts)
- General settings (port, log level, auto-start, etc.)
- Plugin management (WASM)
- Theme (dark/light/system)
- Language selection

#### 7.3 Tauri Architecture

```
coalesce-desktop/
├── src-tauri/                    # Rust backend (Tauri)
│   ├── src/
│   │   ├── main.rs              # Tauri app entry
│   │   ├── tray.rs              # System tray (dynamic menu, events)
│   │   ├── commands.rs          # Tauri IPC commands
│   │   ├── state.rs             # Shared app state
│   │   └── proxy_bridge.rs      # Bridge to coalesce-core
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── icons/                   # App + tray icons (all platforms)
│
├── src/                          # React frontend
│   ├── App.tsx
│   ├── main.tsx
│   ├── components/
│   │   ├── layout/              # Shell, sidebar, header
│   │   ├── providers/           # Provider cards, forms, presets
│   │   ├── dashboard/           # Overview, charts, timeline
│   │   ├── routing/             # Playground, profile editor
│   │   ├── settings/            # Config forms
│   │   └── shared/              # Buttons, inputs, modals
│   ├── hooks/                   # React hooks (useProviders, useStats, etc.)
│   ├── api/                     # API client (Tauri IPC or HTTP fetch)
│   ├── stores/                  # TanStack Query keys + fetchers
│   ├── types/                   # TypeScript types
│   ├── i18n/                    # Translation files (en, zh, ja)
│   └── lib/                     # Utilities
│
├── package.json
├── vite.config.ts
├── tailwind.config.ts
├── tsconfig.json
└── index.html
```

**Key: Dual-mode frontend.** The React app works in two modes:
1. **Desktop mode** (Tauri): Communicates via Tauri IPC commands to the Rust backend
2. **Web mode** (browser): Communicates via HTTP to `localhost:8402/api/v1/*` REST endpoints

The `api/` layer abstracts this: detects Tauri runtime → uses IPC; otherwise → uses fetch. Same components, same UI, two transports.

#### 7.4 Dashboard REST API (JSON)
All dashboard data available as JSON for the web UI and custom integrations:
```
GET  /api/v1/providers              — provider status + economics
GET  /api/v1/providers/quotas       — current quota state
POST /api/v1/providers              — add a provider
PUT  /api/v1/providers/:id          — update provider config
DEL  /api/v1/providers/:id          — remove provider

GET  /api/v1/routing/explain        — explain routing for a hypothetical request
POST /api/v1/routing/playground     — score a prompt, return full routing decision
GET  /api/v1/routing/profiles       — list routing profiles
PUT  /api/v1/routing/profiles/:id   — update profile

GET  /api/v1/stats/summary          — cost savings summary
GET  /api/v1/stats/timeline         — request timeline (paginated)
GET  /api/v1/stats/savings          — savings breakdown
GET  /api/v1/stats/by-provider      — per-provider analytics

SSE  /api/v1/events                 — live event stream (routing decisions, state changes)
```

#### 7.5 Platform-Specific Behavior

| Feature | macOS | Windows | Linux |
|---------|-------|---------|-------|
| Tray icon | Template (adapts to dark/light) | Standard colored icon | Standard icon |
| Left-click tray | Opens menu | Toggles dashboard window | Opens menu |
| Auto-start | Login items | Registry / Task Scheduler | XDG autostart |
| Config dir | `~/Library/Application Support/Coalesce/` | `%APPDATA%/Coalesce/` | `~/.config/coalesce/` |
| Minimize behavior | Close window → tray | Minimize to tray | Close window → tray |
| Native dialogs | macOS sheets | Win32 dialogs | GTK/Qt dialogs |

#### 7.6 Tauri Plugins Used
- `tauri-plugin-log` — logging with rotation
- `tauri-plugin-updater` — auto-updates from GitHub releases
- `tauri-plugin-dialog` — file open/save dialogs
- `tauri-plugin-opener` — open URLs in default browser
- `tauri-plugin-store` — persistent local preferences
- `tauri-plugin-single-instance` — prevent multiple instances
- `tauri-plugin-process` — manage proxy subprocess
- `tauri-plugin-autostart` — launch on system startup (optional)

---

### PHASE 8 — Built-in Load Testing

#### 7.1 `coalesce bench`
```
coalesce bench                          # Quick benchmark (100 requests)
coalesce bench --requests 10000         # Custom count
coalesce bench --concurrency 50         # Concurrent clients
coalesce bench --profile eco            # Test specific profile
coalesce bench --provider copilot       # Test specific provider
coalesce bench --report bench-results/  # Save detailed report
```

Measures:
- Routing decision throughput (requests/sec)
- End-to-end latency (p50, p95, p99)
- Provider latency by model
- Cost analysis
- Error rates

---

## Repository Structure (Cargo Workspace)

```
coalesce/
├── Cargo.toml                       # Workspace root
├── Cargo.lock
├── README.md
├── LICENSE                          # MIT
│
├── crates/
│   ├── coalesce-core/            # Core library (routing, economics, providers)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs            # Global config types
│   │       ├── error.rs             # Error types
│   │       ├── types.rs             # Shared types
│   │       │
│   │       ├── router/              # Smart routing engine
│   │       │   ├── mod.rs
│   │       │   ├── scorer.rs        # 15-dimension weighted scorer
│   │       │   ├── selector.rs      # Tier → model selection
│   │       │   ├── config.rs        # Routing profile config
│   │       │   ├── types.rs         # Tier, ScoringResult, etc.
│   │       │   ├── keywords.rs      # Multilingual keyword sets (9 langs)
│   │       │   └── adaptive.rs      # Quality-based adaptive routing
│   │       │
│   │       ├── economics/           # Provider economics engine
│   │       │   ├── mod.rs
│   │       │   ├── marginal_cost.rs # Real-time cost calculator
│   │       │   ├── quota_tracker.rs # Quota state machine
│   │       │   ├── billing.rs       # Billing type definitions
│   │       │   └── optimizer.rs     # Priority cascade (free → wait → paid)
│   │       │
│   │       ├── providers/           # Provider integrations
│   │       │   ├── mod.rs           # Provider trait + registry
│   │       │   ├── openrouter.rs
│   │       │   ├── copilot.rs       # GitHub Copilot (OAuth)
│   │       │   ├── ollama.rs
│   │       │   ├── glm.rs           # Zhipu AI / GLM
│   │       │   ├── kimi.rs          # Moonshot / Kimi
│   │       │   ├── openai.rs
│   │       │   ├── anthropic.rs     # + message format translation
│   │       │   ├── google.rs        # AI Studio / Vertex
│   │       │   ├── deepseek.rs
│   │       │   ├── xai.rs
│   │       │   └── health.rs        # Circuit breaker, health tracking
│   │       │
│   │       ├── auth/                # Authentication
│   │       │   ├── mod.rs
│   │       │   ├── copilot_oauth.rs # GitHub device flow
│   │       │   ├── token_manager.rs # Token caching & refresh
│   │       │   └── credentials.rs   # Credential storage
│   │       │
│   │       ├── cache/               # Caching layers
│   │       │   ├── mod.rs
│   │       │   ├── dedup.rs         # Request deduplication
│   │       │   ├── response.rs      # Response cache
│   │       │   ├── semantic.rs      # Semantic similarity cache
│   │       │   └── session.rs       # Session persistence
│   │       │
│   │       ├── storage/             # Persistent storage
│   │       │   ├── mod.rs
│   │       │   ├── sqlite.rs        # SQLite operations
│   │       │   ├── migrations.rs    # Schema migrations
│   │       │   └── models.rs        # DB models (requests, quotas, health)
│   │       │
│   │       ├── metrics/             # Observability
│   │       │   ├── mod.rs
│   │       │   ├── prometheus.rs    # Metrics registration
│   │       │   └── budget.rs        # Cost tracking & alerts
│   │       │
│   │       ├── plugins/             # WASM plugin system
│   │       │   ├── mod.rs
│   │       │   ├── host.rs          # wasmtime host
│   │       │   ├── interface.rs     # Plugin trait/ABI
│   │       │   └── loader.rs        # Plugin discovery & loading
│   │       │
│   │       └── sanitize/            # Request validation
│   │           ├── mod.rs
│   │           ├── validator.rs     # Schema validation
│   │           └── injection.rs     # Prompt injection detection
│   │
│   ├── coalesce-proxy/           # HTTP/gRPC proxy server
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs            # axum server setup
│   │       ├── handlers.rs          # Route handlers (/v1/chat/completions, etc.)
│   │       ├── api.rs               # Dashboard REST API (/api/v1/*)
│   │       ├── streaming.rs         # SSE streaming (upstream + downstream)
│   │       ├── middleware.rs         # Auth, logging, metrics, rate limiting
│   │       ├── grpc.rs              # gRPC server (tonic)
│   │       └── dashboard.rs         # Serve embedded web assets
│   │
│   └── coalesce-cli/             # CLI binary
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs              # Entry point, clap dispatch
│           ├── serve.rs             # Start proxy
│           ├── auth.rs              # Auth commands
│           ├── stats.rs             # Stats display
│           ├── history.rs           # Request history
│           ├── config_cmd.rs        # Config management
│           ├── doctor.rs            # Health checks
│           ├── bench.rs             # Load testing
│           ├── models.rs            # Model listing
│           └── exclude.rs           # Model blocklist
│
├── desktop/                         # Tauri 2 desktop app
│   ├── src-tauri/                   # Rust backend (Tauri shell)
│   │   ├── Cargo.toml              # depends on coalesce-core + coalesce-proxy
│   │   ├── tauri.conf.json
│   │   ├── capabilities/           # Tauri security capabilities
│   │   ├── icons/                  # App + tray icons (all platforms)
│   │   └── src/
│   │       ├── main.rs             # Tauri app entry
│   │       ├── tray.rs             # System tray (dynamic menu, events)
│   │       ├── commands.rs         # Tauri IPC commands → core
│   │       ├── state.rs            # Shared app state
│   │       └── proxy_manager.rs    # Start/stop/monitor embedded proxy
│   │
│   ├── src/                        # React frontend (shared web + desktop)
│   │   ├── App.tsx
│   │   ├── main.tsx
│   │   ├── components/
│   │   │   ├── layout/            # Shell, sidebar, header
│   │   │   ├── providers/         # Provider cards, forms, presets
│   │   │   ├── dashboard/         # Overview, charts, timeline
│   │   │   ├── routing/           # Playground, profile editor
│   │   │   ├── economics/         # Cost cascade, quota burndown, savings
│   │   │   ├── settings/          # Config forms
│   │   │   └── shared/            # Buttons, inputs, modals
│   │   ├── hooks/                 # React hooks (useProviders, useStats, etc.)
│   │   ├── api/                   # Dual-mode: Tauri IPC or HTTP fetch
│   │   │   ├── client.ts          # Auto-detects Tauri vs browser
│   │   │   ├── providers.ts
│   │   │   ├── routing.ts
│   │   │   ├── stats.ts
│   │   │   └── events.ts          # SSE event subscription
│   │   ├── stores/                # TanStack Query keys + fetchers
│   │   ├── types/                 # TypeScript types
│   │   ├── i18n/                  # en.json, zh.json, ja.json
│   │   └── lib/                   # Utilities
│   │
│   ├── package.json
│   ├── vite.config.ts
│   ├── tailwind.config.ts
│   ├── tsconfig.json
│   └── index.html
│
├── proto/                           # gRPC definitions
│   └── coalesce.proto
│
├── config/                          # Default configs shipped with binary
│   ├── default.toml                 # Default configuration
│   ├── providers.toml               # Default provider definitions
│   └── routing.toml                 # Default routing profiles
│
├── tests/                           # Integration tests
│   ├── routing_test.rs
│   ├── economics_test.rs
│   ├── proxy_test.rs
│   ├── provider_test.rs
│   ├── dedup_test.rs
│   └── streaming_test.rs
│
├── benches/                         # Benchmarks
│   ├── routing_bench.rs
│   ├── economics_bench.rs
│   └── parsing_bench.rs
│
└── .github/
    └── workflows/
        ├── ci.yml                   # Build, test, lint, bench
        ├── release-cli.yml          # CLI binary releases (all platforms)
        └── release-desktop.yml      # Desktop app releases (Tauri bundler)
```

---

## Configuration

### Main Config (`~/.coalesce/config.toml`)

```toml
[server]
port = 8402
grpc_port = 8403
host = "127.0.0.1"

[providers.openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"  # or inline: api_key = "sk-or-v1-..."
max_connections = 10
rate_limit_rpm = 60

[providers.copilot]
enabled = true
# Token stored in ~/.coalesce/copilot_token after `coalesce auth copilot`

[providers.ollama]
enabled = true
endpoint = "http://localhost:11434"
# Routes SIMPLE tier here when available

[providers.openai]
enabled = false
api_key_env = "OPENAI_API_KEY"

[providers.anthropic]
enabled = false
api_key_env = "ANTHROPIC_API_KEY"

[routing]
default_profile = "auto"
fallback_depth = 3

[cache]
dedup_ttl_secs = 30
semantic = false
session_timeout_mins = 30

[storage]
database = "~/.coalesce/coalesce.db"
log_file = "~/.coalesce/logs/coalesce.log"

[metrics]
enabled = true
port = 9090  # Prometheus scrape port

[budget]
daily_limit_usd = 0  # 0 = unlimited
monthly_limit_usd = 0
alert_threshold = 0.80
# webhook_url = "https://..."

[sanitize]
max_request_size_bytes = 1048576  # 1MB
max_messages = 100
injection_detection = "low"  # off/low/medium/high
rate_limit_rpm = 120  # per client IP

[plugins]
enabled = false
directory = "~/.coalesce/plugins"
```

---

## Implementation Phases

### Phase 1: Core Proxy & Router (Week 1-2)
- Cargo workspace scaffolding (coalesce-core, coalesce-proxy, coalesce-cli)
- axum HTTP server with /v1/chat/completions, /v1/models, /health
- 15-dimension scorer (port from ClawRouter TypeScript)
- TOML routing config with quality-based profiles
- OpenRouter provider (simplest — just API key + model discovery)
- Basic SSE streaming passthrough
- CLI: `coalesce serve`

### Phase 2: Economics Engine & Providers (Week 3-4)
- Provider economics engine (marginal cost calculator, billing types)
- Quota tracker state machine (monthly, refreshing, free credits)
- Priority cascade optimizer (free → wait → paid)
- GitHub Copilot OAuth device flow + token manager
- Ollama auto-detection and local provider
- GLM / Zhipu AI provider
- Kimi (Moonshot) provider
- Direct providers: Anthropic (with format translation), Google, OpenAI, xAI, DeepSeek
- CLI: `coalesce auth copilot`, `coalesce models`

### Phase 3: Reliability & Caching (Week 5)
- Fallback chain with circuit breakers per provider
- Request deduplication (SHA-256, dashmap)
- Session persistence (model pinning)
- Connection pooling tuning per provider
- Error classification and smart cooldowns (429, 503, etc.)

### Phase 4: Observability & Storage (Week 6)
- SQLite storage layer (requests, quotas, provider health)
- Prometheus metrics endpoint (/metrics)
- Structured logging (tracing crate, JSON output)
- Dashboard REST API (/api/v1/*)
- CLI: `coalesce stats`, `coalesce history`, `coalesce doctor`

### Phase 5: Advanced Features (Week 7-8)
- Multi-tenant profiles
- Request priority queues
- Cost budgeting with alerts (webhook, command)
- Adaptive routing (response quality scoring over time)
- Semantic similarity cache (opt-in)

### Phase 6: Plugins, gRPC & Security (Week 9)
- WASM plugin system (wasmtime)
- gRPC transport (tonic, alongside HTTP)
- Request sandboxing / prompt injection detection
- Load testing (`coalesce bench`)

### Phase 7: Desktop App (Week 10-12)
- Tauri 2 project scaffolding
- React + TailwindCSS + shadcn/ui frontend
- System tray with dynamic provider menu
- Dashboard views: Overview, Timeline, Economics, Config, Playground
- Dual-mode API client (Tauri IPC + HTTP fetch)
- Platform-specific behavior (macOS, Windows, Linux)
- Auto-start, auto-update, i18n (en, zh, ja)
- Web dashboard served from proxy binary (embedded assets via rust-embed)

### Phase 8: Polish & Release (Week 13)
- CI/CD: GitHub Actions for CLI + Desktop releases
- Cross-platform binary builds (x86_64 + aarch64 for macOS, Windows, Linux)
- Tauri bundler for .dmg, .msi, .AppImage, .deb
- Documentation (README, user guide)
- Provider presets library (one-click setup for common providers)
- End-to-end integration tests

---

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Cargo workspace (core/proxy/cli) + Tauri desktop | Shared Rust core between CLI and desktop app |
| HTTP framework | axum | Best tokio integration, tower middleware, streaming-first |
| Desktop framework | Tauri 2 | Rust backend, lightweight (vs Electron), native system tray, cross-platform |
| Frontend | React + TailwindCSS + shadcn/ui | Same stack as CC Switch (proven for this use case), rich component library |
| No x402 | Removed entirely | Not useful — replaced with economics engine |
| Routing philosophy | Dynamic marginal cost optimization | Not static tier-to-model mapping — understands subscriptions, quotas, free credits |
| Copilot OAuth | GitHub device flow | Free/included models, familiar pattern |
| OpenRouter | API key + live pricing sync | 300+ models, dynamic pricing for cost optimization |
| GLM / Zhipu AI | API key, OpenAI-compatible | Popular in Chinese market, cheap flash tier |
| Config format | TOML (files) + SQLite (state) | TOML for human-editable config, SQLite for quotas/history/state |
| Storage | SQLite (rusqlite) | Zero-config, embedded, queryable, shared by core + desktop |
| Plugin system | WASM (wasmtime) | Language-agnostic, sandboxed, Rust-native |
| gRPC | tonic | De facto Rust gRPC, shares tokio runtime |
| Metrics | metrics crate | More ergonomic than prometheus-rs, same Prometheus output |
| Dual UI | Web dashboard + Desktop app | Web = master config (accessible anywhere), Desktop = system tray quick access |

---

## Compatibility

- **API**: OpenAI-compatible (`/v1/chat/completions`, `/v1/models`)
- **Drop-in**: Works with any OpenAI SDK by setting base URL to `http://localhost:8402/v1`
- **Port**: Default 8402 (same as ClawRouter for familiarity)
- **Config dir**: Platform-aware (`~/.config/coalesce/` Linux, `~/Library/Application Support/Coalesce/` macOS, `%APPDATA%/Coalesce/` Windows)
- **Desktop**: .dmg (macOS), .msi (Windows), .AppImage + .deb (Linux)
- **CLI**: Single binary, all platforms
