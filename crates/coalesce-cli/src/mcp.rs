use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, Write};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "coalesce";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// --- JSON-RPC 2.0 types ---

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// --- MCP Tool definitions ---

fn tool_definitions() -> Value {
    serde_json::json!({
        "tools": [
            {
                "name": "route_request",
                "description": "Route a prompt through Coalesce's scoring engine. Returns the quality tier (Simple/Medium/Complex/Reasoning), confidence score, dimension breakdown, and ranked candidate models sorted by cost.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prompt": {
                            "type": "string",
                            "description": "The user prompt to analyze and route"
                        },
                        "max_tokens": {
                            "type": "integer",
                            "description": "Optional max tokens for the request"
                        }
                    },
                    "required": ["prompt"]
                }
            },
            {
                "name": "chat",
                "description": "Send a chat completion request through Coalesce's routing proxy. The proxy automatically selects the best model based on prompt complexity and cost optimization.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prompt": {
                            "type": "string",
                            "description": "The user message to send"
                        },
                        "model": {
                            "type": "string",
                            "description": "Optional model override (default: auto-routed)"
                        },
                        "max_tokens": {
                            "type": "integer",
                            "description": "Maximum tokens in the response"
                        },
                        "temperature": {
                            "type": "number",
                            "description": "Sampling temperature (0.0 - 2.0)"
                        }
                    },
                    "required": ["prompt"]
                }
            },
            {
                "name": "list_models",
                "description": "List all available models across all configured providers. Includes pricing, quality tier, capabilities (reasoning, vision, tool calling), and current marginal cost.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "provider": {
                            "type": "string",
                            "description": "Optional: filter models by provider name"
                        },
                        "tier": {
                            "type": "string",
                            "description": "Optional: filter by quality tier (Simple, Medium, Complex, Reasoning)"
                        }
                    }
                }
            },
            {
                "name": "get_stats",
                "description": "Get usage statistics including total requests, success rate, total cost, average latency, token counts, quota states, and recent request history.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "recent_limit": {
                            "type": "integer",
                            "description": "Number of recent requests to include (default: 10)"
                        }
                    }
                }
            },
            {
                "name": "get_provider_status",
                "description": "Get the health and status of all configured providers including circuit breaker state, billing type, model count, and availability.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "get_health",
                "description": "Quick health check of the Coalesce proxy. Returns provider count, model count, circuit breaker states, and dedup cache info.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    })
}

// --- MCP Server ---

pub async fn run_mcp_server(base_url: String) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l.trim().to_string(),
            Err(_) => break,
        };

        if line.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_error(&stdout, Value::Null, -32700, &format!("Parse error: {}", e));
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            if let Some(id) = request.id {
                write_error(&stdout, id, -32600, "Invalid JSON-RPC version");
            }
            continue;
        }

        let id = request.id.clone().unwrap_or(Value::Null);

        match request.method.as_str() {
            // Notifications (no id) — just ignore
            "notifications/initialized" | "notifications/cancelled" => continue,

            "initialize" => {
                let result = serde_json::json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION
                    }
                });
                write_result(&stdout, id, result);
            }

            "tools/list" => {
                write_result(&stdout, id, tool_definitions());
            }

            "tools/call" => {
                let tool_name = request.params.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let arguments = request.params.get("arguments")
                    .cloned()
                    .unwrap_or(Value::Object(serde_json::Map::new()));

                let result = handle_tool_call(&client, &base_url, tool_name, &arguments).await;

                match result {
                    Ok(content) => {
                        write_result(&stdout, id, serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": serde_json::to_string_pretty(&content).unwrap_or_default()
                            }]
                        }));
                    }
                    Err(e) => {
                        write_result(&stdout, id, serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": format!("Error: {}", e)
                            }],
                            "isError": true
                        }));
                    }
                }
            }

            "ping" => {
                write_result(&stdout, id, serde_json::json!({}));
            }

            _ => {
                // Unknown method — if it has an id, send error; if notification, ignore
                if request.id.is_some() {
                    write_error(&stdout, id, -32601, &format!("Method not found: {}", request.method));
                }
            }
        }
    }

    Ok(())
}

async fn handle_tool_call(
    client: &reqwest::Client,
    base_url: &str,
    tool_name: &str,
    arguments: &Value,
) -> anyhow::Result<Value> {
    match tool_name {
        "route_request" => {
            let prompt = arguments.get("prompt")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required parameter: prompt"))?;

            let resp = client
                .post(format!("{}/api/v1/routing/playground", base_url))
                .json(&serde_json::json!({ "prompt": prompt }))
                .send()
                .await?
                .json::<Value>()
                .await?;

            Ok(resp)
        }

        "chat" => {
            let prompt = arguments.get("prompt")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing required parameter: prompt"))?;

            let model = arguments.get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("auto");

            let mut body = serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
                "stream": false
            });

            if let Some(max_tokens) = arguments.get("max_tokens").and_then(|v| v.as_u64()) {
                body["max_tokens"] = Value::from(max_tokens);
            }
            if let Some(temperature) = arguments.get("temperature").and_then(|v| v.as_f64()) {
                body["temperature"] = Value::from(temperature);
            }

            let resp = client
                .post(format!("{}/v1/chat/completions", base_url))
                .json(&body)
                .send()
                .await?
                .json::<Value>()
                .await?;

            Ok(resp)
        }

        "list_models" => {
            let resp = client
                .get(format!("{}/v1/models", base_url))
                .send()
                .await?
                .json::<Value>()
                .await?;

            // Apply optional filters
            if let Some(filter_provider) = arguments.get("provider").and_then(|v| v.as_str()) {
                if let Some(data) = resp.get("data").and_then(|v| v.as_array()) {
                    let filtered: Vec<&Value> = data.iter()
                        .filter(|m| m.get("owned_by").and_then(|v| v.as_str()) == Some(filter_provider))
                        .collect();
                    return Ok(serde_json::json!({
                        "object": "list",
                        "data": filtered,
                        "count": filtered.len()
                    }));
                }
            }

            if let Some(filter_tier) = arguments.get("tier").and_then(|v| v.as_str()) {
                if let Some(data) = resp.get("data").and_then(|v| v.as_array()) {
                    let filtered: Vec<&Value> = data.iter()
                        .filter(|m| {
                            m.get("quality_tier")
                                .and_then(|v| v.as_str())
                                .map(|t| t.eq_ignore_ascii_case(filter_tier))
                                .unwrap_or(false)
                        })
                        .collect();
                    return Ok(serde_json::json!({
                        "object": "list",
                        "data": filtered,
                        "count": filtered.len()
                    }));
                }
            }

            Ok(resp)
        }

        "get_stats" => {
            let resp = client
                .get(format!("{}/v1/stats", base_url))
                .send()
                .await?
                .json::<Value>()
                .await?;

            Ok(resp)
        }

        "get_provider_status" => {
            let resp = client
                .get(format!("{}/api/v1/providers", base_url))
                .send()
                .await?
                .json::<Value>()
                .await?;

            Ok(resp)
        }

        "get_health" => {
            let resp = client
                .get(format!("{}/health", base_url))
                .send()
                .await?
                .json::<Value>()
                .await?;

            Ok(resp)
        }

        _ => Err(anyhow::anyhow!("Unknown tool: {}", tool_name)),
    }
}

fn write_result(stdout: &io::Stdout, id: Value, result: Value) {
    let response = JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(result),
        error: None,
    };
    write_response(stdout, &response);
}

fn write_error(stdout: &io::Stdout, id: Value, code: i32, message: &str) {
    let response = JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data: None,
        }),
    };
    write_response(stdout, &response);
}

fn write_response(stdout: &io::Stdout, response: &JsonRpcResponse) {
    let json = serde_json::to_string(response).unwrap_or_default();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{}", json);
    let _ = out.flush();
}
