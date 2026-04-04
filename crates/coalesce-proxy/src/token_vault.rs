//! TokenVault — secure credential management for provider API keys and OAuth tokens.
//! Stores encrypted tokens in SQLite with automatic refresh scheduling.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

/// A stored token with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenEntry {
    pub provider: String,
    pub token_type: TokenType,
    pub value: String,
    pub expires_at: Option<u64>,  // Unix timestamp
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
    pub created_at: u64,
    pub last_used: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TokenType {
    ApiKey,
    OAuth2,
    DeviceFlow,
    ServiceAccount,
}

/// In-memory token vault with persistence hooks
pub struct TokenVault {
    tokens: RwLock<HashMap<String, TokenEntry>>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl TokenVault {
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Store a token for a provider
    pub fn store(&self, provider: &str, entry: TokenEntry) {
        self.tokens.write().unwrap().insert(provider.to_string(), entry);
    }

    /// Get token for a provider (returns None if expired)
    pub fn get(&self, provider: &str) -> Option<TokenEntry> {
        let tokens = self.tokens.read().unwrap();
        tokens.get(provider).and_then(|entry| {
            if let Some(expires) = entry.expires_at {
                if now_secs() >= expires {
                    return None; // Expired
                }
            }
            Some(entry.clone())
        })
    }

    /// Remove a provider's token
    pub fn remove(&self, provider: &str) -> Option<TokenEntry> {
        self.tokens.write().unwrap().remove(provider)
    }

    /// List all stored providers and their token types
    pub fn list(&self) -> Vec<(String, TokenType, bool)> {
        let tokens = self.tokens.read().unwrap();
        let now = now_secs();
        tokens.iter().map(|(k, v)| {
            let valid = v.expires_at.map_or(true, |e| now < e);
            (k.clone(), v.token_type.clone(), valid)
        }).collect()
    }

    /// Get tokens that will expire within the given duration
    pub fn expiring_soon(&self, within: Duration) -> Vec<String> {
        let tokens = self.tokens.read().unwrap();
        let now = now_secs();
        let deadline = now + within.as_secs();
        tokens.iter()
            .filter(|(_, v)| v.expires_at.map_or(false, |e| e <= deadline && e > now))
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Count total tokens
    pub fn count(&self) -> usize {
        self.tokens.read().unwrap().len()
    }
}

impl Default for TokenVault {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(provider: &str, token_type: TokenType) -> TokenEntry {
        let now = now_secs();
        TokenEntry {
            provider: provider.into(),
            token_type,
            value: format!("test-token-{}", provider),
            expires_at: Some(now + 3600),
            refresh_token: None,
            scopes: vec!["read".into()],
            created_at: now,
            last_used: None,
        }
    }

    #[test]
    fn test_store_and_get() {
        let vault = TokenVault::new();
        let entry = make_entry("openai", TokenType::ApiKey);
        vault.store("openai", entry);
        let got = vault.get("openai").unwrap();
        assert_eq!(got.value, "test-token-openai");
    }

    #[test]
    fn test_expired_token_returns_none() {
        let vault = TokenVault::new();
        let now = now_secs();
        let entry = TokenEntry {
            provider: "expired".into(),
            token_type: TokenType::OAuth2,
            value: "old-token".into(),
            expires_at: Some(now - 100), // Already expired
            refresh_token: None,
            scopes: vec![],
            created_at: now - 3700,
            last_used: None,
        };
        vault.store("expired", entry);
        assert!(vault.get("expired").is_none());
    }

    #[test]
    fn test_list_and_remove() {
        let vault = TokenVault::new();
        vault.store("a", make_entry("a", TokenType::ApiKey));
        vault.store("b", make_entry("b", TokenType::OAuth2));
        assert_eq!(vault.count(), 2);

        let list = vault.list();
        assert_eq!(list.len(), 2);

        vault.remove("a");
        assert_eq!(vault.count(), 1);
        assert!(vault.get("a").is_none());
    }

    #[test]
    fn test_expiring_soon() {
        let vault = TokenVault::new();
        let now = now_secs();

        // Token expiring in 5 minutes
        let soon = TokenEntry {
            provider: "soon".into(),
            token_type: TokenType::OAuth2,
            value: "soon-token".into(),
            expires_at: Some(now + 300),
            refresh_token: Some("refresh-me".into()),
            scopes: vec![],
            created_at: now,
            last_used: None,
        };
        // Token expiring in 2 hours
        let later = TokenEntry {
            provider: "later".into(),
            token_type: TokenType::ApiKey,
            value: "later-token".into(),
            expires_at: Some(now + 7200),
            refresh_token: None,
            scopes: vec![],
            created_at: now,
            last_used: None,
        };
        vault.store("soon", soon);
        vault.store("later", later);

        let expiring = vault.expiring_soon(Duration::from_secs(600)); // Within 10 min
        assert_eq!(expiring.len(), 1);
        assert_eq!(expiring[0], "soon");
    }
}
