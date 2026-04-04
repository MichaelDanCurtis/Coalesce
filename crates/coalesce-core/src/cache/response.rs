use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// SHA256 content-based response cache with configurable TTL.
/// Caches exact request→response mappings for deterministic queries.
pub struct ResponseCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
    config: ResponseCacheConfig,
}

#[derive(Debug, Clone)]
pub struct ResponseCacheConfig {
    pub enabled: bool,
    pub max_entries: usize,
    pub ttl_secs: u64,
    /// Only cache requests with temperature == 0 (deterministic)
    pub deterministic_only: bool,
}

impl Default for ResponseCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_entries: 5000,
            ttl_secs: 1800, // 30 min
            deterministic_only: true,
        }
    }
}

struct CacheEntry {
    response: serde_json::Value,
    created_at: Instant,
    hit_count: u64,
    provider: String,
    model: String,
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl ResponseCache {
    pub fn new(config: ResponseCacheConfig) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Generate a SHA256 cache key from model + messages.
    pub fn cache_key(model: &str, messages: &[serde_json::Value], tools: Option<&[serde_json::Value]>) -> String {
        let mut hasher = Sha256::new();
        hasher.update(model.as_bytes());
        hasher.update(b"|");
        for msg in messages {
            hasher.update(msg.to_string().as_bytes());
            hasher.update(b"|");
        }
        if let Some(tools) = tools {
            for tool in tools {
                hasher.update(tool.to_string().as_bytes());
                hasher.update(b"|");
            }
        }
        format!("{:x}", hasher.finalize())
    }

    /// Look up a cached response by key.
    pub fn get(&self, key: &str) -> Option<(serde_json::Value, String, String)> {
        if !self.config.enabled {
            return None;
        }

        let mut entries = self.entries.write().ok()?;
        let ttl = Duration::from_secs(self.config.ttl_secs);

        if let Some(entry) = entries.get_mut(key) {
            if entry.created_at.elapsed() < ttl {
                entry.hit_count += 1;
                return Some((entry.response.clone(), entry.provider.clone(), entry.model.clone()));
            } else {
                entries.remove(key);
            }
        }

        None
    }

    /// Store a response in the cache.
    pub fn put(&self, key: String, response: serde_json::Value, provider: &str, model: &str) {
        if !self.config.enabled {
            return;
        }

        let mut entries = self.entries.write().unwrap();

        // Evict expired
        let ttl = Duration::from_secs(self.config.ttl_secs);
        entries.retain(|_, e| e.created_at.elapsed() < ttl);

        // Evict LRU if at capacity
        if entries.len() >= self.config.max_entries {
            if let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, e)| e.created_at)
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest_key);
            }
        }

        entries.insert(key, CacheEntry {
            response,
            created_at: Instant::now(),
            hit_count: 0,
            provider: provider.to_string(),
            model: model.to_string(),
        });
    }

    /// Check if a request should be cached based on temperature.
    pub fn should_cache(&self, temperature: Option<f64>) -> bool {
        if !self.config.enabled {
            return false;
        }
        if self.config.deterministic_only {
            temperature.map(|t| t == 0.0).unwrap_or(false)
        } else {
            true
        }
    }

    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
    }

    pub fn stats(&self) -> CacheStats {
        let entries = self.entries.read().unwrap();
        let hits: u64 = entries.values().map(|e| e.hit_count).sum();
        CacheStats {
            total_entries: entries.len(),
            hits,
            misses: 0, // tracked externally
            evictions: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_cache_hit() {
        let cache = ResponseCache::new(ResponseCacheConfig {
            enabled: true,
            deterministic_only: false,
            ..Default::default()
        });

        let msgs = vec![json!({"role": "user", "content": "hello"})];
        let key = ResponseCache::cache_key("gpt-4", &msgs, None);
        cache.put(key.clone(), json!({"answer": "hi"}), "openai", "gpt-4");

        let result = cache.get(&key);
        assert!(result.is_some());
        let (resp, provider, model) = result.unwrap();
        assert_eq!(resp, json!({"answer": "hi"}));
        assert_eq!(provider, "openai");
        assert_eq!(model, "gpt-4");
    }

    #[test]
    fn test_cache_miss_disabled() {
        let cache = ResponseCache::new(ResponseCacheConfig {
            enabled: false,
            ..Default::default()
        });

        let key = "test_key".to_string();
        cache.put(key.clone(), json!({"answer": "hi"}), "openai", "gpt-4");
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_deterministic_only() {
        let cache = ResponseCache::new(ResponseCacheConfig {
            enabled: true,
            deterministic_only: true,
            ..Default::default()
        });

        assert!(cache.should_cache(Some(0.0)));
        assert!(!cache.should_cache(Some(0.7)));
        assert!(!cache.should_cache(None));
    }

    #[test]
    fn test_cache_key_deterministic() {
        let msgs = vec![json!({"role": "user", "content": "hello"})];
        let k1 = ResponseCache::cache_key("gpt-4", &msgs, None);
        let k2 = ResponseCache::cache_key("gpt-4", &msgs, None);
        let k3 = ResponseCache::cache_key("gpt-3.5", &msgs, None);
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn test_max_entries_eviction() {
        let cache = ResponseCache::new(ResponseCacheConfig {
            enabled: true,
            max_entries: 2,
            deterministic_only: false,
            ..Default::default()
        });

        cache.put("k1".into(), json!(1), "p", "m");
        cache.put("k2".into(), json!(2), "p", "m");
        assert_eq!(cache.len(), 2);

        cache.put("k3".into(), json!(3), "p", "m");
        assert_eq!(cache.len(), 2); // oldest evicted
    }
}
