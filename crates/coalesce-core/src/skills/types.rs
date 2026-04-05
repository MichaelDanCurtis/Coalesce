use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A skill that can be installed and enabled for specific harnesses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub source: SkillSource,
    pub enabled: bool,
    /// Which harnesses this skill is enabled for.
    pub harnesses: Vec<String>,
    /// Skill content (prompt text, tool definitions, etc.)
    pub content: SkillContent,
    pub installed_at: String,
    pub updated_at: Option<String>,
}

/// Where a skill was sourced from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SkillSource {
    #[serde(rename = "local")]
    Local { path: PathBuf },
    #[serde(rename = "github")]
    GitHub { repo: String, path: Option<String> },
    #[serde(rename = "builtin")]
    Builtin,
}

/// The payload of a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SkillContent {
    #[serde(rename = "prompt")]
    Prompt { text: String },
    #[serde(rename = "tool")]
    Tool { definition: serde_json::Value },
    #[serde(rename = "composite")]
    Composite {
        prompts: Vec<String>,
        tools: Vec<serde_json::Value>,
    },
}
