use serde::{Deserialize, Serialize};

/// Transport type for MCP server connections
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Sse,
    StreamableHttp,
    WebSocket,
    Grpc,
    InProcess,
}

/// Status of an MCP server
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum McpServerStatus {
    Connected,
    Disconnected,
    Error,
    Starting,
    Unknown,
}

/// An MCP tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<serde_json::Value>,
}

/// An MCP server definition with connection info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub url: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub enabled: bool,
    #[serde(default)]
    pub source: McpConfigSource,
}

/// Where an MCP server config was discovered
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpConfigSource {
    ClaudeCode,
    Cursor,
    Vscode,
    Windsurf,
    #[default]
    Manual,
}

/// Runtime state of a registered MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerState {
    pub config: McpServerConfig,
    pub status: McpServerStatus,
    pub tools: Vec<McpTool>,
    pub last_connected: Option<u64>,
    pub error: Option<String>,
}
