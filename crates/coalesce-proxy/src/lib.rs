pub mod grpc;
pub mod harness;
pub mod rules;
pub mod token_vault;

use token_vault::TokenVault;
use coalesce_core::cache::dedup::{DedupAction, DedupResult, RequestDedup};
use coalesce_core::cache::semantic::SemanticCache;
use coalesce_core::economics::budget::BudgetTracker;
use coalesce_core::config::{AppConfig, ProviderConfig};
use coalesce_core::economics::billing::BillingType;
use coalesce_core::economics::marginal_cost::MarginalCost;
use coalesce_core::economics::optimizer::EconomicsEngine;
use coalesce_core::providers::anthropic::AnthropicProvider;
use coalesce_core::providers::copilot::CopilotProvider;
use coalesce_core::providers::google_cloudcode::GoogleCloudCodeProvider;
use coalesce_core::providers::health::CircuitBreaker;
use coalesce_core::providers::ollama::OllamaProvider;
use coalesce_core::providers::openai_compat::factories;
use coalesce_core::providers::openrouter::OpenRouterProvider;
use coalesce_core::providers::Provider;
use coalesce_core::cache::response::{ResponseCache, ResponseCacheConfig};
use coalesce_core::providers::mock::MockProvider;
use coalesce_core::rosetta::RosettaContext;
use coalesce_core::mcp::{McpRegistry, McpScanner, McpServerConfig, McpTransport, McpConfigSource};
use coalesce_core::rosetta::thinking_optimizer::{ThinkingOptimizer, ThinkingOptimizerConfig};
use coalesce_core::storage::{RequestLogEntry, Storage};
use coalesce_core::types::{ChatRequest, ContentPart, Message, MessageContent, ModelInfo, QualityTier, derive_canonical_family};
use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
    Router,
};
use dashmap::DashMap;
use futures::StreamExt;
use metrics_exporter_prometheus::PrometheusHandle;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Semaphore};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{debug, error, info, warn};
use base64::Engine as _;

/// Maximum fallback attempts before giving up
const MAX_FALLBACK_ATTEMPTS: usize = 3;

#[derive(Debug, Deserialize)]
struct PaginationParams {
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct PlaygroundRequest {
    prompt: String,
    weights: Option<coalesce_core::router::config::DimensionWeights>,
}

/// Session tracking for model pinning
pub struct SessionInfo {
    model_id: String,
    provider: String,
    last_seen: Instant,
    request_count: u64,
}

/// Default session timeout: 30 minutes
const SESSION_TIMEOUT_SECS: u64 = 1800;

/// Events broadcast to SSE subscribers
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum ProxyEvent {
    #[serde(rename = "routing_decision")]
    RoutingDecision {
        tier: String,
        provider: String,
        model: String,
        score: f64,
        cost_usd: f64,
        attempt: usize,
    },
    #[serde(rename = "request_complete")]
    RequestComplete {
        provider: String,
        model: String,
        latency_ms: u64,
        success: bool,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },
    #[serde(rename = "provider_status")]
    ProviderStatus {
        provider: String,
        state: String,
        failures: u64,
    },
    #[serde(rename = "budget_alert")]
    BudgetAlert {
        threshold_pct: u32,
        spent_usd: f64,
        limit_usd: f64,
    },
}

/// Per-tier concurrency limiters
pub struct PriorityRouter {
    pub reasoning: Arc<Semaphore>,
    pub complex: Arc<Semaphore>,
    pub medium: Arc<Semaphore>,
    pub simple: Arc<Semaphore>,
}

impl PriorityRouter {
    pub fn new() -> Self {
        Self {
            reasoning: Arc::new(Semaphore::new(50)),
            complex: Arc::new(Semaphore::new(75)),
            medium: Arc::new(Semaphore::new(100)),
            simple: Arc::new(Semaphore::new(200)),
        }
    }

    pub fn semaphore_for(&self, tier: &str) -> Arc<Semaphore> {
        match tier {
            "REASONING" => self.reasoning.clone(),
            "COMPLEX" => self.complex.clone(),
            "MEDIUM" => self.medium.clone(),
            _ => self.simple.clone(),
        }
    }
}

/// Adaptive quality scoring with moving average
pub struct QualityScorer {
    scores: DashMap<String, MovingAverage>,
}

struct MovingAverage {
    sum: f64,
    count: u64,
}

impl QualityScorer {
    pub fn new() -> Self {
        Self {
            scores: DashMap::new(),
        }
    }

    pub fn record(&self, provider: &str, model: &str, success: bool, latency_ms: u64) {
        let key = format!("{}:{}", provider, model);
        let mut entry = self.scores.entry(key).or_insert(MovingAverage { sum: 0.0, count: 0 });
        // Score: 1.0 for success, 0.0 for failure, penalize high latency
        let score = if success {
            (1.0 - (latency_ms as f64 / 30000.0).min(0.5)).max(0.5)
        } else {
            0.0
        };
        entry.sum += score;
        entry.count += 1;
    }

    pub fn score(&self, provider: &str, model: &str) -> f64 {
        let key = format!("{}:{}", provider, model);
        self.scores
            .get(&key)
            .map(|e| if e.count > 0 { e.sum / e.count as f64 } else { 0.5 })
            .unwrap_or(0.5)
    }

    pub fn all_scores(&self) -> Vec<(String, f64, u64)> {
        self.scores
            .iter()
            .map(|e| {
                let avg = if e.count > 0 { e.sum / e.count as f64 } else { 0.5 };
                (e.key().clone(), avg, e.count)
            })
            .collect()
    }
}

pub struct ProxyState {
    pub config: AppConfig,
    pub providers: RwLock<Vec<Arc<dyn Provider>>>,
    pub models: RwLock<Vec<ModelInfo>>,
    pub economics: EconomicsEngine,
    pub circuit_breakers: DashMap<String, CircuitBreaker>,
    pub storage: Storage,
    pub dedup: RequestDedup,
    pub budget: BudgetTracker,
    pub sessions: DashMap<String, SessionInfo>,
    pub event_tx: broadcast::Sender<ProxyEvent>,
    pub prometheus_handle: PrometheusHandle,
    pub priority: PriorityRouter,
    pub quality: QualityScorer,
    pub semantic_cache: SemanticCache,
    pub ollama_preload: RwLock<Vec<String>>,
    pub model_aliases: DashMap<String, String>,
    pub model_pins: RwLock<std::collections::HashMap<coalesce_core::types::QualityTier, Vec<coalesce_core::router::config::ModelPin>>>,
    /// Provider priority (lower = tried first). Key = provider name.
    pub provider_priorities: DashMap<String, u32>,
    /// Provider pricing mode. Key = provider name, value = "subscription" or "metered".
    pub provider_pricing_modes: DashMap<String, String>,
    /// Disabled providers — excluded from routing. Key = provider name.
    pub disabled_providers: DashMap<String, bool>,
    /// Disabled models — excluded from routing. Key = "provider::model_id".
    pub disabled_models: DashMap<String, bool>,
    /// Auto-failover rules engine
    pub rules: rules::RulesEngine,
    /// Rosetta: canonical tool types, equivalence classes, capability-aware routing
    pub rosetta: RosettaContext,
    /// Thinking optimizer — auto-enables extended thinking for capable models
    pub thinking_optimizer: ThinkingOptimizer,
    /// Secure credential vault for provider tokens
    pub token_vault: TokenVault,
    /// Response cache (content-hash based)
    pub response_cache: ResponseCache,
    /// MCP server registry
    pub mcp_registry: McpRegistry,
    /// Mock provider enabled flag
    pub mock_enabled: std::sync::atomic::AtomicBool,
}

impl ProxyState {
    /// Resolve a model ID to its canonical name using both config equivalences and runtime aliases.
    fn canonical_model_id(&self, model_id: &str) -> String {
        // Check runtime aliases first (populated from persisted equivalences)
        if let Some(canonical) = self.model_aliases.get(model_id) {
            return canonical.clone();
        }
        // Fall back to config equivalences
        self.config.routing.canonical_model_id(model_id)
    }
}

pub async fn start_server(mut config: AppConfig) -> anyhow::Result<()> {
    let addr = format!("{}:{}", config.server.host, config.server.port);

    // Initialize storage
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("coalesce");
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("coalesce.db");
    let storage = Storage::open(&db_path)?;
    info!("Database: {}", db_path.display());

    // Try to get a fresh Google token:
    // 1. Read from Antigravity's DB and validate it
    // 2. Fall back to refreshing our own stored refresh token
    let antigravity_token = read_antigravity_token();
    let google_token = if let Some(ref at) = antigravity_token {
        // Validate the Antigravity token with a quick API call
        let valid = validate_google_token(at).await;
        if valid {
            antigravity_token
        } else {
            info!("  google — Antigravity token expired, trying refresh token");
            None
        }
    } else {
        None
    };
    // Fall back to our own refresh token if Antigravity token was missing or expired
    let google_token = google_token.or_else(|| {
        if let Ok(Some(rt)) = storage.get("google_refresh_token") {
            let rt_clone = rt.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    match refresh_google_token(&rt_clone).await {
                        Ok(token) => {
                            info!("  google — refreshed token via stored refresh token");
                            Some(token)
                        }
                        Err(e) => {
                            warn!("  google — refresh token failed: {}", e);
                            None
                        }
                    }
                })
            })
        } else {
            None
        }
    });

    if let Some(ref token) = google_token {
        let _ = storage.set("google_access_token", token);

        // Discover Google models via loadCodeAssist (gets tier + project)
        let models_for_config = discover_google_models(token).await;
        let model_count = models_for_config.len();

        if !config.providers.contains_key("google") {
            config.providers.insert("google".into(), ProviderConfig {
                enabled: true,
                api_key: Some(token.clone()),
                ..Default::default()
            });
        } else if let Some(gc) = config.providers.get_mut("google") {
            gc.api_key = Some(token.clone());
            gc.enabled = true;
        }

        // Store the project ID for chat requests
        if let Some(project_id) = get_google_project_id(token).await {
            let _ = storage.set("google_project_id", &project_id);
        }

        info!("  google — token ready, {} models from tier", model_count);
    }

    // Initialize providers and discover models
    let (mut providers, mut models, economics, circuit_breakers) = init_providers(&config).await;

    // Inject Google Cloud Code provider and models
    if let Some(ref token) = google_token {
        let project_id = storage.get("google_project_id").ok().flatten().unwrap_or_default();
        let google_provider: Arc<dyn Provider> = Arc::new(GoogleCloudCodeProvider::new(token.clone(), project_id));
        providers.push(google_provider);
        circuit_breakers.insert("google".into(), CircuitBreaker::new(5, 60));

        let google_models = discover_google_models(token).await;
        models.retain(|m| m.provider != "google");
        for m in &google_models {
            economics.register("google", Some(&m.id), BillingType::PerToken);
        }
        models.extend(google_models);
    }

    info!(
        "Loaded {} providers with {} models",
        providers.len(),
        models.len()
    );

    let budget = BudgetTracker::new(config.budget.total_limit_usd, config.budget.daily_limit_usd);

    // Initialize Prometheus metrics
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder");

    // Event broadcast channel for SSE subscribers
    let (event_tx, _) = broadcast::channel::<ProxyEvent>(256);

    // Semantic cache
    let semantic_cache = SemanticCache::new(coalesce_core::cache::semantic::SemanticCacheConfig {
        enabled: config.semantic_cache.enabled,
        similarity_threshold: config.semantic_cache.similarity_threshold,
        max_entries: config.semantic_cache.max_entries,
        ttl_secs: config.semantic_cache.ttl_secs,
    });

    let state = Arc::new(ProxyState {
        config,
        providers: RwLock::new(providers),
        models: RwLock::new(models),
        economics,
        circuit_breakers,
        storage,
        dedup: RequestDedup::new(30),
        budget,
        sessions: DashMap::new(),
        event_tx,
        prometheus_handle,
        priority: PriorityRouter::new(),
        quality: QualityScorer::new(),
        semantic_cache,
        ollama_preload: RwLock::new(Vec::new()),
        model_aliases: DashMap::new(),
        model_pins: RwLock::new(std::collections::HashMap::new()),
        provider_priorities: DashMap::new(),
        provider_pricing_modes: DashMap::new(),
        disabled_providers: DashMap::new(),
        disabled_models: DashMap::new(),
        rules: rules::RulesEngine::new(),
        rosetta: RosettaContext::new(),
        thinking_optimizer: ThinkingOptimizer::new(ThinkingOptimizerConfig::default()),
        token_vault: TokenVault::new(),
        response_cache: ResponseCache::new(ResponseCacheConfig::default()),
        mcp_registry: McpRegistry::new(),
        mock_enabled: std::sync::atomic::AtomicBool::new(false),
    });

    // Load disabled providers/models from DB
    if let Ok(entries) = state.storage.get_matching("disabled_provider:%") {
        for (key, val) in entries {
            if val == "1" {
                if let Some(name) = key.strip_prefix("disabled_provider:") {
                    state.disabled_providers.insert(name.to_string(), true);
                    info!("  {} — disabled", name);
                }
            }
        }
    }
    if let Ok(entries) = state.storage.get_matching("disabled_model:%") {
        for (key, val) in entries {
            if val == "1" {
                if let Some(k) = key.strip_prefix("disabled_model:") {
                    state.disabled_models.insert(k.to_string(), true);
                }
            }
        }
    }

    // Initialize provider priorities, pricing modes, and billing from config
    for (name, pconfig) in &state.config.providers {
        state.provider_priorities.insert(name.clone(), pconfig.priority);
        state.provider_pricing_modes.insert(name.clone(), pconfig.pricing_mode.clone());
    }

    // Load persisted provider priorities/pricing modes (override config)
    {
        let prio_path = data_dir.join("provider_priorities.json");
        if prio_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&prio_path) {
                if let Ok(saved) = serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(&contents) {
                    for (name, val) in &saved {
                        if let Some(p) = val.get("priority").and_then(|v| v.as_u64()) {
                            state.provider_priorities.insert(name.clone(), p as u32);
                        }
                        if let Some(m) = val.get("pricing_mode").and_then(|v| v.as_str()) {
                            state.provider_pricing_modes.insert(name.clone(), m.to_string());
                        }
                    }
                    info!("Loaded provider priorities for {} providers", saved.len());
                }
            }
        }
    }

    // Load persisted dynamic providers (added via UI) so they survive restarts
    {
        let providers_path = data_dir.join("providers.json");
        if providers_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&providers_path) {
                if let Ok(saved) = serde_json::from_str::<std::collections::HashMap<String, ProviderConfig>>(&contents) {
                    let mut loaded_count = 0;
                    for (name, pconfig) in &saved {
                        // Skip if already loaded from config file
                        if state.providers.read().unwrap().iter().any(|p| p.name() == name.as_str()) {
                            continue;
                        }
                        let api_key = resolve_api_key(pconfig);
                        let provider: Option<Arc<dyn Provider>> = match name.as_str() {
                            "openrouter" => api_key.map(|k| Arc::new(OpenRouterProvider::new(k)) as Arc<dyn Provider>),
                            "ollama" => Some(Arc::new(OllamaProvider::new(pconfig.endpoint.clone()))),
                            "copilot" => Some(Arc::new(if let Some(t) = api_key { CopilotProvider::with_token(t) } else { CopilotProvider::new() })),
                            "anthropic" | "claude" => api_key.map(|k| Arc::new(AnthropicProvider::new(k)) as Arc<dyn Provider>),
                            "glm" | "zhipu" => api_key.map(|k| Arc::new(factories::glm(k)) as Arc<dyn Provider>),
                            "deepseek" => api_key.map(|k| Arc::new(factories::deepseek(k)) as Arc<dyn Provider>),
                            "openai" => api_key.map(|k| Arc::new(factories::openai(k)) as Arc<dyn Provider>),
                            "xai" | "grok" => api_key.map(|k| Arc::new(factories::xai(k)) as Arc<dyn Provider>),
                            "kimi" | "moonshot" => api_key.map(|k| Arc::new(factories::kimi(k)) as Arc<dyn Provider>),
                            "google" | "gemini" => api_key.map(|k| build_google_provider(k, &state.storage)),
                            _ => None,
                        };
                        if let Some(p) = provider {
                            match p.list_models().await {
                                Ok(new_models) => {
                                    let billing = parse_billing(pconfig);
                                    state.economics.register(name, None::<&str>, billing);
                                    state.circuit_breakers.insert(name.clone(), CircuitBreaker::default_provider());
                                    let count = new_models.len();
                                    state.models.write().unwrap().extend(new_models);
                                    state.providers.write().unwrap().push(p);
                                    state.provider_priorities.entry(name.clone()).or_insert(pconfig.priority);
                                    state.provider_pricing_modes.entry(name.clone()).or_insert(pconfig.pricing_mode.clone());
                                    info!("  {} — {} models (restored)", name, count);
                                    loaded_count += 1;
                                }
                                Err(e) => {
                                    warn!("  {} — failed to restore: {}", name, e);
                                }
                            }
                        }
                    }
                    if loaded_count > 0 {
                        info!("Restored {} dynamic providers from {}", loaded_count, providers_path.display());
                    }
                }
            }
        }
    }

    // Restore saved Copilot OAuth tokens from DB
    {
        if let Ok(tokens) = state.storage.get_matching("%_github_token") {
            for (key, token) in &tokens {
                let prov_name = key.trim_end_matches("_github_token").to_string();
                // Skip if already loaded
                if state.providers.read().unwrap().iter().any(|p| p.name() == prov_name) {
                    continue;
                }
                let provider = Arc::new(CopilotProvider::with_token_and_name(token.clone(), prov_name.clone()));
                match provider.list_models().await {
                    Ok(new_models) => {
                        let count = new_models.len();
                        state.economics.register(&prov_name, None::<&str>, BillingType::QuotaRefreshing { quota_per_window: 50, refresh_interval_secs: 18000 });
                        state.circuit_breakers.insert(prov_name.clone(), CircuitBreaker::default_provider());
                        state.models.write().unwrap().extend(new_models);
                        state.providers.write().unwrap().push(provider);
                        info!("  {} — {} models (restored from saved token)", prov_name, count);
                    }
                    Err(e) => {
                        warn!("  {} — failed to restore Copilot token: {}", prov_name, e);
                    }
                }
            }
            if !tokens.is_empty() {
                info!("Restored {} Copilot account(s)", tokens.len());
            }
        }
    }

    // Load persisted preload list and auto-load models
    {
        let preload_path = data_dir.join("ollama_preload.json");
        if preload_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&preload_path) {
                if let Ok(list) = serde_json::from_str::<Vec<String>>(&contents) {
                    info!("Loaded {} preload models from {}", list.len(), preload_path.display());
                    *state.ollama_preload.write().unwrap() = list;
                }
            }
        }
        // Spawn background task to warm preloaded models
        let preload_state = state.clone();
        tokio::spawn(async move {
            // Wait for Ollama to be ready
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            let models: Vec<String> = preload_state.ollama_preload.read().unwrap().clone();
            if models.is_empty() { return; }
            let client = reqwest::Client::new();
            for model_name in &models {
                info!("Preloading model: {}", model_name);
                let _ = client.post("http://localhost:11434/api/generate")
                    .json(&serde_json::json!({
                        "model": model_name,
                        "prompt": "",
                        "stream": false,
                        "keep_alive": "-1",
                    }))
                    .send()
                    .await;
            }
            info!("Preload complete: {} models loaded", models.len());
        });
    }

    // Load persisted model pins
    {
        let pins_path = data_dir.join("model_pins.json");
        if pins_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&pins_path) {
                if let Ok(pins) = serde_json::from_str::<std::collections::HashMap<coalesce_core::types::QualityTier, Vec<coalesce_core::router::config::ModelPin>>>(&contents) {
                    let count: usize = pins.values().map(|v| v.len()).sum();
                    info!("Loaded {} model pins across {} tiers", count, pins.len());
                    *state.model_pins.write().unwrap() = pins;
                }
            }
        }
    }

    // Load persisted model equivalences
    {
        let eq_path = data_dir.join("model_equivalences.json");
        if eq_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&eq_path) {
                if let Ok(eqs) = serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(&contents) {
                    info!("Loaded {} model equivalence groups", eqs.len());
                    // Populate model_aliases for immediate routing
                    for (canonical, aliases) in &eqs {
                        for alias in aliases {
                            if alias != canonical {
                                state.model_aliases.insert(alias.clone(), canonical.clone());
                            }
                        }
                    }
                    // Store in routing config — we have Arc so we need to work around immutability
                    // The equivalences are used via model_aliases DashMap at runtime
                }
            }
        }
    }

    // Apply model overrides from DB
    apply_all_overrides(&state);
    if let Ok(overrides) = state.storage.get_all_model_overrides() {
        if !overrides.is_empty() {
            info!("Applied {} model overrides", overrides.len());
        }
    }

    // Spawn periodic cleanup task (dedup + sessions)
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            cleanup_state.dedup.cleanup();
            // Clean expired sessions
            cleanup_state.sessions.retain(|_, s| {
                s.last_seen.elapsed().as_secs() < SESSION_TIMEOUT_SECS
            });
        }
    });

    // Spawn Google OAuth token refresh loop (tokens expire after ~1 hour)
    let google_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(45 * 60));
        interval.tick().await; // skip immediate tick
        loop {
            interval.tick().await;
            // Check if we have a refresh token
            let refresh_token = google_state.storage.get("google_refresh_token").ok().flatten();
            if let Some(rt) = refresh_token {
                match refresh_google_token(&rt).await {
                    Ok(new_token) => {
                        let _ = google_state.storage.set("google_access_token", &new_token);
                        // Replace the Google provider with a fresh one
                        let new_provider: Arc<dyn Provider> = Arc::new(
                            GoogleCloudCodeProvider::new(new_token.clone(), google_state.storage.get("google_project_id").ok().flatten().unwrap_or_default())
                        );
                        let mut providers = google_state.providers.write().unwrap();
                        if let Some(pos) = providers.iter().position(|p| p.name() == "google") {
                            providers[pos] = new_provider;
                            info!("  google — background token refresh succeeded");
                        }
                        // Also update config so new provider instances use fresh token
                        drop(providers);
                    }
                    Err(e) => {
                        warn!("  google — background token refresh failed: {}", e);
                    }
                }
            }
        }
    });

    // Spawn gRPC server on port + 1 (default 8403)
    let grpc_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = grpc::start_grpc_server(grpc_state).await {
            error!("gRPC server error: {}", e);
        }
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers([
            "x-coalesce-model".parse().unwrap(),
            "x-coalesce-provider".parse().unwrap(),
            "x-coalesce-tier".parse().unwrap(),
            "x-coalesce-attempt".parse().unwrap(),
            "x-coalesce-session-id".parse().unwrap(),
        ]);

    // Check if React web UI is available — if so, it takes over / and /dashboard
    let web_dirs = vec![
        std::path::PathBuf::from("desktop/dist"),
        std::path::PathBuf::from("../desktop/dist"),
        data_dir.join("web"),
    ];
    let has_web_ui = web_dirs.iter().any(|d| d.join("index.html").exists());

    let mut app = Router::new()
        .route("/dashboard/embedded", get(dashboard))
        .route("/health", get(health))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/stats", get(stats))
        // Dashboard REST API
        .route("/api/v1/providers", get(api_providers))
        .route("/api/v1/providers/quotas", get(api_providers_quotas))
        .route("/api/v1/routing/playground", post(api_routing_playground))
        .route("/api/v1/routing/profiles", get(api_routing_profiles))
        .route("/api/v1/routing/pins", get(api_routing_pins_get).put(api_routing_pins_set))
        .route("/api/v1/routing/equivalences", get(api_equivalences_get).put(api_equivalences_set))
        .route("/api/v1/providers/priorities", get(api_provider_priorities_get).put(api_provider_priorities_set))
        .route("/api/v1/stats/summary", get(api_stats_summary))
        .route("/api/v1/stats/timeline", get(api_stats_timeline))
        .route("/api/v1/stats/costs", get(api_stats_costs))
        .route("/api/v1/events", get(api_events))
        .route("/api/v1/providers/manage", post(api_create_provider))
        .route("/api/v1/providers/manage/{name}", put(api_update_provider).delete(api_delete_provider))
        .route("/api/v1/providers/{name}/toggle", post(api_provider_toggle))
        .route("/api/v1/providers/{name}/models/{model}/toggle", post(api_model_toggle))
        .route("/api/v1/providers/{name}/billing", put(api_update_billing))
        .route("/api/v1/providers/{name}/test", post(api_test_provider))
        .route("/api/v1/providers/{name}/refresh", post(api_provider_refresh))
        .route("/api/v1/auth/copilot/start", post(api_copilot_auth_start))
        .route("/api/v1/auth/copilot/poll", post(api_copilot_auth_poll))
        .route("/api/v1/providers/ollama/models", get(api_ollama_models))
        .route("/api/v1/ollama/pull", post(api_ollama_pull))
        .route("/api/v1/ollama/models/{model}", delete(api_ollama_delete_model))
        .route("/api/v1/ollama/running", get(api_ollama_running))
        .route("/api/v1/ollama/start", post(api_ollama_start))
        .route("/api/v1/ollama/stop", post(api_ollama_stop))
        .route("/api/v1/ollama/status", get(api_ollama_status))
        .route("/api/v1/ollama/library/search", get(api_ollama_library_search))
        .route("/api/v1/ollama/library/{model}/tags", get(api_ollama_library_tags))
        .route("/api/v1/ollama/models/{model}/keepalive", post(api_ollama_keepalive))
        .route("/api/v1/ollama/models/{model}/benchmark", post(api_ollama_benchmark))
        .route("/api/v1/ollama/models/{model}/alias", post(api_ollama_alias))
        .route("/api/v1/ollama/models/{model}/preload", post(api_ollama_preload))
        .route("/api/v1/ollama/models/{model}/load", post(api_ollama_load))
        .route("/api/v1/ollama/models/{model}/gpu-layers", post(api_ollama_gpu_layers))
        .route("/api/v1/ollama/preload", get(api_ollama_preload_list))
        .route("/api/v1/ollama/import", post(api_ollama_import))
        .route("/api/v1/ollama/resources", get(api_ollama_resources))
        .route("/api/v1/auth/google/start", post(api_google_auth_start))
        .route("/api/v1/auth/google/callback", get(api_google_auth_callback))
        .route("/api/v1/auth/google/poll", post(api_google_auth_poll))
        .route("/api/v1/parse", post(api_parse_document))
        .route("/api/v1/feedback", post(api_feedback))
        .route("/api/v1/quality/scores", get(api_quality_scores))
        // Profiles
        .route("/api/v1/profiles", get(api_profiles_list).post(api_profile_save))
        .route("/api/v1/profiles/import", post(api_profile_import))
        .route("/api/v1/profiles/{name}", get(api_profile_get).put(api_profile_update).delete(api_profile_delete))
        .route("/api/v1/profiles/{name}/activate", post(api_profile_activate))
        // Enhanced search & export
        .route("/api/v1/stats/search", get(api_stats_search))
        .route("/api/v1/stats/export/json", get(api_export_json))
        .route("/api/v1/stats/export/csv", get(api_export_csv))
        .route("/api/v1/stats/export/costs/csv", get(api_export_costs_csv))
        // Anthropic Messages API compatibility (for Claude Code harness)
        .route("/v1/messages", post(anthropic_messages))
        // Harness management
        .route("/api/v1/harnesses", get(api_harnesses_list))
        .route("/api/v1/harnesses/takeover", post(api_harness_takeover))
        .route("/api/v1/harnesses/restore-all", post(api_harness_restore_all))
        .route("/api/v1/harnesses/{id}/configure", post(api_harness_configure))
        .route("/api/v1/harnesses/{id}/restore", post(api_harness_restore))
        // Token vault
        .route("/api/v1/tokens", get(api_tokens_list))
        .route("/api/v1/tokens/expiring", get(api_tokens_expiring))
        // Failover rules
        .route("/api/v1/rules", get(api_rules_list).post(api_rules_create))
        .route("/api/v1/rules/presets", get(api_rules_presets))
        .route("/api/v1/rules/{id}", put(api_rules_update).delete(api_rules_delete))
        .route("/api/v1/rules/{id}/toggle", post(api_rules_toggle))
        // Model overrides
        .route("/api/v1/overrides", get(api_overrides_list))
        .route("/api/v1/overrides/{provider}/{model}", get(api_overrides_get).put(api_overrides_set).delete(api_overrides_clear))
        // Response cache & mock provider
        .route("/api/v1/cache/stats", get(api_cache_stats))
        .route("/api/v1/cache/clear", post(api_cache_clear))
        .route("/api/v1/mock/status", get(api_mock_status))
        .route("/api/v1/mock/toggle", post(api_mock_toggle))
        // Thinking optimizer
        .route("/api/v1/thinking/status", get(api_thinking_status))
        .route("/api/v1/thinking/config", put(api_thinking_config))
        // MCP server management
        .route("/api/v1/mcp/servers", get(api_mcp_servers).post(api_mcp_register))
        .route("/api/v1/mcp/scan", post(api_mcp_scan))
        .route("/api/v1/mcp/servers/{id}/toggle", post(api_mcp_toggle))
        .route("/api/v1/mcp/servers/{id}", delete(api_mcp_remove))
        .route("/metrics", get(api_metrics))
        .layer(cors)
        .with_state(state);

    // Serve the React web UI as the primary interface.
    // Falls back to the embedded dashboard if no built React app is found.
    if let Some(web_dir) = web_dirs.iter().find(|d| d.join("index.html").exists()) {
        let index = web_dir.join("index.html");
        info!("Serving web UI from {}", web_dir.display());
        let serve = ServeDir::new(web_dir.clone()).not_found_service(ServeFile::new(index));
        app = app.fallback_service(serve);
    } else {
        // No React build — fall back to embedded dashboard at /
        info!("No web UI found; using embedded dashboard at /");
        app = app.route("/", get(dashboard)).route("/dashboard", get(dashboard));
    }

    info!("Coalesce proxy listening on {}", addr);
    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Initialize all configured providers, discover their models, and register economics.
async fn init_providers(
    config: &AppConfig,
) -> (
    Vec<Arc<dyn Provider>>,
    Vec<ModelInfo>,
    EconomicsEngine,
    DashMap<String, CircuitBreaker>,
) {
    let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
    let mut all_models: Vec<ModelInfo> = Vec::new();
    let economics = EconomicsEngine::new();
    let circuit_breakers = DashMap::new();

    for (name, pconfig) in &config.providers {
        if !pconfig.enabled {
            continue;
        }

        let api_key = resolve_api_key(pconfig);

        let provider: Option<Arc<dyn Provider>> = match name.as_str() {
            "openrouter" => {
                if let Some(key) = api_key {
                    Some(Arc::new(OpenRouterProvider::new(key)))
                } else {
                    warn!("OpenRouter configured but no API key found");
                    None
                }
            }
            "ollama" => {
                let endpoint = pconfig.endpoint.clone();
                Some(Arc::new(OllamaProvider::new(endpoint)))
            }
            "copilot" => {
                if let Some(token) = api_key {
                    Some(Arc::new(CopilotProvider::with_token(token)))
                } else {
                    Some(Arc::new(CopilotProvider::new()))
                }
            }
            "anthropic" | "claude" => {
                api_key.map(|k| Arc::new(AnthropicProvider::new(k)) as Arc<dyn Provider>)
            }
            "glm" | "zhipu" => api_key.map(|k| Arc::new(factories::glm(k)) as Arc<dyn Provider>),
            "kimi" | "moonshot" => {
                api_key.map(|k| Arc::new(factories::kimi(k)) as Arc<dyn Provider>)
            }
            "deepseek" => {
                api_key.map(|k| Arc::new(factories::deepseek(k)) as Arc<dyn Provider>)
            }
            "openai" => api_key.map(|k| Arc::new(factories::openai(k)) as Arc<dyn Provider>),
            "xai" | "grok" => api_key.map(|k| Arc::new(factories::xai(k)) as Arc<dyn Provider>),
            "google" | "gemini" => {
                // Google Cloud Code uses a custom provider, initialized separately
                // after init_providers with project_id from storage
                None
            }
            other => {
                warn!("Unknown provider: {}", other);
                None
            }
        };

        if let Some(p) = provider {
            let billing = parse_billing(pconfig);
            economics.register(name, None::<&str>, billing);
            circuit_breakers.insert(name.clone(), CircuitBreaker::default_provider());

            match p.list_models().await {
                Ok(models) => {
                    info!("  {} — {} models", p.name(), models.len());
                    all_models.extend(models);
                }
                Err(e) => {
                    warn!("  {} — model discovery failed: {}", p.name(), e);
                }
            }

            providers.push(p);
        }
    }

    // Ollama auto-detect
    if !config.providers.contains_key("ollama") {
        let ollama = OllamaProvider::new(None);
        if let Ok(true) = ollama.health_check().await {
            info!("  ollama — auto-detected");
            economics.register("ollama", None::<&str>, BillingType::Local);
            circuit_breakers.insert("ollama".into(), CircuitBreaker::default_provider());
            if let Ok(models) = ollama.list_models().await {
                info!("  ollama — {} models", models.len());
                all_models.extend(models);
            }
            providers.push(Arc::new(ollama));
        }
    }

    (providers, all_models, economics, circuit_breakers)
}

fn resolve_api_key(config: &ProviderConfig) -> Option<String> {
    if let Some(ref key) = config.api_key {
        if !key.is_empty() {
            return Some(key.clone());
        }
    }
    if let Some(ref env_var) = config.api_key_env {
        if let Ok(key) = std::env::var(env_var) {
            if !key.is_empty() {
                return Some(key);
            }
        }
    }
    None
}

fn parse_billing(config: &ProviderConfig) -> BillingType {
    match config.billing.as_deref() {
        Some("local") => BillingType::Local,
        Some("free") => BillingType::FreeIncluded,
        Some("unlimited") => BillingType::UnlimitedSubscription,
        Some("per_token") | Some("per-token") => BillingType::PerToken,
        Some(s) if s.starts_with("quota_monthly:") => {
            let total = s.trim_start_matches("quota_monthly:").parse().unwrap_or(100);
            BillingType::QuotaMonthly { quota_total: total }
        }
        Some(s) if s.starts_with("quota_refreshing:") => {
            let parts: Vec<&str> = s.trim_start_matches("quota_refreshing:").split(':').collect();
            let per_window = parts.first().and_then(|s| s.parse().ok()).unwrap_or(50);
            let interval = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(18000);
            BillingType::QuotaRefreshing {
                quota_per_window: per_window,
                refresh_interval_secs: interval,
            }
        }
        Some(s) if s.starts_with("free_credits:") => {
            let credits = s.trim_start_matches("free_credits:").parse().unwrap_or(0.0);
            BillingType::FreeTierCredits {
                free_credits_usd: credits,
            }
        }
        Some(s) if s.starts_with("quota_only:") => {
            let parts: Vec<&str> = s.trim_start_matches("quota_only:").split(':').collect();
            let per_window = parts.first().and_then(|s| s.parse().ok()).unwrap_or(50);
            let interval = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            BillingType::QuotaOnly {
                quota_per_window: per_window,
                refresh_interval_secs: interval,
            }
        }
        Some(s) if s.starts_with("quota_metered:") => {
            let parts: Vec<&str> = s.trim_start_matches("quota_metered:").split(':').collect();
            let per_window = parts.first().and_then(|s| s.parse().ok()).unwrap_or(50);
            let interval = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(18000);
            BillingType::QuotaMetered {
                quota_per_window: per_window,
                refresh_interval_secs: interval,
            }
        }
        _ => BillingType::PerToken,
    }
}

/// Persist a dynamically-added provider config to `providers.json` so it survives restarts.
fn persist_dynamic_provider(name: &str, config: &ProviderConfig) {
    let path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("coalesce")
        .join("providers.json");
    let mut saved: std::collections::HashMap<String, ProviderConfig> = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };
    saved.insert(name.to_string(), config.clone());
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&saved).unwrap_or_default());
}

/// Remove a dynamically-added provider from `providers.json`.
fn remove_dynamic_provider(name: &str) {
    let path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("coalesce")
        .join("providers.json");
    if path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(mut saved) = serde_json::from_str::<std::collections::HashMap<String, ProviderConfig>>(&contents) {
                saved.remove(name);
                let _ = std::fs::write(&path, serde_json::to_string_pretty(&saved).unwrap_or_default());
            }
        }
    }
}

// --- Handlers ---

async fn dashboard() -> Response {
    let html = include_str!("dashboard/index.html");
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap()
}

async fn health(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let breaker_status: Vec<serde_json::Value> = state
        .circuit_breakers
        .iter()
        .map(|entry| {
            let stats = entry.value().stats();
            serde_json::json!({
                "provider": entry.key(),
                "state": format!("{:?}", stats.state),
                "failures": stats.consecutive_failures,
                "total_requests": stats.total_requests,
                "total_failures": stats.total_failures,
            })
        })
        .collect();

    Json(serde_json::json!({
        "status": "ok",
        "service": "coalesce",
        "providers": state.providers.read().unwrap().len(),
        "models": state.models.read().unwrap().len(),
        "circuit_breakers": breaker_status,
        "dedup_cache_size": state.dedup.cache_size(),
        "dedup_in_flight": state.dedup.in_flight_count(),
    }))
}

async fn chat_completions(
    State(state): State<Arc<ProxyState>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<ChatRequest>,
) -> Response {
    let start = Instant::now();

    // Check for session pinning
    let session_id = headers
        .get("x-coalesce-session")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if let Some(ref sid) = session_id {
        if let Some(mut session) = state.sessions.get_mut(sid) {
            if session.last_seen.elapsed().as_secs() < SESSION_TIMEOUT_SECS {
                session.last_seen = Instant::now();
                session.request_count += 1;
                let pinned_model = session.model_id.clone();
                let pinned_provider = session.provider.clone();
                drop(session);
                // Use pinned model — skip routing, jump to provider dispatch
                let provider = state.providers.read().unwrap().iter().find(|p| p.name() == pinned_provider).cloned();
                if let Some(provider) = provider {
                    let mut forwarded = request.clone();
                    forwarded.model = pinned_model.clone();
                    if request.stream {
                        if let Ok(stream) = provider.chat_stream(&forwarded).await {
                            if let Some(cb) = state.circuit_breakers.get(&pinned_provider) { cb.record_success(); }
                            let _ = state.storage.log_request(&RequestLogEntry {
                                id: None, timestamp: None,
                                tier: "pinned".into(), score: 0.0,
                                provider: pinned_provider.clone(), model: pinned_model.clone(),
                                input_tokens: None, output_tokens: None, cost_usd: None,
                                latency_ms: Some(start.elapsed().as_millis() as u64), success: true,
                            });
                            let body_stream = stream.map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
                            return Response::builder()
                                .status(StatusCode::OK)
                                .header("Content-Type", "text/event-stream")
                                .header("X-Coalesce-Session-Id", sid.as_str())
                                .header("X-Coalesce-Model", &pinned_model)
                                .body(Body::from_stream(body_stream))
                                .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Stream failed").into_response());
                        }
                    } else if let Ok(resp) = provider.chat(&forwarded).await {
                        if let Some(cb) = state.circuit_breakers.get(&pinned_provider) { cb.record_success(); }
                        let fallback_model = state.models.read().unwrap().iter().find(|m| m.id == pinned_model && m.provider == pinned_provider).cloned();
                        let (it, ot, cost) = if let Some(ref m) = fallback_model {
                            extract_usage(&resp, m)
                        } else {
                            (None, None, None)
                        };
                        let _ = state.storage.log_request(&RequestLogEntry {
                            id: None, timestamp: None,
                            tier: "pinned".into(), score: 0.0,
                            provider: pinned_provider, model: pinned_model,
                            input_tokens: it, output_tokens: ot, cost_usd: cost,
                            latency_ms: Some(start.elapsed().as_millis() as u64), success: true,
                        });
                        return Json(resp).into_response();
                    }
                }
            }
        }
    }

    // 0. Rosetta ingress: normalize tools to canonical form
    let t0 = Instant::now();
    let normalized_tools = request.tools.as_ref().map(|tools| {
        state.rosetta.normalize_request_tools(tools, request.tool_choice.as_ref())
    });
    let rosetta_ms = t0.elapsed().as_millis();

    // 0b. Check for specific model request (not "auto" or a tier name)
    let tier_names = ["auto", "simple", "medium", "complex", "reasoning"];
    let requested_model_lower = request.model.to_lowercase();
    if !tier_names.contains(&requested_model_lower.as_str()) {
        // Client requested a specific model — find it and dispatch directly
        let models_snapshot: Vec<ModelInfo> = state.models.read().unwrap().clone();
        let providers_snapshot: Vec<Arc<dyn Provider>> = state.providers.read().unwrap().clone();

        // Find the model — for explicit requests, skip circuit breaker check (user chose this model)
        let matched = models_snapshot.iter().find(|m| m.id == request.model);

        if let Some(target_model) = matched {
            let provider = providers_snapshot.iter().find(|p| p.name() == target_model.provider);
            if let Some(provider) = provider {
                info!(
                    model = %target_model.id,
                    provider = %target_model.provider,
                    "Direct model request — bypassing router"
                );

                let mut forwarded = request.clone();
                forwarded.model = target_model.id.clone();

                // Rosetta egress for direct requests
                if let Some(ref nt) = normalized_tools {
                    if let Ok(translated) = state.rosetta.translate_tools_for_provider(&target_model.provider, nt) {
                        forwarded.tools = Some(translated);
                    }
                    if let Some(ref tc) = nt.tool_choice {
                        forwarded.tool_choice = Some(
                            state.rosetta.translate_tool_choice_for_provider(&target_model.provider, tc)
                        );
                    }
                }

                if request.stream {
                    match provider.chat_stream(&forwarded).await {
                        Ok(stream) => {
                            if let Some(cb) = state.circuit_breakers.get(&target_model.provider) { cb.record_success(); }
                            let _ = state.storage.log_request(&RequestLogEntry {
                                id: None, timestamp: None,
                                tier: "direct".into(), score: 0.0,
                                provider: target_model.provider.clone(), model: target_model.id.clone(),
                                input_tokens: None, output_tokens: None, cost_usd: None,
                                latency_ms: Some(start.elapsed().as_millis() as u64), success: true,
                            });
                            let body_stream = stream.map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
                            return Response::builder()
                                .status(StatusCode::OK)
                                .header("Content-Type", "text/event-stream")
                                .header("Cache-Control", "no-cache")
                                .header("X-Coalesce-Provider", &target_model.provider)
                                .header("X-Coalesce-Model", &target_model.id)
                                .header("X-Coalesce-Tier", "direct")
                                .body(Body::from_stream(body_stream))
                                .unwrap();
                        }
                        Err(e) => {
                            if let Some(cb) = state.circuit_breakers.get(&target_model.provider) { cb.record_failure(); }
                            warn!(model = %target_model.id, provider = %target_model.provider, error = %e, "Direct model request failed");
                            return Response::builder()
                                .status(StatusCode::BAD_GATEWAY)
                                .header("Content-Type", "application/json")
                                .header("X-Coalesce-Provider", &target_model.provider)
                                .header("X-Coalesce-Model", &target_model.id)
                                .header("X-Coalesce-Tier", "direct")
                                .body(Body::from(serde_json::json!({
                                    "error": {
                                        "message": format!("Provider error: {} - {}", target_model.provider, e),
                                        "type": "provider_error",
                                        "provider": &target_model.provider,
                                        "model": &target_model.id,
                                    }
                                }).to_string()))
                                .unwrap();
                        }
                    }
                } else {
                    match provider.chat(&forwarded).await {
                        Ok(mut resp) => {
                            if let Some(cb) = state.circuit_breakers.get(&target_model.provider) { cb.record_success(); }
                            let _ = state.storage.log_request(&RequestLogEntry {
                                id: None, timestamp: None,
                                tier: "direct".into(), score: 0.0,
                                provider: target_model.provider.clone(), model: target_model.id.clone(),
                                input_tokens: None, output_tokens: None, cost_usd: None,
                                latency_ms: Some(start.elapsed().as_millis() as u64), success: true,
                            });
                            resp["x_coalesce"] = serde_json::json!({
                                "tier": "direct", "score": 0.0,
                                "provider": &target_model.provider, "model": &target_model.id,
                                "attempt": 1
                            });
                            return Json(resp).into_response();
                        }
                        Err(e) => {
                            if let Some(cb) = state.circuit_breakers.get(&target_model.provider) { cb.record_failure(); }
                            warn!(model = %target_model.id, provider = %target_model.provider, error = %e, "Direct model request failed");
                            return (
                                StatusCode::BAD_GATEWAY,
                                Json(serde_json::json!({
                                    "error": {
                                        "message": format!("Provider error: {} - {}", target_model.provider, e),
                                        "type": "provider_error",
                                        "provider": &target_model.provider,
                                        "model": &target_model.id,
                                    }
                                })),
                            ).into_response();
                        }
                    }
                }
            }
        }
        // Model not found in loaded models — return error, don't silently reroute
        warn!(model = %request.model, "Specific model requested but not found in loaded models");
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Model '{}' not found. It may not be loaded — check provider configuration and API key.", request.model),
                    "type": "model_not_found",
                    "model": &request.model,
                }
            })),
        ).into_response();
    }

    // Response cache check (deterministic requests only)
    if state.response_cache.should_cache(request.temperature) {
        let msgs_json: Vec<serde_json::Value> = request.messages.iter()
            .filter_map(|m| serde_json::to_value(m).ok())
            .collect();
        let tools_json: Option<Vec<serde_json::Value>> = request.tools.clone();
        let cache_key = ResponseCache::cache_key(
            &request.model,
            &msgs_json,
            tools_json.as_deref(),
        );
        if let Some((cached_resp, _provider, _model)) = state.response_cache.get(&cache_key) {
            info!("Response cache hit");
            return Json(cached_resp).into_response();
        }
    }

    // 1. Score and route
    let t1 = Instant::now();
    let scoring = coalesce_core::router::route(&request, &state.config.routing);
    let route_ms = t1.elapsed().as_millis();
    info!(
        tier = %scoring.tier,
        score = scoring.score,
        reasoning = %scoring.reasoning,
        rosetta_ms = rosetta_ms,
        route_ms = route_ms,
        "Routing request"
    );

    // Acquire priority queue permit (limits per-tier concurrency)
    let tier_str = scoring.tier.to_string();
    let sem = state.priority.semaphore_for(&tier_str);
    let _permit = sem.acquire_owned().await.ok();

    // Snapshot models and providers under read lock
    let models_snapshot: Vec<ModelInfo> = state.models.read().unwrap().clone();
    let providers_snapshot: Vec<Arc<dyn Provider>> = state.providers.read().unwrap().clone();

    // Extract tenant context for multi-tenant mode
    let tenant_id = headers
        .get("x-coalesce-tenant")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| state.config.multi_tenant.default_tenant.clone());

    let tenant_config = if state.config.multi_tenant.enabled {
        state.config.multi_tenant.tenants.get(&tenant_id).cloned()
    } else {
        None
    };

    // 2. Compute marginal costs for all models
    let t2 = Instant::now();
    let costs: Vec<MarginalCost> = models_snapshot
        .iter()
        .map(|m| state.economics.marginal_cost(m, 1000, 500))
        .collect();
    let econ_ms = t2.elapsed().as_millis();
    info!(models = models_snapshot.len(), econ_ms = econ_ms, "Economics computed");

    // 3. Get routing strategy
    let _strategy = state
        .config
        .routing
        .profiles
        .get("auto")
        .map(|p| p.strategy.as_str())
        .unwrap_or("cheapest_capable");

    // Detect if request contains image content (requires vision-capable model)
    let has_images = request.messages.iter().any(|msg| {
        match &msg.content {
            Some(MessageContent::Parts(parts)) => {
                parts.iter().any(|p| matches!(p, ContentPart::ImageUrl { .. }))
            }
            _ => false,
        }
    });

    // 4. Build candidate list sorted by cost, filtering out open circuit breakers
    // For Reasoning tier: if no Reasoning models exist, fall back to Complex
    let effective_tier = scoring.tier;
    let mut candidates: Vec<(usize, &ModelInfo)> = models_snapshot
        .iter()
        .enumerate()
        .filter(|(_, m)| m.quality_tier.can_handle(&effective_tier))
        .filter(|(_, m)| {
            state
                .circuit_breakers
                .get(&m.provider)
                .map(|cb| cb.is_available())
                .unwrap_or(true)
        })
        .filter(|(_, m)| {
            // Multi-tenant provider filtering
            if let Some(ref tc) = tenant_config {
                if let Some(ref allowed) = tc.allowed_providers {
                    return allowed.contains(&m.provider);
                }
            }
            true
        })
        .filter(|(_, m)| {
            // Skip models with negative pricing (OpenRouter uses this to disable beta models)
            m.input_price_per_m >= 0.0 && m.output_price_per_m >= 0.0
        })
        .filter(|(_, m)| {
            // Skip disabled providers and models
            !state.disabled_providers.contains_key(&m.provider)
                && !state.disabled_models.contains_key(&format!("{}::{}", m.provider, m.id))
        })
        .filter(|(_, m)| {
            // If request contains images, only allow vision-capable models
            if has_images { m.vision } else { true }
        })
        .collect();

    // 4a. Rosetta: filter candidates by tool capabilities
    if let Some(ref nt) = normalized_tools {
        let has_thinking = request.extra.contains_key("thinking")
            || request.extra.contains_key("reasoning_effort");
        candidates.retain(|(_, m)| {
            state.rosetta.filter_by_tool_capabilities(&m.provider, nt, has_thinking).passes
        });
    }

    // 4b. Evaluate failover rules and apply triggered actions
    let rule_actions = {
        let mut quota_percent = std::collections::HashMap::new();
        let mut error_rates = std::collections::HashMap::new();
        let mut avg_latency_ms = std::collections::HashMap::new();
        let mut circuit_open = std::collections::HashMap::new();

        for entry in state.circuit_breakers.iter() {
            circuit_open.insert(entry.key().clone(), !entry.value().is_available());
        }

        // Compute budget percent (spent / limit * 100)
        let total_limit = state.config.budget.total_limit_usd;
        let budget_percent = if total_limit > 0.0 {
            (state.budget.total_spent_usd() / total_limit) * 100.0
        } else {
            0.0
        };

        // Compute per-provider error rates and latency from recent request logs
        if let Ok(recent) = state.storage.recent_requests(100) {
            let mut provider_totals: std::collections::HashMap<String, (u64, u64, u64, u64)> = std::collections::HashMap::new();
            for req in &recent {
                let entry = provider_totals.entry(req.provider.clone()).or_insert((0, 0, 0, 0));
                entry.0 += 1; // total requests
                if !req.success { entry.1 += 1; } // failures
                if let Some(ms) = req.latency_ms { entry.2 += ms; entry.3 += 1; } // latency sum, count
            }
            for (prov, (total, failures, lat_sum, lat_count)) in &provider_totals {
                if *total > 0 {
                    error_rates.insert(prov.clone(), (*failures as f64 / *total as f64) * 100.0);
                }
                if *lat_count > 0 {
                    avg_latency_ms.insert(prov.clone(), lat_sum / lat_count);
                }
            }
        }

        // Quota percent is harder to compute generically; use 100.0 as default (no quota info)
        for entry in state.circuit_breakers.iter() {
            quota_percent.insert(entry.key().clone(), 100.0_f64);
        }

        let ctx = rules::EvalContext {
            quota_percent,
            error_rates,
            avg_latency_ms,
            budget_percent,
            circuit_open,
        };
        state.rules.evaluate(&ctx)
    };

    // Apply triggered rule actions
    let mut rules_disabled_providers: Vec<String> = Vec::new();
    let mut rules_preferred_provider: Option<(String, u32)> = None;
    for action in &rule_actions {
        match &action.action {
            rules::RuleAction::DisableProvider { provider } => {
                info!(rule = %action.rule_name, provider = %provider, "Rule disabled provider");
                rules_disabled_providers.push(provider.clone());
            }
            rules::RuleAction::PreferProvider { provider, priority } => {
                info!(rule = %action.rule_name, provider = %provider, priority = priority, "Rule preferred provider");
                rules_preferred_provider = Some((provider.clone(), *priority));
            }
            rules::RuleAction::Notify { message } => {
                info!(rule = %action.rule_name, message = %message, "Rule notification");
                let _ = state.event_tx.send(ProxyEvent::BudgetAlert {
                    threshold_pct: 0,
                    spent_usd: state.budget.total_spent_usd(),
                    limit_usd: state.config.budget.total_limit_usd,
                });
            }
            rules::RuleAction::SwitchProfile { profile } => {
                info!(rule = %action.rule_name, profile = %profile, "Rule switching profile");
                // Profile switch is a heavy operation; just log for now.
                // Full profile switching would reload providers, which we don't want mid-request.
            }
        }
    }

    // Filter out providers disabled by rules
    if !rules_disabled_providers.is_empty() {
        candidates.retain(|(_, m)| !rules_disabled_providers.contains(&m.provider));
    }

    // If Reasoning tier found no candidates, fall back to Complex tier
    if candidates.is_empty() && scoring.tier == coalesce_core::types::QualityTier::Reasoning {
        info!("No Reasoning-tier models available, falling back to Complex tier");
        candidates = models_snapshot
            .iter()
            .enumerate()
            .filter(|(_, m)| m.quality_tier.can_handle(&coalesce_core::types::QualityTier::Complex))
            .filter(|(_, m)| {
                state.circuit_breakers.get(&m.provider)
                    .map(|cb| cb.is_available()).unwrap_or(true)
            })
            .filter(|(_, m)| !state.disabled_providers.contains_key(&m.provider)
                && !state.disabled_models.contains_key(&format!("{}::{}", m.provider, m.id)))
            .filter(|(_, m)| m.input_price_per_m >= 0.0 && m.output_price_per_m >= 0.0)
            .collect();
        if !rules_disabled_providers.is_empty() {
            candidates.retain(|(_, m)| !rules_disabled_providers.contains(&m.provider));
        }
    }

    if has_images && candidates.is_empty() {
        return Json(serde_json::json!({
            "error": {
                "message": "No vision-capable model is available to handle this request containing images. Enable a vision-capable model or provider.",
                "type": "routing_error",
                "code": "no_vision_model"
            }
        })).into_response();
    }

    // Build ordered candidate list: pinned models/providers first, then cost-sorted remainder
    let tier_pins: Vec<coalesce_core::router::config::ModelPin> = state.model_pins.read().unwrap()
        .get(&scoring.tier)
        .cloned()
        .unwrap_or_default();

    // Expand pins into (canonical_model_id, provider, pin_rank) tuples
    // Pin 0 provider 0 = rank 0, pin 0 provider 1 = rank 1, pin 1 provider 0 = rank 2, etc.
    // Uses model equivalences so "claude-opus-4.6" pin matches "claude-opus-4-6-thinking" on Google
    let mut pin_ranks: Vec<(String, String, usize)> = Vec::new();
    for pin in &tier_pins {
        let canonical = state.canonical_model_id(&pin.model_id);
        for prov in &pin.providers {
            let rank = pin_ranks.len();
            pin_ranks.push((canonical.clone(), prov.clone(), rank));
        }
    }

    candidates.sort_by(|(idx_a, model_a), (idx_b, model_b)| {
        let canon_a = state.canonical_model_id(&model_a.id);
        let canon_b = state.canonical_model_id(&model_b.id);
        let rank_a = pin_ranks.iter().find(|(mid, prov, _)| mid == &canon_a && prov == &model_a.provider).map(|(_, _, r)| *r);
        let rank_b = pin_ranks.iter().find(|(mid, prov, _)| mid == &canon_b && prov == &model_b.provider).map(|(_, _, r)| *r);

        match (rank_a, rank_b) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => {
                // Sort by provider priority first (lower = better)
                // Rules-preferred provider gets boosted priority
                let mut prio_a = state.provider_priorities.get(&model_a.provider).map(|v| *v).unwrap_or(50);
                let mut prio_b = state.provider_priorities.get(&model_b.provider).map(|v| *v).unwrap_or(50);
                if let Some((ref preferred, ref rule_prio)) = rules_preferred_provider {
                    if model_a.provider == *preferred { prio_a = prio_a.min(*rule_prio); }
                    if model_b.provider == *preferred { prio_b = prio_b.min(*rule_prio); }
                }
                if prio_a != prio_b {
                    return prio_a.cmp(&prio_b);
                }

                // Within same priority, sort by marginal cost (economics engine handles
                // billing mode: quota-only=$0 while in quota, quota+metered=$0 then paid,
                // metered=always paid, unavailable=MAX)
                let cost_a = costs.get(*idx_a).map(|c| c.usd_value()).unwrap_or(f64::MAX);
                let cost_b = costs.get(*idx_b).map(|c| c.usd_value()).unwrap_or(f64::MAX);
                let quality_a = state.quality.score(&model_a.provider, &model_a.id);
                let quality_b = state.quality.score(&model_b.provider, &model_b.id);
                let adjusted_a = cost_a / quality_a.max(0.01);
                let adjusted_b = cost_b / quality_b.max(0.01);
                adjusted_a.partial_cmp(&adjusted_b).unwrap_or(std::cmp::Ordering::Equal)
            }
        }
    });

    // Family-aware dedup: within each canonical family, keep only the best (first-sorted) candidate.
    // This ensures the fallback chain tries different families rather than the same model on
    // different providers (e.g., Claude Opus on copilot then Claude Opus on google).
    {
        let mut seen_families: std::collections::HashSet<String> = std::collections::HashSet::new();
        candidates.retain(|(_, m)| {
            let family = m.canonical_family.clone()
                .unwrap_or_else(|| derive_canonical_family(&m.id));
            // Always keep pinned candidates (they were sorted to the front)
            let canon = state.canonical_model_id(&m.id);
            let is_pinned = pin_ranks.iter().any(|(mid, prov, _)| mid == &canon && prov == &m.provider);
            if is_pinned {
                seen_families.insert(family);
                true
            } else {
                seen_families.insert(family.clone())
            }
        });
    }

    if candidates.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "message": format!("No model available for tier {}", scoring.tier),
                    "type": "no_available_model",
                    "code": "model_unavailable",
                }
            })),
        )
            .into_response();
    }

    // Thinking optimization: auto-enable extended thinking for capable models
    let has_tools = request.tools.as_ref().map_or(false, |t| !t.is_empty());
    let thinking_decision = state.thinking_optimizer.decide(
        &request.model,
        "", // provider not yet known — will refine per-candidate
        scoring.score,
        &scoring.tier,
        has_tools,
    );
    if thinking_decision.enable_thinking {
        debug!(
            budget = ?thinking_decision.budget_tokens,
            reason = thinking_decision.reason,
            "Thinking optimizer: enabled"
        );
    }

    // 5. Fallback chain — try candidates in order
    let pre_fallback_ms = start.elapsed().as_millis();
    info!(candidates = candidates.len(), pre_fallback_ms = pre_fallback_ms, "Entering fallback loop");
    let mut last_error = String::new();
    // Track providers that returned auth errors (401/403) — skip them on subsequent attempts
    let mut auth_failed_providers: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut attempts_made = 0usize;

    for candidate_idx in 0..candidates.len() {
        if attempts_made >= MAX_FALLBACK_ATTEMPTS {
            break;
        }
        let (_, selected_model) = candidates[candidate_idx];

        // Skip providers that already returned auth errors — don't count as an attempt
        if auth_failed_providers.contains(&selected_model.provider) {
            continue;
        }

        let provider = match providers_snapshot
            .iter()
            .find(|p| p.name() == selected_model.provider)
        {
            Some(p) => p,
            None => continue,
        };

        let attempt = attempts_made;
        attempts_made += 1;

        if attempt > 0 {
            warn!(
                attempt = attempt + 1,
                model = %selected_model.id,
                provider = %selected_model.provider,
                "Fallback attempt"
            );
        } else {
            info!(
                model = %selected_model.id,
                provider = %selected_model.provider,
                "Selected model"
            );
        }

        // Rewrite model in request
        let mut forwarded_request = request.clone();
        forwarded_request.model = selected_model.id.clone();

        // Rosetta egress: translate tools to provider-native format
        if let Some(ref nt) = normalized_tools {
            if let Ok(translated) = state.rosetta.translate_tools_for_provider(&selected_model.provider, nt) {
                forwarded_request.tools = Some(translated);
            }
            if let Some(ref tc) = nt.tool_choice {
                forwarded_request.tool_choice = Some(
                    state.rosetta.translate_tool_choice_for_provider(&selected_model.provider, tc)
                );
            }
        }

        // Apply thinking optimizer decision per-model
        let model_decision = state.thinking_optimizer.decide(
            &forwarded_request.model,
            &selected_model.provider,
            scoring.score,
            &scoring.tier,
            has_tools,
        );
        if model_decision.enable_thinking {
            if let Some(budget) = model_decision.budget_tokens {
                forwarded_request.extra.insert("thinking".into(), serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget,
                }));
            }
        }

        // 6. Forward — streaming or non-streaming
        let provider_call_start = Instant::now();
        if request.stream {
            match provider.chat_stream(&forwarded_request).await {
                Ok(byte_stream) => {
                    // Record success
                    if let Some(cb) = state.circuit_breakers.get(&selected_model.provider) {
                        cb.record_success();
                    }

                    // Log request (no token counts for streaming)
                    let _ = state.storage.log_request(&RequestLogEntry {
                        id: None, timestamp: None,
                        tier: scoring.tier.to_string(),
                        score: scoring.score,
                        provider: selected_model.provider.clone(),
                        model: selected_model.id.clone(),
                        input_tokens: None,
                        output_tokens: None,
                        cost_usd: None,
                        latency_ms: Some(start.elapsed().as_millis() as u64),
                        success: true,
                    });

                    // Prepend a routing metadata SSE event so the client gets
                    // routing info even when CORS blocks response headers
                    let routing_meta = format!(
                        "data: {}\n\n",
                        serde_json::json!({
                            "x_coalesce": {
                                "tier": scoring.tier.to_string(),
                                "score": scoring.score,
                                "provider": selected_model.provider,
                                "model": selected_model.id,
                                "attempt": attempt + 1,
                            }
                        })
                    );
                    let meta_stream = futures::stream::once(async move {
                        Ok::<_, std::io::Error>(bytes::Bytes::from(routing_meta))
                    });

                    let body_stream = byte_stream.map(|result| {
                        result
                            .map(|bytes| bytes)
                            .map_err(|e| {
                                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                            })
                    });

                    let combined = meta_stream.chain(body_stream);

                    return Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/event-stream")
                        .header("Cache-Control", "no-cache")
                        .header("Connection", "keep-alive")
                        .header("X-Coalesce-Model", &selected_model.id)
                        .header("X-Coalesce-Provider", &selected_model.provider)
                        .header("X-Coalesce-Tier", scoring.tier.to_string())
                        .header("X-Coalesce-Attempt", (attempt + 1).to_string())
                        .body(Body::from_stream(combined))
                        .unwrap_or_else(|_| {
                            (StatusCode::INTERNAL_SERVER_ERROR, "Stream setup failed")
                                .into_response()
                        });
                }
                Err(e) => {
                    let err_str = e.to_string();
                    warn!(
                        provider = %provider.name(),
                        error = %err_str,
                        attempt = attempt + 1,
                        "Stream failed, trying fallback"
                    );
                    if let Some(cb) = state.circuit_breakers.get(&selected_model.provider) {
                        cb.record_failure();
                    }
                    // Auth errors (401/403) mean the entire provider is broken — skip it
                    if err_str.contains("401") || err_str.contains("403") || err_str.contains("Unauthorized") || err_str.contains("Forbidden") {
                        warn!(provider = %selected_model.provider, "Auth error — skipping provider for remaining attempts");
                        auth_failed_providers.insert(selected_model.provider.clone());
                        // Google: try reactive token refresh on auth failure
                        if selected_model.provider == "google" {
                            if let Ok(Some(rt)) = state.storage.get("google_refresh_token") {
                                if let Ok(new_token) = refresh_google_token(&rt).await {
                                    let _ = state.storage.set("google_access_token", &new_token);
                                    let new_provider: Arc<dyn Provider> = Arc::new(
                                        GoogleCloudCodeProvider::new(new_token, state.storage.get("google_project_id").ok().flatten().unwrap_or_default())
                                    );
                                    let mut providers = state.providers.write().unwrap();
                                    if let Some(pos) = providers.iter().position(|p| p.name() == "google") {
                                        providers[pos] = new_provider;
                                        info!("  google — reactive token refresh succeeded, removing from auth-failed set");
                                        drop(providers);
                                        auth_failed_providers.remove("google");
                                    }
                                }
                            }
                        }
                    }
                    last_error = err_str;
                    continue;
                }
            }
        } else {
            // Non-streaming: check dedup cache first
            let dedup_hash = RequestDedup::hash_request(
                &serde_json::to_vec(&forwarded_request).unwrap_or_default(),
            );

            match state.dedup.try_dedup(&dedup_hash) {
                DedupAction::Cached(result) => {
                    info!("Dedup cache hit");
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&result.body) {
                        return Json(json).into_response();
                    }
                }
                DedupAction::Wait(mut rx) => {
                    info!("Dedup: waiting for in-flight request");
                    if let Ok(result) = rx.recv().await {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&result.body) {
                            return Json(json).into_response();
                        }
                    }
                    // If we failed to receive, fall through to execute
                }
                DedupAction::Execute(guard) => {
                    match provider.chat(&forwarded_request).await {
                        Ok(mut response_json) => {
                            let provider_call_ms = provider_call_start.elapsed().as_millis();
                            info!(
                                provider = %selected_model.provider,
                                model = %selected_model.id,
                                provider_call_ms = provider_call_ms,
                                total_ms = start.elapsed().as_millis(),
                                "Provider call succeeded"
                            );
                            // Record success
                            if let Some(cb) =
                                state.circuit_breakers.get(&selected_model.provider)
                            {
                                cb.record_success();
                            }

                            // Inject routing metadata
                            if let Some(obj) = response_json.as_object_mut() {
                                obj.insert(
                                    "x_coalesce".to_string(),
                                    serde_json::json!({
                                        "tier": scoring.tier.to_string(),
                                        "score": scoring.score,
                                        "provider": selected_model.provider,
                                        "model": selected_model.id,
                                        "attempt": attempt + 1,
                                    }),
                                );
                            }

                            // Extract usage for logging
                            let (input_tokens, output_tokens, cost_usd) =
                                extract_usage(&response_json, selected_model);

                            // Record usage in economics engine
                            let cost_val = cost_usd.unwrap_or(0.0);
                            state.economics.record_usage(
                                &selected_model.provider,
                                &selected_model.id,
                                cost_val,
                            );

                            let elapsed_ms = start.elapsed().as_millis() as u64;

                            // Record quality score
                            state.quality.record(&selected_model.provider, &selected_model.id, true, elapsed_ms);

                            // Log request
                            let _ = state.storage.log_request(&RequestLogEntry {
                                id: None, timestamp: None,
                                tier: scoring.tier.to_string(),
                                score: scoring.score,
                                provider: selected_model.provider.clone(),
                                model: selected_model.id.clone(),
                                input_tokens,
                                output_tokens,
                                cost_usd,
                                latency_ms: Some(elapsed_ms),
                                success: true,
                            });

                            // Prometheus metrics
                            metrics::counter!("coalesce_requests_total",
                                "tier" => scoring.tier.to_string(),
                                "provider" => selected_model.provider.clone(),
                                "success" => "true"
                            ).increment(1);
                            metrics::histogram!("coalesce_request_duration_seconds")
                                .record(start.elapsed().as_secs_f64());
                            if let Some(it) = input_tokens {
                                metrics::counter!("coalesce_tokens_total", "direction" => "input").increment(it as u64);
                            }
                            if let Some(ot) = output_tokens {
                                metrics::counter!("coalesce_tokens_total", "direction" => "output").increment(ot as u64);
                            }
                            metrics::counter!("coalesce_cost_usd_total",
                                "provider" => selected_model.provider.clone()
                            ).increment(cost_val as u64);

                            // Check budget alerts
                            state.budget.record_spending(cost_val);
                            let alerts = state.budget.check_thresholds(&state.config.budget.alert_thresholds);
                            for alert in &alerts {
                                let _ = state.event_tx.send(ProxyEvent::BudgetAlert {
                                    threshold_pct: alert.threshold_pct,
                                    spent_usd: alert.spent_usd,
                                    limit_usd: alert.limit_usd,
                                });
                                // Fire webhook if configured
                                if let Some(ref url) = state.config.budget.alert_webhook {
                                    let url = url.clone();
                                    let alert_json = serde_json::json!({
                                        "type": "budget_alert",
                                        "threshold_pct": alert.threshold_pct,
                                        "spent_usd": alert.spent_usd,
                                        "limit_usd": alert.limit_usd,
                                    });
                                    tokio::spawn(async move {
                                        let _ = reqwest::Client::new()
                                            .post(&url)
                                            .json(&alert_json)
                                            .send()
                                            .await;
                                    });
                                }
                                // Fire command if configured
                                if let Some(ref cmd) = state.config.budget.alert_command {
                                    let alert_json = serde_json::json!({
                                        "type": "budget_alert",
                                        "threshold_pct": alert.threshold_pct,
                                        "spent_usd": alert.spent_usd,
                                        "limit_usd": alert.limit_usd,
                                    });
                                    let cmd = cmd.clone();
                                    let json_str = alert_json.to_string();
                                    tokio::spawn(async move {
                                        let mut child = match tokio::process::Command::new("sh")
                                            .arg("-c")
                                            .arg(&cmd)
                                            .stdin(std::process::Stdio::piped())
                                            .spawn()
                                        {
                                            Ok(c) => c,
                                            Err(e) => {
                                                warn!("Budget alert command failed to start: {}", e);
                                                return;
                                            }
                                        };
                                        if let Some(mut stdin) = child.stdin.take() {
                                            use tokio::io::AsyncWriteExt;
                                            let _ = stdin.write_all(json_str.as_bytes()).await;
                                        }
                                        let _ = child.wait().await;
                                    });
                                }
                            }

                            // Emit SSE event
                            let _ = state.event_tx.send(ProxyEvent::RequestComplete {
                                provider: selected_model.provider.clone(),
                                model: selected_model.id.clone(),
                                latency_ms: elapsed_ms,
                                success: true,
                                input_tokens,
                                output_tokens,
                            });

                            // Pin session if header present
                            if let Some(ref sid) = session_id {
                                state.sessions.insert(sid.clone(), SessionInfo {
                                    model_id: selected_model.id.clone(),
                                    provider: selected_model.provider.clone(),
                                    last_seen: Instant::now(),
                                    request_count: 1,
                                });
                            }

                            // Cache in dedup
                            let body =
                                serde_json::to_string(&response_json).unwrap_or_default();
                            guard.complete(DedupResult {
                                body,
                                is_error: false,
                            });

                            // Store in response cache for deterministic requests
                            if state.response_cache.should_cache(request.temperature) {
                                let msgs_json: Vec<serde_json::Value> = request.messages.iter()
                                    .filter_map(|m| serde_json::to_value(m).ok())
                                    .collect();
                                let tools_json: Option<Vec<serde_json::Value>> = request.tools.clone();
                                let cache_key = ResponseCache::cache_key(
                                    &request.model,
                                    &msgs_json,
                                    tools_json.as_deref(),
                                );
                                state.response_cache.put(
                                    cache_key,
                                    response_json.clone(),
                                    &selected_model.provider,
                                    &selected_model.id,
                                );
                            }

                            return Json(response_json).into_response();
                        }
                        Err(e) => {
                            warn!(
                                provider = %provider.name(),
                                error = %e,
                                attempt = attempt + 1,
                                "Chat failed, trying fallback"
                            );
                            if let Some(cb) =
                                state.circuit_breakers.get(&selected_model.provider)
                            {
                                cb.record_failure();
                            }

                            // Log failure
                            let _ = state.storage.log_request(&RequestLogEntry {
                                id: None, timestamp: None,
                                tier: scoring.tier.to_string(),
                                score: scoring.score,
                                provider: selected_model.provider.clone(),
                                model: selected_model.id.clone(),
                                input_tokens: None,
                                output_tokens: None,
                                cost_usd: None,
                                latency_ms: Some(start.elapsed().as_millis() as u64),
                                success: false,
                            });

                            // Don't cache errors in dedup
                            guard.complete(DedupResult {
                                body: String::new(),
                                is_error: true,
                            });

                            let err_str = e.to_string();
                            if err_str.contains("401") || err_str.contains("403") || err_str.contains("Unauthorized") || err_str.contains("Forbidden") {
                                warn!(provider = %selected_model.provider, "Auth error — skipping provider for remaining attempts");
                                auth_failed_providers.insert(selected_model.provider.clone());
                                // Google: try reactive token refresh on auth failure
                                if selected_model.provider == "google" {
                                    if let Ok(Some(rt)) = state.storage.get("google_refresh_token") {
                                        if let Ok(new_token) = refresh_google_token(&rt).await {
                                            let _ = state.storage.set("google_access_token", &new_token);
                                            let new_provider: Arc<dyn Provider> = Arc::new(
                                                GoogleCloudCodeProvider::new(new_token, state.storage.get("google_project_id").ok().flatten().unwrap_or_default())
                                            );
                                            let mut providers = state.providers.write().unwrap();
                                            if let Some(pos) = providers.iter().position(|p| p.name() == "google") {
                                                providers[pos] = new_provider;
                                                info!("  google — reactive token refresh succeeded, removing from auth-failed set");
                                                drop(providers);
                                                auth_failed_providers.remove("google");
                                            }
                                        }
                                    }
                                }
                            }
                            last_error = err_str;
                            continue;
                        }
                    }
                }
            }
        }
    }

    // All attempts exhausted
    error!(
        attempts = attempts_made,
        last_error = %last_error,
        "All fallback attempts exhausted"
    );

    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header("Content-Type", "application/json")
        .header("x-coalesce-tier", scoring.tier.to_string())
        .header("x-coalesce-attempt", attempts_made.to_string())
        .header("Access-Control-Expose-Headers", "x-coalesce-tier, x-coalesce-attempt")
        .body(Body::from(serde_json::to_string(&serde_json::json!({
            "error": {
                "message": format!(
                    "All providers failed after {} attempts. Last error: {}",
                    attempts_made, last_error
                ),
                "type": "all_providers_exhausted",
                "code": "provider_error",
            }
        })).unwrap()))
        .unwrap()
}

fn extract_usage(
    response: &serde_json::Value,
    model: &ModelInfo,
) -> (Option<u32>, Option<u32>, Option<f64>) {
    if let Some(usage) = response.get("usage") {
        let input = usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let output = usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let cost = match (input, output) {
            (Some(i), Some(o)) => {
                Some(
                    (i as f64 / 1_000_000.0) * model.input_price_per_m
                        + (o as f64 / 1_000_000.0) * model.output_price_per_m,
                )
            }
            _ => None,
        };
        (input, output, cost)
    } else {
        (None, None, None)
    }
}

async fn list_models(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let models_lock = state.models.read().unwrap();
    let models: Vec<serde_json::Value> = models_lock
        .iter()
        .filter(|m| m.input_price_per_m >= 0.0 && m.output_price_per_m >= 0.0)
        .map(|m| {
            let cost = state.economics.marginal_cost(m, 1000, 500);
            let cb_state = state
                .circuit_breakers
                .get(&m.provider)
                .map(|cb| match format!("{:?}", cb.stats().state).as_str() {
                    "Closed" => "Healthy".to_string(),
                    "Open" => "Unavailable".to_string(),
                    "HalfOpen" => "Recovering".to_string(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| "Unknown".into());

            let provider_disabled = state.disabled_providers.contains_key(&m.provider);
            let model_disabled = state.disabled_models.contains_key(&format!("{}::{}", m.provider, m.id));

            serde_json::json!({
                "id": m.id,
                "object": "model",
                "created": 0,
                "owned_by": m.provider,
                "name": m.name,
                "quality_tier": m.quality_tier,
                "context_window": m.context_window,
                "max_output": m.max_output,
                "reasoning": m.reasoning,
                "vision": m.vision,
                "tool_calling": m.tool_calling,
                "canonical_family": m.canonical_family,
                "capabilities": m.capabilities,
                "pricing": {
                    "input_per_m": m.input_price_per_m,
                    "output_per_m": m.output_price_per_m,
                },
                "marginal_cost": {
                    "usd": cost.usd_value(),
                    "is_free": cost.is_free(),
                    "is_available": cost.is_available(),
                },
                "circuit_breaker": cb_state,
                "is_disabled": model_disabled || provider_disabled,
            })
        })
        .collect();

    Json(serde_json::json!({
        "object": "list",
        "data": models,
    }))
}

async fn stats(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let request_stats = state.storage.stats().unwrap_or_else(|_| {
        coalesce_core::storage::RequestStats {
            total_requests: 0,
            successful_requests: 0,
            total_cost_usd: 0.0,
            avg_latency_ms: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    });

    let recent = state
        .storage
        .recent_requests(20)
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "tier": r.tier,
                "score": r.score,
                "provider": r.provider,
                "model": r.model,
                "input_tokens": r.input_tokens,
                "output_tokens": r.output_tokens,
                "cost_usd": r.cost_usd,
                "latency_ms": r.latency_ms,
                "success": r.success,
            })
        })
        .collect::<Vec<_>>();

    let quotas: Vec<serde_json::Value> = state
        .economics
        .all_quotas()
        .into_iter()
        .map(|q| {
            serde_json::json!({
                "provider": q.provider,
                "model": q.model,
                "billing": q.billing.to_string(),
                "used": q.used,
                "remaining": q.remaining(),
                "is_depleted": q.is_depleted(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "stats": {
            "total_requests": request_stats.total_requests,
            "successful_requests": request_stats.successful_requests,
            "success_rate": if request_stats.total_requests > 0 {
                request_stats.successful_requests as f64 / request_stats.total_requests as f64
            } else { 1.0 },
            "total_cost_usd": request_stats.total_cost_usd,
            "avg_latency_ms": request_stats.avg_latency_ms,
            "total_input_tokens": request_stats.total_input_tokens,
            "total_output_tokens": request_stats.total_output_tokens,
        },
        "quotas": quotas,
        "recent_requests": recent,
    }))
}

// --- Dashboard REST API Handlers ---

/// GET /api/v1/providers — list all providers with status, billing, circuit breaker state, model count
async fn api_providers(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let providers_lock = state.providers.read().unwrap();
    let models_lock = state.models.read().unwrap();
    let providers: Vec<serde_json::Value> = providers_lock
        .iter()
        .map(|p| {
            let name = p.name().to_string();
            let model_count = models_lock.iter().filter(|m| m.provider == name).count();

            let cb_info = state.circuit_breakers.get(&name).map(|cb| {
                let s = cb.stats();
                let raw = format!("{:?}", s.state);
                let state_label = match raw.as_str() {
                    "Closed" => "Healthy",
                    "Open" => "Unavailable",
                    "HalfOpen" => "Recovering",
                    _ => raw.as_str(),
                };
                serde_json::json!({
                    "state": state_label,
                    "consecutive_failures": s.consecutive_failures,
                    "total_requests": s.total_requests,
                    "total_failures": s.total_failures,
                })
            });

            // Read billing directly from the economics engine (single source of truth)
            let billing = state.economics.get_billing(&name)
                .map(|bt| bt.to_string())
                .unwrap_or_else(|| "per_token".into());

            let is_disabled = state.disabled_providers.contains_key(&name);

            let priority = state.provider_priorities.get(&name).map(|v| *v).unwrap_or(50);
            let pricing_mode = state.provider_pricing_modes.get(&name)
                .map(|v| v.value().clone())
                .unwrap_or_else(|| "metered".to_string());

            serde_json::json!({
                "name": name,
                "model_count": model_count,
                "billing": billing,
                "circuit_breaker": cb_info,
                "is_available": !is_disabled && state.circuit_breakers.get(&name).map(|cb| cb.is_available()).unwrap_or(true),
                "is_disabled": is_disabled,
                "priority": priority,
                "pricing_mode": pricing_mode,
            })
        })
        .collect();

    // Attach per-provider usage stats from DB
    let provider_stats = state.storage.stats_by_provider().unwrap_or_default();
    let stats_map: std::collections::HashMap<String, _> = provider_stats.into_iter().map(|s| (s.provider.clone(), s)).collect();

    // Merge stats into provider entries
    let providers: Vec<serde_json::Value> = providers.into_iter().map(|mut p| {
        let name = p["name"].as_str().unwrap_or("").to_string();
        if let Some(s) = stats_map.get(&name) {
            p.as_object_mut().map(|obj| {
                obj.insert("total_requests".into(), serde_json::json!(s.total_requests));
                obj.insert("successful_requests".into(), serde_json::json!(s.successful_requests));
                obj.insert("total_input_tokens".into(), serde_json::json!(s.total_input_tokens));
                obj.insert("total_output_tokens".into(), serde_json::json!(s.total_output_tokens));
                obj.insert("total_cost_usd".into(), serde_json::json!(s.total_cost_usd));
                obj.insert("avg_latency_ms".into(), serde_json::json!(s.avg_latency_ms));
            });
        }
        p
    }).collect();

    Json(serde_json::json!({
        "providers": providers,
    }))
}

/// GET /api/v1/providers/quotas — quota states for all providers
async fn api_providers_quotas(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let quotas: Vec<serde_json::Value> = state
        .economics
        .all_quotas()
        .into_iter()
        .map(|q| {
            serde_json::json!({
                "provider": q.provider,
                "model": q.model,
                "billing": q.billing.to_string(),
                "used": q.used,
                "remaining": q.remaining(),
                "is_depleted": q.is_depleted(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "quotas": quotas,
    }))
}

/// POST /api/v1/routing/playground — dry-run the router scorer on a prompt
async fn api_routing_playground(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<PlaygroundRequest>,
) -> Json<serde_json::Value> {
    // Build a dummy ChatRequest with a single user message
    let request = ChatRequest {
        model: "auto".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(MessageContent::Text(body.prompt.clone())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: std::collections::HashMap::new(),
        }],
        stream: false,
        max_tokens: None,
        temperature: None,
        top_p: None,
        stop: None,
        tools: None,
        tool_choice: None,
        response_format: None,
        extra: std::collections::HashMap::new(),
    };

    // Use custom weights if provided, otherwise use config defaults
    let routing_config = if let Some(ref weights) = body.weights {
        let mut rc = state.config.routing.clone();
        rc.weights = weights.clone();
        rc
    } else {
        state.config.routing.clone()
    };

    let scoring = coalesce_core::router::route(&request, &routing_config);

    // Build candidate rankings (same logic as chat_completions routing)
    let models_lock = state.models.read().unwrap();
    let costs: Vec<MarginalCost> = models_lock
        .iter()
        .map(|m| state.economics.marginal_cost(m, 1000, 500))
        .collect();

    let tier_pins: Vec<coalesce_core::router::config::ModelPin> = state.model_pins.read().unwrap()
        .get(&scoring.tier)
        .cloned()
        .unwrap_or_default();

    // Expand pins for ranking (using canonical model IDs for cross-provider equivalence)
    let mut pin_ranks: Vec<(String, String, usize)> = Vec::new();
    for pin in &tier_pins {
        let canonical = state.canonical_model_id(&pin.model_id);
        for prov in &pin.providers {
            let rank = pin_ranks.len();
            pin_ranks.push((canonical.clone(), prov.clone(), rank));
        }
    }

    let mut candidates: Vec<serde_json::Value> = models_lock
        .iter()
        .enumerate()
        .filter(|(_, m)| m.quality_tier.can_handle(&scoring.tier))
        .filter(|(_, m)| {
            state
                .circuit_breakers
                .get(&m.provider)
                .map(|cb| cb.is_available())
                .unwrap_or(true)
        })
        .map(|(idx, m)| {
            let cost = costs.get(idx).map(|c| c.usd_value()).unwrap_or(f64::MAX);
            let canon = state.canonical_model_id(&m.id);
            let rank = pin_ranks.iter().find(|(mid, prov, _)| mid == &canon && prov == &m.provider).map(|(_, _, r)| *r);
            serde_json::json!({
                "model": m.id,
                "provider": m.provider,
                "quality_tier": m.quality_tier,
                "marginal_cost_usd": cost,
                "is_free": costs.get(idx).map(|c| c.is_free()).unwrap_or(false),
                "pinned": rank.is_some(),
                "pin_rank": rank,
            })
        })
        .collect();

    // Sort: pinned first (by rank), then by cost
    candidates.sort_by(|a, b| {
        let rank_a = a["pin_rank"].as_u64();
        let rank_b = b["pin_rank"].as_u64();
        match (rank_a, rank_b) {
            (Some(pa), Some(pb)) => pa.cmp(&pb),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => {
                let ca = a["marginal_cost_usd"].as_f64().unwrap_or(f64::MAX);
                let cb = b["marginal_cost_usd"].as_f64().unwrap_or(f64::MAX);
                ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
            }
        }
    });

    Json(serde_json::json!({
        "scoring": {
            "tier": scoring.tier.to_string(),
            "score": scoring.score,
            "confidence": scoring.confidence,
            "method": scoring.method,
            "reasoning": scoring.reasoning,
            "dimensions": {
                "token_count": scoring.dimensions.token_count,
                "code_presence": scoring.dimensions.code_presence,
                "reasoning_markers": scoring.dimensions.reasoning_markers,
                "technical_terms": scoring.dimensions.technical_terms,
                "creative_markers": scoring.dimensions.creative_markers,
                "simple_indicators": scoring.dimensions.simple_indicators,
                "multi_step": scoring.dimensions.multi_step,
                "question_complexity": scoring.dimensions.question_complexity,
                "imperative_verbs": scoring.dimensions.imperative_verbs,
                "constraints": scoring.dimensions.constraints,
                "output_format": scoring.dimensions.output_format,
                "reference_keywords": scoring.dimensions.reference_keywords,
                "negation_keywords": scoring.dimensions.negation_keywords,
                "domain_specific": scoring.dimensions.domain_specific,
                "agentic_keywords": scoring.dimensions.agentic_keywords,
            },
        },
        "candidates": candidates,
        "total_eligible_models": candidates.len(),
    }))
}

/// GET /api/v1/routing/profiles — list routing profiles from config
async fn api_routing_profiles(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let profiles: Vec<serde_json::Value> = state
        .config
        .routing
        .profiles
        .iter()
        .map(|(name, profile)| {
            let tiers: serde_json::Value = serde_json::to_value(&profile.tiers)
                .unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "name": name,
                "description": profile.description,
                "strategy": profile.strategy,
                "tiers": tiers,
            })
        })
        .collect();

    Json(serde_json::json!({
        "profiles": profiles,
    }))
}

/// GET /api/v1/routing/pins — get model pins per tier
async fn api_routing_pins_get(
    State(state): State<Arc<ProxyState>>,
) -> Json<serde_json::Value> {
    let pins = state.model_pins.read().unwrap();
    Json(serde_json::json!({ "pins": *pins }))
}

/// PUT /api/v1/routing/pins — set model pins per tier
async fn api_routing_pins_set(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    // Accept { "pins": { "simple": [...], "complex": [...] } }
    if let Some(pins_val) = body.get("pins") {
        match serde_json::from_value::<std::collections::HashMap<coalesce_core::types::QualityTier, Vec<coalesce_core::router::config::ModelPin>>>(pins_val.clone()) {
            Ok(new_pins) => {
                let count: usize = new_pins.values().map(|v| v.len()).sum();
                *state.model_pins.write().unwrap() = new_pins.clone();

                // Persist to disk
                let pins_path = dirs::data_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join("coalesce")
                    .join("model_pins.json");
                let _ = std::fs::write(&pins_path, serde_json::to_string_pretty(&new_pins).unwrap_or_default());

                Json(serde_json::json!({"status": "ok", "total_pins": count}))
            }
            Err(e) => {
                Json(serde_json::json!({"status": "error", "error": format!("Invalid pins format: {e}")}))
            }
        }
    } else {
        Json(serde_json::json!({"status": "error", "error": "Missing 'pins' field"}))
    }
}

/// GET /api/v1/routing/equivalences — get model equivalence groups
async fn api_equivalences_get(
    State(state): State<Arc<ProxyState>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "equivalences": state.config.routing.model_equivalences }))
}

/// PUT /api/v1/routing/equivalences — set model equivalence groups
/// Body: { "equivalences": { "claude-opus-4.6": ["claude-opus-4-6-thinking", "claude-opus-4.6"], ... } }
async fn api_equivalences_set(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    if let Some(eq_val) = body.get("equivalences") {
        match serde_json::from_value::<std::collections::HashMap<String, Vec<String>>>(eq_val.clone()) {
            Ok(new_eq) => {
                let count = new_eq.len();
                // We need interior mutability for config — use unsafe-free approach via
                // storing equivalences separately and reading from there
                // For now, persist to disk and it'll load on next restart
                let eq_path = dirs::data_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join("coalesce")
                    .join("model_equivalences.json");
                let _ = std::fs::write(&eq_path, serde_json::to_string_pretty(&new_eq).unwrap_or_default());

                // Also update the live model_aliases DashMap for immediate routing
                state.model_aliases.clear();
                for (canonical, aliases) in &new_eq {
                    for alias in aliases {
                        if alias != canonical {
                            state.model_aliases.insert(alias.clone(), canonical.clone());
                        }
                    }
                }

                Json(serde_json::json!({"status": "ok", "groups": count}))
            }
            Err(e) => {
                Json(serde_json::json!({"status": "error", "error": format!("Invalid format: {e}")}))
            }
        }
    } else {
        Json(serde_json::json!({"status": "error", "error": "Missing 'equivalences' field"}))
    }
}

/// GET /api/v1/providers/priorities — get provider priorities and pricing modes
async fn api_provider_priorities_get(
    State(state): State<Arc<ProxyState>>,
) -> Json<serde_json::Value> {
    let mut result = serde_json::Map::new();
    for entry in state.provider_priorities.iter() {
        let name = entry.key().clone();
        let priority = *entry.value();
        let pricing_mode = state.provider_pricing_modes.get(&name)
            .map(|v| v.value().clone())
            .unwrap_or_else(|| "metered".to_string());
        result.insert(name, serde_json::json!({
            "priority": priority,
            "pricing_mode": pricing_mode,
        }));
    }
    Json(serde_json::json!({ "providers": result }))
}

/// PUT /api/v1/providers/priorities — set provider priorities and pricing modes
/// Body: { "providers": { "google": { "priority": 1, "pricing_mode": "subscription" }, ... } }
async fn api_provider_priorities_set(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    if let Some(providers) = body.get("providers").and_then(|v| v.as_object()) {
        for (name, val) in providers {
            if let Some(p) = val.get("priority").and_then(|v| v.as_u64()) {
                state.provider_priorities.insert(name.clone(), p as u32);
            }
            if let Some(m) = val.get("pricing_mode").and_then(|v| v.as_str()) {
                state.provider_pricing_modes.insert(name.clone(), m.to_string());
            }
        }

        // Persist to disk
        let mut save_data = serde_json::Map::new();
        for entry in state.provider_priorities.iter() {
            let name = entry.key().clone();
            let priority = *entry.value();
            let pricing_mode = state.provider_pricing_modes.get(&name)
                .map(|v| v.value().clone())
                .unwrap_or_else(|| "metered".to_string());
            save_data.insert(name, serde_json::json!({
                "priority": priority,
                "pricing_mode": pricing_mode,
            }));
        }
        let prio_path = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("coalesce")
            .join("provider_priorities.json");
        let _ = std::fs::write(&prio_path, serde_json::to_string_pretty(&save_data).unwrap_or_default());

        Json(serde_json::json!({"status": "ok"}))
    } else {
        Json(serde_json::json!({"status": "error", "error": "Missing 'providers' field"}))
    }
}

/// GET /api/v1/stats/summary — same as /v1/stats but under api prefix
async fn api_stats_summary(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    stats(State(state)).await
}

/// GET /api/v1/stats/timeline — recent requests with pagination
async fn api_stats_timeline(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<PaginationParams>,
) -> Json<serde_json::Value> {
    let limit = params.limit.unwrap_or(50).min(500);
    let offset = params.offset.unwrap_or(0);

    // Fetch limit + offset entries, then skip offset
    let all_recent = state
        .storage
        .recent_requests(limit + offset)
        .unwrap_or_default();

    let entries: Vec<serde_json::Value> = all_recent
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|r| {
            serde_json::json!({
                "tier": r.tier,
                "score": r.score,
                "provider": r.provider,
                "model": r.model,
                "input_tokens": r.input_tokens,
                "output_tokens": r.output_tokens,
                "cost_usd": r.cost_usd,
                "latency_ms": r.latency_ms,
                "success": r.success,
            })
        })
        .collect();

    Json(serde_json::json!({
        "entries": entries,
        "limit": limit,
        "offset": offset,
        "count": entries.len(),
    }))
}

#[derive(Debug, Deserialize)]
struct CostParams {
    days: Option<u32>,
}

/// GET /api/v1/stats/costs — cost analytics: per-provider, per-model, daily trends
async fn api_stats_costs(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<CostParams>,
) -> Json<serde_json::Value> {
    let days = params.days.unwrap_or(30);

    let by_provider = state.storage.costs_by_provider().unwrap_or_default();
    let by_model = state.storage.costs_by_model().unwrap_or_default();
    let daily = state.storage.costs_by_day(days).unwrap_or_default();

    let overall_stats = state.storage.stats().unwrap_or_else(|_| {
        coalesce_core::storage::RequestStats {
            total_requests: 0,
            successful_requests: 0,
            total_cost_usd: 0.0,
            avg_latency_ms: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        }
    });

    // Calculate savings: count free requests and estimate what they would have cost
    let total_free_requests: u64 = daily.iter().map(|d| d.free_requests).sum();
    let budget_status = serde_json::json!({
        "total_spent_usd": overall_stats.total_cost_usd,
        "daily_limit_usd": state.config.budget.daily_limit_usd,
        "total_limit_usd": state.config.budget.total_limit_usd,
    });

    Json(serde_json::json!({
        "by_provider": by_provider.iter().map(|b| serde_json::json!({
            "provider": b.group,
            "requests": b.requests,
            "input_tokens": b.input_tokens,
            "output_tokens": b.output_tokens,
            "total_cost_usd": b.total_cost_usd,
            "avg_latency_ms": b.avg_latency_ms,
        })).collect::<Vec<_>>(),
        "by_model": by_model.iter().map(|b| serde_json::json!({
            "model": b.group,
            "provider": b.subgroup,
            "requests": b.requests,
            "input_tokens": b.input_tokens,
            "output_tokens": b.output_tokens,
            "total_cost_usd": b.total_cost_usd,
            "avg_latency_ms": b.avg_latency_ms,
        })).collect::<Vec<_>>(),
        "daily": daily.iter().map(|d| serde_json::json!({
            "date": d.date,
            "requests": d.requests,
            "input_tokens": d.input_tokens,
            "output_tokens": d.output_tokens,
            "total_cost_usd": d.total_cost_usd,
            "free_requests": d.free_requests,
        })).collect::<Vec<_>>(),
        "summary": {
            "total_requests": overall_stats.total_requests,
            "total_cost_usd": overall_stats.total_cost_usd,
            "total_input_tokens": overall_stats.total_input_tokens,
            "total_output_tokens": overall_stats.total_output_tokens,
            "total_free_requests": total_free_requests,
            "avg_latency_ms": overall_stats.avg_latency_ms,
        },
        "budget": budget_status,
    }))
}

// --- Provider CRUD ---

#[derive(Debug, Deserialize)]
struct CreateProviderRequest {
    name: String,
    #[serde(flatten)]
    config: ProviderConfig,
}

/// POST /api/v1/providers/manage — create a new provider at runtime
async fn api_create_provider(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<CreateProviderRequest>,
) -> Response {
    let api_key = resolve_api_key(&body.config);

    let provider: Option<Arc<dyn Provider>> = match body.name.as_str() {
        "openrouter" => api_key.map(|k| Arc::new(OpenRouterProvider::new(k)) as Arc<dyn Provider>),
        "ollama" => Some(Arc::new(OllamaProvider::new(body.config.endpoint.clone()))),
        "copilot" => Some(Arc::new(if let Some(t) = api_key { CopilotProvider::with_token(t) } else { CopilotProvider::new() })),
        "anthropic" | "claude" => api_key.map(|k| Arc::new(AnthropicProvider::new(k)) as Arc<dyn Provider>),
        "glm" | "zhipu" => api_key.map(|k| Arc::new(factories::glm(k)) as Arc<dyn Provider>),
        "deepseek" => api_key.map(|k| Arc::new(factories::deepseek(k)) as Arc<dyn Provider>),
        "openai" => api_key.map(|k| Arc::new(factories::openai(k)) as Arc<dyn Provider>),
        "xai" | "grok" => api_key.map(|k| Arc::new(factories::xai(k)) as Arc<dyn Provider>),
        "kimi" | "moonshot" => api_key.map(|k| Arc::new(factories::kimi(k)) as Arc<dyn Provider>),
        "google" | "gemini" => api_key.map(|k| build_google_provider(k, &state.storage)),
        _ => None,
    };

    let Some(p) = provider else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Unknown or unconfigured provider"}))).into_response();
    };

    // Discover models
    let new_models = match p.list_models().await {
        Ok(m) => m,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("Model discovery failed: {}", e)}))).into_response(),
    };

    let billing = parse_billing(&body.config);
    state.economics.register(&body.name, None::<&str>, billing);
    state.circuit_breakers.insert(body.name.clone(), CircuitBreaker::default_provider());

    let model_count = new_models.len();
    state.models.write().unwrap().extend(new_models);
    state.providers.write().unwrap().push(p);

    // Set default priority, pricing mode, and billing for new provider
    state.provider_priorities.entry(body.name.clone()).or_insert(body.config.priority);
    state.provider_pricing_modes.entry(body.name.clone()).or_insert(body.config.pricing_mode.clone());

    // Persist provider config so it survives restarts
    persist_dynamic_provider(&body.name, &body.config);

    info!("Provider '{}' added with {} models", body.name, model_count);
    Json(serde_json::json!({"status": "created", "provider": body.name, "models": model_count})).into_response()
}

/// PUT /api/v1/providers/manage/{name} — update a provider's config
async fn api_update_provider(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
    Json(body): Json<ProviderConfig>,
) -> Response {
    // Remove old models for this provider
    state.models.write().unwrap().retain(|m| m.provider != name);
    // Remove old provider
    state.providers.write().unwrap().retain(|p| p.name() != name);

    // Re-create with new config
    let api_key = resolve_api_key(&body);
    let provider: Option<Arc<dyn Provider>> = match name.as_str() {
        "openrouter" => api_key.map(|k| Arc::new(OpenRouterProvider::new(k)) as Arc<dyn Provider>),
        "ollama" => Some(Arc::new(OllamaProvider::new(body.endpoint.clone()))),
        "copilot" => Some(Arc::new(if let Some(t) = api_key { CopilotProvider::with_token(t) } else { CopilotProvider::new() })),
        "anthropic" | "claude" => api_key.map(|k| Arc::new(AnthropicProvider::new(k)) as Arc<dyn Provider>),
        "glm" | "zhipu" => api_key.map(|k| Arc::new(factories::glm(k)) as Arc<dyn Provider>),
        "deepseek" => api_key.map(|k| Arc::new(factories::deepseek(k)) as Arc<dyn Provider>),
        "openai" => api_key.map(|k| Arc::new(factories::openai(k)) as Arc<dyn Provider>),
        "xai" | "grok" => api_key.map(|k| Arc::new(factories::xai(k)) as Arc<dyn Provider>),
        "kimi" | "moonshot" => api_key.map(|k| Arc::new(factories::kimi(k)) as Arc<dyn Provider>),
        "google" | "gemini" => api_key.map(|k| build_google_provider(k, &state.storage)),
        _ => None,
    };

    let Some(p) = provider else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Unknown provider"}))).into_response();
    };

    let new_models = match p.list_models().await {
        Ok(m) => m,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("Model discovery failed: {}", e)}))).into_response(),
    };

    let billing = parse_billing(&body);
    state.economics.register(&name, None::<&str>, billing);

    let model_count = new_models.len();
    state.models.write().unwrap().extend(new_models);
    state.providers.write().unwrap().push(p);

    persist_dynamic_provider(&name, &body);
    info!("Provider '{}' updated with {} models", name, model_count);
    Json(serde_json::json!({"status": "updated", "provider": name, "models": model_count})).into_response()
}

/// DELETE /api/v1/providers/manage/{name} — remove a provider
async fn api_delete_provider(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
) -> Json<serde_json::Value> {
    state.models.write().unwrap().retain(|m| m.provider != name);
    state.providers.write().unwrap().retain(|p| p.name() != name);
    state.circuit_breakers.remove(&name);
    remove_dynamic_provider(&name);
    info!("Provider '{}' removed", name);
    Json(serde_json::json!({"status": "deleted", "provider": name}))
}

/// PUT /api/v1/providers/{name}/billing — update billing type without re-creating provider
async fn api_update_billing(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let billing_str = body.get("billing").and_then(|v| v.as_str()).unwrap_or("per_token");
    let pconfig = ProviderConfig {
        billing: Some(billing_str.to_string()),
        ..Default::default()
    };
    let billing = parse_billing(&pconfig);
    state.economics.register(&name, None::<&str>, billing);

    // Persist to providers.json so it survives restarts
    let dp_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("coalesce")
        .join("providers.json");
    if dp_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&dp_path) {
            if let Ok(mut saved) = serde_json::from_str::<std::collections::HashMap<String, ProviderConfig>>(&contents) {
                saved.entry(name.clone()).and_modify(|pc| pc.billing = Some(billing_str.to_string()))
                    .or_insert_with(|| ProviderConfig { billing: Some(billing_str.to_string()), ..Default::default() });
                let _ = std::fs::write(&dp_path, serde_json::to_string_pretty(&saved).unwrap_or_default());
            }
        }
    }

    info!("Updated billing for '{}' to '{}'", name, billing_str);
    Json(serde_json::json!({"status": "ok", "provider": name, "billing": billing_str}))
}

/// POST /api/v1/providers/{name}/toggle — enable/disable a provider from routing
async fn api_provider_toggle(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let enabled = body.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let key = format!("disabled_provider:{}", name);

    if enabled {
        state.disabled_providers.remove(&name);
        let _ = state.storage.set(&key, "");
        // Remove the key entirely by setting empty — or we can delete
        // For simplicity, remove from kv by setting empty and checking on load
        info!("Enabled provider '{}'", name);
    } else {
        state.disabled_providers.insert(name.clone(), true);
        let _ = state.storage.set(&key, "1");
        info!("Disabled provider '{}'", name);
    }

    Json(serde_json::json!({
        "status": "ok",
        "provider": name,
        "enabled": enabled,
    }))
}

/// POST /api/v1/providers/{name}/models/{model}/toggle — enable/disable a model from routing
async fn api_model_toggle(
    State(state): State<Arc<ProxyState>>,
    AxumPath((provider, model)): AxumPath<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let enabled = body.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    // Model names may contain colons (e.g. phi4-mini:latest) — URL-encoded as ---
    let model_id = model.replace("---", ":");
    let compound_key = format!("{}::{}", provider, model_id);
    let storage_key = format!("disabled_model:{}", compound_key);

    if enabled {
        state.disabled_models.remove(&compound_key);
        let _ = state.storage.set(&storage_key, "");
        info!("Enabled model '{}' on '{}'", model_id, provider);
    } else {
        state.disabled_models.insert(compound_key.clone(), true);
        let _ = state.storage.set(&storage_key, "1");
        info!("Disabled model '{}' on '{}'", model_id, provider);
    }

    Json(serde_json::json!({
        "status": "ok",
        "provider": provider,
        "model": model_id,
        "enabled": enabled,
    }))
}

// --- Provider Test / Copilot OAuth / Ollama Models ---

/// POST /api/v1/providers/{name}/test — test connectivity to a provider
async fn api_test_provider(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
) -> Json<serde_json::Value> {
    let providers_snapshot: Vec<Arc<dyn Provider>> = state.providers.read().unwrap().clone();
    let provider = providers_snapshot.iter().find(|p| p.name() == name);

    let Some(p) = provider else {
        return Json(serde_json::json!({"ok": false, "error": "Provider not found"}));
    };

    match p.health_check().await {
        Ok(true) => {
            // Also try listing models as a deeper test
            match p.list_models().await {
                Ok(models) => Json(serde_json::json!({
                    "ok": true,
                    "models": models.len(),
                    "message": format!("Connected — {} models available", models.len())
                })),
                Err(e) => Json(serde_json::json!({
                    "ok": true,
                    "models": 0,
                    "message": format!("Connected but model discovery failed: {}", e)
                })),
            }
        }
        Ok(false) => Json(serde_json::json!({"ok": false, "error": "Health check returned false"})),
        Err(e) => Json(serde_json::json!({"ok": false, "error": format!("{}", e)})),
    }
}

/// POST /api/v1/providers/{name}/refresh — re-fetch models from provider API
async fn api_provider_refresh(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
) -> Json<serde_json::Value> {
    let providers_snapshot: Vec<Arc<dyn Provider>> = state.providers.read().unwrap().clone();
    let provider = providers_snapshot.iter().find(|p| p.name() == name);

    let Some(p) = provider else {
        return Json(serde_json::json!({"ok": false, "error": "Provider not found"}));
    };

    match p.list_models().await {
        Ok(new_models) => {
            let count = new_models.len();
            // Replace this provider's models in the global model list
            let mut models = state.models.write().unwrap();
            models.retain(|m| m.provider != name);
            models.extend(new_models);
            drop(models); // release lock before applying overrides
            apply_all_overrides(&state);
            info!(provider = %name, count, "Refreshed models from API");
            Json(serde_json::json!({
                "ok": true,
                "models": count,
                "message": format!("Refreshed — {} models loaded", count)
            }))
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("Model refresh failed: {}", e)
        })),
    }
}

/// POST /api/v1/auth/copilot/start — begin GitHub device flow
async fn api_copilot_auth_start(
    State(_state): State<Arc<ProxyState>>,
) -> Response {
    let provider = CopilotProvider::new();
    match provider.start_device_flow().await {
        Ok(flow) => Json(serde_json::json!({
            "user_code": flow.user_code,
            "verification_uri": flow.verification_uri,
            "device_code": flow.device_code,
            "expires_in": flow.expires_in,
            "interval": flow.interval,
        })).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({
            "error": format!("Device flow failed: {}", e)
        }))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CopilotPollRequest {
    device_code: String,
    /// Optional provider name for multi-account (e.g. "copilot-work")
    provider_name: Option<String>,
}

/// POST /api/v1/auth/copilot/poll — poll for OAuth token (single attempt)
async fn api_copilot_auth_poll(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<CopilotPollRequest>,
) -> Response {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", "Iv1.b507a08c87ecfe98"),
            ("device_code", &body.device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"status": "error", "error": format!("{}", e)}))).into_response(),
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"status": "error", "error": format!("{}", e)}))).into_response(),
    };

    if let Some(token) = json.get("access_token").and_then(|v| v.as_str()) {
        // Token obtained! Create copilot provider and add it
        let token = token.to_string();
        let prov_name = body.provider_name.unwrap_or_else(|| "copilot".to_string());
        let provider = Arc::new(CopilotProvider::with_token_and_name(token.clone(), prov_name.clone()));

        // Remove any existing provider with this name (re-auth replaces same account)
        state.models.write().unwrap().retain(|m| m.provider != prov_name);
        state.providers.write().unwrap().retain(|p| p.name() != prov_name);

        // Add new copilot provider with token
        let new_models = provider.list_models().await.unwrap_or_default();
        let model_count = new_models.len();
        state.economics.register(&prov_name, None::<&str>, BillingType::QuotaRefreshing { quota_per_window: 50, refresh_interval_secs: 18000 });
        state.circuit_breakers.insert(prov_name.clone(), CircuitBreaker::default_provider());
        state.models.write().unwrap().extend(new_models);
        state.providers.write().unwrap().push(provider);

        // Save token to storage for persistence
        if let Some(dir) = dirs::data_dir() {
            let db_path = dir.join("coalesce").join("coalesce.db");
            if let Ok(storage) = Storage::open(&db_path) {
                let _ = storage.set(&format!("{}_github_token", prov_name), &token);
            }
        }

        info!("Copilot OAuth completed for '{}' — {} models registered", prov_name, model_count);
        return Json(serde_json::json!({
            "status": "complete",
            "models": model_count,
        })).into_response();
    }

    // Include raw GitHub response for debugging
    let raw = json.to_string();
    match json.get("error").and_then(|v| v.as_str()) {
        Some("authorization_pending") => Json(serde_json::json!({"status": "pending", "raw": raw})).into_response(),
        Some("slow_down") => Json(serde_json::json!({"status": "slow_down", "raw": raw})).into_response(),
        Some("expired_token") => (StatusCode::GONE, Json(serde_json::json!({"status": "expired", "raw": raw}))).into_response(),
        Some("access_denied") => (StatusCode::FORBIDDEN, Json(serde_json::json!({"status": "denied", "raw": raw}))).into_response(),
        Some(err) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"status": "error", "error": err, "raw": raw}))).into_response(),
        None => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"status": "error", "error": "Unknown response", "raw": raw}))).into_response(),
    }
}

/// GET /api/v1/providers/ollama/models — list all ollama models with enabled status
async fn api_ollama_models(
    State(state): State<Arc<ProxyState>>,
) -> Response {
    // Get models directly from ollama API
    let client = reqwest::Client::new();
    let resp = client.get("http://localhost:11434/api/tags").send().await;

    let ollama_models: Vec<serde_json::Value> = match resp {
        Ok(r) if r.status().is_success() => {
            match r.json::<serde_json::Value>().await {
                Ok(json) => json.get("models")
                    .and_then(|m| m.as_array())
                    .cloned()
                    .unwrap_or_default(),
                Err(_) => vec![],
            }
        }
        _ => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Cannot connect to Ollama"}))).into_response(),
    };

    // Check which models are currently active in our registry
    let active_models: Vec<String> = state.models.read().unwrap()
        .iter()
        .filter(|m| m.provider == "ollama")
        .map(|m| m.id.clone())
        .collect();

    let models: Vec<serde_json::Value> = ollama_models.iter().map(|m| {
        let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
        let size = m.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
        let details = m.get("details").cloned().unwrap_or(serde_json::json!({}));
        let param_size = details.get("parameter_size").and_then(|p| p.as_str()).unwrap_or("");
        let family = details.get("family").and_then(|f| f.as_str()).unwrap_or("");
        let enabled = active_models.contains(&name.to_string());

        serde_json::json!({
            "name": name,
            "size_bytes": size,
            "parameter_size": param_size,
            "family": family,
            "enabled": enabled,
        })
    }).collect();

    Json(serde_json::json!({"models": models})).into_response()
}

#[derive(Debug, Deserialize)]
struct ToggleModelRequest {
    enabled: bool,
}

/// POST /api/v1/providers/ollama/models/{model}/toggle — enable/disable an ollama model
async fn api_ollama_toggle_model(
    State(state): State<Arc<ProxyState>>,
    AxumPath(model): AxumPath<String>,
    Json(body): Json<ToggleModelRequest>,
) -> Json<serde_json::Value> {
    let model_name = model.replace("---", ":"); // URL-safe encoding: phi4-mini---latest -> phi4-mini:latest

    if body.enabled {
        // Check if already exists
        let exists = state.models.read().unwrap().iter().any(|m| m.provider == "ollama" && m.id == model_name);
        if exists {
            return Json(serde_json::json!({"status": "already_enabled", "model": model_name}));
        }
        // Re-discover from ollama to get the model info
        let provider = OllamaProvider::new(None);
        if let Ok(models) = provider.list_models().await {
            if let Some(m) = models.into_iter().find(|m| m.id == model_name) {
                state.models.write().unwrap().push(m);
                return Json(serde_json::json!({"status": "enabled", "model": model_name}));
            }
        }
        Json(serde_json::json!({"status": "error", "error": "Model not found in ollama"}))
    } else {
        // Remove model from active list
        state.models.write().unwrap().retain(|m| !(m.provider == "ollama" && m.id == model_name));
        Json(serde_json::json!({"status": "disabled", "model": model_name}))
    }
}

// --- Ollama Management ---

#[derive(Debug, Deserialize)]
struct OllamaPullRequest {
    model: String,
}

/// POST /api/v1/ollama/pull — pull a model from Ollama registry (streaming progress)
async fn api_ollama_pull(
    Json(body): Json<OllamaPullRequest>,
) -> Response {
    let client = reqwest::Client::new();
    let resp = client
        .post("http://localhost:11434/api/pull")
        .json(&serde_json::json!({"name": body.model, "stream": true}))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            // Stream the progress events back to the client as SSE
            let stream = r.bytes_stream().filter_map(|chunk| async move {
                match chunk {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        // Ollama returns newline-delimited JSON, forward as SSE
                        let mut events = String::new();
                        for line in text.lines() {
                            if !line.trim().is_empty() {
                                events.push_str(&format!("data: {line}\n\n"));
                            }
                        }
                        if events.is_empty() {
                            None
                        } else {
                            Some(Ok::<_, Infallible>(events))
                        }
                    }
                    Err(_) => None,
                }
            });

            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/event-stream")
                .header("Cache-Control", "no-cache")
                .header("Connection", "keep-alive")
                .body(Body::from_stream(stream))
                .unwrap()
        }
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            (StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
             Json(serde_json::json!({"error": body}))).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("Cannot connect to Ollama: {e}")}))).into_response()
        }
    }
}

/// DELETE /api/v1/ollama/models/{model} — delete a model from Ollama
async fn api_ollama_delete_model(
    State(state): State<Arc<ProxyState>>,
    AxumPath(model): AxumPath<String>,
) -> Json<serde_json::Value> {
    let model_name = model.replace("---", ":");

    let client = reqwest::Client::new();
    let resp = client
        .delete("http://localhost:11434/api/delete")
        .json(&serde_json::json!({"name": model_name}))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            // Also remove from our active model registry
            state.models.write().unwrap().retain(|m| !(m.provider == "ollama" && m.id == model_name));
            Json(serde_json::json!({"status": "deleted", "model": model_name}))
        }
        Ok(r) => {
            let body = r.text().await.unwrap_or_default();
            Json(serde_json::json!({"status": "error", "error": body}))
        }
        Err(e) => {
            Json(serde_json::json!({"status": "error", "error": format!("Cannot connect to Ollama: {e}")}))
        }
    }
}

/// GET /api/v1/ollama/running — list currently running/loaded models (ollama ps)
async fn api_ollama_running() -> Response {
    let client = reqwest::Client::new();
    let resp = client.get("http://localhost:11434/api/ps").send().await;

    match resp {
        Ok(r) if r.status().is_success() => {
            match r.json::<serde_json::Value>().await {
                Ok(json) => {
                    let models = json.get("models")
                        .and_then(|m| m.as_array())
                        .cloned()
                        .unwrap_or_default();

                    let running: Vec<serde_json::Value> = models.iter().map(|m| {
                        let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
                        let size = m.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                        let vram = m.get("size_vram").and_then(|s| s.as_u64()).unwrap_or(0);
                        let expires = m.get("expires_at").and_then(|e| e.as_str()).unwrap_or("");
                        let details = m.get("details").cloned().unwrap_or(serde_json::json!({}));
                        let param_size = details.get("parameter_size").and_then(|p| p.as_str()).unwrap_or("");
                        let family = details.get("family").and_then(|f| f.as_str()).unwrap_or("");
                        let quant = details.get("quantization_level").and_then(|q| q.as_str()).unwrap_or("");

                        // Determine if GPU accelerated: if size_vram > 0 and close to total size
                        let gpu_percent = if size > 0 { (vram as f64 / size as f64 * 100.0) as u64 } else { 0 };
                        let processor = if gpu_percent > 90 { "GPU" } else if gpu_percent > 0 { "GPU/CPU" } else { "CPU" };

                        serde_json::json!({
                            "name": name,
                            "size_bytes": size,
                            "size_vram": vram,
                            "gpu_percent": gpu_percent,
                            "processor": processor,
                            "parameter_size": param_size,
                            "family": family,
                            "quantization": quant,
                            "expires_at": expires,
                        })
                    }).collect();

                    Json(serde_json::json!({"models": running})).into_response()
                }
                Err(_) => Json(serde_json::json!({"models": []})).into_response(),
            }
        }
        _ => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Cannot connect to Ollama"}))).into_response(),
    }
}

// --- Ollama Process Management ---

/// POST /api/v1/ollama/start — start Ollama process
async fn api_ollama_start() -> Json<serde_json::Value> {
    // Check if already running
    let client = reqwest::Client::new();
    if client.get("http://localhost:11434/api/tags").send().await.is_ok() {
        return Json(serde_json::json!({"status": "already_running"}));
    }

    // Try to start Ollama
    #[cfg(target_os = "macos")]
    {
        // Try the app first, then fall back to CLI
        let result = tokio::process::Command::new("open")
            .args(["-a", "Ollama"])
            .output()
            .await;
        if result.is_err() || !result.as_ref().unwrap().status.success() {
            // Try ollama serve in background
            let _ = tokio::process::Command::new("ollama")
                .arg("serve")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = tokio::process::Command::new("ollama")
            .arg("serve")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = tokio::process::Command::new("ollama")
            .arg("serve")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    // Wait a bit and check if it started
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    if client.get("http://localhost:11434/api/tags").send().await.is_ok() {
        Json(serde_json::json!({"status": "started"}))
    } else {
        Json(serde_json::json!({"status": "error", "error": "Ollama failed to start. Is it installed?"}))
    }
}

/// POST /api/v1/ollama/stop — stop Ollama process
async fn api_ollama_stop() -> Json<serde_json::Value> {
    #[cfg(target_os = "macos")]
    {
        let _ = tokio::process::Command::new("pkill")
            .args(["-f", "ollama"])
            .output()
            .await;
    }
    #[cfg(target_os = "linux")]
    {
        let _ = tokio::process::Command::new("pkill")
            .args(["-f", "ollama"])
            .output()
            .await;
    }
    #[cfg(target_os = "windows")]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/IM", "ollama.exe", "/F"])
            .output()
            .await;
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let client = reqwest::Client::new();
    let still_running = client.get("http://localhost:11434/api/tags").send().await.is_ok();
    if still_running {
        Json(serde_json::json!({"status": "error", "error": "Ollama is still running"}))
    } else {
        Json(serde_json::json!({"status": "stopped"}))
    }
}

/// GET /api/v1/ollama/status — check if running + GPU hardware info
async fn api_ollama_status() -> Json<serde_json::Value> {
    let client = reqwest::Client::new();
    let running = client
        .get("http://localhost:11434/api/tags")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .is_ok();

    // Get GPU info
    let gpu_info = get_gpu_info().await;

    // Get Ollama version if running
    let version = if running {
        client.get("http://localhost:11434/api/version")
            .send().await.ok()
            .and_then(|r| futures::executor::block_on(r.json::<serde_json::Value>()).ok())
            .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_default()
    } else {
        String::new()
    };

    Json(serde_json::json!({
        "running": running,
        "version": version,
        "gpu": gpu_info,
    }))
}

async fn get_gpu_info() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        // Use system_profiler for Metal GPU info
        if let Ok(output) = tokio::process::Command::new("system_profiler")
            .args(["SPDisplaysDataType", "-json"])
            .output()
            .await
        {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                let displays = json.get("SPDisplaysDataType")
                    .and_then(|d| d.as_array())
                    .cloned()
                    .unwrap_or_default();

                let gpus: Vec<serde_json::Value> = displays.iter().map(|gpu| {
                    let name = gpu.get("sppci_model").and_then(|n| n.as_str()).unwrap_or("Unknown GPU");
                    let vram = gpu.get("spdisplays_vram_shared")
                        .or_else(|| gpu.get("spdisplays_vram"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let metal = gpu.get("sppci_metal").and_then(|m| m.as_str()).unwrap_or("Not Supported");
                    let cores = gpu.get("sppci_cores").and_then(|c| c.as_str()).unwrap_or("");

                    serde_json::json!({
                        "name": name,
                        "vram": vram,
                        "metal": metal,
                        "cores": cores,
                        "type": "metal",
                    })
                }).collect();

                return serde_json::json!({"gpus": gpus, "acceleration": "Metal"});
            }
        }
        serde_json::json!({"gpus": [], "acceleration": "none"})
    }

    #[cfg(target_os = "linux")]
    {
        // Try nvidia-smi first
        if let Ok(output) = tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=name,memory.total,memory.used,memory.free,utilization.gpu", "--format=csv,noheader,nounits"])
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let gpus: Vec<serde_json::Value> = stdout.lines().filter_map(|line| {
                    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                    if parts.len() >= 5 {
                        Some(serde_json::json!({
                            "name": parts[0],
                            "vram_total_mb": parts[1].parse::<u64>().unwrap_or(0),
                            "vram_used_mb": parts[2].parse::<u64>().unwrap_or(0),
                            "vram_free_mb": parts[3].parse::<u64>().unwrap_or(0),
                            "utilization": parts[4].parse::<u64>().unwrap_or(0),
                            "type": "cuda",
                        }))
                    } else { None }
                }).collect();
                return serde_json::json!({"gpus": gpus, "acceleration": "CUDA"});
            }
        }
        serde_json::json!({"gpus": [], "acceleration": "none"})
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=name,memory.total,memory.used,memory.free,utilization.gpu", "--format=csv,noheader,nounits"])
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let gpus: Vec<serde_json::Value> = stdout.lines().filter_map(|line| {
                    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                    if parts.len() >= 5 {
                        Some(serde_json::json!({
                            "name": parts[0],
                            "vram_total_mb": parts[1].parse::<u64>().unwrap_or(0),
                            "vram_used_mb": parts[2].parse::<u64>().unwrap_or(0),
                            "vram_free_mb": parts[3].parse::<u64>().unwrap_or(0),
                            "utilization": parts[4].parse::<u64>().unwrap_or(0),
                            "type": "cuda",
                        }))
                    } else { None }
                }).collect();
                return serde_json::json!({"gpus": gpus, "acceleration": "CUDA"});
            }
        }
        serde_json::json!({"gpus": [], "acceleration": "none"})
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    serde_json::json!({"gpus": [], "acceleration": "none"})
}

// --- Library Search & Tags ---

#[derive(Debug, Deserialize)]
struct LibrarySearchParams {
    q: Option<String>,
}

/// Popular Ollama models with metadata for browsing
const OLLAMA_LIBRARY: &[(&str, &str, &str, &str)] = &[
    ("llama3.2", "Meta Llama 3.2", "1B, 3B", "Fast, lightweight, great for simple tasks"),
    ("llama3.3", "Meta Llama 3.3", "70B", "Top-tier open model for complex reasoning"),
    ("phi4-mini", "Microsoft Phi-4 Mini", "3.8B", "Small but powerful reasoning model"),
    ("qwen2.5", "Alibaba Qwen 2.5", "0.5B-72B", "Excellent multilingual, code, math"),
    ("qwen2.5-coder", "Qwen 2.5 Coder", "1.5B-32B", "Specialized for code generation"),
    ("deepseek-r1", "DeepSeek R1", "1.5B-671B", "Chain-of-thought reasoning model"),
    ("mistral", "Mistral 7B", "7B", "Fast, efficient general-purpose model"),
    ("mixtral", "Mixtral 8x7B", "47B MoE", "Mixture of experts, fast for its size"),
    ("gemma2", "Google Gemma 2", "2B, 9B, 27B", "Google's efficient open model"),
    ("codellama", "Code Llama", "7B-70B", "Meta's code-specialized model"),
    ("llava", "LLaVA", "7B, 13B", "Vision + language multimodal model"),
    ("nomic-embed-text", "Nomic Embed", "137M", "Text embeddings model"),
    ("starcoder2", "StarCoder 2", "3B-15B", "Code completion and generation"),
    ("command-r", "Cohere Command R", "35B", "Enterprise RAG and tool use"),
    ("dolphin-mixtral", "Dolphin Mixtral", "47B MoE", "Uncensored, creative responses"),
    ("neural-chat", "Intel Neural Chat", "7B", "Conversational AI optimized"),
    ("yi", "01.AI Yi", "6B-34B", "Strong bilingual EN/ZH model"),
    ("solar", "Upstage Solar", "10.7B", "High-quality Korean/English model"),
    ("nous-hermes2", "Nous Hermes 2", "7B-34B", "Fine-tuned for helpfulness"),
    ("wizard-vicuna", "Wizard Vicuna", "13B", "Creative writing and conversation"),
];

/// GET /api/v1/ollama/library/search — search available models
async fn api_ollama_library_search(
    Query(params): Query<LibrarySearchParams>,
) -> Json<serde_json::Value> {
    let query = params.q.unwrap_or_default().to_lowercase();

    let results: Vec<serde_json::Value> = OLLAMA_LIBRARY.iter()
        .filter(|(name, label, sizes, desc)| {
            query.is_empty()
                || name.contains(&query)
                || label.to_lowercase().contains(&query)
                || sizes.to_lowercase().contains(&query)
                || desc.to_lowercase().contains(&query)
        })
        .map(|(name, label, sizes, desc)| {
            serde_json::json!({
                "name": name,
                "label": label,
                "sizes": sizes,
                "description": desc,
            })
        })
        .collect();

    Json(serde_json::json!({"models": results}))
}

/// GET /api/v1/ollama/library/{model}/tags — get available tags for a model from registry
async fn api_ollama_library_tags(
    AxumPath(model): AxumPath<String>,
) -> Response {
    let client = reqwest::Client::new();
    // Try the Ollama registry API
    let url = format!("https://registry.ollama.ai/v2/library/{}/tags/list", model);
    match client.get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send().await
    {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let tags = json.get("tags")
                        .and_then(|t| t.as_array())
                        .cloned()
                        .unwrap_or_default();
                    Json(serde_json::json!({"model": model, "tags": tags})).into_response()
                }
                Err(_) => Json(serde_json::json!({"model": model, "tags": []})).into_response(),
            }
        }
        _ => {
            // Fallback: common tag patterns
            Json(serde_json::json!({
                "model": model,
                "tags": ["latest", "q4_0", "q4_K_M", "q5_0", "q5_K_M", "q8_0", "fp16"],
                "source": "fallback"
            })).into_response()
        }
    }
}

// --- Keep-Alive, Benchmark, Import, Alias, Preload, GPU Layers ---

#[derive(Debug, Deserialize)]
struct KeepAliveRequest {
    duration: String, // e.g. "5m", "1h", "0" (unload immediately)
}

/// POST /api/v1/ollama/models/{model}/keepalive — set keep-alive duration
async fn api_ollama_keepalive(
    AxumPath(model): AxumPath<String>,
    Json(body): Json<KeepAliveRequest>,
) -> Response {
    let model_name = model.replace("---", ":");
    let client = reqwest::Client::new();

    // Send a generate request with keep_alive to set the duration
    let resp = client.post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model_name,
            "prompt": "",
            "keep_alive": body.duration,
            "stream": false,
        }))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            Json(serde_json::json!({"status": "ok", "model": model_name, "keep_alive": body.duration})).into_response()
        }
        Ok(r) => {
            let err = r.text().await.unwrap_or_default();
            Json(serde_json::json!({"status": "error", "error": err})).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("{e}")}))).into_response()
        }
    }
}

/// POST /api/v1/ollama/models/{model}/benchmark — run a speed test
async fn api_ollama_benchmark(
    AxumPath(model): AxumPath<String>,
) -> Response {
    let model_name = model.replace("---", ":");
    let client = reqwest::Client::new();

    let start = Instant::now();
    let resp = client.post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model_name,
            "prompt": "Write a short poem about the moon.",
            "stream": false,
            "options": { "num_predict": 100 },
        }))
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let elapsed = start.elapsed();
            match r.json::<serde_json::Value>().await {
                Ok(json) => {
                    let eval_count = json.get("eval_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let eval_duration_ns = json.get("eval_duration").and_then(|v| v.as_u64()).unwrap_or(1);
                    let prompt_eval_count = json.get("prompt_eval_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let prompt_eval_duration_ns = json.get("prompt_eval_duration").and_then(|v| v.as_u64()).unwrap_or(1);

                    let tokens_per_sec = if eval_duration_ns > 0 {
                        (eval_count as f64) / (eval_duration_ns as f64 / 1_000_000_000.0)
                    } else { 0.0 };

                    let prompt_tokens_per_sec = if prompt_eval_duration_ns > 0 {
                        (prompt_eval_count as f64) / (prompt_eval_duration_ns as f64 / 1_000_000_000.0)
                    } else { 0.0 };

                    Json(serde_json::json!({
                        "model": model_name,
                        "tokens_generated": eval_count,
                        "generation_tokens_per_sec": (tokens_per_sec * 10.0).round() / 10.0,
                        "prompt_tokens": prompt_eval_count,
                        "prompt_tokens_per_sec": (prompt_tokens_per_sec * 10.0).round() / 10.0,
                        "total_duration_ms": elapsed.as_millis(),
                    })).into_response()
                }
                Err(e) => {
                    Json(serde_json::json!({"status": "error", "error": format!("{e}")})).into_response()
                }
            }
        }
        Ok(r) => {
            let err = r.text().await.unwrap_or_default();
            Json(serde_json::json!({"status": "error", "error": err})).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("{e}")}))).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct AliasRequest {
    alias: String,
}

/// POST /api/v1/ollama/models/{model}/alias — create a model alias
async fn api_ollama_alias(
    State(state): State<Arc<ProxyState>>,
    AxumPath(model): AxumPath<String>,
    Json(body): Json<AliasRequest>,
) -> Json<serde_json::Value> {
    let model_name = model.replace("---", ":");

    // Use Ollama's copy API to create the alias
    let client = reqwest::Client::new();
    let resp = client.post("http://localhost:11434/api/copy")
        .json(&serde_json::json!({
            "source": model_name,
            "destination": body.alias,
        }))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            state.model_aliases.insert(body.alias.clone(), model_name.clone());
            Json(serde_json::json!({"status": "created", "alias": body.alias, "source": model_name}))
        }
        Ok(r) => {
            let err = r.text().await.unwrap_or_default();
            Json(serde_json::json!({"status": "error", "error": err}))
        }
        Err(e) => {
            Json(serde_json::json!({"status": "error", "error": format!("{e}")}))
        }
    }
}

#[derive(Debug, Deserialize)]
struct PreloadRequest {
    enabled: bool,
}

/// POST /api/v1/ollama/models/{model}/preload — mark model for preload on startup
async fn api_ollama_preload(
    State(state): State<Arc<ProxyState>>,
    AxumPath(model): AxumPath<String>,
    Json(body): Json<PreloadRequest>,
) -> Json<serde_json::Value> {
    let model_name = model.replace("---", ":");
    let mut preload = state.ollama_preload.write().unwrap();

    if body.enabled {
        if !preload.contains(&model_name) {
            preload.push(model_name.clone());
        }
    } else {
        preload.retain(|m| m != &model_name);
    }

    // Persist to disk
    let preload_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("coalesce")
        .join("ollama_preload.json");
    let _ = std::fs::write(&preload_path, serde_json::to_string_pretty(&*preload).unwrap_or_default());

    let status = if body.enabled { "enabled" } else { "disabled" };
    Json(serde_json::json!({"status": status, "model": model_name}))
}

/// POST /api/v1/ollama/models/{model}/load — load model into memory
async fn api_ollama_load(
    AxumPath(model): AxumPath<String>,
) -> Response {
    let model_name = model.replace("---", ":");
    let client = reqwest::Client::new();

    let resp = client.post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model_name,
            "prompt": "",
            "stream": false,
            "keep_alive": "30m",
        }))
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            Json(serde_json::json!({"status": "loaded", "model": model_name})).into_response()
        }
        Ok(r) => {
            let err = r.text().await.unwrap_or_default();
            Json(serde_json::json!({"status": "error", "error": err})).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("{e}")}))).into_response()
        }
    }
}

/// GET /api/v1/ollama/preload — get list of preloaded models
async fn api_ollama_preload_list(
    State(state): State<Arc<ProxyState>>,
) -> Json<serde_json::Value> {
    let preload = state.ollama_preload.read().unwrap();
    Json(serde_json::json!({"models": *preload}))
}

#[derive(Debug, Deserialize)]
struct GpuLayersRequest {
    num_gpu: i64, // -1 = all layers, 0 = CPU only, >0 = specific count
}

/// POST /api/v1/ollama/models/{model}/gpu-layers — set GPU layer count
async fn api_ollama_gpu_layers(
    AxumPath(model): AxumPath<String>,
    Json(body): Json<GpuLayersRequest>,
) -> Response {
    let model_name = model.replace("---", ":");
    let client = reqwest::Client::new();

    // Use generate with num_gpu option to configure GPU offloading
    let resp = client.post("http://localhost:11434/api/generate")
        .json(&serde_json::json!({
            "model": model_name,
            "prompt": "",
            "stream": false,
            "options": { "num_gpu": body.num_gpu },
        }))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            Json(serde_json::json!({"status": "ok", "model": model_name, "num_gpu": body.num_gpu})).into_response()
        }
        Ok(r) => {
            let err = r.text().await.unwrap_or_default();
            Json(serde_json::json!({"status": "error", "error": err})).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("{e}")}))).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ImportRequest {
    path: String,
    name: String,
}

/// POST /api/v1/ollama/import — import a GGUF file as a new model
async fn api_ollama_import(
    Json(body): Json<ImportRequest>,
) -> Response {
    // Create a Modelfile that references the GGUF
    let modelfile = format!("FROM {}", body.path);

    let client = reqwest::Client::new();
    let resp = client.post("http://localhost:11434/api/create")
        .json(&serde_json::json!({
            "name": body.name,
            "modelfile": modelfile,
            "stream": false,
        }))
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            Json(serde_json::json!({"status": "created", "name": body.name})).into_response()
        }
        Ok(r) => {
            let err = r.text().await.unwrap_or_default();
            Json(serde_json::json!({"status": "error", "error": err})).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": format!("{e}")}))).into_response()
        }
    }
}

/// GET /api/v1/ollama/resources — live resource usage (memory, loaded models)
async fn api_ollama_resources() -> Response {
    let client = reqwest::Client::new();

    // Get running models for memory usage
    let ps_resp = client.get("http://localhost:11434/api/ps").send().await;
    let running_models = match ps_resp {
        Ok(r) if r.status().is_success() => {
            r.json::<serde_json::Value>().await.ok()
                .and_then(|j| j.get("models").and_then(|m| m.as_array()).cloned())
                .unwrap_or_default()
        }
        _ => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Cannot connect to Ollama"}))).into_response(),
    };

    let mut total_ram: u64 = 0;
    let mut total_vram: u64 = 0;
    let models: Vec<serde_json::Value> = running_models.iter().map(|m| {
        let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
        let size = m.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
        let vram = m.get("size_vram").and_then(|s| s.as_u64()).unwrap_or(0);
        let ram = size.saturating_sub(vram);
        total_ram += ram;
        total_vram += vram;
        serde_json::json!({"name": name, "ram_bytes": ram, "vram_bytes": vram})
    }).collect();

    // Get system memory info
    let sys_mem = get_system_memory().await;

    Json(serde_json::json!({
        "models": models,
        "total_model_ram": total_ram,
        "total_model_vram": total_vram,
        "system": sys_mem,
    })).into_response()
}

async fn get_system_memory() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = tokio::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .await
        {
            let total = String::from_utf8_lossy(&output.stdout).trim().parse::<u64>().unwrap_or(0);
            // Get used memory via vm_stat
            if let Ok(vm_output) = tokio::process::Command::new("vm_stat").output().await {
                let vm_str = String::from_utf8_lossy(&vm_output.stdout);
                let page_size: u64 = 16384; // Apple Silicon default
                let mut active: u64 = 0;
                let mut wired: u64 = 0;
                let mut compressed: u64 = 0;
                for line in vm_str.lines() {
                    if line.contains("Pages active") {
                        active = line.split(':').nth(1).and_then(|v| v.trim().trim_end_matches('.').parse().ok()).unwrap_or(0) * page_size;
                    } else if line.contains("Pages wired") {
                        wired = line.split(':').nth(1).and_then(|v| v.trim().trim_end_matches('.').parse().ok()).unwrap_or(0) * page_size;
                    } else if line.contains("Pages occupied by compressor") {
                        compressed = line.split(':').nth(1).and_then(|v| v.trim().trim_end_matches('.').parse().ok()).unwrap_or(0) * page_size;
                    }
                }
                let used = active + wired + compressed;
                return serde_json::json!({"total_bytes": total, "used_bytes": used, "free_bytes": total.saturating_sub(used)});
            }
            return serde_json::json!({"total_bytes": total});
        }
        serde_json::json!({})
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = tokio::process::Command::new("cat").arg("/proc/meminfo").output().await {
            let info = String::from_utf8_lossy(&output.stdout);
            let mut total: u64 = 0;
            let mut available: u64 = 0;
            for line in info.lines() {
                if line.starts_with("MemTotal:") {
                    total = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0) * 1024;
                } else if line.starts_with("MemAvailable:") {
                    available = line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0) * 1024;
                }
            }
            return serde_json::json!({"total_bytes": total, "used_bytes": total.saturating_sub(available), "free_bytes": available});
        }
        serde_json::json!({})
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    serde_json::json!({})
}

// --- Google OAuth (Antigravity-style Cloud Code Assist) ---
//
// Uses Google's Cloud Code Assist OAuth credentials for free Gemini access.
// Auth code flow: user signs in via browser → redirect to local callback → exchange for tokens.

const GOOGLE_OAUTH_CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const GOOGLE_OAUTH_CLIENT_SECRET: &str = "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";

/// Refresh a Google OAuth access token using a refresh token
async fn refresh_google_token(refresh_token: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", GOOGLE_OAUTH_CLIENT_ID),
            ("client_secret", GOOGLE_OAUTH_CLIENT_SECRET),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Token refresh failed: {}", body));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("Parse failed: {}", e))?;
    body.get("access_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "No access_token in response".to_string())
}
const GOOGLE_OAUTH_CODE_VERIFIER: &str = "cFH3lPzU2FhJjQhHlGqKqQhHlGqKqQhHlGqKqQhHlGq";
const CLOUDCODE_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";

/// Quick validation of a Google OAuth token via tokeninfo endpoint
async fn validate_google_token(token: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    client
        .get(format!("https://oauth2.googleapis.com/tokeninfo?access_token={}", token))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// Build a Google Cloud Code provider, reading project_id from storage
fn build_google_provider(token: String, storage: &Storage) -> Arc<dyn Provider> {
    let project_id = storage.get("google_project_id").ok().flatten().unwrap_or_default();
    Arc::new(GoogleCloudCodeProvider::new(token, project_id))
}

/// Read the fresh OAuth token from Antigravity's local SQLite DB
fn read_antigravity_token() -> Option<String> {
    let home = dirs::home_dir()?;
    let db_path = home.join("Library/Application Support/Antigravity/User/globalStorage/state.vscdb");
    if !db_path.exists() { return None; }
    let conn = rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let json_str: String = conn.query_row(
        "SELECT value FROM ItemTable WHERE key = 'antigravityAuthStatus'", [], |row| row.get(0),
    ).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    parsed.get("apiKey").and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Call loadCodeAssist to get project ID
async fn get_google_project_id(token: &str) -> Option<String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1internal:loadCodeAssist", CLOUDCODE_ENDPOINT))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("X-Client-Name", "antigravity")
        .header("X-Client-Version", "1.107.0")
        .header("x-goog-api-client", "gl-node/18.18.2 fire/0.8.6 grpc/1.10.x")
        .json(&serde_json::json!({"metadata":{"ideType":6,"platform":1,"pluginType":2},"mode":1}))
        .send().await.ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("cloudaicompanionProject")
        .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.get("id").and_then(|id| id.as_str()).map(|s| s.to_string())))
}

/// Discover available Google models based on subscription tier
async fn discover_google_models(token: &str) -> Vec<ModelInfo> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1internal:loadCodeAssist", CLOUDCODE_ENDPOINT))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("X-Client-Name", "antigravity")
        .header("X-Client-Version", "1.107.0")
        .header("x-goog-api-client", "gl-node/18.18.2 fire/0.8.6 grpc/1.10.x")
        .json(&serde_json::json!({"metadata":{"ideType":6,"platform":1,"pluginType":2},"mode":1}))
        .send().await;

    let tier = match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            let paid = body.get("paidTier").and_then(|t| t.get("id")).and_then(|v| v.as_str()).unwrap_or("");
            let current = body.get("currentTier").and_then(|t| t.get("id")).and_then(|v| v.as_str()).unwrap_or("");
            if paid.contains("ultra") || current.contains("ultra") { "ultra" }
            else if paid.contains("pro") || current == "standard-tier" { "pro" }
            else { "free" }
        }
        Ok(r) => {
            warn!("  google — loadCodeAssist failed: {}", r.status());
            "free"
        }
        Err(e) => {
            warn!("  google — loadCodeAssist request failed: {}", e);
            "free"
        }
    };
    info!("  google — detected tier: {}", tier);

    let p = "google";
    let mut models = vec![
        ModelInfo { id: "gemini-3-flash".into(), name: "Gemini 3 Flash".into(), provider: p.into(),
            input_price_per_m: 0.10, output_price_per_m: 0.40, context_window: 1048576,
            max_output: Some(65536), quality_tier: QualityTier::Medium, reasoning: false, vision: true, tool_calling: true,
            canonical_family: Some("gemini-3-flash".into()), capabilities: None },
        ModelInfo { id: "gemini-3.1-pro-low".into(), name: "Gemini 3.1 Pro (Low)".into(), provider: p.into(),
            input_price_per_m: 1.25, output_price_per_m: 5.0, context_window: 1048576,
            max_output: Some(65536), quality_tier: QualityTier::Reasoning, reasoning: true, vision: true, tool_calling: true,
            canonical_family: Some("gemini-3.1-pro".into()), capabilities: None },
    ];
    if tier == "pro" || tier == "ultra" {
        models.extend(vec![
            ModelInfo { id: "gemini-3.1-pro-high".into(), name: "Gemini 3.1 Pro (High)".into(), provider: p.into(),
                input_price_per_m: 1.25, output_price_per_m: 10.0, context_window: 1048576,
                max_output: Some(65536), quality_tier: QualityTier::Reasoning, reasoning: true, vision: true, tool_calling: true,
                canonical_family: Some("gemini-3.1-pro".into()), capabilities: None },
            ModelInfo { id: "claude-sonnet-4-6-thinking".into(), name: "Claude Sonnet 4.6 (Thinking)".into(), provider: p.into(),
                input_price_per_m: 3.0, output_price_per_m: 15.0, context_window: 200000,
                max_output: Some(16384), quality_tier: QualityTier::Reasoning, reasoning: true, vision: true, tool_calling: true,
                canonical_family: Some("claude-sonnet-4.6".into()), capabilities: None },
            ModelInfo { id: "claude-opus-4-6-thinking".into(), name: "Claude Opus 4.6 (Thinking)".into(), provider: p.into(),
                input_price_per_m: 15.0, output_price_per_m: 75.0, context_window: 200000,
                max_output: Some(16384), quality_tier: QualityTier::Reasoning, reasoning: true, vision: true, tool_calling: true,
                canonical_family: Some("claude-opus-4.6".into()), capabilities: None },
            ModelInfo { id: "gpt-oss-120b-medium".into(), name: "GPT-OSS 120B (Medium)".into(), provider: p.into(),
                input_price_per_m: 1.0, output_price_per_m: 4.0, context_window: 128000,
                max_output: Some(16384), quality_tier: QualityTier::Medium, reasoning: false, vision: true, tool_calling: true,
                canonical_family: Some(derive_canonical_family("gpt-oss-120b-medium")), capabilities: None },
        ]);
    }
    models
}

/// POST /api/v1/auth/google/start — returns the Google OAuth URL to open in browser
async fn api_google_auth_start(
    Json(_body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?\
         client_id={}&\
         redirect_uri={}&\
         response_type=code&\
         scope={}&\
         access_type=offline&\
         prompt=consent&\
         code_challenge={}&\
         code_challenge_method=plain",
        GOOGLE_OAUTH_CLIENT_ID,
        "http%3A%2F%2F127.0.0.1%3A8402%2Fapi%2Fv1%2Fauth%2Fgoogle%2Fcallback",
        "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcloud-platform+https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fuserinfo.email+https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fuserinfo.profile",
        GOOGLE_OAUTH_CODE_VERIFIER,
    );

    Json(serde_json::json!({
        "auth_url": auth_url,
        "status": "redirect",
    }))
}

/// GET /api/v1/auth/google/callback — OAuth redirect callback (browser lands here)
async fn api_google_auth_callback(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let code = match params.get("code") {
        Some(c) => c.clone(),
        None => {
            let err = params.get("error").map(|e| e.as_str()).unwrap_or("No authorization code received");
            return axum::response::Html(format!(
                "<html><body style='font-family:sans-serif;background:#1a1a2e;color:#e0e0e0;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>\
                 <div style='text-align:center'><h2 style='color:#ef4444'>Authorization Failed</h2><p>{err}</p>\
                 <p style='color:#888'>You can close this tab.</p></div></body></html>"
            )).into_response();
        }
    };

    // Exchange code for tokens
    let client = reqwest::Client::new();
    let token_resp = client.post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", GOOGLE_OAUTH_CLIENT_ID),
            ("client_secret", GOOGLE_OAUTH_CLIENT_SECRET),
            ("redirect_uri", "http://127.0.0.1:8402/api/v1/auth/google/callback"),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("code_verifier", GOOGLE_OAUTH_CODE_VERIFIER),
        ])
        .send()
        .await;

    let token_json = match token_resp {
        Ok(r) if r.status().is_success() => {
            match r.json::<serde_json::Value>().await {
                Ok(j) => j,
                Err(e) => return axum::response::Html(format!(
                    "<html><body style='font-family:sans-serif;background:#1a1a2e;color:#e0e0e0;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>\
                     <div style='text-align:center'><h2 style='color:#ef4444'>Token Error</h2><p>{e}</p></div></body></html>"
                )).into_response(),
            }
        }
        Ok(r) => {
            let body = r.text().await.unwrap_or_default();
            return axum::response::Html(format!(
                "<html><body style='font-family:sans-serif;background:#1a1a2e;color:#e0e0e0;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>\
                 <div style='text-align:center'><h2 style='color:#ef4444'>Token Exchange Failed</h2><p>{body}</p></div></body></html>"
            )).into_response();
        }
        Err(e) => return axum::response::Html(format!(
            "<html><body style='font-family:sans-serif;background:#1a1a2e;color:#e0e0e0;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>\
             <div style='text-align:center'><h2 style='color:#ef4444'>Connection Error</h2><p>{e}</p></div></body></html>"
        )).into_response(),
    };

    let access_token = token_json.get("access_token").and_then(|v| v.as_str()).unwrap_or("");
    let refresh_token = token_json.get("refresh_token").and_then(|v| v.as_str()).unwrap_or("");

    if access_token.is_empty() {
        return axum::response::Html(
            "<html><body style='font-family:sans-serif;background:#1a1a2e;color:#e0e0e0;display:flex;align-items:center;justify-content:center;height:100vh;margin:0'>\
             <div style='text-align:center'><h2 style='color:#ef4444'>No Access Token</h2><p>Google did not return an access token.</p></div></body></html>".to_string()
        ).into_response();
    }

    // Save tokens
    let _ = state.storage.set("google_access_token", access_token);
    if !refresh_token.is_empty() {
        let _ = state.storage.set("google_refresh_token", refresh_token);
    }

    // Register Google/Gemini provider
    let google = GoogleCloudCodeProvider::new(access_token.to_string(), state.storage.get("google_project_id").ok().flatten().unwrap_or_default());
    let model_count = match google.list_models().await {
        Ok(models) => {
            let count = models.len();
            for m in &models {
                state.economics.register(&m.provider, Some(&m.id), BillingType::PerToken);
            }
            state.models.write().unwrap().extend(models);
            count
        }
        Err(_) => 0,
    };

    state.providers.write().unwrap().push(Arc::new(google));
    state.circuit_breakers.insert("google".into(), CircuitBreaker::new(5, 60));

    // Return success page that auto-closes and notifies the frontend
    axum::response::Html(format!(
        r#"<html><body style="font-family:sans-serif;background:#1a1a2e;color:#e0e0e0;display:flex;align-items:center;justify-content:center;height:100vh;margin:0">
        <div style="text-align:center">
          <h2 style="color:#10b981">✓ Google Connected!</h2>
          <p>{model_count} Gemini models available</p>
          <p style="color:#888">You can close this tab and return to Coalesce.</p>
        </div>
        <script>
          // Notify opener window
          if (window.opener) {{
            window.opener.postMessage({{ type: 'google-auth-complete', models: {model_count} }}, '*');
          }}
          setTimeout(() => window.close(), 2000);
        </script>
        </body></html>"#
    )).into_response()
}

/// POST /api/v1/auth/google/poll — check if Google auth is complete (frontend polls this)
async fn api_google_auth_poll(
    State(state): State<Arc<ProxyState>>,
    Json(_body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    // Check if we have a google provider registered
    let has_google = state.providers.read().unwrap().iter().any(|p| p.name() == "google");
    if has_google {
        let model_count = state.models.read().unwrap().iter().filter(|m| m.provider == "google").count();
        Json(serde_json::json!({"status": "complete", "models": model_count}))
    } else {
        Json(serde_json::json!({"status": "pending"}))
    }
}

// --- Quality Feedback ---

#[derive(Debug, Deserialize)]
struct FeedbackRequest {
    provider: String,
    model: String,
    rating: f64, // 0.0 to 1.0
}

/// POST /api/v1/feedback — submit quality rating for a model
/// POST /api/v1/parse — extract text from uploaded documents (PDF, TXT, MD, CSV, JSON)
async fn api_parse_document(
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let data_str = match body.get("data").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return Json(serde_json::json!({"error": "missing 'data' field (base64)"})).into_response(),
    };
    let filename = body.get("filename").and_then(|v| v.as_str()).unwrap_or("file.txt");

    // Strip data URL prefix if present (e.g. "data:application/pdf;base64,...")
    let raw_b64 = if let Some(pos) = data_str.find(",") {
        &data_str[pos + 1..]
    } else {
        data_str
    };

    let file_bytes = match base64::engine::general_purpose::STANDARD.decode(raw_b64) {
        Ok(b) => b,
        Err(_) => return Json(serde_json::json!({"error": "invalid base64 data"})).into_response(),
    };

    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();

    match ext.as_str() {
        // Plain text formats — decode directly
        "txt" | "md" | "json" | "csv" | "html" | "xml" => {
            let text = String::from_utf8_lossy(&file_bytes).to_string();
            Json(serde_json::json!({"text": text, "filename": filename})).into_response()
        }
        // PDF — use pdf_oxide
        "pdf" => {
            let fname = filename.to_string();
            match tokio::task::spawn_blocking(move || {
                // Write to temp file (pdf_oxide needs a file path)
                let mut tmp = tempfile::Builder::new()
                    .suffix(".pdf")
                    .tempfile()
                    .map_err(|e| format!("temp file error: {}", e))?;
                std::io::Write::write_all(&mut tmp, &file_bytes)
                    .map_err(|e| format!("write error: {}", e))?;
                let path = tmp.path().to_path_buf();

                let mut doc = pdf_oxide::PdfDocument::open(path.to_str().unwrap_or(""))
                    .map_err(|e| format!("PDF open error: {}", e))?;
                let page_count = doc.page_count()
                    .map_err(|e| format!("PDF page count error: {}", e))?;
                let mut all_text = String::new();
                for i in 0..page_count {
                    match doc.extract_text(i) {
                        Ok(text) => {
                            if !all_text.is_empty() {
                                all_text.push('\n');
                            }
                            all_text.push_str(&text);
                        }
                        Err(_) => {} // skip unreadable pages
                    }
                }
                Ok::<String, String>(all_text)
            }).await {
                Ok(Ok(text)) => Json(serde_json::json!({"text": text, "filename": fname})).into_response(),
                Ok(Err(e)) => Json(serde_json::json!({"error": e})).into_response(),
                Err(e) => Json(serde_json::json!({"error": format!("extraction failed: {}", e)})).into_response(),
            }
        }
        // DOCX — extract text from word/document.xml inside the ZIP
        "docx" => {
            let fname = filename.to_string();
            match tokio::task::spawn_blocking(move || {
                let cursor = std::io::Cursor::new(file_bytes);
                let mut archive = zip::ZipArchive::new(cursor)
                    .map_err(|e| format!("DOCX open error: {}", e))?;
                let mut xml = String::new();
                if let Ok(mut file) = archive.by_name("word/document.xml") {
                    std::io::Read::read_to_string(&mut file, &mut xml)
                        .map_err(|e| format!("DOCX read error: {}", e))?;
                }
                // Strip XML tags to get plain text
                let mut text = String::new();
                let mut in_tag = false;
                for ch in xml.chars() {
                    match ch {
                        '<' => in_tag = true,
                        '>' => { in_tag = false; }
                        _ if !in_tag => text.push(ch),
                        _ => {}
                    }
                }
                // Clean up whitespace
                let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                Ok::<String, String>(text)
            }).await {
                Ok(Ok(text)) => Json(serde_json::json!({"text": text, "filename": fname})).into_response(),
                Ok(Err(e)) => Json(serde_json::json!({"error": e})).into_response(),
                Err(e) => Json(serde_json::json!({"error": format!("extraction failed: {}", e)})).into_response(),
            }
        }
        _ => {
            Json(serde_json::json!({
                "error": format!("Unsupported format '.{}'. Supported: pdf, docx, txt, md, json, csv, html, xml", ext)
            })).into_response()
        }
    }
}

async fn api_feedback(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<FeedbackRequest>,
) -> Json<serde_json::Value> {
    let rating = body.rating.clamp(0.0, 1.0);
    // Convert rating to success/latency equivalent
    let success = rating >= 0.5;
    let latency_ms = ((1.0 - rating) * 10000.0) as u64;
    state.quality.record(&body.provider, &body.model, success, latency_ms);
    Json(serde_json::json!({
        "status": "recorded",
        "provider": body.provider,
        "model": body.model,
        "current_score": state.quality.score(&body.provider, &body.model),
    }))
}

/// GET /api/v1/quality/scores — all quality scores
async fn api_quality_scores(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let scores: Vec<serde_json::Value> = state.quality.all_scores()
        .into_iter()
        .map(|(key, score, count)| {
            serde_json::json!({
                "key": key,
                "score": score,
                "sample_count": count,
            })
        })
        .collect();

    Json(serde_json::json!({"scores": scores}))
}

// ---------------------------------------------------------------------------
// Harness management endpoints
// ---------------------------------------------------------------------------

/// GET /api/v1/harnesses — list all harnesses and their status
async fn api_harnesses_list(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let proxy_url = format!("http://{}:{}", state.config.server.host, state.config.server.port);
    let harnesses = harness::detect_all(&proxy_url);
    Json(serde_json::json!({ "harnesses": harnesses }))
}

/// POST /api/v1/harnesses/{id}/configure — configure a harness to use Coalesce
async fn api_harness_configure(
    State(state): State<Arc<ProxyState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let proxy_url = format!("http://{}:{}", state.config.server.host, state.config.server.port);
    let result = harness::configure_harness(&id, &proxy_url);
    Json(serde_json::json!(result))
}

/// POST /api/v1/harnesses/{id}/restore — restore a harness to its original config
async fn api_harness_restore(
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let result = harness::restore_harness(&id);
    Json(serde_json::json!(result))
}

/// POST /api/v1/harnesses/takeover — configure ALL detected harnesses at once
async fn api_harness_takeover(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let proxy_url = format!("http://{}:{}", state.config.server.host, state.config.server.port);
    let results = harness::takeover_all(&proxy_url);
    Json(serde_json::json!({ "results": results }))
}

/// POST /api/v1/harnesses/restore-all — restore ALL harnesses to original config
async fn api_harness_restore_all() -> Json<serde_json::Value> {
    let results = harness::restore_all();
    Json(serde_json::json!({ "results": results }))
}

// ---------------------------------------------------------------------------
// Token vault endpoints
// ---------------------------------------------------------------------------

/// GET /api/v1/tokens — list all stored tokens (values redacted)
async fn api_tokens_list(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let tokens = state.token_vault.list();
    let entries: Vec<serde_json::Value> = tokens.iter().map(|(provider, token_type, valid)| {
        serde_json::json!({
            "provider": provider,
            "type": format!("{:?}", token_type),
            "valid": valid,
        })
    }).collect();
    Json(serde_json::json!({ "tokens": entries, "count": entries.len() }))
}

/// GET /api/v1/tokens/expiring — tokens expiring within 15 minutes
async fn api_tokens_expiring(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let expiring = state.token_vault.expiring_soon(std::time::Duration::from_secs(900));
    Json(serde_json::json!({ "expiring_soon": expiring }))
}

// ---------------------------------------------------------------------------
// Failover rules endpoints
// ---------------------------------------------------------------------------

async fn api_rules_list(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "rules": state.rules.list() }))
}

async fn api_rules_create(
    State(state): State<Arc<ProxyState>>,
    Json(rule): Json<rules::FailoverRule>,
) -> Json<serde_json::Value> {
    state.rules.add(rule.clone());
    Json(serde_json::json!({ "status": "created", "rule": rule }))
}

async fn api_rules_presets() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "presets": rules::preset_rules() }))
}

async fn api_rules_update(
    State(state): State<Arc<ProxyState>>,
    AxumPath(id): AxumPath<String>,
    Json(rule): Json<rules::FailoverRule>,
) -> Json<serde_json::Value> {
    if state.rules.update(&id, rule) {
        Json(serde_json::json!({ "status": "updated" }))
    } else {
        Json(serde_json::json!({ "status": "not_found" }))
    }
}

async fn api_rules_delete(
    State(state): State<Arc<ProxyState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    if state.rules.delete(&id) {
        Json(serde_json::json!({ "status": "deleted" }))
    } else {
        Json(serde_json::json!({ "status": "not_found" }))
    }
}

async fn api_rules_toggle(
    State(state): State<Arc<ProxyState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    match state.rules.toggle(&id) {
        Some(enabled) => Json(serde_json::json!({ "status": "toggled", "enabled": enabled })),
        None => Json(serde_json::json!({ "status": "not_found" })),
    }
}

// ---------------------------------------------------------------------------
// Anthropic Messages API compatibility — POST /v1/messages
// Accepts Anthropic Messages format, translates to internal ChatRequest,
// routes through the same engine, translates response back.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AnthropicMessagesRequest {
    model: String,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    #[serde(default)]
    system: Option<serde_json::Value>, // String or array of content blocks
    messages: Vec<serde_json::Value>,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    top_p: Option<f64>,
    #[serde(default)]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    tool_choice: Option<serde_json::Value>,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

fn default_max_tokens() -> u32 { 4096 }

/// Translate Anthropic Messages request to internal ChatRequest (OpenAI format)
fn anthropic_to_chat_request(req: AnthropicMessagesRequest) -> ChatRequest {
    let mut messages = Vec::new();

    // System message
    if let Some(system) = req.system {
        let system_text = match system {
            serde_json::Value::String(s) => s,
            serde_json::Value::Array(arr) => {
                arr.iter()
                    .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => String::new(),
        };
        if !system_text.is_empty() {
            messages.push(Message {
                role: "system".into(),
                content: Some(MessageContent::Text(system_text)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            });
        }
    }

    // User/assistant messages
    for msg in &req.messages {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");

        // Handle content as string or array of blocks
        let content_val = msg.get("content");

        match content_val {
            Some(serde_json::Value::String(text)) => {
                messages.push(Message {
                    role: role.into(),
                    content: Some(MessageContent::Text(text.clone())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                });
            }
            Some(serde_json::Value::Array(blocks)) => {
                let mut text_parts = Vec::new();
                let mut tool_calls_out = Vec::new();
                let mut tool_result_id = None;
                let mut image_parts: Vec<ContentPart> = Vec::new();

                for block in blocks {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                text_parts.push(t.to_string());
                            }
                        }
                        "image" => {
                            if let Some(source) = block.get("source") {
                                let media_type = source.get("media_type").and_then(|m| m.as_str()).unwrap_or("image/png");
                                let data = source.get("data").and_then(|d| d.as_str()).unwrap_or("");
                                image_parts.push(ContentPart::ImageUrl {
                                    image_url: coalesce_core::types::ImageUrl {
                                        url: format!("data:{};base64,{}", media_type, data),
                                        detail: None,
                                    },
                                });
                            }
                        }
                        "tool_use" => {
                            let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
                            tool_calls_out.push(serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(&input).unwrap_or_default(),
                                }
                            }));
                        }
                        "tool_result" => {
                            tool_result_id = block.get("tool_use_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                            if let Some(content) = block.get("content") {
                                match content {
                                    serde_json::Value::String(s) => text_parts.push(s.clone()),
                                    serde_json::Value::Array(arr) => {
                                        for item in arr {
                                            if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                                                text_parts.push(t.to_string());
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if let Some(tcid) = tool_result_id {
                    // This is a tool result message
                    messages.push(Message {
                        role: "tool".into(),
                        content: Some(MessageContent::Text(text_parts.join("\n"))),
                        name: None,
                        tool_calls: None,
                        tool_call_id: Some(tcid),
                        extra: Default::default(),
                    });
                } else if !tool_calls_out.is_empty() {
                    // Assistant with tool calls
                    messages.push(Message {
                        role: role.into(),
                        content: if text_parts.is_empty() { None } else { Some(MessageContent::Text(text_parts.join("\n"))) },
                        name: None,
                        tool_calls: Some(tool_calls_out),
                        tool_call_id: None,
                        extra: Default::default(),
                    });
                } else if !image_parts.is_empty() {
                    // Message with images
                    let mut parts: Vec<ContentPart> = text_parts.iter().map(|t| ContentPart::Text { text: t.clone() }).collect();
                    parts.extend(image_parts);
                    messages.push(Message {
                        role: role.into(),
                        content: Some(MessageContent::Parts(parts)),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        extra: Default::default(),
                    });
                } else {
                    messages.push(Message {
                        role: role.into(),
                        content: Some(MessageContent::Text(text_parts.join("\n"))),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        extra: Default::default(),
                    });
                }
            }
            _ => {
                messages.push(Message {
                    role: role.into(),
                    content: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                });
            }
        }
    }

    // Translate Anthropic tools to OpenAI format
    let tools = req.tools.map(|tools| {
        tools.iter().filter_map(|t| {
            let name = t.get("name")?.as_str()?;
            let desc = t.get("description").and_then(|d| d.as_str()).unwrap_or("");
            let schema = t.get("input_schema").cloned().unwrap_or(serde_json::json!({"type": "object"}));
            Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": desc,
                    "parameters": schema,
                }
            }))
        }).collect()
    });

    ChatRequest {
        model: req.model,
        messages,
        stream: req.stream,
        max_tokens: Some(req.max_tokens),
        temperature: req.temperature,
        top_p: req.top_p,
        stop: None,
        tools,
        tool_choice: req.tool_choice.map(|tc| {
            if let Some(obj) = tc.as_object() {
                if let Some(tc_type) = obj.get("type").and_then(|t| t.as_str()) {
                    match tc_type {
                        "auto" => serde_json::json!("auto"),
                        "any" => serde_json::json!("required"),
                        "tool" => {
                            if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
                                serde_json::json!({"type": "function", "function": {"name": name}})
                            } else {
                                serde_json::json!("auto")
                            }
                        }
                        _ => serde_json::json!("auto"),
                    }
                } else {
                    tc
                }
            } else {
                tc
            }
        }),
        response_format: None,
        extra: Default::default(),
    }
}

/// Translate OpenAI-format response back to Anthropic Messages format
fn chat_response_to_anthropic(resp: &serde_json::Value, model: &str) -> serde_json::Value {
    let choices = resp.get("choices").and_then(|c| c.as_array());
    let mut content_blocks = Vec::new();
    let mut stop_reason = "end_turn".to_string();

    if let Some(choices) = choices {
        if let Some(choice) = choices.first() {
            // Finish reason
            if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                stop_reason = match fr {
                    "stop" => "end_turn",
                    "length" => "max_tokens",
                    "tool_calls" => "tool_use",
                    _ => "end_turn",
                }.to_string();
            }

            let msg = choice.get("message").unwrap_or(&serde_json::Value::Null);

            // Text content
            if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
                if !text.is_empty() {
                    content_blocks.push(serde_json::json!({
                        "type": "text",
                        "text": text,
                    }));
                }
            }

            // Tool calls
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|tc| tc.as_array()) {
                for tc in tool_calls {
                    if let Some(func) = tc.get("function") {
                        let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("call_0");
                        let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        let args_str = func.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                        let input: serde_json::Value = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                        content_blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        }));
                    }
                }
            }
        }
    }

    if content_blocks.is_empty() {
        content_blocks.push(serde_json::json!({
            "type": "text",
            "text": "",
        }));
    }

    // Usage
    let usage = resp.get("usage").map(|u| {
        serde_json::json!({
            "input_tokens": u.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0),
            "output_tokens": u.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0),
        })
    }).unwrap_or(serde_json::json!({"input_tokens": 0, "output_tokens": 0}));

    let msg_id = resp.get("id").and_then(|i| i.as_str()).unwrap_or("msg_coalesce");

    serde_json::json!({
        "id": msg_id,
        "type": "message",
        "role": "assistant",
        "content": content_blocks,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage,
    })
}

/// Translate OpenAI SSE chunk to Anthropic SSE events.
/// Optionally uses a think-split parser to extract `<think>` tags into thinking blocks.
fn chat_stream_chunk_to_anthropic(
    chunk_data: &str,
    block_idx: &mut usize,
    started: &mut bool,
    think_parser: Option<&std::sync::Arc<std::sync::Mutex<coalesce_core::rosetta::ThinkingSplitParser>>>,
) -> String {
    let mut output = String::new();

    if chunk_data == "[DONE]" {
        // Close any open block
        if *block_idx > 0 || *started {
            output.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type": "content_block_stop", "index": *block_idx})
            ));
        }
        output.push_str("event: message_stop\ndata: {\"type\": \"message_stop\"}\n\n");
        return output;
    }

    if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(chunk_data) {
        if !*started {
            *started = true;
            let model = chunk.get("model").and_then(|m| m.as_str()).unwrap_or("unknown");
            output.push_str(&format!(
                "event: message_start\ndata: {}\n\n",
                serde_json::json!({
                    "type": "message_start",
                    "message": {
                        "id": chunk.get("id").and_then(|i| i.as_str()).unwrap_or("msg_coalesce"),
                        "type": "message",
                        "role": "assistant",
                        "content": [],
                        "model": model,
                        "stop_reason": null,
                        "stop_sequence": null,
                        "usage": {"input_tokens": 0, "output_tokens": 0}
                    }
                })
            ));
        }

        if let Some(choices) = chunk.get("choices").and_then(|c| c.as_array()) {
            if let Some(choice) = choices.first() {
                let delta = choice.get("delta").unwrap_or(&serde_json::Value::Null);

                // Text content — may contain <think> tags that need splitting
                if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                    if let Some(parser) = think_parser {
                        if let Ok(mut parser) = parser.lock() {
                            let split = parser.process_chunk(text);

                            // Emit thinking blocks
                            for thinking_block in &split.thinking_completed {
                                // Start a thinking content block
                                output.push_str(&format!(
                                    "event: content_block_start\ndata: {}\n\n",
                                    serde_json::json!({
                                        "type": "content_block_start",
                                        "index": *block_idx,
                                        "content_block": {"type": "thinking", "thinking": ""}
                                    })
                                ));
                                output.push_str(&format!(
                                    "event: content_block_delta\ndata: {}\n\n",
                                    serde_json::json!({
                                        "type": "content_block_delta",
                                        "index": *block_idx,
                                        "delta": {"type": "thinking_delta", "thinking": thinking_block.text}
                                    })
                                ));
                                output.push_str(&format!(
                                    "event: content_block_stop\ndata: {}\n\n",
                                    serde_json::json!({"type": "content_block_stop", "index": *block_idx})
                                ));
                                *block_idx += 1;
                            }

                            // Emit regular content
                            if !split.content.is_empty() {
                                // Start text block if this is the first content after thinking
                                if *block_idx > 0 && split.thinking_completed.len() > 0 {
                                    output.push_str(&format!(
                                        "event: content_block_start\ndata: {}\n\n",
                                        serde_json::json!({
                                            "type": "content_block_start",
                                            "index": *block_idx,
                                            "content_block": {"type": "text", "text": ""}
                                        })
                                    ));
                                } else if *block_idx == 0 {
                                    // First content block
                                    output.push_str(&format!(
                                        "event: content_block_start\ndata: {}\n\n",
                                        serde_json::json!({
                                            "type": "content_block_start",
                                            "index": *block_idx,
                                            "content_block": {"type": "text", "text": ""}
                                        })
                                    ));
                                }
                                output.push_str(&format!(
                                    "event: content_block_delta\ndata: {}\n\n",
                                    serde_json::json!({
                                        "type": "content_block_delta",
                                        "index": *block_idx,
                                        "delta": {"type": "text_delta", "text": split.content}
                                    })
                                ));
                            }
                        }
                    } else {
                        // No think parser — emit as-is
                        if *block_idx == 0 {
                            output.push_str(&format!(
                                "event: content_block_start\ndata: {}\n\n",
                                serde_json::json!({
                                    "type": "content_block_start",
                                    "index": 0,
                                    "content_block": {"type": "text", "text": ""}
                                })
                            ));
                        }
                        output.push_str(&format!(
                            "event: content_block_delta\ndata: {}\n\n",
                            serde_json::json!({
                                "type": "content_block_delta",
                                "index": *block_idx,
                                "delta": {"type": "text_delta", "text": text}
                            })
                        ));
                    }
                }

                // Tool call deltas — translate to Anthropic tool_use blocks
                if let Some(tool_calls) = delta.get("tool_calls").and_then(|tc| tc.as_array()) {
                    for tc in tool_calls {
                        let tc_idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let func = tc.get("function").unwrap_or(&serde_json::Value::Null);

                        // First chunk for this tool call — start a new block
                        if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                            let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            // Close previous block if any
                            if *block_idx > 0 || *started {
                                output.push_str(&format!(
                                    "event: content_block_stop\ndata: {}\n\n",
                                    serde_json::json!({"type": "content_block_stop", "index": *block_idx})
                                ));
                            }
                            *block_idx = tc_idx + 1; // offset by 1 for the text block
                            output.push_str(&format!(
                                "event: content_block_start\ndata: {}\n\n",
                                serde_json::json!({
                                    "type": "content_block_start",
                                    "index": *block_idx,
                                    "content_block": {
                                        "type": "tool_use",
                                        "id": id,
                                        "name": name,
                                        "input": {}
                                    }
                                })
                            ));
                        }

                        // Arguments delta
                        if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                            if !args.is_empty() {
                                output.push_str(&format!(
                                    "event: content_block_delta\ndata: {}\n\n",
                                    serde_json::json!({
                                        "type": "content_block_delta",
                                        "index": *block_idx,
                                        "delta": {"type": "input_json_delta", "partial_json": args}
                                    })
                                ));
                            }
                        }
                    }
                }

                // Finish reason
                if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                    let stop_reason = match fr {
                        "stop" => "end_turn",
                        "length" => "max_tokens",
                        "tool_calls" => "tool_use",
                        _ => "end_turn",
                    };
                    output.push_str(&format!(
                        "event: content_block_stop\ndata: {}\n\n",
                        serde_json::json!({"type": "content_block_stop", "index": *block_idx})
                    ));
                    output.push_str(&format!(
                        "event: message_delta\ndata: {}\n\n",
                        serde_json::json!({
                            "type": "message_delta",
                            "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                            "usage": {"output_tokens": 0}
                        })
                    ));
                }
            }
        }
    }
    output
}

/// POST /v1/messages — Anthropic Messages API compatibility endpoint
async fn anthropic_messages(
    State(state): State<Arc<ProxyState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AnthropicMessagesRequest>,
) -> Response {
    // Auth: check x-api-key or Authorization header
    let has_auth = headers.get("x-api-key").is_some()
        || headers.get("authorization").is_some();
    if !has_auth {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_string(&serde_json::json!({
                "type": "error",
                "error": {"type": "authentication_error", "message": "Missing x-api-key or Authorization header"}
            })).unwrap()))
            .unwrap();
    }

    let is_stream = req.stream;
    let start = std::time::Instant::now();

    // Rosetta ingress: normalize Anthropic-format tools to canonical form
    let normalized_tools = req.tools.as_ref().map(|tools| {
        state.rosetta.normalize_request_tools(tools, req.tool_choice.as_ref())
    });

    // Detect thinking request
    let has_thinking = req.extra.contains_key("thinking")
        || req.extra.contains_key("reasoning_effort");

    // Translate to internal ChatRequest
    let chat_request = anthropic_to_chat_request(req);

    // Route
    let scoring = coalesce_core::router::route(&chat_request, &state.config.routing);
    let models = state.models.read().unwrap().clone();

    // Build candidate list with Rosetta filtering
    let mut candidates: Vec<_> = models.iter()
        .filter(|m| m.quality_tier.can_handle(&scoring.tier))
        .filter(|m| {
            state.circuit_breakers.get(&m.provider)
                .map(|cb| cb.is_available())
                .unwrap_or(true)
        })
        .filter(|m| !state.disabled_providers.contains_key(&m.provider))
        .filter(|m| !state.disabled_models.contains_key(&format!("{}::{}", m.provider, m.id)))
        .cloned()
        .collect();

    // Rosetta: filter by tool capabilities
    if let Some(ref nt) = normalized_tools {
        candidates.retain(|m| {
            state.rosetta.filter_by_tool_capabilities(&m.provider, nt, has_thinking).passes
        });
    }

    candidates.sort_by(|a, b| {
        let pa = state.provider_priorities.get(&a.provider).map(|v| *v).unwrap_or(50);
        let pb = state.provider_priorities.get(&b.provider).map(|v| *v).unwrap_or(50);
        pa.cmp(&pb)
    });

    let error_response = || -> Response {
        Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_string(&serde_json::json!({
                "type": "error",
                "error": {"type": "overloaded_error", "message": "All providers failed"}
            })).unwrap()))
            .unwrap()
    };

    for model in candidates.iter().take(MAX_FALLBACK_ATTEMPTS) {
        let provider = state.providers.read().unwrap().iter()
            .find(|p| p.name() == model.provider)
            .cloned();

        let provider = match provider {
            Some(p) => p,
            None => continue,
        };

        let mut forwarded = chat_request.clone();
        forwarded.model = model.id.clone();

        // Rosetta egress: translate tools to provider-native format
        if let Some(ref nt) = normalized_tools {
            if let Ok(translated) = state.rosetta.translate_tools_for_provider(&model.provider, nt) {
                forwarded.tools = Some(translated);
            }
            if let Some(ref tc) = nt.tool_choice {
                forwarded.tool_choice = Some(
                    state.rosetta.translate_tool_choice_for_provider(&model.provider, tc)
                );
            }
        }

        if is_stream {
            forwarded.stream = true;
            if let Ok(stream) = provider.chat_stream(&forwarded).await {
                if let Some(cb) = state.circuit_breakers.get(&model.provider) { cb.record_success(); }

                // Log request
                let _ = state.storage.log_request(&RequestLogEntry {
                    id: None, timestamp: None,
                    tier: scoring.tier.to_string(), score: scoring.score,
                    provider: model.provider.clone(), model: model.id.clone(),
                    input_tokens: None, output_tokens: None, cost_usd: None,
                    latency_ms: Some(start.elapsed().as_millis() as u64), success: true,
                });

                let block_idx = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let provider_name = model.provider.clone();
                let think_parser = state.rosetta.create_think_parser(&provider_name)
                    .map(|p| std::sync::Arc::new(std::sync::Mutex::new(p)));

                let anthropic_stream = stream.map(move |result| {
                    result.map(|bytes| {
                        let text = String::from_utf8_lossy(&bytes);
                        let mut output = String::new();
                        let mut bi = block_idx.load(std::sync::atomic::Ordering::Relaxed);
                        let mut s = started.load(std::sync::atomic::Ordering::Relaxed);

                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                output.push_str(&chat_stream_chunk_to_anthropic(
                                    data, &mut bi, &mut s, think_parser.as_ref(),
                                ));
                            }
                        }

                        block_idx.store(bi, std::sync::atomic::Ordering::Relaxed);
                        started.store(s, std::sync::atomic::Ordering::Relaxed);
                        bytes::Bytes::from(output)
                    })
                });

                let body_stream = anthropic_stream.map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
                return Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/event-stream")
                    .header("Cache-Control", "no-cache")
                    .body(Body::from_stream(body_stream))
                    .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Stream failed").into_response());
            }
        } else {
            forwarded.stream = false;
            if let Ok(resp) = provider.chat(&forwarded).await {
                if let Some(cb) = state.circuit_breakers.get(&model.provider) { cb.record_success(); }

                // Log request
                let (it, ot, cost) = extract_usage(&resp, model);
                let _ = state.storage.log_request(&RequestLogEntry {
                    id: None, timestamp: None,
                    tier: scoring.tier.to_string(), score: scoring.score,
                    provider: model.provider.clone(), model: model.id.clone(),
                    input_tokens: it, output_tokens: ot, cost_usd: cost,
                    latency_ms: Some(start.elapsed().as_millis() as u64), success: true,
                });

                // Rosetta: extract thinking from provider response
                let thinking = state.rosetta.normalize_response_thinking(&model.provider, &resp);
                let mut anthropic_resp = chat_response_to_anthropic(&resp, &model.id);

                // Inject thinking blocks into Anthropic response
                if thinking.has_content() {
                    if let Some(content) = anthropic_resp.get_mut("content").and_then(|c| c.as_array_mut()) {
                        let thinking_blocks = thinking.to_anthropic_blocks();
                        for (i, block) in thinking_blocks.into_iter().enumerate() {
                            content.insert(i, block);
                        }
                    }
                }

                return Json(anthropic_resp).into_response();
            }
        }

        if let Some(cb) = state.circuit_breakers.get(&model.provider) { cb.record_failure(); }
    }

    error_response()
}

/// GET /metrics — Prometheus metrics endpoint
async fn api_metrics(State(state): State<Arc<ProxyState>>) -> Response {
    let body = state.prometheus_handle.render();
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
        .body(Body::from(body))
        .unwrap()
}

/// GET /api/v1/events — Server-Sent Events stream
async fn api_events(State(state): State<Arc<ProxyState>>) -> Response {
    let rx = state.event_tx.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|result| async move {
        match result {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok::<_, Infallible>(format!("data: {data}\n\n")))
            }
            Err(_) => None,
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

// ==================== Configuration Profiles (DB-backed) ====================

/// Capture current runtime state as a profile config JSON
fn capture_current_config(state: &Arc<ProxyState>) -> serde_json::Value {
    // Read current providers.json
    let providers_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("coalesce")
        .join("providers.json");
    let dynamic_providers: std::collections::HashMap<String, ProviderConfig> = if providers_path.exists() {
        std::fs::read_to_string(&providers_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    let mut all_providers = state.config.providers.clone();
    for (k, v) in dynamic_providers {
        all_providers.insert(k, v);
    }

    // Capture priorities
    let mut priorities = std::collections::HashMap::new();
    for entry in state.provider_priorities.iter() {
        let pname = entry.key().clone();
        let priority = *entry.value();
        let pricing_mode = state.provider_pricing_modes.get(&pname)
            .map(|v| v.value().clone())
            .unwrap_or_else(|| "metered".to_string());
        priorities.insert(pname, serde_json::json!({
            "priority": priority,
            "pricing_mode": pricing_mode,
        }));
    }

    // Capture model pins
    let pins = state.model_pins.read().unwrap().clone();
    let pins_json = serde_json::to_value(&pins).unwrap_or(serde_json::json!({}));

    // Capture equivalences from disk
    let eq_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("coalesce")
        .join("model_equivalences.json");
    let equivalences: std::collections::HashMap<String, Vec<String>> = if eq_path.exists() {
        std::fs::read_to_string(&eq_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        state.config.routing.model_equivalences.clone()
    };

    serde_json::json!({
        "providers": all_providers,
        "priorities": priorities,
        "model_pins": pins_json,
        "equivalences": equivalences,
    })
}

/// GET /api/v1/profiles — list all saved profiles
async fn api_profiles_list(
    State(state): State<Arc<ProxyState>>,
) -> Json<serde_json::Value> {
    match state.storage.list_profiles() {
        Ok(profiles) => {
            let active = state.storage.get_active_profile_name().ok().flatten();
            // Count providers from config_json for each profile
            let profile_list: Vec<serde_json::Value> = profiles.iter().map(|p| {
                // Get provider count from the full profile
                let provider_count = state.storage.get_profile(&p.name).ok().flatten()
                    .and_then(|row| serde_json::from_str::<serde_json::Value>(&row.config_json).ok())
                    .and_then(|v| v.get("providers").and_then(|p| p.as_object().map(|o| o.len())))
                    .unwrap_or(0);
                serde_json::json!({
                    "name": p.name,
                    "description": p.description,
                    "created_at": p.created_at,
                    "updated_at": p.updated_at,
                    "provider_count": provider_count,
                })
            }).collect();
            Json(serde_json::json!({
                "profiles": profile_list,
                "active": active,
            }))
        }
        Err(e) => Json(serde_json::json!({
            "profiles": [],
            "active": null,
            "error": format!("{e}"),
        })),
    }
}

/// GET /api/v1/profiles/{name} — get full profile details
async fn api_profile_get(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
) -> Json<serde_json::Value> {
    match state.storage.get_profile(&name) {
        Ok(Some(row)) => {
            let config: serde_json::Value = serde_json::from_str(&row.config_json)
                .unwrap_or(serde_json::json!({}));
            Json(serde_json::json!({
                "status": "ok",
                "profile": {
                    "name": row.name,
                    "description": row.description,
                    "created_at": row.created_at,
                    "updated_at": row.updated_at,
                    "is_active": row.is_active,
                    "providers": config.get("providers"),
                    "priorities": config.get("priorities"),
                    "model_pins": config.get("model_pins"),
                    "equivalences": config.get("equivalences"),
                }
            }))
        }
        Ok(None) => Json(serde_json::json!({"status": "error", "error": "Profile not found"})),
        Err(e) => Json(serde_json::json!({"status": "error", "error": format!("{e}")})),
    }
}

/// POST /api/v1/profiles — save current config as a new profile
async fn api_profile_save(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if name.is_empty() {
        return Json(serde_json::json!({"status": "error", "error": "Name is required"}));
    }
    let description = body.get("description").and_then(|v| v.as_str());

    let config = capture_current_config(&state);
    let config_json = serde_json::to_string(&config).unwrap_or_default();

    match state.storage.save_profile(&name, description, &config_json) {
        Ok(_) => {
            info!("Saved profile '{}' to database", name);
            Json(serde_json::json!({"status": "ok", "name": name}))
        }
        Err(e) => Json(serde_json::json!({"status": "error", "error": format!("{e}")})),
    }
}

/// PUT /api/v1/profiles/{name} — update profile metadata (name/description)
async fn api_profile_update(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let new_name = body.get("name").and_then(|v| v.as_str());
    let description = body.get("description").and_then(|v| v.as_str());

    match state.storage.update_profile_meta(&name, new_name, description) {
        Ok(true) => {
            let result_name = new_name.unwrap_or(&name);
            Json(serde_json::json!({"status": "ok", "name": result_name}))
        }
        Ok(false) => Json(serde_json::json!({"status": "error", "error": "Profile not found"})),
        Err(e) => Json(serde_json::json!({"status": "error", "error": format!("{e}")})),
    }
}

/// DELETE /api/v1/profiles/{name} — delete a profile
async fn api_profile_delete(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
) -> Json<serde_json::Value> {
    // If this was the active profile, clear that
    if let Ok(Some(active_name)) = state.storage.get_active_profile_name() {
        if active_name == name {
            let _ = state.storage.clear_active_profile();
        }
    }

    match state.storage.delete_profile(&name) {
        Ok(true) => Json(serde_json::json!({"status": "ok"})),
        Ok(false) => Json(serde_json::json!({"status": "error", "error": "Profile not found"})),
        Err(e) => Json(serde_json::json!({"status": "error", "error": format!("{e}")})),
    }
}

/// POST /api/v1/profiles/{name}/activate — apply a profile's config
async fn api_profile_activate(
    State(state): State<Arc<ProxyState>>,
    AxumPath(name): AxumPath<String>,
) -> Json<serde_json::Value> {
    let row = match state.storage.get_profile(&name) {
        Ok(Some(r)) => r,
        Ok(None) => return Json(serde_json::json!({"status": "error", "error": "Profile not found"})),
        Err(e) => return Json(serde_json::json!({"status": "error", "error": format!("{e}")})),
    };

    let config: serde_json::Value = match serde_json::from_str(&row.config_json) {
        Ok(v) => v,
        Err(e) => return Json(serde_json::json!({"status": "error", "error": format!("Invalid config: {e}")})),
    };

    let providers: std::collections::HashMap<String, ProviderConfig> = config.get("providers")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let priorities: std::collections::HashMap<String, serde_json::Value> = config.get("priorities")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let model_pins_val = config.get("model_pins").cloned().unwrap_or(serde_json::json!({}));
    let equivalences: std::collections::HashMap<String, Vec<String>> = config.get("equivalences")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // 1. Write providers.json
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("coalesce");
    let _ = std::fs::write(
        data_dir.join("providers.json"),
        serde_json::to_string_pretty(&providers).unwrap_or_default(),
    );

    // 2. Apply priorities
    state.provider_priorities.clear();
    state.provider_pricing_modes.clear();
    for (pname, val) in &priorities {
        if let Some(p) = val.get("priority").and_then(|v| v.as_u64()) {
            state.provider_priorities.insert(pname.clone(), p as u32);
        }
        if let Some(m) = val.get("pricing_mode").and_then(|v| v.as_str()) {
            state.provider_pricing_modes.insert(pname.clone(), m.to_string());
        }
    }
    let _ = std::fs::write(
        data_dir.join("provider_priorities.json"),
        serde_json::to_string_pretty(&priorities).unwrap_or_default(),
    );

    // 3. Apply model pins
    if let Ok(pins) = serde_json::from_value(model_pins_val.clone()) {
        *state.model_pins.write().unwrap() = pins;
    }
    let _ = std::fs::write(
        data_dir.join("model_pins.json"),
        serde_json::to_string_pretty(&model_pins_val).unwrap_or_default(),
    );

    // 4. Apply equivalences
    let _ = std::fs::write(
        data_dir.join("model_equivalences.json"),
        serde_json::to_string_pretty(&equivalences).unwrap_or_default(),
    );
    state.model_aliases.clear();
    for (canonical, aliases) in &equivalences {
        for alias in aliases {
            if alias != canonical {
                state.model_aliases.insert(alias.clone(), canonical.clone());
            }
        }
    }

    // 5. Record active profile in DB
    let _ = state.storage.set_active_profile(&name);

    info!("Activated profile '{}' with {} providers", name, providers.len());
    Json(serde_json::json!({
        "status": "ok",
        "name": name,
        "providers_applied": providers.len(),
        "message": "Profile activated. Restart proxy to fully reload providers."
    }))
}

/// POST /api/v1/profiles/import — import a profile from JSON body
async fn api_profile_import(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if name.is_empty() {
        return Json(serde_json::json!({"status": "error", "error": "Profile name is required"}));
    }
    let description = body.get("description").and_then(|v| v.as_str());

    // Build the config_json from the imported data (may be full profile or just config)
    let config = if body.get("providers").is_some() {
        // Direct config fields in body
        serde_json::json!({
            "providers": body.get("providers"),
            "priorities": body.get("priorities"),
            "model_pins": body.get("model_pins"),
            "equivalences": body.get("equivalences"),
        })
    } else {
        // Fallback: store the whole body as config
        body.clone()
    };
    let config_json = serde_json::to_string(&config).unwrap_or_default();

    match state.storage.save_profile(&name, description, &config_json) {
        Ok(_) => Json(serde_json::json!({"status": "ok", "name": name})),
        Err(e) => Json(serde_json::json!({"status": "error", "error": format!("{e}")})),
    }
}

// ==================== Enhanced Search & Export ====================

#[derive(Debug, Deserialize)]
struct SearchParams {
    limit: Option<u32>,
    offset: Option<u32>,
    provider: Option<String>,
    tier: Option<String>,
    model: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    search: Option<String>,
    failures_only: Option<bool>,
}

/// GET /api/v1/stats/search — advanced request search with filters
async fn api_stats_search(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<SearchParams>,
) -> Json<serde_json::Value> {
    let limit = params.limit.unwrap_or(50).min(500);
    let offset = params.offset.unwrap_or(0);
    let failures_only = params.failures_only.unwrap_or(false);

    let entries = state.storage.search_requests(
        limit, offset,
        params.provider.as_deref(),
        params.tier.as_deref(),
        params.model.as_deref(),
        params.from,
        params.to,
        params.search.as_deref(),
        failures_only,
    ).unwrap_or_default();

    let total = state.storage.count_requests(
        params.provider.as_deref(),
        params.tier.as_deref(),
        params.model.as_deref(),
        params.from,
        params.to,
        params.search.as_deref(),
        failures_only,
    ).unwrap_or(0);

    Json(serde_json::json!({
        "entries": entries,
        "total": total,
        "limit": limit,
        "offset": offset,
        "count": entries.len(),
    }))
}

/// GET /api/v1/stats/export/json — export all requests as JSON
async fn api_export_json(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<SearchParams>,
) -> Response {
    let limit = params.limit.unwrap_or(10000).min(100000);
    let failures_only = params.failures_only.unwrap_or(false);

    let entries = state.storage.search_requests(
        limit, 0,
        params.provider.as_deref(),
        params.tier.as_deref(),
        params.model.as_deref(),
        params.from,
        params.to,
        params.search.as_deref(),
        failures_only,
    ).unwrap_or_default();

    let json = serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string());

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .header("Content-Disposition", "attachment; filename=\"coalesce-requests.json\"")
        .body(Body::from(json))
        .unwrap()
}

/// GET /api/v1/stats/export/csv — export requests as CSV
async fn api_export_csv(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<SearchParams>,
) -> Response {
    let limit = params.limit.unwrap_or(10000).min(100000);
    let failures_only = params.failures_only.unwrap_or(false);

    let entries = state.storage.search_requests(
        limit, 0,
        params.provider.as_deref(),
        params.tier.as_deref(),
        params.model.as_deref(),
        params.from,
        params.to,
        params.search.as_deref(),
        failures_only,
    ).unwrap_or_default();

    let mut csv = String::from("id,timestamp,tier,score,provider,model,input_tokens,output_tokens,cost_usd,latency_ms,success\n");
    for e in &entries {
        csv.push_str(&format!(
            "{},{},{},{:.4},{},{},{},{},{},{},{}\n",
            e.id.unwrap_or(0),
            e.timestamp.unwrap_or(0),
            e.tier,
            e.score,
            e.provider,
            e.model,
            e.input_tokens.map_or(String::new(), |v| v.to_string()),
            e.output_tokens.map_or(String::new(), |v| v.to_string()),
            e.cost_usd.map_or(String::new(), |v| format!("{:.6}", v)),
            e.latency_ms.map_or(String::new(), |v| v.to_string()),
            if e.success { "true" } else { "false" },
        ));
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/csv")
        .header("Content-Disposition", "attachment; filename=\"coalesce-requests.csv\"")
        .body(Body::from(csv))
        .unwrap()
}

/// GET /api/v1/stats/export/costs/csv — export cost analytics as CSV
async fn api_export_costs_csv(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<CostParams>,
) -> Response {
    let days = params.days.unwrap_or(30);

    let by_provider = state.storage.costs_by_provider().unwrap_or_default();
    let daily = state.storage.costs_by_day(days).unwrap_or_default();

    let mut csv = String::from("# Cost by Provider\nprovider,requests,input_tokens,output_tokens,total_cost_usd,avg_latency_ms\n");
    for p in &by_provider {
        csv.push_str(&format!(
            "{},{},{},{},{:.6},{:.0}\n",
            p.group, p.requests, p.input_tokens, p.output_tokens, p.total_cost_usd, p.avg_latency_ms
        ));
    }

    csv.push_str("\n# Daily Costs\ndate,requests,input_tokens,output_tokens,total_cost_usd,free_requests\n");
    for d in &daily {
        csv.push_str(&format!(
            "{},{},{},{},{:.6},{}\n",
            d.date, d.requests, d.input_tokens, d.output_tokens, d.total_cost_usd, d.free_requests
        ));
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/csv")
        .header("Content-Disposition", "attachment; filename=\"coalesce-costs.csv\"")
        .body(Body::from(csv))
        .unwrap()
}

// --- Model Overrides API ---

/// Apply stored overrides to a ModelInfo in place
fn apply_overrides_to_model(model: &mut ModelInfo, overrides: &[(String, String)]) {
    for (field, value) in overrides {
        match field.as_str() {
            "quality_tier" => {
                if let Some(tier) = match value.to_lowercase().as_str() {
                    "simple" => Some(QualityTier::Simple),
                    "medium" => Some(QualityTier::Medium),
                    "complex" => Some(QualityTier::Complex),
                    "reasoning" => Some(QualityTier::Reasoning),
                    _ => None,
                } {
                    model.quality_tier = tier;
                }
            }
            "reasoning" => model.reasoning = value == "true",
            "vision" => model.vision = value == "true",
            "tool_calling" => model.tool_calling = value == "true",
            "canonical_family" => model.canonical_family = Some(value.clone()),
            "input_price_per_m" => {
                if let Ok(v) = value.parse::<f64>() { model.input_price_per_m = v; }
            }
            "output_price_per_m" => {
                if let Ok(v) = value.parse::<f64>() { model.output_price_per_m = v; }
            }
            "context_window" => {
                if let Ok(v) = value.parse::<u32>() { model.context_window = v; }
            }
            _ => {}
        }
    }
}

/// Load all overrides from DB and apply to the current model list
fn apply_all_overrides(state: &ProxyState) {
    if let Ok(all_overrides) = state.storage.get_all_model_overrides() {
        let mut models = state.models.write().unwrap();
        for model in models.iter_mut() {
            let relevant: Vec<(String, String)> = all_overrides.iter()
                .filter(|o| o.provider == model.provider && o.model_id == model.id)
                .map(|o| (o.field.clone(), o.value.clone()))
                .collect();
            if !relevant.is_empty() {
                apply_overrides_to_model(model, &relevant);
            }
        }
    }
}

/// GET /api/v1/overrides — list all overrides
async fn api_overrides_list(
    State(state): State<Arc<ProxyState>>,
) -> Json<serde_json::Value> {
    match state.storage.get_all_model_overrides() {
        Ok(overrides) => Json(serde_json::json!({ "overrides": overrides })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// GET /api/v1/overrides/:provider/:model — get overrides for a model
async fn api_overrides_get(
    State(state): State<Arc<ProxyState>>,
    AxumPath((provider, model)): AxumPath<(String, String)>,
) -> Json<serde_json::Value> {
    match state.storage.get_model_overrides(&provider, &model) {
        Ok(overrides) => {
            let map: serde_json::Map<String, serde_json::Value> = overrides.into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect();
            Json(serde_json::json!({ "provider": provider, "model_id": model, "overrides": map }))
        }
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

#[derive(Deserialize)]
struct OverrideSetRequest {
    overrides: std::collections::HashMap<String, String>,
}

/// PUT /api/v1/overrides/:provider/:model — set overrides (merge)
async fn api_overrides_set(
    State(state): State<Arc<ProxyState>>,
    AxumPath((provider, model)): AxumPath<(String, String)>,
    Json(body): Json<OverrideSetRequest>,
) -> Json<serde_json::Value> {
    let valid_fields = ["quality_tier", "reasoning", "vision", "tool_calling",
                        "canonical_family", "input_price_per_m", "output_price_per_m", "context_window"];

    for (field, value) in &body.overrides {
        if !valid_fields.contains(&field.as_str()) {
            return Json(serde_json::json!({ "error": format!("Invalid field: {}", field) }));
        }
        if let Err(e) = state.storage.set_model_override(&provider, &model, field, value) {
            return Json(serde_json::json!({ "error": e.to_string() }));
        }
    }

    // Re-apply all overrides to refresh in-memory state
    apply_all_overrides(&state);

    Json(serde_json::json!({ "ok": true, "fields_set": body.overrides.len() }))
}

/// DELETE /api/v1/overrides/:provider/:model — clear all overrides for a model
async fn api_overrides_clear(
    State(state): State<Arc<ProxyState>>,
    AxumPath((provider, model)): AxumPath<(String, String)>,
) -> Json<serde_json::Value> {
    match state.storage.clear_model_overrides(&provider, &model) {
        Ok(count) => {
            // Re-apply remaining overrides
            apply_all_overrides(&state);
            Json(serde_json::json!({ "ok": true, "removed": count }))
        }
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}


// ── Response Cache API ──────────────────────────────────────────────────────

/// GET /api/v1/cache/stats — return response cache statistics
async fn api_cache_stats(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    Json(state.response_cache.stats_json())
}

/// POST /api/v1/cache/clear — flush the response cache
async fn api_cache_clear(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    state.response_cache.clear();
    Json(serde_json::json!({"cleared": true}))
}

// ── Mock Provider API ───────────────────────────────────────────────────────

/// GET /api/v1/mock/status — check if mock provider is enabled
async fn api_mock_status(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "enabled": state.mock_enabled.load(std::sync::atomic::Ordering::Relaxed),
    }))
}

/// POST /api/v1/mock/toggle — enable/disable mock provider
async fn api_mock_toggle(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let was = state.mock_enabled.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
    let now = !was;

    if now {
        // Add mock provider
        let mock = MockProvider::default_provider();
        if let Ok(models) = mock.list_models().await {
            state.models.write().unwrap().extend(models);
            state.providers.write().unwrap().push(Arc::new(mock));
        }
    } else {
        // Remove mock provider
        state.providers.write().unwrap().retain(|p| p.name() != "mock");
        state.models.write().unwrap().retain(|m| m.provider != "mock");
    }

    Json(serde_json::json!({"enabled": now}))
}

/// GET /api/v1/thinking/status — current thinking optimizer configuration
async fn api_thinking_status(
    State(state): State<Arc<ProxyState>>,
) -> Json<serde_json::Value> {
    let config = &state.thinking_optimizer.config;
    Json(serde_json::json!({
        "enabled": config.enabled,
        "min_complexity": config.min_complexity_for_thinking,
        "max_budget_tokens": config.max_budget_tokens,
        "default_budget_tokens": config.default_budget_tokens,
    }))
}

/// PUT /api/v1/thinking/config — update thinking optimizer configuration
async fn api_thinking_config(
    State(state): State<Arc<ProxyState>>,
    Json(_body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    // ThinkingOptimizer config is not behind a lock — return current config (read-only for now)
    let config = &state.thinking_optimizer.config;
    Json(serde_json::json!({
        "enabled": config.enabled,
        "min_complexity": config.min_complexity_for_thinking,
        "max_budget_tokens": config.max_budget_tokens,
        "default_budget_tokens": config.default_budget_tokens,
    }))
}

// ---------------------------------------------------------------------------
// MCP server management endpoints
// ---------------------------------------------------------------------------

/// GET /api/v1/mcp/servers — list all registered MCP servers
async fn api_mcp_servers(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let servers: Vec<serde_json::Value> = state.mcp_registry.list().iter().map(|s| {
        serde_json::json!({
            "id": s.config.id,
            "name": s.config.name,
            "transport": s.config.transport,
            "status": s.status,
            "tools": s.tools.iter().map(|t| serde_json::json!({
                "name": t.name,
                "description": t.description,
            })).collect::<Vec<_>>(),
            "enabled": s.config.enabled,
            "source": s.config.source,
            "command": s.config.command,
            "url": s.config.url,
            "last_connected": s.last_connected,
            "error": s.error,
        })
    }).collect();
    Json(serde_json::json!({ "servers": servers }))
}

/// POST /api/v1/mcp/scan — scan for MCP server configs in known locations
async fn api_mcp_scan(State(state): State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let configs = McpScanner::scan_all();
    let mut registered = 0;
    for config in configs {
        if state.mcp_registry.get(&config.id).is_none() {
            state.mcp_registry.register(config);
            registered += 1;
        }
    }
    let servers: Vec<serde_json::Value> = state.mcp_registry.list().iter().map(|s| {
        serde_json::json!({
            "id": s.config.id,
            "name": s.config.name,
            "transport": s.config.transport,
            "status": s.status,
            "tools": s.tools.iter().map(|t| serde_json::json!({
                "name": t.name,
                "description": t.description,
            })).collect::<Vec<_>>(),
            "enabled": s.config.enabled,
            "source": s.config.source,
            "command": s.config.command,
            "url": s.config.url,
            "last_connected": s.last_connected,
            "error": s.error,
        })
    }).collect();
    info!("MCP scan: found {} new servers", registered);
    Json(serde_json::json!({ "servers": servers, "new": registered }))
}

/// POST /api/v1/mcp/servers/{id}/toggle — toggle a server's enabled state
async fn api_mcp_toggle(
    State(state): State<Arc<ProxyState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    match state.mcp_registry.toggle(&id) {
        Some(enabled) => Json(serde_json::json!({ "id": id, "enabled": enabled })),
        None => Json(serde_json::json!({ "error": format!("Server '{}' not found", id) })),
    }
}

/// DELETE /api/v1/mcp/servers/{id} — remove a server from the registry
async fn api_mcp_remove(
    State(state): State<Arc<ProxyState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    match state.mcp_registry.unregister(&id) {
        Some(_) => Json(serde_json::json!({ "ok": true, "removed": id })),
        None => Json(serde_json::json!({ "error": format!("Server '{}' not found", id) })),
    }
}

/// POST /api/v1/mcp/servers — register a new MCP server manually
async fn api_mcp_register(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let name = body["name"].as_str().unwrap_or("unnamed").to_string();
    let id = body["id"].as_str().map(String::from)
        .unwrap_or_else(|| format!("manual-{}", name.to_lowercase().replace(' ', "-")));

    let transport = match body["transport"].as_str().unwrap_or("stdio") {
        "sse" => McpTransport::Sse,
        "streamablehttp" | "streamable_http" => McpTransport::StreamableHttp,
        "websocket" | "ws" => McpTransport::WebSocket,
        "grpc" => McpTransport::Grpc,
        "inprocess" | "in_process" => McpTransport::InProcess,
        _ => McpTransport::Stdio,
    };

    let config = McpServerConfig {
        id: id.clone(),
        name,
        transport,
        command: body["command"].as_str().map(String::from),
        args: body["args"].as_array().map(|a| {
            a.iter().filter_map(|v| v.as_str().map(String::from)).collect()
        }),
        url: body["url"].as_str().map(String::from),
        env: body["env"].as_object().map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        }),
        enabled: true,
        source: McpConfigSource::Manual,
    };

    state.mcp_registry.register(config);
    Json(serde_json::json!({ "ok": true, "id": id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_to_chat_simple_text() {
        let req = AnthropicMessagesRequest {
            model: "claude-3-opus".into(),
            max_tokens: 1024,
            system: Some(serde_json::json!("You are helpful.")),
            messages: vec![serde_json::json!({"role": "user", "content": "Hello"})],
            stream: false,
            temperature: Some(0.7),
            top_p: None,
            tools: None,
            tool_choice: None,
            extra: Default::default(),
        };
        let chat = anthropic_to_chat_request(req);
        assert_eq!(chat.messages.len(), 2); // system + user
        assert_eq!(chat.messages[0].role, "system");
        assert_eq!(chat.messages[1].role, "user");
        match &chat.messages[1].content {
            Some(MessageContent::Text(t)) => assert_eq!(t, "Hello"),
            _ => panic!("Expected text content"),
        }
        assert_eq!(chat.temperature, Some(0.7));
    }

    #[test]
    fn test_anthropic_to_chat_system_array() {
        let req = AnthropicMessagesRequest {
            model: "claude-3".into(),
            max_tokens: 100,
            system: Some(serde_json::json!([
                {"type": "text", "text": "Part 1"},
                {"type": "text", "text": "Part 2"}
            ])),
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            stream: false,
            temperature: None, top_p: None, tools: None, tool_choice: None,
            extra: Default::default(),
        };
        let chat = anthropic_to_chat_request(req);
        match &chat.messages[0].content {
            Some(MessageContent::Text(t)) => assert_eq!(t, "Part 1\nPart 2"),
            _ => panic!("Expected text"),
        }
    }

    #[test]
    fn test_anthropic_to_chat_content_blocks() {
        let req = AnthropicMessagesRequest {
            model: "m".into(),
            max_tokens: 100,
            system: None,
            messages: vec![serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "What is this?"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc123"}}
                ]
            })],
            stream: false,
            temperature: None, top_p: None, tools: None, tool_choice: None,
            extra: Default::default(),
        };
        let chat = anthropic_to_chat_request(req);
        assert_eq!(chat.messages.len(), 1);
        match &chat.messages[0].content {
            Some(MessageContent::Parts(parts)) => {
                assert_eq!(parts.len(), 2); // text + image
            }
            _ => panic!("Expected parts content"),
        }
    }

    #[test]
    fn test_anthropic_to_chat_tool_use() {
        let req = AnthropicMessagesRequest {
            model: "m".into(),
            max_tokens: 100,
            system: None,
            messages: vec![
                serde_json::json!({"role": "user", "content": "Search for cats"}),
                serde_json::json!({
                    "role": "assistant",
                    "content": [
                        {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "cats"}}
                    ]
                }),
                serde_json::json!({
                    "role": "user",
                    "content": [
                        {"type": "tool_result", "tool_use_id": "call_1", "content": "Found 42 cats"}
                    ]
                }),
            ],
            stream: false,
            temperature: None, top_p: None, tools: None, tool_choice: None,
            extra: Default::default(),
        };
        let chat = anthropic_to_chat_request(req);
        assert_eq!(chat.messages.len(), 3);
        // Assistant message should have tool_calls
        assert!(chat.messages[1].tool_calls.is_some());
        // Tool result should be role "tool" with tool_call_id
        assert_eq!(chat.messages[2].role, "tool");
        assert_eq!(chat.messages[2].tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn test_chat_response_to_anthropic_text() {
        let openai_resp = serde_json::json!({
            "id": "chatcmpl-123",
            "choices": [{
                "message": {"role": "assistant", "content": "Hello world"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let anthropic = chat_response_to_anthropic(&openai_resp, "claude-3-opus");
        assert_eq!(anthropic["type"], "message");
        assert_eq!(anthropic["role"], "assistant");
        assert_eq!(anthropic["model"], "claude-3-opus");
        assert_eq!(anthropic["stop_reason"], "end_turn");
        assert_eq!(anthropic["content"][0]["type"], "text");
        assert_eq!(anthropic["content"][0]["text"], "Hello world");
        assert_eq!(anthropic["usage"]["input_tokens"], 10);
        assert_eq!(anthropic["usage"]["output_tokens"], 5);
    }

    #[test]
    fn test_chat_response_to_anthropic_tool_calls() {
        let openai_resp = serde_json::json!({
            "id": "chatcmpl-456",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\":\"NYC\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 10}
        });
        let anthropic = chat_response_to_anthropic(&openai_resp, "claude-3");
        assert_eq!(anthropic["stop_reason"], "tool_use");
        let content = anthropic["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "get_weather");
        assert_eq!(content[0]["id"], "call_abc");
        assert_eq!(content[0]["input"]["city"], "NYC");
    }

    #[test]
    fn test_chat_stream_chunk_done() {
        let mut block_idx = 0;
        let mut started = false;
        let output = chat_stream_chunk_to_anthropic("[DONE]", &mut block_idx, &mut started, None);
        assert!(output.contains("message_stop"));
    }

    #[test]
    fn test_chat_stream_chunk_text_delta() {
        let mut block_idx = 0;
        let mut started = false;
        let chunk = serde_json::json!({
            "id": "chatcmpl-1",
            "model": "gpt-4",
            "choices": [{"delta": {"content": "Hello"}, "index": 0}]
        });
        let output = chat_stream_chunk_to_anthropic(
            &serde_json::to_string(&chunk).unwrap(),
            &mut block_idx,
            &mut started,
            None,
        );
        // Should contain message_start (first chunk) and content_block_delta
        assert!(started);
        assert!(output.contains("message_start") || output.contains("content_block_delta"));
    }
}
