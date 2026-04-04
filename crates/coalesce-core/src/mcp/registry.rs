use super::types::*;
use std::collections::HashMap;
use std::sync::RwLock;

/// Registry of MCP servers -- manages lifecycle and tool discovery
pub struct McpRegistry {
    servers: RwLock<HashMap<String, McpServerState>>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new MCP server
    pub fn register(&self, config: McpServerConfig) {
        let id = config.id.clone();
        let state = McpServerState {
            config,
            status: McpServerStatus::Disconnected,
            tools: Vec::new(),
            last_connected: None,
            error: None,
        };
        self.servers.write().unwrap().insert(id, state);
    }

    /// Unregister an MCP server
    pub fn unregister(&self, id: &str) -> Option<McpServerState> {
        self.servers.write().unwrap().remove(id)
    }

    /// Get all registered servers
    pub fn list(&self) -> Vec<McpServerState> {
        self.servers.read().unwrap().values().cloned().collect()
    }

    /// Get a specific server
    pub fn get(&self, id: &str) -> Option<McpServerState> {
        self.servers.read().unwrap().get(id).cloned()
    }

    /// Update server status
    pub fn set_status(&self, id: &str, status: McpServerStatus, error: Option<String>) {
        if let Some(server) = self.servers.write().unwrap().get_mut(id) {
            server.status = status;
            server.error = error;
            if server.status == McpServerStatus::Connected {
                server.last_connected = Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );
            }
        }
    }

    /// Update tools for a server
    pub fn set_tools(&self, id: &str, tools: Vec<McpTool>) {
        if let Some(server) = self.servers.write().unwrap().get_mut(id) {
            server.tools = tools;
        }
    }

    /// Get all tools across all connected servers
    pub fn all_tools(&self) -> Vec<(String, McpTool)> {
        let servers = self.servers.read().unwrap();
        servers
            .values()
            .filter(|s| s.status == McpServerStatus::Connected)
            .flat_map(|s| {
                s.tools
                    .iter()
                    .map(move |t| (s.config.id.clone(), t.clone()))
            })
            .collect()
    }

    /// Count of registered servers
    pub fn count(&self) -> usize {
        self.servers.read().unwrap().len()
    }

    /// Count of connected servers
    pub fn connected_count(&self) -> usize {
        self.servers
            .read()
            .unwrap()
            .values()
            .filter(|s| s.status == McpServerStatus::Connected)
            .count()
    }

    /// Toggle a server's enabled state
    pub fn toggle(&self, id: &str) -> Option<bool> {
        if let Some(server) = self.servers.write().unwrap().get_mut(id) {
            server.config.enabled = !server.config.enabled;
            Some(server.config.enabled)
        } else {
            None
        }
    }
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            name: format!("Test Server {id}"),
            transport: McpTransport::Stdio,
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), format!("@test/{id}")]),
            url: None,
            env: None,
            enabled: true,
            source: McpConfigSource::Manual,
        }
    }

    #[test]
    fn test_register_and_list() {
        let reg = McpRegistry::new();
        reg.register(test_config("a"));
        reg.register(test_config("b"));
        assert_eq!(reg.count(), 2);
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn test_unregister() {
        let reg = McpRegistry::new();
        reg.register(test_config("a"));
        let removed = reg.unregister("a");
        assert!(removed.is_some());
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn test_status_and_tools() {
        let reg = McpRegistry::new();
        reg.register(test_config("srv"));
        reg.set_status("srv", McpServerStatus::Connected, None);
        reg.set_tools(
            "srv",
            vec![
                McpTool {
                    name: "read".into(),
                    description: Some("Read a file".into()),
                    input_schema: None,
                },
                McpTool {
                    name: "write".into(),
                    description: Some("Write a file".into()),
                    input_schema: None,
                },
            ],
        );

        let srv = reg.get("srv").unwrap();
        assert_eq!(srv.status, McpServerStatus::Connected);
        assert_eq!(srv.tools.len(), 2);
        assert!(srv.last_connected.is_some());

        let all_tools = reg.all_tools();
        assert_eq!(all_tools.len(), 2);
    }

    #[test]
    fn test_connected_count() {
        let reg = McpRegistry::new();
        reg.register(test_config("a"));
        reg.register(test_config("b"));
        reg.register(test_config("c"));

        assert_eq!(reg.connected_count(), 0);
        reg.set_status("a", McpServerStatus::Connected, None);
        reg.set_status("b", McpServerStatus::Error, Some("timeout".into()));
        assert_eq!(reg.connected_count(), 1);
    }

    #[test]
    fn test_toggle() {
        let reg = McpRegistry::new();
        reg.register(test_config("a"));

        let result = reg.toggle("a");
        assert_eq!(result, Some(false));

        let result = reg.toggle("a");
        assert_eq!(result, Some(true));

        assert!(reg.toggle("nonexistent").is_none());
    }

    #[test]
    fn test_all_tools_only_connected() {
        let reg = McpRegistry::new();
        reg.register(test_config("a"));
        reg.register(test_config("b"));

        reg.set_status("a", McpServerStatus::Connected, None);
        reg.set_tools(
            "a",
            vec![McpTool {
                name: "tool1".into(),
                description: None,
                input_schema: None,
            }],
        );
        reg.set_tools(
            "b",
            vec![McpTool {
                name: "tool2".into(),
                description: None,
                input_schema: None,
            }],
        );

        // Only "a" is connected, so only its tools appear
        let tools = reg.all_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].1.name, "tool1");
    }
}
