# Coalesce Architecture

This document explains how the Coalesce LLM router works internally.

## Overview

Coalesce is a Rust proxy that sits between LLM clients and LLM providers. It receives OpenAI-format (or Anthropic-format) requests, scores them, picks the cheapest capable model, and forwards the request. It handles failures, retries, streaming, quota tracking, and cost accounting transparently.

## Crate Structure

```
crates/
  coalesce-core/       # No network code. Pure logic.
    src/
      router/          # 15-dimension scorer, quality tiers, model pins
      economics/       # BillingType, marginal cost, quota tracker, budget
      providers/       # Provider trait + implementations (Copilot, OpenRouter, Ollama, etc.)
      storage/         # SQLite: requests, stats, key-value config
      plugins/         # Hook trait (on_request, on_route, on_response)
      sanitize/        # Prompt injection detection
      types.rs         # ChatRequest, ModelInfo, QualityTier, etc.
      config.rs        # AppConfig, ProviderConfig, deserialization
      presets.rs       # Default provider configs

  coalesce-proxy/      # The actual server. All network I/O lives here.
    src/
      lib.rs           # ~3000 lines. ProxyState, all HTTP handlers, startup, routing logic
      dashboard/       # Embedded HTML fallback dashboard

  coalesce-cli/        # Thin wrapper. Parses args, calls proxy::start_server()
    src/main.rs

desktop/               # Tauri 2 app + React frontend
  src/                 # React components (Chat, Providers, ProviderConfig, CostAnalytics, etc.)
  src-tauri/           # Tauri shell: system tray, window management
```

## Request Flow (Detailed)

### 1. Ingress

Three entry points, all in `coalesce-proxy/src/lib.rs`:

- **`POST /v1/chat/completions`** -- OpenAI-compatible. This is what Claude Code, Cursor, and most tools use.
- **`POST /v1/messages`** -- Anthropic Messages API. Converts to internal format, routes, converts back.
- **gRPC `ChatCompletion`** -- Protobuf on port 8403. Same routing logic, different wire format.

All three converge into the same `chat_completions` handler (or equivalent for Anthropic/gRPC).

### 2. Request Parsing

The handler parses the incoming JSON into a `ChatRequest`:

```rust
pub struct ChatRequest {
    pub model: String,           // "auto", "gpt-4o", specific model, or tier name
    pub messages: Vec<Message>,
    pub stream: Option<bool>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub tools: Option<Vec<Value>>,
    pub top_p: Option<f64>,
    // ... other OpenAI fields
}
```

If `model` is `"auto"` or a tier name like `"complex"`, the router picks the best model. If it's a specific model ID, the router still validates it exists and checks the provider is available.

### 3. Scoring (Router)

The 15-dimension scorer in `coalesce-core/src/router/` analyzes the last user message:

| Dimension | Weight | What it measures |
|-----------|--------|------------------|
| token_count | 0.15 | Message length |
| code_presence | 0.12 | Code blocks, programming keywords |
| reasoning_markers | 0.15 | "analyze", "compare", "why" |
| technical_terms | 0.08 | Domain-specific vocabulary |
| multi_step | 0.10 | Numbered lists, sequential instructions |
| creativity | 0.08 | Creative writing signals |
| conversation_depth | 0.07 | Number of prior messages |
| ambiguity | 0.05 | Vague or open-ended phrasing |
| domain_specificity | 0.05 | Specialized fields |
| output_format | 0.04 | JSON/structured output requests |
| instruction_complexity | 0.04 | Nested or conditional instructions |
| context_window | 0.03 | Total context size |
| safety_sensitivity | 0.02 | Sensitive topics |
| language_complexity | 0.01 | Vocabulary sophistication |
| recency_requirement | 0.01 | Needs current information |

The weighted sum produces a score (0.0 - 1.0) mapped to a tier:

| Score Range | Tier |
|-------------|------|
| 0.00 - 0.12 | Simple |
| 0.12 - 0.25 | Medium |
| 0.25 - 0.40 | Complex |
| 0.40+ | Reasoning |

### 4. Candidate Selection

All models matching the tier are gathered from `state.models`. Filters:

- **Disabled** -- Skip models/providers the user has bypassed
- **Circuit breaker** -- Skip providers in Open (failed) state
- **Capabilities** -- If request has `tools`, only models with `tool_calling = true`. If request has images, only `vision = true` models
- **Model pins** -- If routing pins are configured for this tier, only those providers/models are considered

### 5. Economics Ranking

For each candidate, the `EconomicsEngine` computes marginal cost:

```
EconomicsEngine::marginal_cost(model, est_input_tokens, est_output_tokens)
  -> MarginalCost { Free { reason } | Paid { usd } | Depleted | Unavailable }
```

The cost depends on the provider's `BillingType` and current quota state:

- **Local/Free/Unlimited/QuotaWithinLimit** -> `MarginalCost::Free`
- **QuotaOnly + depleted** -> `MarginalCost::Unavailable` (skipped entirely)
- **QuotaMetered + depleted** -> `MarginalCost::Paid` (falls back to per-token)
- **PerToken** -> `MarginalCost::Paid { estimated_usd }` (computed from model pricing)

Candidates are sorted: Free first, then by USD cost ascending. Within equal cost, provider priority (lower number = first) breaks ties. Subscription-mode providers are preferred over metered-mode at equal priority.

### 6. Rosetta Layer

If the request includes tools, the Rosetta context (`coalesce-core/src/rosetta/`) handles:

- **Canonical tool types** -- Maps provider-specific tools to canonical equivalents
- **Equivalence classes** -- Groups tools that do the same thing across providers
- **Capability routing** -- Ensures the selected provider supports the required tool capabilities
- **Egress substitution** -- On the way out, substitutes canonical tool references with provider-specific ones

### 7. Fallback Loop

The routing loop iterates over ranked candidates:

```rust
for candidate_idx in 0..candidates.len() {
    if attempts_made >= attempt_limit { break; }

    // Skip if provider auth already failed this request
    if auth_failed_providers.contains(&candidate.provider) { continue; }

    // Try the provider
    match provider.chat_stream(&request).await {
        Ok(stream) => return stream,    // Success!
        Err(e) if is_auth_error(e) => {
            auth_failed_providers.insert(candidate.provider);
            // DON'T increment attempts_made -- try next candidate
            continue;
        }
        Err(e) => {
            attempts_made += 1;         // Real failure, burns an attempt
            circuit_breaker.record_failure();
            continue;
        }
    }
}
```

Key design: auth-skipped providers don't burn attempt slots. This ensures the router reaches lower-priority providers even if higher-priority ones have expired tokens.

### 8. Provider Dispatch

Each provider implements the `Provider` trait:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    async fn chat(&self, request: &ChatRequest) -> Result<Value>;
    async fn chat_stream(&self, request: &ChatRequest) -> Result<ByteStream>;
    async fn health_check(&self) -> Result<bool>;
}
```

Provider implementations:
- **`OpenAICompatProvider`** -- Generic provider for any OpenAI-format API. Used by OpenRouter, GLM, Kimi, DeepSeek, OpenAI, xAI via factory functions that set the endpoint URL
- **`CopilotProvider`** -- GitHub Copilot with OAuth device flow, token refresh, multi-account support
- **`OllamaProvider`** -- Local Ollama with model pulling, preloading, and tag-based discovery
- **`AnthropicProvider`** -- Direct Anthropic API with thinking blocks and tool use
- **`GoogleCloudCodeProvider`** -- Google Cloud Code Assist internal API (uses Antigravity OAuth tokens)
- **`OpenRouterProvider`** -- OpenRouter with model discovery from their API

### 9. Streaming

For streaming requests, the proxy:
1. Opens a streaming connection to the upstream provider
2. Wraps it in an SSE transform that adds routing metadata headers
3. Pipes bytes through to the client without buffering the full response
4. Records token counts and cost from the final usage chunk

Thinking/reasoning tokens (from models like Claude, DeepSeek R1, GLM) are passed through as `reasoning_content` in the delta.

### 10. Accounting

After each request:
- **Quota state updated** -- `QuotaState.used += 1`, credits consumed
- **Circuit breaker notified** -- success resets failure count, failure increments it
- **Request logged** -- SQLite row with provider, model, tier, cost, latency, status
- **Budget checked** -- If spending exceeds thresholds, alerts are emitted
- **Stats aggregated** -- Per-provider totals for the dashboard

## State Management

All runtime state lives in `ProxyState` (a single `Arc`):

```rust
pub struct ProxyState {
    pub config: AppConfig,                    // Loaded at startup, immutable
    pub providers: RwLock<Vec<Arc<dyn Provider>>>,  // Hot-swappable provider list
    pub models: RwLock<Vec<ModelInfo>>,        // Discovered models from all providers
    pub economics: EconomicsEngine,           // Billing types + quota tracking (DashMap inside)
    pub circuit_breakers: DashMap<String, CircuitBreaker>,
    pub storage: Storage,                     // SQLite connection
    pub provider_priorities: DashMap<String, u32>,   // Lower = tried first
    pub provider_pricing_modes: DashMap<String, String>,  // "subscription" or "metered"
    pub disabled_providers: DashMap<String, bool>,
    pub disabled_models: DashMap<String, bool>,
    pub rules: RulesEngine,                   // Auto-failover rules
    pub rosetta: RosettaContext,              // Tool equivalence + capability routing
    // ... budget, dedup, sessions, events, etc.
}
```

### Persistence

- **SQLite** (`~/.local/share/coalesce/coalesce.db`) -- Request history, stats, key-value store (tokens, disabled states, settings)
- **providers.json** (`~/.local/share/coalesce/providers.json`) -- Dynamically added providers with API keys, billing config
- **provider_priorities.json** -- Provider priority and pricing mode overrides
- **ollama_preload.json** -- Models to auto-load on startup

The `config.toml` is read-only at startup. All runtime changes go through the JSON files and SQLite.

## Frontend Architecture

The React frontend (`desktop/src/`) communicates with the proxy via REST API at `http://127.0.0.1:8402/api/v1/...`. It's served as static files from `desktop/dist/` by the proxy's Axum server.

Key components:
- **ProviderConfig.tsx** -- Add/remove providers, set billing and priority, Copilot account ordering
- **Providers.tsx** -- Model browser with per-model overrides, enable/disable toggles
- **Chat.tsx** -- Streaming chat with model selection, thinking token display
- **CostAnalytics.tsx** -- Savings waterfall, budget tracking
- **UsageMetrics.tsx** -- Per-provider token usage and quota burndown
- **RoutingPlayground.tsx** -- Dry-run scorer with full dimension breakdown

The Tauri desktop app (`desktop/src-tauri/`) wraps the React app in a native window with system tray integration. It starts the proxy as a subprocess.
