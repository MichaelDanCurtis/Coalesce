//! Tool Equivalence Classes — mapping semantically identical capabilities across providers.
//!
//! An equivalence class groups tools that accomplish the same task
//! (e.g., web search) across different providers and implementations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::types::ToolExecution;

/// A group of tools that accomplish the same semantic task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEquivalenceClass {
    /// Canonical name for this capability (e.g., "web_search").
    pub name: String,

    /// Human-readable description of what this class of tools does.
    pub description: String,

    /// Members of the equivalence class, ordered by preference.
    pub members: Vec<EquivalenceMember>,

    /// Whether members are truly interchangeable or have known fidelity differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fidelity_notes: Option<String>,
}

/// A specific implementation of an equivalence class capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquivalenceMember {
    /// Which provider offers this implementation.
    pub provider: String,

    /// The provider-specific tool identifier
    /// (e.g., "web_search_20260209" for Anthropic, "$web_search" for Kimi).
    pub provider_tool_id: String,

    /// How this tool is executed on this provider.
    pub execution: ToolExecution,

    /// Capability flags that differ between members.
    #[serde(default)]
    pub capabilities: MemberCapabilities,

    /// Fidelity score (0.0 - 1.0) relative to the "best" implementation.
    /// Used in routing cost calculations.
    #[serde(default = "default_fidelity")]
    pub fidelity: f64,
}

fn default_fidelity() -> f64 {
    1.0
}

/// Capability flags that differ between equivalence class members.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemberCapabilities {
    /// Does this member return structured data or just text?
    #[serde(default)]
    pub structured_output: bool,

    /// Does it support citations / source attribution?
    #[serde(default)]
    pub citations: bool,

    /// Maximum input size (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_input: Option<usize>,

    /// Any known limitations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

/// Registry of all known equivalence classes.
#[derive(Debug, Clone, Default)]
pub struct EquivalenceRegistry {
    classes: HashMap<String, ToolEquivalenceClass>,
}

impl EquivalenceRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.register_builtins();
        registry
    }

    /// Register a new equivalence class.
    pub fn register(&mut self, class: ToolEquivalenceClass) {
        self.classes.insert(class.name.clone(), class);
    }

    /// Look up an equivalence class by name.
    pub fn get(&self, name: &str) -> Option<&ToolEquivalenceClass> {
        self.classes.get(name)
    }

    /// Find which equivalence class a provider-specific tool belongs to.
    pub fn find_class_for_tool(&self, provider: &str, tool_id: &str) -> Option<&ToolEquivalenceClass> {
        self.classes.values().find(|class| {
            class.members.iter().any(|m| {
                m.provider == provider && m.provider_tool_id == tool_id
            })
        })
    }

    /// Find the member for a specific provider within an equivalence class.
    pub fn find_member(&self, class_name: &str, provider: &str) -> Option<&EquivalenceMember> {
        self.classes
            .get(class_name)?
            .members
            .iter()
            .find(|m| m.provider == provider)
    }

    /// Find the best available member for a provider, falling back to proxy-managed.
    pub fn resolve_for_provider(
        &self,
        class_name: &str,
        provider: &str,
    ) -> Option<&EquivalenceMember> {
        let class = self.classes.get(class_name)?;

        // First: exact provider match
        if let Some(member) = class.members.iter().find(|m| m.provider == provider) {
            return Some(member);
        }

        // Second: proxy-managed fallback
        class.members.iter().find(|m| {
            matches!(m.execution, ToolExecution::ProxyManaged { .. })
        })
    }

    /// List all registered class names.
    pub fn class_names(&self) -> Vec<&str> {
        self.classes.keys().map(|s| s.as_str()).collect()
    }

    /// Register built-in equivalence classes for common tools.
    fn register_builtins(&mut self) {
        // Web Search
        self.register(ToolEquivalenceClass {
            name: "web_search".into(),
            description: "Search the web for information".into(),
            members: vec![
                EquivalenceMember {
                    provider: "anthropic".into(),
                    provider_tool_id: "web_search".into(),
                    execution: ToolExecution::ServerSide {
                        native_provider: "anthropic".into(),
                    },
                    capabilities: MemberCapabilities {
                        structured_output: true,
                        citations: true,
                        ..Default::default()
                    },
                    fidelity: 1.0,
                },
                EquivalenceMember {
                    provider: "openai".into(),
                    provider_tool_id: "web_search".into(),
                    execution: ToolExecution::ServerSide {
                        native_provider: "openai".into(),
                    },
                    capabilities: MemberCapabilities {
                        structured_output: true,
                        citations: true,
                        ..Default::default()
                    },
                    fidelity: 0.95,
                },
                EquivalenceMember {
                    provider: "kimi".into(),
                    provider_tool_id: "$web_search".into(),
                    execution: ToolExecution::ServerSide {
                        native_provider: "kimi".into(),
                    },
                    capabilities: MemberCapabilities {
                        structured_output: false,
                        citations: false,
                        limitations: vec!["Special echo-back execution model".into()],
                        ..Default::default()
                    },
                    fidelity: 0.7,
                },
            ],
            fidelity_notes: Some(
                "Kimi's $web_search has different execution model (echo-back). \
                 Results may differ in structure and citation quality."
                    .into(),
            ),
        });

        // Code Execution
        self.register(ToolEquivalenceClass {
            name: "code_execution".into(),
            description: "Execute code in a sandboxed environment".into(),
            members: vec![
                EquivalenceMember {
                    provider: "anthropic".into(),
                    provider_tool_id: "code_execution".into(),
                    execution: ToolExecution::ServerSide {
                        native_provider: "anthropic".into(),
                    },
                    capabilities: MemberCapabilities {
                        structured_output: true,
                        ..Default::default()
                    },
                    fidelity: 1.0,
                },
                EquivalenceMember {
                    provider: "openai".into(),
                    provider_tool_id: "code_interpreter".into(),
                    execution: ToolExecution::ServerSide {
                        native_provider: "openai".into(),
                    },
                    capabilities: MemberCapabilities {
                        structured_output: true,
                        ..Default::default()
                    },
                    fidelity: 0.95,
                },
            ],
            fidelity_notes: None,
        });

        // Web Fetch
        self.register(ToolEquivalenceClass {
            name: "web_fetch".into(),
            description: "Fetch content from a URL".into(),
            members: vec![EquivalenceMember {
                provider: "anthropic".into(),
                provider_tool_id: "web_fetch".into(),
                execution: ToolExecution::ServerSide {
                    native_provider: "anthropic".into(),
                },
                capabilities: MemberCapabilities {
                    structured_output: true,
                    ..Default::default()
                },
                fidelity: 1.0,
            }],
            fidelity_notes: Some("Only Anthropic offers native web_fetch. Others need proxy-managed fallback.".into()),
        });

        // Text Editor (coding harness tool)
        self.register(ToolEquivalenceClass {
            name: "text_editor".into(),
            description: "Edit text files with view/create/replace operations".into(),
            members: vec![EquivalenceMember {
                provider: "anthropic".into(),
                provider_tool_id: "text_editor_20250124".into(),
                execution: ToolExecution::ClientSide,
                capabilities: Default::default(),
                fidelity: 1.0,
            }],
            fidelity_notes: None,
        });

        // Bash (coding harness tool)
        self.register(ToolEquivalenceClass {
            name: "bash".into(),
            description: "Execute shell commands".into(),
            members: vec![EquivalenceMember {
                provider: "anthropic".into(),
                provider_tool_id: "bash_20250124".into(),
                execution: ToolExecution::ClientSide,
                capabilities: Default::default(),
                fidelity: 1.0,
            }],
            fidelity_notes: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_builtins() {
        let registry = EquivalenceRegistry::new();
        assert!(registry.get("web_search").is_some());
        assert!(registry.get("code_execution").is_some());
        assert!(registry.get("web_fetch").is_some());
        assert!(registry.get("text_editor").is_some());
        assert!(registry.get("bash").is_some());
    }

    #[test]
    fn test_find_class_for_tool() {
        let registry = EquivalenceRegistry::new();

        let class = registry.find_class_for_tool("kimi", "$web_search");
        assert!(class.is_some());
        assert_eq!(class.unwrap().name, "web_search");

        let class = registry.find_class_for_tool("openai", "code_interpreter");
        assert!(class.is_some());
        assert_eq!(class.unwrap().name, "code_execution");

        assert!(registry.find_class_for_tool("openai", "nonexistent").is_none());
    }

    #[test]
    fn test_resolve_for_provider() {
        let registry = EquivalenceRegistry::new();

        // Direct match
        let member = registry.resolve_for_provider("web_search", "anthropic");
        assert!(member.is_some());
        assert_eq!(member.unwrap().provider_tool_id, "web_search");

        // No match, no fallback
        let member = registry.resolve_for_provider("web_search", "ollama");
        assert!(member.is_none());

        // No match, but proxy-managed fallback would be found if registered
        let member = registry.resolve_for_provider("code_execution", "ollama");
        assert!(member.is_none()); // No proxy fallback registered for code_execution yet
    }

    #[test]
    fn test_find_member() {
        let registry = EquivalenceRegistry::new();
        let member = registry.find_member("web_search", "kimi");
        assert!(member.is_some());
        assert_eq!(member.unwrap().provider_tool_id, "$web_search");
        assert!(!member.unwrap().capabilities.citations);
    }
}
