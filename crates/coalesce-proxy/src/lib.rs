pub mod grpc;

use coalesce_core::cache::dedup::{DedupAction, DedupResult, RequestDedup};
use coalesce_core::cache::semantic::SemanticCache;
use coalesce_core::economics::budget::BudgetTracker;
use coalesce_core::config::{AppConfig, ProviderConfig};
use coalesce_core::economics::billing::BillingType;
use coalesce_core::economics::marginal_cost::MarginalCost;
use coalesce_core::economics::optimizer::EconomicsEngine;
use coalesce_core::providers::anthropic::AnthropicProvider;
use coalesce_core::providers::copilot::CopilotProvider;
use coalesce_core::providers::health::CircuitBreaker;
use coalesce_core::providers::ollama::OllamaProvider;
use coalesce_core::providers::openai_compat::factories;
use coalesce_core::providers::openrouter::OpenRouterProvider;
use coalesce_core::providers::Provider;
use coalesce_core::storage::{RequestLogEntry, Storage};
use coalesce_core::types::{ChatRequest, Message, MessageContent, ModelInfo, QualityTier};
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
use tracing::{error, info, warn};

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
    // 1. Read from Antigravity's DB (always fresh if app is installed)
    // 2. Fall back to refreshing our own stored refresh token
    let google_token = read_antigravity_token()
        .or_else(|| {
            // Fall back to refresh token
            if let Ok(Some(rt)) = storage.get("google_refresh_token") {
                let rt_clone = rt.clone();
                // Use a blocking approach since we're in async context
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        refresh_google_token(&rt_clone).await.ok()
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
    let (providers, mut models, economics, circuit_breakers) = init_providers(&config).await;

    // Inject Google models discovered via Cloud Code API (since OpenAI-compat /models fails with OAuth)
    if google_token.is_some() {
        let google_models = discover_google_models(google_token.as_deref().unwrap()).await;
        // Remove any empty google models from init_providers, add the real ones
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
    });

    // Initialize provider priorities and pricing modes from config
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
                            "google" | "gemini" => api_key.map(|k| Arc::new(factories::google(k)) as Arc<dyn Provider>),
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
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(dashboard))
        .route("/dashboard", get(dashboard))
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
        .route("/api/v1/providers/{name}/billing", put(api_update_billing))
        .route("/api/v1/providers/{name}/test", post(api_test_provider))
        .route("/api/v1/auth/copilot/start", post(api_copilot_auth_start))
        .route("/api/v1/auth/copilot/poll", post(api_copilot_auth_poll))
        .route("/api/v1/providers/ollama/models", get(api_ollama_models))
        .route("/api/v1/providers/ollama/models/{model}/toggle", post(api_ollama_toggle_model))
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
        .route("/api/v1/feedback", post(api_feedback))
        .route("/api/v1/quality/scores", get(api_quality_scores))
        .route("/metrics", get(api_metrics))
        .layer(cors)
        .with_state(state);

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
                api_key.map(|k| Arc::new(factories::google(k)) as Arc<dyn Provider>)
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

    // 1. Score and route
    let scoring = coalesce_core::router::route(&request, &state.config.routing);
    info!(
        tier = %scoring.tier,
        score = scoring.score,
        reasoning = %scoring.reasoning,
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
    let costs: Vec<MarginalCost> = models_snapshot
        .iter()
        .map(|m| state.economics.marginal_cost(m, 1000, 500))
        .collect();

    // 3. Get routing strategy
    let _strategy = state
        .config
        .routing
        .profiles
        .get("auto")
        .map(|p| p.strategy.as_str())
        .unwrap_or("cheapest_capable");

    // 4. Build candidate list sorted by cost, filtering out open circuit breakers
    let mut candidates: Vec<(usize, &ModelInfo)> = models_snapshot
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
        .collect();

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
                let prio_a = state.provider_priorities.get(&model_a.provider).map(|v| *v).unwrap_or(50);
                let prio_b = state.provider_priorities.get(&model_b.provider).map(|v| *v).unwrap_or(50);
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

    // 5. Fallback chain — try candidates in order
    let mut last_error = String::new();
    let attempt_limit = candidates.len().min(MAX_FALLBACK_ATTEMPTS);

    for attempt in 0..attempt_limit {
        let (_, selected_model) = candidates[attempt];

        let provider = match providers_snapshot
            .iter()
            .find(|p| p.name() == selected_model.provider)
        {
            Some(p) => p,
            None => continue,
        };

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

        // 6. Forward — streaming or non-streaming
        if request.stream {
            match provider.chat_stream(&forwarded_request).await {
                Ok(byte_stream) => {
                    // Record success
                    if let Some(cb) = state.circuit_breakers.get(&selected_model.provider) {
                        cb.record_success();
                    }

                    // Log request (no token counts for streaming)
                    let _ = state.storage.log_request(&RequestLogEntry {
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

                    let body_stream = byte_stream.map(|result| {
                        result
                            .map(|bytes| bytes)
                            .map_err(|e| {
                                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                            })
                    });

                    return Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/event-stream")
                        .header("Cache-Control", "no-cache")
                        .header("Connection", "keep-alive")
                        .header("X-Coalesce-Model", &selected_model.id)
                        .header("X-Coalesce-Provider", &selected_model.provider)
                        .header("X-Coalesce-Tier", scoring.tier.to_string())
                        .header("X-Coalesce-Attempt", (attempt + 1).to_string())
                        .body(Body::from_stream(body_stream))
                        .unwrap_or_else(|_| {
                            (StatusCode::INTERNAL_SERVER_ERROR, "Stream setup failed")
                                .into_response()
                        });
                }
                Err(e) => {
                    warn!(
                        provider = %provider.name(),
                        error = %e,
                        attempt = attempt + 1,
                        "Stream failed, trying fallback"
                    );
                    if let Some(cb) = state.circuit_breakers.get(&selected_model.provider) {
                        cb.record_failure();
                    }
                    last_error = e.to_string();
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

                            last_error = e.to_string();
                            continue;
                        }
                    }
                }
            }
        }
    }

    // All attempts exhausted
    error!(
        attempts = attempt_limit,
        last_error = %last_error,
        "All fallback attempts exhausted"
    );

    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": {
                "message": format!(
                    "All providers failed after {} attempts. Last error: {}",
                    attempt_limit, last_error
                ),
                "type": "all_providers_exhausted",
                "code": "provider_error",
            }
        })),
    )
        .into_response()
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
                "billing": format!("{:?}", q.billing),
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

            let billing = state
                .config
                .providers
                .get(&name)
                .map(|pc| pc.billing.clone().unwrap_or_else(|| "per_token".into()))
                .unwrap_or_else(|| "unknown".into());

            let priority = state.provider_priorities.get(&name).map(|v| *v).unwrap_or(50);
            let pricing_mode = state.provider_pricing_modes.get(&name)
                .map(|v| v.value().clone())
                .unwrap_or_else(|| "metered".to_string());

            serde_json::json!({
                "name": name,
                "model_count": model_count,
                "billing": billing,
                "circuit_breaker": cb_info,
                "is_available": state.circuit_breakers.get(&name).map(|cb| cb.is_available()).unwrap_or(true),
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
                "billing": format!("{:?}", q.billing),
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
        "google" | "gemini" => api_key.map(|k| Arc::new(factories::google(k)) as Arc<dyn Provider>),
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

    // Set default priority and pricing mode for new provider
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
        "google" | "gemini" => api_key.map(|k| Arc::new(factories::google(k)) as Arc<dyn Provider>),
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
    info!("Updated billing for '{}' to '{}'", name, billing_str);
    Json(serde_json::json!({"status": "ok", "provider": name, "billing": billing_str}))
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
        _ => "free",
    };
    info!("  google — detected tier: {}", tier);

    let p = "google";
    let mut models = vec![
        ModelInfo { id: "gemini-3-flash".into(), name: "Gemini 3 Flash".into(), provider: p.into(),
            input_price_per_m: 0.10, output_price_per_m: 0.40, context_window: 1048576,
            max_output: Some(65536), quality_tier: QualityTier::Medium, reasoning: false, vision: true, tool_calling: true },
        ModelInfo { id: "gemini-3.1-pro-low".into(), name: "Gemini 3.1 Pro (Low)".into(), provider: p.into(),
            input_price_per_m: 1.25, output_price_per_m: 5.0, context_window: 1048576,
            max_output: Some(65536), quality_tier: QualityTier::Complex, reasoning: false, vision: true, tool_calling: true },
    ];
    if tier == "pro" || tier == "ultra" {
        models.extend(vec![
            ModelInfo { id: "gemini-3.1-pro-high".into(), name: "Gemini 3.1 Pro (High)".into(), provider: p.into(),
                input_price_per_m: 1.25, output_price_per_m: 10.0, context_window: 1048576,
                max_output: Some(65536), quality_tier: QualityTier::Reasoning, reasoning: true, vision: true, tool_calling: true },
            ModelInfo { id: "claude-sonnet-4-6-thinking".into(), name: "Claude Sonnet 4.6 (Thinking)".into(), provider: p.into(),
                input_price_per_m: 3.0, output_price_per_m: 15.0, context_window: 200000,
                max_output: Some(16384), quality_tier: QualityTier::Reasoning, reasoning: true, vision: true, tool_calling: true },
            ModelInfo { id: "claude-opus-4-6-thinking".into(), name: "Claude Opus 4.6 (Thinking)".into(), provider: p.into(),
                input_price_per_m: 15.0, output_price_per_m: 75.0, context_window: 200000,
                max_output: Some(16384), quality_tier: QualityTier::Reasoning, reasoning: true, vision: true, tool_calling: true },
            ModelInfo { id: "gpt-oss-120b-medium".into(), name: "GPT-OSS 120B (Medium)".into(), provider: p.into(),
                input_price_per_m: 1.0, output_price_per_m: 4.0, context_window: 128000,
                max_output: Some(16384), quality_tier: QualityTier::Medium, reasoning: false, vision: true, tool_calling: true },
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
    let google = factories::google(access_token.to_string());
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
