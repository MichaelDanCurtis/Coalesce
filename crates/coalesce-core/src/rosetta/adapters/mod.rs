//! Concrete `ProviderStreamAdapter` implementations for each provider family.

pub mod openai_compat;
pub mod anthropic;
pub mod google;

pub use openai_compat::OpenAiCompatAdapter;
pub use anthropic::AnthropicAdapter;
pub use google::GoogleAdapter;
