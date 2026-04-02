use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoalesceError {
    #[error("Provider error: {provider} - {message}")]
    Provider {
        provider: String,
        message: String,
        status: Option<u16>,
    },

    #[error("No available model for tier {tier} in profile {profile}")]
    NoAvailableModel { tier: String, profile: String },

    #[error("All providers exhausted after {attempts} attempts")]
    AllProvidersExhausted { attempts: usize },

    #[error("Provider {provider} is unavailable: {reason}")]
    ProviderUnavailable { provider: String, reason: String },

    #[error("Rate limited by {provider}, retry after {retry_after_secs}s")]
    RateLimited {
        provider: String,
        retry_after_secs: u64,
    },

    #[error("Request too large: {size} bytes (max: {max} bytes)")]
    RequestTooLarge { size: usize, max: usize },

    #[error("Invalid request: {message}")]
    InvalidRequest { message: String },

    #[error("Auth error: {message}")]
    Auth { message: String },

    #[error("Config error: {message}")]
    Config { message: String },

    #[error("Storage error: {0}")]
    Storage(#[from] rusqlite::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, CoalesceError>;
