# Coalesce RFC-001: Rosetta — Semantic Tool Bridging with Capability-Aware Routing and Tool Equivalence Classes

**Status:** Draft  
**Author:** Michael Curtis  
**Date:** 2026-04-02  
**Applies to:** Coalesce v2.x  

---

## Abstract

Coalesce routes LLM requests to the cheapest capable provider. Today, that routing is content-aware but tool-blind — it scores the *prompt* across 15 dimensions but treats attached tools as opaque pass-through payloads. This means a request carrying 40 tools with `strict: true` and parallel call requirements might get routed to a provider that silently degrades on all three.

This RFC proposes **Rosetta**, a semantic tool bridging layer that makes tools first-class citizens in the routing decision. Rosetta introduces three concepts:

1. **Universal Tool Plugins** — a canonical tool definition format that authors write once, with Coalesce handling per-provider translation at route time.
2. **Tool Equivalence Classes** — a registry that maps semantically identical capabilities across providers (e.g., Claude's `web_search` server tool, Kimi's `$web_search` built-in, OpenAI's `web_search` built-in, and a user-provided scraper function are all members of the `web_search` equivalence class).
3. **Capability-Aware Routing** — extending the 15-dimension scorer with tool-derived signals so the router can eliminate, penalize, or prefer providers based on their tool support characteristics.

The system exposes both OpenAI (`/v1/chat/completions`) and Anthropic (`/v1/messages`) frontend APIs, enabling Claude Code, Cursor, and other native clients to speak their preferred protocol while Coalesce handles translation and routing underneath.

---

## Motivation

### The Tool Compatibility Problem

Every major LLM provider supports tool/function calling. They all converged on roughly the same idea — JSON Schema definitions, model-initiated calls, user-executed results — but diverged on everything else:

| Dimension | OpenAI (Chat Completions) | OpenAI (Responses API) | Anthropic (Messages) | Kimi / Moonshot | GLM / Zhipu |
|---|---|---|---|---|---|
| **Tool definition wrapper** | `{type: "function", function: {name, description, parameters}}` | `{type: "function", name, description, parameters}` (flat) | `{name, description, input_schema}` | OpenAI-compatible | OpenAI-compatible |
| **Tool call location** | `message.tool_calls[]` | `output[].type == "function_call"` | `content[].type == "tool_use"` | `message.tool_calls[]` | `message.tool_calls[]` |
| **Tool result role** | `role: "tool"` with `tool_call_id` | `type: "function_call_output"` with `call_id` | `role: "user"` with `content[].type: "tool_result"` and `tool_use_id` | `role: "tool"` with `tool_call_id` | `role: "tool"` with `tool_call_id` |
| **Parallel calls** | Yes, controllable via `parallel_tool_calls` | Yes | Yes, native | Yes | Yes |
| **Strict schema** | `strict: true` (requires `additionalProperties: false`) | `strict: true` | `strict: true` (beta) | No | No |
| **Built-in tools** | `web_search`, `code_interpreter`, `file_search`, `computer_use`, MCP | `web_search`, `code_execution`, MCP, `tool_search` | `web_search`, `web_fetch`, `code_execution`, `bash`, `text_editor`, `computer_use`, `memory`, `tool_search` | `$web_search` (built-in, special execution model) | None documented |
| **Tool search / deferred loading** | `defer_loading: true` + `tool_search` (gpt-5.4+) | Same | `tool_search` (beta) | No | No |
| **Thinking + tools** | o-series constraints on `tool_choice` | Same | Extended thinking interacts with tool blocks | `tool_choice` restricted to auto/none with thinking; must preserve `reasoning_content` | Unknown |
| **Streaming tool deltas** | `delta.tool_calls[].function.arguments` (chunked JSON) | Event-based | `content_block_delta` with `input_json_delta` | OpenAI-compatible | OpenAI-compatible |
| **Max tools before degradation** | ~128 (documented), effective ~20-30 | Namespaces help | ~128, but `tool_search` addresses this | Unknown, likely <20 | Unknown |

The OpenAI-compatible surface that Coalesce currently exposes papers over the definition format, but it cannot bridge the behavioral and capability differences. A request that works perfectly on Claude (using `web_search` as a server tool with `tool_result` blocks) will fail on Kimi (which uses `$web_search` with a completely different execution model) even though both providers accomplish the same thing.

### What This Costs Today

Without tool-aware routing, Coalesce users hit these failure modes:

- **Silent degradation**: Request routed to a provider that can't enforce `strict: true`, resulting in malformed tool arguments that break downstream code.
- **Missing built-in tools**: Client expects provider-side web search, but the selected provider has no equivalent — request fails or model hallucinates a response.
- **Parallel call failures**: Provider returns parallel tool calls, but in an order or format the client doesn't expect because it was built for a different provider's behavior.
- **Thinking mode conflicts**: Request with extended thinking enabled gets routed to a provider where thinking + tools has constraints the client didn't account for.
- **Tool count overflow**: 50-tool request routed to a provider that degrades badly past 15 tools, producing garbage tool selections.

### The Opportunity

A routing proxy is the *ideal* place to solve this. Coalesce already sits between client and provider. It already scores requests. It already handles fallback chains. Adding tool awareness to the scoring and translation to the provider adapters turns Coalesce from a smart cost optimizer into the only LLM gateway that can genuinely promise tool portability.

---

## Design

### Architecture Overview

```
                    Clients
              (Claude Code, Cursor, SDKs, agents)
                         |
            +------------+------------+
            |                         |
   Anthropic Frontend         OpenAI Frontend
   /v1/messages               /v1/chat/completions
            |                         |
            +------------+------------+
                         |
                  +------+------+
                  |   Rosetta   |
                  |  Ingress    |
                  |  Normalizer |
                  +------+------+
                         |
              Canonical Request + Canonical Tools
                         |
                  +------+------+
                  |   Router    |
                  | 15+N dim    |
                  |  scorer     |
                  +------+------+
                         |
                  +------+------+
                  |  Rosetta    |
                  |  Egress     |
                  |  Translator |
                  +------+------+
                         |
              Provider-Native Request
              (with tool schema translation,
               built-in tool substitution,
               capability constraints applied)
                         |
            +------+-----+-----+------+
            |      |           |      |
         Claude  OpenAI     Kimi    GLM  ...
```

Rosetta operates as two phases — **ingress normalization** and **egress translation** — with the router making decisions on the canonical form in between.

### Canonical Tool Representation

All tools flowing through Coalesce are normalized to a `CanonicalTool` representation. This is the internal lingua franca — a superset that can express every provider's native capabilities without information loss.

```rust
/// A tool definition in Coalesce's canonical form.
/// This is a superset — fields that don't apply to a given
/// provider are simply dropped during egress translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalTool {
    /// Unique name. For equivalence class members, this is
    /// the canonical name (e.g., "web_search"), not the
    /// provider-specific name (e.g., "$web_search").
    pub name: String,

    /// Human-readable description for the model.
    pub description: String,

    /// JSON Schema for the tool's input parameters.
    pub input_schema: serde_json::Value,

    /// Whether the client requires strict schema enforcement.
    /// Providers that can't enforce this get penalized in routing.
    pub strict: bool,

    /// Tool execution model.
    pub execution: ToolExecution,

    /// Equivalence class membership, if any.
    pub equivalence_class: Option<String>,

    /// Provider-specific metadata that should be preserved
    /// through the round-trip (e.g., Anthropic's cache_control).
    pub provider_hints: HashMap<String, serde_json::Value>,

    /// Whether this tool can be deferred (loaded on demand).
    pub defer_loading: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolExecution {
    /// Client executes the tool and returns results.
    /// This is the standard model for all providers.
    ClientSide,

    /// Provider executes the tool server-side.
    /// The proxy may need to substitute a client-side
    /// implementation if the routed provider lacks this.
    ServerSide {
        /// Provider that offers this as a server tool.
        /// If routing goes elsewhere, Rosetta must either
        /// substitute a client-side fallback or skip this tool.
        native_provider: String,
    },

    /// Coalesce-managed tool (Universal Tool Plugin).
    /// Coalesce executes this itself, injecting results
    /// back into the conversation regardless of provider.
    ProxyManaged {
        /// Reference to the WASM plugin or native handler.
        handler: ToolHandlerRef,
    },
}
```

### Canonical Tool Call / Result

The response side is similarly normalized:

```rust
/// A tool invocation as returned by the model,
/// normalized to canonical form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalToolCall {
    /// Unique ID for this invocation. Generated by the model
    /// or by Rosetta if the provider doesn't produce one.
    pub id: String,

    /// Tool name (canonical, not provider-specific).
    pub name: String,

    /// Arguments as a JSON value.
    pub arguments: serde_json::Value,
}

/// A tool result being sent back to the model,
/// normalized to canonical form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalToolResult {
    /// The tool_call ID this result corresponds to.
    pub call_id: String,

    /// Result content. Can be text, structured JSON,
    /// images, or error messages.
    pub content: ToolResultContent,

    /// Whether this result represents an error.
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolResultContent {
    Text(String),
    Json(serde_json::Value),
    Mixed(Vec<ContentBlock>),  // For Anthropic-style multi-block results
}
```

### Tool Equivalence Classes

An equivalence class groups tools that accomplish the same semantic task across different providers and implementations. This is the core abstraction that enables transparent tool substitution.

```rust
/// A group of tools that accomplish the same semantic task.
/// When routing, Rosetta can substitute any member for another
/// based on what the target provider supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEquivalenceClass {
    /// Canonical name for this capability (e.g., "web_search").
    pub name: String,

    /// Human-readable description of what this class of tools does.
    pub description: String,

    /// Members of the equivalence class, ordered by preference.
    pub members: Vec<EquivalenceMember>,

    /// Whether members are truly interchangeable or have
    /// known fidelity differences that should be surfaced.
    pub fidelity_notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquivalenceMember {
    /// Which provider offers this implementation.
    pub provider: String,

    /// The provider-specific tool identifier.
    /// e.g., "web_search_20260209" for Anthropic,
    ///       "$web_search" for Kimi,
    ///       "web_search" built-in for OpenAI.
    pub provider_tool_id: String,

    /// How this tool is executed on this provider.
    pub execution: ToolExecution,

    /// Capability flags that differ between members.
    pub capabilities: MemberCapabilities,

    /// Fidelity score (0.0 - 1.0) relative to the "best"
    /// implementation. Used in routing cost calculations.
    pub fidelity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberCapabilities {
    /// Does this member return structured data or just text?
    pub structured_output: bool,

    /// Does it support citations / source attribution?
    pub citations: bool,

    /// Maximum input size (if applicable).
    pub max_input: Option<usize>,

    /// Any known limitations.
    pub limitations: Vec<String>,
}
```

#### Built-in Equivalence Classes

Coalesce ships with equivalence classes for common built-in tools:

| Class | Anthropic | OpenAI | Kimi | GLM | Fallback |
|---|---|---|---|---|---|
| `web_search` | `web_search_20260209` (server) | `web_search_20250305` (built-in) | `$web_search` (built-in, special) | None | User-provided or proxy-managed plugin |
| `code_execution` | `code_execution_20250825` (server) | `code_interpreter` (built-in) | None | None | Proxy-managed sandbox |
| `web_fetch` | `web_fetch_20250305` (server) | None | None | None | Proxy-managed HTTP client |
| `computer_use` | `computer_20250124` (client) | `computer_use_preview` (built-in) | None | None | None (capability gate) |
| `text_editor` | `text_editor_20250124` (client) | None | None | None | Proxy-managed file editor |
| `bash` | `bash_20250124` (client) | None | None | None | Proxy-managed shell |

When a request arrives using, say, Anthropic's `web_search` server tool and gets routed to Kimi, Rosetta can:
1. Recognize `web_search` as a member of the `web_search` equivalence class.
2. Look up Kimi's equivalent (`$web_search`).
3. Translate the tool definition and adjust the execution flow to match Kimi's special built-in model.

If no provider equivalent exists and a proxy-managed plugin is registered for that class, Coalesce executes the tool itself and injects the result, making the tool available on *every* provider.

### Universal Tool Plugins

Universal Tool Plugins are the mechanism by which users and third parties create tools that work across all providers. A plugin author defines the tool's schema and implementation once; Coalesce handles everything else.

```rust
/// Trait that Universal Tool Plugins implement.
/// Can be compiled to native Rust or WASM.
pub trait UniversalTool: Send + Sync {
    /// Returns the tool's canonical definition.
    fn definition(&self) -> CanonicalTool;

    /// Executes the tool with the given arguments.
    /// Returns a result that Rosetta will translate
    /// into the appropriate provider format.
    fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<CanonicalToolResult, ToolError>> + Send>>;

    /// Optional: declare equivalence class membership.
    /// If this tool is a fallback for a provider-native capability,
    /// register it here so Rosetta can substitute when needed.
    fn equivalence_class(&self) -> Option<String> {
        None
    }

    /// Optional: declare provider-specific optimizations.
    /// If the plugin knows how to produce a better definition
    /// for a specific provider, it can override here.
    fn provider_override(&self, provider: &str) -> Option<CanonicalTool> {
        None
    }
}

/// Context available to tool plugins during execution.
pub struct ToolContext {
    /// The original request metadata (model, tier, etc.).
    pub request_meta: RequestMeta,

    /// HTTP client for tools that need network access.
    pub http_client: reqwest::Client,

    /// Key-value store for tool state persistence.
    pub store: Arc<dyn ToolStore>,

    /// Budget remaining (tools can be cost-aware).
    pub budget_remaining: Option<f64>,
}
```

#### Plugin Lifecycle

```
  Plugin Author                     Coalesce
  ─────────────                     ────────
  
  1. Implement UniversalTool trait
     (Rust or WASM)
                    ─────────►
                                2. Load plugin at startup
                                   (native) or on-demand (WASM)
                                
                                3. Register tool definition in
                                   the CanonicalTool registry
                                
                                4. If equivalence_class() is Some,
                                   add as fallback member in the
                                   equivalence class registry
                                   
  ═══════════════════════════════════════════════
  At request time:
  ═══════════════════════════════════════════════
  
                                5. Client sends request with tools
                                
                                6. Rosetta Ingress normalizes to
                                   CanonicalTool form
                                   
                                7. Router scores, selects provider
                                   (tool capabilities influence score)
                                   
                                8. Rosetta Egress checks each tool:
                                   
                                   a. Provider has native equivalent?
                                      → Use provider's native tool
                                      
                                   b. Provider supports client-side
                                      tool with same schema?
                                      → Translate schema, pass through
                                      
                                   c. Neither? Plugin is ProxyManaged?
                                      → Coalesce will execute it
                                      → Inject as tool definition for
                                        the model to call
                                      → Intercept the tool_call
                                      → Execute plugin
                                      → Inject result into conversation
                                      
                                   d. None of the above?
                                      → Drop tool + warn, or
                                        reject route + try fallback
```

#### Plugin Packaging

Plugins are distributed as either:

- **Native Rust crates** — compiled into the Coalesce binary or loaded as dynamic libraries. Best performance, used for core/built-in tools.
- **WASM modules** — sandboxed, portable, hot-loadable. The intended distribution format for community plugins. Loaded from local filesystem or fetched from a plugin registry.

```toml
# coalesce.toml — plugin configuration

[[plugins]]
name = "web-scraper"
source = "wasm:///plugins/web-scraper.wasm"
equivalence_class = "web_search"  # Acts as fallback for web_search
config = { max_results = 10, timeout_ms = 5000 }

[[plugins]]
name = "sql-query"
source = "wasm:///plugins/sql-query.wasm"
config = { connection_string = "postgres://..." }

[[plugins]]
name = "jira-lookup"
source = "native"  # Built into the binary
config = { base_url = "https://mycompany.atlassian.net", token_env = "JIRA_TOKEN" }
```

### Capability-Aware Routing

The existing 15-dimension scorer evaluates the prompt content. Rosetta adds **tool-derived dimensions** that feed into the same scoring pipeline.

#### New Scoring Dimensions

| Dimension | Signal | Effect |
|---|---|---|
| `tool_count` | Number of tools in the request | Penalize providers known to degrade with many tools; prefer providers with `tool_search`/deferred loading if count > threshold |
| `tool_schema_complexity` | Nesting depth, number of properties, use of `anyOf`/`oneOf` | Penalize providers without `strict` mode if schemas are complex |
| `strict_required` | Any tool has `strict: true` | Eliminate providers that don't support strict schema enforcement |
| `parallel_calls_expected` | Heuristic from tool count + prompt phrasing | Penalize providers with known parallel call bugs |
| `server_tools_requested` | Request includes provider-specific server tools | Strong affinity to the native provider; fallback requires equivalence class resolution |
| `thinking_plus_tools` | Request has `thinking` enabled AND tools | Eliminate providers with known thinking+tools incompatibilities |
| `equivalence_coverage` | What % of requested tools have equivalents on each provider | Prefer providers that can handle all tools natively |
| `proxy_managed_count` | Number of tools Coalesce would need to execute itself | Cost signal — proxy execution adds latency and compute |

#### Modified Scoring Flow

```
  Existing 15 dimensions (content-based)
              │
              ▼
  ┌─────────────────────┐
  │  Content Score       │  (0.0 - 1.0 → tier classification)
  │  Simple/Med/Complex/ │
  │  Reasoning           │
  └──────────┬──────────┘
             │
             ▼
  ┌─────────────────────┐
  │  Tool Capability     │  NEW: per-provider compatibility
  │  Filter              │  check against tool dimensions
  │                      │
  │  - Hard gates:       │  strict_required + no strict → ELIMINATE
  │    provider must     │  server_tool + no equivalent → ELIMINATE
  │    meet minimums     │  thinking+tools conflict → ELIMINATE
  │                      │
  │  - Soft penalties:   │  tool_count > threshold → penalize
  │    adjust effective  │  low equivalence_coverage → penalize
  │    cost              │  high proxy_managed_count → penalize
  └──────────┬──────────┘
             │
             ▼
  ┌─────────────────────┐
  │  Economics Engine    │  Marginal cost calculation
  │  (existing)          │  now includes tool execution
  │                      │  cost estimates
  └──────────┬──────────┘
             │
             ▼
  ┌─────────────────────┐
  │  Provider Selection  │  Cheapest provider that passed
  │                      │  all hard gates, with soft
  │                      │  penalties factored into cost
  └─────────────────────┘
```

### Provider Capability Registry

Each provider adapter declares its tool capabilities statically. This registry is the source of truth for the routing filter.

```rust
/// Declares what a provider supports for tool calling.
/// Each provider adapter populates this at registration time.
#[derive(Debug, Clone)]
pub struct ProviderToolCapabilities {
    /// Maximum number of tools before known degradation.
    pub max_tools_comfortable: usize,

    /// Maximum number of tools the API accepts at all.
    pub max_tools_hard_limit: usize,

    /// Supports `strict: true` schema enforcement.
    pub strict_mode: bool,

    /// Supports parallel tool calls.
    pub parallel_tool_calls: bool,

    /// Can the client control parallel calls (e.g., `parallel_tool_calls: false`)?
    pub parallel_calls_controllable: bool,

    /// Supports tool_search / deferred loading.
    pub tool_search: bool,

    /// Supports thinking/reasoning mode concurrently with tools.
    pub thinking_with_tools: ThinkingToolsSupport,

    /// Built-in tools this provider offers (by equivalence class name).
    pub builtin_tools: HashMap<String, BuiltinToolInfo>,

    /// Provider-specific streaming behavior for tool calls.
    pub streaming_tool_deltas: StreamingToolBehavior,

    /// Known quirks that affect tool call reliability.
    pub quirks: Vec<ProviderQuirk>,
}

#[derive(Debug, Clone)]
pub enum ThinkingToolsSupport {
    /// No thinking mode available.
    NoThinking,
    /// Thinking works with tools, no restrictions.
    Full,
    /// Thinking works with tools but has constraints.
    Constrained {
        /// e.g., "tool_choice must be auto or none"
        constraints: Vec<String>,
    },
    /// Thinking and tools are mutually exclusive.
    Incompatible,
}

#[derive(Debug, Clone)]
pub enum ProviderQuirk {
    /// Provider sometimes duplicates tool calls when parallel is enabled.
    ParallelCallDuplication,
    /// Provider strips trailing newlines from string parameters.
    StripsTrailingNewlines,
    /// Provider has trouble with deeply nested schemas (>3 levels).
    DeepSchemaDegradation { max_depth: usize },
    /// Provider's tool_call IDs are not globally unique across turns.
    NonUniqueToolCallIds,
    /// Custom quirk with description.
    Other(String),
}
```

### Anthropic Frontend API

To support Claude Code and other Anthropic-native clients, Coalesce exposes the Anthropic Messages API as a first-class frontend alongside the existing OpenAI endpoint.

```
  Port 8402 (existing)              Port 8404 (new)
  ─────────────────────             ─────────────────────
  OpenAI-compatible                 Anthropic-compatible
  /v1/chat/completions              /v1/messages
  /v1/models                        /v1/messages (streaming)
                                    /v1/models (mapped)
          │                                  │
          ▼                                  ▼
  ┌───────────────┐                ┌─────────────────┐
  │ OpenAI        │                │ Anthropic       │
  │ Ingress       │                │ Ingress         │
  │ Normalizer    │                │ Normalizer      │
  └───────┬───────┘                └────────┬────────┘
          │                                  │
          └──────────────┬───────────────────┘
                         │
              Canonical Request + Canonical Tools
                         │
                    (same routing pipeline)
```

This means Claude Code can point at Coalesce with:

```bash
export ANTHROPIC_BASE_URL=http://localhost:8404
export ANTHROPIC_API_KEY=anything  # Coalesce manages real keys
```

And Coalesce will:
1. Parse the Anthropic Messages format natively (thinking blocks, tool_use/tool_result blocks, system as top-level field, etc.).
2. Normalize to canonical form.
3. Route to the cheapest capable provider — which might be Claude via direct API, or might be GPT-4o via OpenAI if the request is simple enough and Claude is more expensive.
4. Translate back to Anthropic Messages format for the response, regardless of which provider actually served it.

This preserves Anthropic-native features when routing to Claude, and gracefully degrades them when routing elsewhere.

### Egress Translation — The Provider Adapters

Each provider has an egress adapter that translates from canonical form to native API format. The adapter is responsible for:

1. **Schema translation** — converting `CanonicalTool` definitions into the provider's expected format.
2. **Built-in tool substitution** — replacing equivalence class members with the provider's native built-in when available.
3. **Result normalization** — converting provider-specific tool call responses back to `CanonicalToolCall` form.
4. **Quirk mitigation** — applying workarounds for known provider quirks.

```rust
/// Trait implemented by each provider's egress adapter.
pub trait ProviderToolAdapter: Send + Sync {
    /// Translate canonical tool definitions to provider-native format.
    fn translate_tools(
        &self,
        tools: &[CanonicalTool],
        equivalence_registry: &EquivalenceRegistry,
    ) -> Result<ProviderToolPayload, TranslationError>;

    /// Translate a provider-native tool call response
    /// back to canonical form.
    fn normalize_tool_calls(
        &self,
        raw_response: &serde_json::Value,
    ) -> Result<Vec<CanonicalToolCall>, TranslationError>;

    /// Translate canonical tool results into provider-native format
    /// for the continuation request.
    fn translate_tool_results(
        &self,
        results: &[CanonicalToolResult],
    ) -> Result<serde_json::Value, TranslationError>;

    /// Declare this provider's tool capabilities.
    fn capabilities(&self) -> &ProviderToolCapabilities;
}
```

### Proxy-Managed Tool Execution

When a tool is `ProxyManaged` (either a Universal Tool Plugin or an equivalence class fallback), Coalesce handles the execution loop:

```
  Client ──► Coalesce ──► Provider
                              │
                         Model returns tool_call
                         for proxy-managed tool
                              │
                         ◄────┘
                    │
                    ▼
            ┌──────────────┐
            │ Coalesce     │
            │ executes the │
            │ WASM/native  │
            │ plugin       │
            └──────┬───────┘
                   │
                   ▼
            Tool result injected
            as tool_result message
                   │
                   ▼
            Continuation request
            sent to SAME provider
            with result in context
                   │
              ──────────────► Provider
                                  │
                             Final response
                                  │
                             ◄────┘
                    │
                    ▼
              Response returned to client
```

This is transparent to both the client and the provider. The model sees a normal tool it can call; the client sees a normal response. Coalesce mediates the execution in the middle.

**Cost implications**: Proxy-managed execution adds latency (plugin execution time) and cost (the continuation request). The economics engine accounts for this when comparing routes.

---

## Implementation Plan

### Phase 1: Canonical Form + Schema Translation (Foundation)

**Goal**: All tools flowing through Coalesce are represented in canonical form. Provider adapters can translate tool definitions and tool calls bidirectionally.

- Define `CanonicalTool`, `CanonicalToolCall`, `CanonicalToolResult` types in `coalesce-core`.
- Implement `ProviderToolAdapter` for each existing provider.
- Add Anthropic frontend on port 8404 with ingress normalizer.
- Tests: round-trip translation fidelity for each provider pair.

**Delivers**: Anthropic clients (Claude Code) can connect to Coalesce. Tool definitions are correctly translated when routing between providers. No new routing intelligence yet.

### Phase 2: Capability-Aware Routing

**Goal**: The router considers tool requirements when scoring and selecting providers.

- Define `ProviderToolCapabilities` for each provider.
- Add tool-derived scoring dimensions to the router.
- Implement hard gates (strict mode, thinking+tools) and soft penalties (tool count, schema complexity).
- Extend the routing playground to show tool capability analysis.

**Delivers**: Requests with tools are routed to providers that can actually handle them. The dashboard shows why a provider was eliminated or penalized.

### Phase 3: Tool Equivalence Classes

**Goal**: Coalesce can substitute semantically equivalent tools across providers.

- Define `ToolEquivalenceClass` and `EquivalenceRegistry`.
- Ship built-in equivalence classes for web_search, code_execution, web_fetch.
- Implement substitution logic in egress adapters.
- Handle the special cases (Kimi's `$web_search` echo-back model, Anthropic's server tool execution model).

**Delivers**: A request using Claude's `web_search` can be transparently routed to Kimi or OpenAI with equivalent functionality. Providers without built-in equivalents get the tool injected as a client-side definition.

### Phase 4: Universal Tool Plugins + WASM

**Goal**: Third parties can create tools that work on every provider.

- Define the `UniversalTool` trait and `ToolContext`.
- Implement WASM plugin loading with wasmtime.
- Build the proxy-managed execution loop (tool call interception → plugin execution → result injection → continuation).
- Create a plugin packaging format and CLI for building/testing plugins.
- Ship 3-5 reference plugins: web scraper, calculator, file reader, HTTP client, SQL query.

**Delivers**: Community-creatable tools that work everywhere. The plugin ecosystem.

### Phase 5: Plugin Registry + Marketplace

**Goal**: Discoverability and distribution for Universal Tool Plugins.

- Plugin registry (simple: Git repo with manifests; later: hosted service).
- `coalesce plugin install <name>` CLI command.
- Plugin signing and verification.
- Usage telemetry (opt-in) for plugin quality signals.

**Delivers**: The community flywheel. People share tools, Coalesce gets more valuable.

---

## Open Questions

1. **Equivalence class fidelity**: When Coalesce substitutes Kimi's `$web_search` for Claude's `web_search`, the results will differ. How much fidelity loss is acceptable before we should block the route rather than substitute? Should this be user-configurable?

2. **Proxy-managed tool cost model**: When Coalesce executes a tool itself, it incurs compute cost and the cost of a continuation request. How should this be represented in the economics engine? As a flat surcharge? As a multiplier on the provider's per-token cost?

3. **Streaming + proxy execution**: If the model streams a tool call for a proxy-managed tool, Coalesce needs to buffer the complete tool call, execute the plugin, then initiate a non-streamed continuation. The client's stream will have a gap. Is this acceptable, or do we need to send keep-alive events during plugin execution?

4. **Multi-turn tool state**: Some tools are stateful across turns (e.g., a database connection, a browser session). The `ToolStore` in `ToolContext` handles this, but what happens when a fallback routes a subsequent turn to a *different* provider? The tool state is in Coalesce, but the conversation context is now on a new provider.

5. **MCP integration**: Should Coalesce speak MCP natively as both a client (connecting to MCP servers) and a host (exposing tools as MCP endpoints)? This would make Coalesce a universal MCP bridge.

6. **Anthropic frontend feature depth**: How much of the Anthropic API should the frontend support? Minimum viable: `/v1/messages` with tools, thinking, and streaming. Full: prompt caching, batch API, citations, files API, etc.

---

## Prior Art and References

- **LiteLLM** (BerriAI) — Python proxy with tool call translation across 100+ providers. Handles schema translation but not semantic bridging or capability-aware routing.
- **ToolRegistry** (Oaklight/Peng Ding) — Protocol-agnostic tool management library with unified registration across Python functions, MCP, OpenAPI, and LangChain. Academic paper: arXiv:2507.10593.
- **StrongDM Attractor** — Unified LLM SDK spec that argues for native provider adapters over compatibility shims. Key insight: provider-native features are lost in translation layers.
- **Mirrowel/LLM-API-Key-Proxy** — FastAPI proxy exposing both OpenAI and Anthropic endpoints. Proves the dual-frontend concept.
- **openziti/llm-gateway** — Go gateway with Anthropic ↔ OpenAI translation and semantic routing.
- **ToolRosetta** (arXiv:2603.09290) — Automated conversion of GitHub repositories into MCP-compatible tool services. Relevant for the plugin ecosystem vision.

---

## Appendix A: Provider Tool Format Quick Reference

### OpenAI Chat Completions — Tool Definition
```json
{
  "type": "function",
  "function": {
    "name": "get_weather",
    "description": "Get weather for a location",
    "strict": true,
    "parameters": {
      "type": "object",
      "properties": {
        "location": { "type": "string" }
      },
      "required": ["location"],
      "additionalProperties": false
    }
  }
}
```

### OpenAI Responses API — Tool Definition
```json
{
  "type": "function",
  "name": "get_weather",
  "description": "Get weather for a location",
  "strict": true,
  "parameters": {
    "type": "object",
    "properties": {
      "location": { "type": "string" }
    },
    "required": ["location"],
    "additionalProperties": false
  }
}
```

### Anthropic Messages API — Tool Definition
```json
{
  "name": "get_weather",
  "description": "Get weather for a location",
  "input_schema": {
    "type": "object",
    "properties": {
      "location": { "type": "string" }
    },
    "required": ["location"]
  }
}
```

### Anthropic — Tool Use Block (in response)
```json
{
  "type": "tool_use",
  "id": "toolu_01ABC",
  "name": "get_weather",
  "input": { "location": "Seattle" }
}
```

### Anthropic — Tool Result (in continuation request)
```json
{
  "role": "user",
  "content": [
    {
      "type": "tool_result",
      "tool_use_id": "toolu_01ABC",
      "content": "72°F, partly cloudy"
    }
  ]
}
```

### OpenAI — Tool Call (in response)
```json
{
  "tool_calls": [
    {
      "id": "call_abc123",
      "type": "function",
      "function": {
        "name": "get_weather",
        "arguments": "{\"location\": \"Seattle\"}"
      }
    }
  ]
}
```

### OpenAI — Tool Result (in continuation request)
```json
{
  "role": "tool",
  "tool_call_id": "call_abc123",
  "content": "72°F, partly cloudy"
}
```

### Kimi — Built-in Web Search (special execution model)
```json
{
  "type": "function",
  "function": {
    "name": "$web_search",
    "description": "",
    "parameters": {}
  }
}
```
*Note: Kimi's `$web_search` is executed by the model itself. The client receives `tool_calls` with `function.name: "$web_search"`, and must return `function.arguments` as-is in the tool result. The model processes the search internally.*

---

## Appendix B: Equivalence Class Resolution Algorithm

```
Given: A set of canonical tools T and a target provider P

For each tool t in T:
  1. If t.execution is ClientSide:
     → Translate t.input_schema to P's format
     → Include in P's tool list
     
  2. If t.execution is ServerSide { native_provider }:
     a. If P == native_provider:
        → Use P's native server tool (no translation needed)
     b. If t.equivalence_class is Some(class_name):
        i.  Look up class_name in EquivalenceRegistry
        ii. Find member where member.provider == P
        iii. If found:
             → Substitute P's native equivalent
             → Adjust execution flow if needed
        iv. If not found but ProxyManaged fallback exists:
             → Register as proxy-managed tool
             → Coalesce will execute it
        v.  If not found and no fallback:
             → Mark tool as unavailable on P
             → If tool is required: ELIMINATE P as candidate
             → If tool is optional: WARN and continue
     c. If t.equivalence_class is None:
        → Tool is provider-specific with no equivalent
        → ELIMINATE P as candidate (tool is required)
        
  3. If t.execution is ProxyManaged:
     → Always available (Coalesce executes it)
     → Translate schema to P's format
     → Include in P's tool list
     → Register for interception in the execution loop
```
