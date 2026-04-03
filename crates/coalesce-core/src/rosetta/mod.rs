//! Rosetta — Semantic Tool Bridging with Capability-Aware Routing
//!
//! Rosetta makes tools first-class citizens in the routing decision by:
//! 1. Normalizing all tool definitions to a canonical form
//! 2. Mapping semantically equivalent tools across providers (equivalence classes)
//! 3. Extending the router with tool-derived scoring dimensions
//! 4. Normalizing thinking/reasoning output across providers
//!
//! See RFC-001 for the full design: coalesce-rfc-001-rosetta-semantic-tool-bridging.md

pub mod types;
pub mod thinking;
pub mod equivalence;
pub mod capabilities;
pub mod adapter;
pub mod context;

pub use types::*;
pub use thinking::{CanonicalThinking, ThinkingBlock, ThinkingSplitParser};
pub use equivalence::{ToolEquivalenceClass, EquivalenceMember, EquivalenceRegistry};
pub use capabilities::{ProviderToolCapabilities, ThinkingToolsSupport, ProviderQuirk, StreamingToolBehavior};
pub use adapter::ProviderToolAdapter;
pub use context::RosettaContext;
