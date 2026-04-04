//! MCP Unified Panel -- registry, client, and config scanning for MCP servers.
//! Provides a unified interface for managing MCP tool servers across transports.

pub mod registry;
pub mod scanner;
pub mod types;

pub use registry::McpRegistry;
pub use scanner::McpScanner;
pub use types::*;
