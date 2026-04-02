//! gRPC transport layer for Coalesce proxy.
//!
//! Runs on port 8403 (HTTP port + 1) alongside the HTTP/REST server.
//! Provides the same routing, model listing, and health functionality
//! over gRPC with protobuf-encoded messages for high-frequency agent communication.

use crate::ProxyState;
use coalesce_core::economics::marginal_cost::MarginalCost;
use coalesce_core::types::{ChatRequest, Message, MessageContent, StopSequence};
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

/// Generated protobuf types and service trait.
pub mod pb {
    tonic::include_proto!("coalesce");
}

use pb::coalesce_service_server::{CoalesceService, CoalesceServiceServer};

/// gRPC service handler backed by the shared proxy state.
pub struct GrpcHandler {
    state: Arc<ProxyState>,
}

impl GrpcHandler {
    pub fn new(state: Arc<ProxyState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl CoalesceService for GrpcHandler {
    async fn chat_completion(
        &self,
        request: Request<pb::ChatCompletionRequest>,
    ) -> Result<Response<pb::ChatCompletionResponse>, Status> {
        let req = request.into_inner();
        let start = std::time::Instant::now();

        // Convert proto messages to core types
        let messages: Vec<Message> = req
            .messages
            .iter()
            .map(|m| Message {
                role: m.role.clone(),
                content: Some(MessageContent::Text(m.content.clone())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            })
            .collect();

        let chat_request = ChatRequest {
            model: if req.model.is_empty() {
                "auto".into()
            } else {
                req.model
            },
            messages,
            stream: false,
            max_tokens: req.max_tokens,
            temperature: req.temperature.map(|t| f64::from(t)),
            top_p: req.top_p.map(|t| f64::from(t)),
            stop: if req.stop.is_empty() {
                None
            } else {
                Some(StopSequence::Multiple(req.stop))
            },
            tools: None,
            tool_choice: None,
            response_format: None,
            extra: Default::default(),
        };

        // Score and route
        let scoring = coalesce_core::router::route(&chat_request, &self.state.config.routing);
        info!(
            tier = %scoring.tier,
            score = scoring.score,
            "gRPC: routing request"
        );

        // Snapshot models and providers (clone to avoid holding RwLockReadGuard across await)
        let models_snapshot: Vec<coalesce_core::types::ModelInfo> = self.state.models.read().unwrap().clone();
        let providers_snapshot: Vec<std::sync::Arc<dyn coalesce_core::providers::Provider>> = self.state.providers.read().unwrap().clone();

        let costs: Vec<MarginalCost> = models_snapshot
            .iter()
            .map(|m| self.state.economics.marginal_cost(m, 1000, 500))
            .collect();

        let mut candidates: Vec<(usize, &coalesce_core::types::ModelInfo)> = models_snapshot
            .iter()
            .enumerate()
            .filter(|(_, m)| m.quality_tier.can_handle(&scoring.tier))
            .filter(|(_, m)| {
                self.state
                    .circuit_breakers
                    .get(&m.provider)
                    .map(|cb| cb.is_available())
                    .unwrap_or(true)
            })
            .collect();

        // Sort by marginal cost (cheapest first)
        candidates.sort_by(|(idx_a, _), (idx_b, _)| {
            let cost_a = costs.get(*idx_a).map(|c| c.usd_value()).unwrap_or(f64::MAX);
            let cost_b = costs.get(*idx_b).map(|c| c.usd_value()).unwrap_or(f64::MAX);
            cost_a
                .partial_cmp(&cost_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if candidates.is_empty() {
            return Err(Status::unavailable(format!(
                "No model available for tier {}",
                scoring.tier
            )));
        }

        // Fallback chain: try up to 3 candidates
        let attempt_limit = candidates.len().min(3);
        let mut last_error = String::new();

        for attempt in 0..attempt_limit {
            let (_, selected) = candidates[attempt];

            let provider = match providers_snapshot
                .iter()
                .find(|p| p.name() == selected.provider)
            {
                Some(p) => p,
                None => continue,
            };

            if attempt > 0 {
                warn!(
                    attempt = attempt + 1,
                    model = %selected.id,
                    provider = %selected.provider,
                    "gRPC: fallback attempt"
                );
            }

            let mut forwarded = chat_request.clone();
            forwarded.model = selected.id.clone();

            match provider.chat(&forwarded).await {
                Ok(response_json) => {
                    // Record success in circuit breaker
                    if let Some(cb) = self.state.circuit_breakers.get(&selected.provider) {
                        cb.record_success();
                    }

                    // Record usage
                    let (input_tokens, output_tokens, cost_usd) =
                        extract_usage(&response_json, selected);
                    self.state.economics.record_usage(
                        &selected.provider,
                        &selected.id,
                        cost_usd.unwrap_or(0.0),
                    );

                    // Log request
                    let _ = self.state.storage.log_request(
                        &coalesce_core::storage::RequestLogEntry {
                            id: None, timestamp: None,
                            tier: scoring.tier.to_string(),
                            score: scoring.score,
                            provider: selected.provider.clone(),
                            model: selected.id.clone(),
                            input_tokens,
                            output_tokens,
                            cost_usd,
                            latency_ms: Some(start.elapsed().as_millis() as u64),
                            success: true,
                        },
                    );

                    // Convert JSON response to proto
                    let choices = response_json
                        .get("choices")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .enumerate()
                                .map(|(i, c)| pb::Choice {
                                    index: i as u32,
                                    message: Some(pb::ChatMessage {
                                        role: c
                                            .get("message")
                                            .and_then(|m| m.get("role"))
                                            .and_then(|r| r.as_str())
                                            .unwrap_or("assistant")
                                            .into(),
                                        content: c
                                            .get("message")
                                            .and_then(|m| m.get("content"))
                                            .and_then(|c| c.as_str())
                                            .unwrap_or("")
                                            .into(),
                                    }),
                                    finish_reason: c
                                        .get("finish_reason")
                                        .and_then(|f| f.as_str())
                                        .unwrap_or("stop")
                                        .into(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    let usage = response_json.get("usage").map(|u| pb::Usage {
                        prompt_tokens: u
                            .get("prompt_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32,
                        completion_tokens: u
                            .get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32,
                        total_tokens: u
                            .get("total_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32,
                    });

                    return Ok(Response::new(pb::ChatCompletionResponse {
                        id: response_json
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .into(),
                        model: selected.id.clone(),
                        choices,
                        usage,
                        routing: Some(pb::RoutingInfo {
                            tier: scoring.tier.to_string(),
                            score: scoring.score,
                            provider: selected.provider.clone(),
                            model: selected.id.clone(),
                            attempt: (attempt + 1) as u32,
                        }),
                    }));
                }
                Err(e) => {
                    warn!(
                        provider = %provider.name(),
                        error = %e,
                        attempt = attempt + 1,
                        "gRPC: chat failed, trying fallback"
                    );
                    if let Some(cb) = self.state.circuit_breakers.get(&selected.provider) {
                        cb.record_failure();
                    }

                    // Log failure
                    let _ = self.state.storage.log_request(
                        &coalesce_core::storage::RequestLogEntry {
                            id: None, timestamp: None,
                            tier: scoring.tier.to_string(),
                            score: scoring.score,
                            provider: selected.provider.clone(),
                            model: selected.id.clone(),
                            input_tokens: None,
                            output_tokens: None,
                            cost_usd: None,
                            latency_ms: Some(start.elapsed().as_millis() as u64),
                            success: false,
                        },
                    );

                    last_error = e.to_string();
                    continue;
                }
            }
        }

        error!(
            attempts = attempt_limit,
            last_error = %last_error,
            "gRPC: all fallback attempts exhausted"
        );
        Err(Status::internal(format!(
            "All providers failed after {} attempts. Last error: {}",
            attempt_limit, last_error
        )))
    }

    type ChatCompletionStreamStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<pb::ChatCompletionChunk, Status>> + Send>,
    >;

    async fn chat_completion_stream(
        &self,
        request: Request<pb::ChatCompletionRequest>,
    ) -> Result<Response<Self::ChatCompletionStreamStream>, Status> {
        let req = request.into_inner();

        // Convert proto messages to core types
        let messages: Vec<Message> = req
            .messages
            .iter()
            .map(|m| Message {
                role: m.role.clone(),
                content: Some(MessageContent::Text(m.content.clone())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            })
            .collect();

        let chat_request = ChatRequest {
            model: if req.model.is_empty() {
                "auto".into()
            } else {
                req.model
            },
            messages,
            stream: true,
            max_tokens: req.max_tokens,
            temperature: req.temperature.map(|t| f64::from(t)),
            top_p: req.top_p.map(|t| f64::from(t)),
            stop: if req.stop.is_empty() {
                None
            } else {
                Some(StopSequence::Multiple(req.stop))
            },
            tools: None,
            tool_choice: None,
            response_format: None,
            extra: Default::default(),
        };

        // Score and route
        let scoring = coalesce_core::router::route(&chat_request, &self.state.config.routing);

        // Find best available model (snapshot to avoid holding lock across await)
        let models_snapshot: Vec<coalesce_core::types::ModelInfo> = self.state.models.read().unwrap().clone();
        let providers_snapshot: Vec<std::sync::Arc<dyn coalesce_core::providers::Provider>> = self.state.providers.read().unwrap().clone();
        let costs: Vec<MarginalCost> = models_snapshot
            .iter()
            .map(|m| self.state.economics.marginal_cost(m, 1000, 500))
            .collect();

        let mut candidates: Vec<(usize, &coalesce_core::types::ModelInfo)> = models_snapshot
            .iter()
            .enumerate()
            .filter(|(_, m)| m.quality_tier.can_handle(&scoring.tier))
            .filter(|(_, m)| {
                self.state
                    .circuit_breakers
                    .get(&m.provider)
                    .map(|cb| cb.is_available())
                    .unwrap_or(true)
            })
            .collect();

        candidates.sort_by(|(idx_a, _), (idx_b, _)| {
            let cost_a = costs.get(*idx_a).map(|c| c.usd_value()).unwrap_or(f64::MAX);
            let cost_b = costs.get(*idx_b).map(|c| c.usd_value()).unwrap_or(f64::MAX);
            cost_a
                .partial_cmp(&cost_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if candidates.is_empty() {
            return Err(Status::unavailable(format!(
                "No model available for tier {}",
                scoring.tier
            )));
        }

        let (_, selected) = candidates[0];

        let provider = providers_snapshot
            .iter()
            .find(|p| p.name() == selected.provider)
            .ok_or_else(|| Status::internal("Provider not found"))?;

        let mut forwarded = chat_request.clone();
        forwarded.model = selected.id.clone();

        let byte_stream = provider
            .chat_stream(&forwarded)
            .await
            .map_err(|e| Status::internal(format!("Stream setup failed: {}", e)))?;

        // Record success in circuit breaker
        if let Some(cb) = self.state.circuit_breakers.get(&selected.provider) {
            cb.record_success();
        }

        let model_id = selected.id.clone();

        // Convert SSE byte stream to gRPC chunk stream
        let chunk_stream = async_stream::stream! {
            use futures::StreamExt;

            let mut pinned = std::pin::pin!(byte_stream);
            while let Some(result) = pinned.next().await {
                match result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        // Parse SSE lines: "data: {...}\n\n"
                        for line in text.lines() {
                            let line = line.trim();
                            if line == "data: [DONE]" {
                                // Send final chunk with finish_reason
                                yield Ok(pb::ChatCompletionChunk {
                                    id: String::new(),
                                    model: model_id.clone(),
                                    choices: vec![pb::ChunkChoice {
                                        index: 0,
                                        delta: Some(pb::ChunkDelta {
                                            role: None,
                                            content: None,
                                        }),
                                        finish_reason: Some("stop".into()),
                                    }],
                                });
                                break;
                            }
                            if let Some(json_str) = line.strip_prefix("data: ") {
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                                    if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                                        let chunk_choices: Vec<pb::ChunkChoice> = choices.iter().map(|c| {
                                            let delta = c.get("delta");
                                            pb::ChunkChoice {
                                                index: c.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32,
                                                delta: Some(pb::ChunkDelta {
                                                    role: delta.and_then(|d| d.get("role")).and_then(|r| r.as_str()).map(String::from),
                                                    content: delta.and_then(|d| d.get("content")).and_then(|c| c.as_str()).map(String::from),
                                                }),
                                                finish_reason: c.get("finish_reason").and_then(|f| f.as_str()).map(String::from),
                                            }
                                        }).collect();

                                        yield Ok(pb::ChatCompletionChunk {
                                            id: parsed.get("id").and_then(|v| v.as_str()).unwrap_or("").into(),
                                            model: model_id.clone(),
                                            choices: chunk_choices,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(Status::internal(format!("Stream error: {}", e)));
                    }
                }
            }
        };

        Ok(Response::new(Box::pin(chunk_stream)))
    }

    async fn list_models(
        &self,
        _request: Request<pb::ListModelsRequest>,
    ) -> Result<Response<pb::ListModelsResponse>, Status> {
        let models_lock = self.state.models.read().unwrap();
        let models = models_lock
            .iter()
            .map(|m| pb::ModelInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                provider: m.provider.clone(),
                quality_tier: m.quality_tier.to_string(),
                context_window: m.context_window,
                input_price_per_m: m.input_price_per_m,
                output_price_per_m: m.output_price_per_m,
                reasoning: m.reasoning,
                vision: m.vision,
                tool_calling: m.tool_calling,
            })
            .collect();

        Ok(Response::new(pb::ListModelsResponse { models }))
    }

    async fn get_health(
        &self,
        _request: Request<pb::HealthRequest>,
    ) -> Result<Response<pb::HealthResponse>, Status> {
        Ok(Response::new(pb::HealthResponse {
            status: "ok".into(),
            provider_count: self.state.providers.read().unwrap().len() as u32,
            model_count: self.state.models.read().unwrap().len() as u32,
        }))
    }
}

/// Extract token usage from a JSON response for logging.
fn extract_usage(
    response: &serde_json::Value,
    model: &coalesce_core::types::ModelInfo,
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
            (Some(i), Some(o)) => Some(
                (i as f64 / 1_000_000.0) * model.input_price_per_m
                    + (o as f64 / 1_000_000.0) * model.output_price_per_m,
            ),
            _ => None,
        };
        (input, output, cost)
    } else {
        (None, None, None)
    }
}

/// Create the gRPC service for adding to a tonic server.
pub fn create_service(state: Arc<ProxyState>) -> CoalesceServiceServer<GrpcHandler> {
    CoalesceServiceServer::new(GrpcHandler::new(state))
}

/// Start the gRPC server on the configured port + 1 (default 8403).
pub async fn start_grpc_server(state: Arc<ProxyState>) -> anyhow::Result<()> {
    let port = state.config.server.port + 1;
    let addr = format!("{}:{}", state.config.server.host, port).parse()?;

    info!("gRPC server listening on {}:{}", state.config.server.host, port);

    tonic::transport::Server::builder()
        .add_service(create_service(state))
        .serve(addr)
        .await?;

    Ok(())
}
