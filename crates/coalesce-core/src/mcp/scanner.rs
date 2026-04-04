//! Scanner for discovering MCP server configurations from known tool config files.

use super::types::*;
use std::path::PathBuf;

/// Scans for MCP server configs in known locations
pub struct McpScanner;

impl McpScanner {
    /// Scan all known config locations
    pub fn scan_all() -> Vec<McpServerConfig> {
        let mut results = Vec::new();
        results.extend(Self::scan_claude_code());
        results.extend(Self::scan_cursor());
        results.extend(Self::scan_vscode());
        results
    }

    /// Scan Claude Code MCP config (`~/.claude/settings.json` or project `.mcp.json`)
    pub fn scan_claude_code() -> Vec<McpServerConfig> {
        let home = dirs::home_dir().unwrap_or_default();
        let paths = vec![
            home.join(".claude").join("settings.json"),
            home.join(".claude.json"),
        ];

        for path in paths {
            if let Some(configs) = Self::parse_claude_mcp(&path) {
                return configs;
            }
        }
        Vec::new()
    }

    /// Scan Cursor MCP config
    pub fn scan_cursor() -> Vec<McpServerConfig> {
        let home = dirs::home_dir().unwrap_or_default();
        let path = home.join(".cursor").join("mcp.json");
        Self::parse_generic_mcp(&path, McpConfigSource::Cursor).unwrap_or_default()
    }

    /// Scan VS Code MCP config
    pub fn scan_vscode() -> Vec<McpServerConfig> {
        let home = dirs::home_dir().unwrap_or_default();
        let paths = vec![
            home.join(".vscode").join("mcp.json"),
            home.join("Library")
                .join("Application Support")
                .join("Code")
                .join("User")
                .join("settings.json"),
        ];

        for path in paths {
            if let Some(configs) = Self::parse_generic_mcp(&path, McpConfigSource::Vscode) {
                return configs;
            }
        }
        Vec::new()
    }

    fn parse_claude_mcp(path: &PathBuf) -> Option<Vec<McpServerConfig>> {
        let content = std::fs::read_to_string(path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;

        let servers = json
            .get("mcpServers")
            .or_else(|| json.get("mcp_servers"))?
            .as_object()?;

        let configs: Vec<McpServerConfig> = servers
            .iter()
            .filter_map(|(name, val)| {
                let transport = if val.get("url").is_some() {
                    McpTransport::Sse
                } else {
                    McpTransport::Stdio
                };

                Some(McpServerConfig {
                    id: format!("claude-{name}"),
                    name: name.clone(),
                    transport,
                    command: val.get("command").and_then(|v| v.as_str()).map(String::from),
                    args: val.get("args").and_then(|v| {
                        v.as_array().map(|arr| {
                            arr.iter()
                                .filter_map(|a| a.as_str().map(String::from))
                                .collect()
                        })
                    }),
                    url: val.get("url").and_then(|v| v.as_str()).map(String::from),
                    env: val.get("env").and_then(|v| {
                        v.as_object().map(|obj| {
                            obj.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect()
                        })
                    }),
                    enabled: true,
                    source: McpConfigSource::ClaudeCode,
                })
            })
            .collect();

        if configs.is_empty() {
            None
        } else {
            Some(configs)
        }
    }

    fn parse_generic_mcp(
        path: &PathBuf,
        source: McpConfigSource,
    ) -> Option<Vec<McpServerConfig>> {
        let content = std::fs::read_to_string(path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;

        // Try "mcpServers" or "servers" key
        let servers = json
            .get("mcpServers")
            .or_else(|| json.get("servers"))?
            .as_object()?;

        let prefix = match source {
            McpConfigSource::Cursor => "cursor",
            McpConfigSource::Vscode => "vscode",
            McpConfigSource::Windsurf => "windsurf",
            _ => "ext",
        };

        let configs: Vec<McpServerConfig> = servers
            .iter()
            .filter_map(|(name, val)| {
                let transport = if val.get("url").is_some() {
                    McpTransport::Sse
                } else {
                    McpTransport::Stdio
                };

                Some(McpServerConfig {
                    id: format!("{prefix}-{name}"),
                    name: name.clone(),
                    transport,
                    command: val.get("command").and_then(|v| v.as_str()).map(String::from),
                    args: val.get("args").and_then(|v| {
                        v.as_array().map(|arr| {
                            arr.iter()
                                .filter_map(|a| a.as_str().map(String::from))
                                .collect()
                        })
                    }),
                    url: val.get("url").and_then(|v| v.as_str()).map(String::from),
                    env: val.get("env").and_then(|v| {
                        v.as_object().map(|obj| {
                            obj.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect()
                        })
                    }),
                    enabled: true,
                    source: source.clone(),
                })
            })
            .collect();

        if configs.is_empty() {
            None
        } else {
            Some(configs)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_claude_mcp_format() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("settings.json");
        std::fs::write(
            &config_path,
            r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
                },
                "remote-server": {
                    "url": "http://localhost:3000/sse"
                }
            }
        }"#,
        )
        .unwrap();

        let configs = McpScanner::parse_claude_mcp(&config_path).unwrap();
        assert_eq!(configs.len(), 2);

        let fs = configs.iter().find(|c| c.name == "filesystem").unwrap();
        assert_eq!(fs.transport, McpTransport::Stdio);
        assert_eq!(fs.command.as_deref(), Some("npx"));
        assert_eq!(fs.source, McpConfigSource::ClaudeCode);

        let remote = configs.iter().find(|c| c.name == "remote-server").unwrap();
        assert_eq!(remote.transport, McpTransport::Sse);
        assert_eq!(remote.url.as_deref(), Some("http://localhost:3000/sse"));
    }

    #[test]
    fn test_parse_generic_mcp() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("mcp.json");
        std::fs::write(
            &config_path,
            r#"{
            "mcpServers": {
                "postgres": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-postgres"],
                    "env": {
                        "DATABASE_URL": "postgresql://localhost/mydb"
                    }
                }
            }
        }"#,
        )
        .unwrap();

        let configs =
            McpScanner::parse_generic_mcp(&config_path, McpConfigSource::Cursor).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].id, "cursor-postgres");
        assert!(configs[0].env.as_ref().unwrap().contains_key("DATABASE_URL"));
    }

    #[test]
    fn test_scan_missing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.json");
        assert!(McpScanner::parse_claude_mcp(&path).is_none());
    }
}
