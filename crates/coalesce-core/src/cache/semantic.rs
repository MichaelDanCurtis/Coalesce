use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// TF-IDF based semantic similarity cache.
/// Caches chat completion responses and returns them for semantically similar prompts.
pub struct SemanticCache {
    entries: RwLock<Vec<CacheEntry>>,
    vocabulary: RwLock<HashMap<String, f64>>,
    config: SemanticCacheConfig,
}

#[derive(Debug, Clone)]
pub struct SemanticCacheConfig {
    pub enabled: bool,
    pub similarity_threshold: f64,
    pub max_entries: usize,
    pub ttl_secs: u64,
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            similarity_threshold: 0.95,
            max_entries: 1000,
            ttl_secs: 3600,
        }
    }
}

struct CacheEntry {
    text: String,
    tfidf_vector: Vec<(String, f64)>,
    response: String,
    created_at: Instant,
}

impl SemanticCache {
    pub fn new(config: SemanticCacheConfig) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            vocabulary: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Look up a semantically similar cached response.
    pub fn lookup(&self, text: &str) -> Option<String> {
        if !self.config.enabled {
            return None;
        }

        let query_vec = self.compute_tfidf(text);
        let entries = self.entries.read().ok()?;
        let ttl = Duration::from_secs(self.config.ttl_secs);

        let mut best_score = 0.0f64;
        let mut best_response = None;

        for entry in entries.iter() {
            if entry.created_at.elapsed() > ttl {
                continue;
            }
            let sim = cosine_similarity(&query_vec, &entry.tfidf_vector);
            if sim > best_score && sim >= self.config.similarity_threshold {
                best_score = sim;
                best_response = Some(entry.response.clone());
            }
        }

        best_response
    }

    /// Store a prompt-response pair in the cache.
    pub fn store(&self, text: &str, response: &str) {
        if !self.config.enabled {
            return;
        }

        // Update vocabulary (IDF)
        let tokens = tokenize(text);
        {
            let mut vocab = self.vocabulary.write().unwrap();
            for token in &tokens {
                *vocab.entry(token.clone()).or_insert(0.0) += 1.0;
            }
        }

        let tfidf_vector = self.compute_tfidf(text);

        let mut entries = self.entries.write().unwrap();

        // Evict expired entries
        let ttl = Duration::from_secs(self.config.ttl_secs);
        entries.retain(|e| e.created_at.elapsed() < ttl);

        // Evict oldest if at capacity
        if entries.len() >= self.config.max_entries {
            entries.remove(0);
        }

        entries.push(CacheEntry {
            text: text.to_string(),
            tfidf_vector,
            response: response.to_string(),
            created_at: Instant::now(),
        });
    }

    /// Number of entries in the cache
    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Cleanup expired entries
    pub fn cleanup(&self) {
        let ttl = Duration::from_secs(self.config.ttl_secs);
        if let Ok(mut entries) = self.entries.write() {
            entries.retain(|e| e.created_at.elapsed() < ttl);
        }
    }

    fn compute_tfidf(&self, text: &str) -> Vec<(String, f64)> {
        let tokens = tokenize(text);
        let total_tokens = tokens.len() as f64;
        if total_tokens == 0.0 {
            return vec![];
        }

        // Term frequency
        let mut tf: HashMap<String, f64> = HashMap::new();
        for t in &tokens {
            *tf.entry(t.clone()).or_insert(0.0) += 1.0;
        }

        let vocab = self.vocabulary.read().unwrap();
        let num_docs = self.entries.read().map(|e| e.len()).unwrap_or(1).max(1) as f64;

        let mut vec: Vec<(String, f64)> = tf
            .into_iter()
            .map(|(term, count)| {
                let tf_val = count / total_tokens;
                let df = vocab.get(&term).copied().unwrap_or(1.0);
                let idf = (num_docs / df).ln() + 1.0;
                (term, tf_val * idf)
            })
            .collect();

        // Normalize
        let magnitude: f64 = vec.iter().map(|(_, v)| v * v).sum::<f64>().sqrt();
        if magnitude > 0.0 {
            for (_, v) in &mut vec {
                *v /= magnitude;
            }
        }

        vec
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() > 1)
        .map(|s| s.to_string())
        .collect()
}

fn cosine_similarity(a: &[(String, f64)], b: &[(String, f64)]) -> f64 {
    let b_map: HashMap<&str, f64> = b.iter().map(|(k, v)| (k.as_str(), *v)).collect();

    let mut dot = 0.0;
    for (term, val) in a {
        if let Some(&b_val) = b_map.get(term.as_str()) {
            dot += val * b_val;
        }
    }

    // Vectors are already normalized, so dot product = cosine similarity
    dot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let cache = SemanticCache::new(SemanticCacheConfig {
            enabled: true,
            similarity_threshold: 0.90,
            max_entries: 100,
            ttl_secs: 3600,
        });

        cache.store("What is the capital of France?", "{\"answer\": \"Paris\"}");

        // Exact match should hit
        let result = cache.lookup("What is the capital of France?");
        assert!(result.is_some());
        assert!(result.unwrap().contains("Paris"));
    }

    #[test]
    fn test_dissimilar_miss() {
        let cache = SemanticCache::new(SemanticCacheConfig {
            enabled: true,
            similarity_threshold: 0.90,
            max_entries: 100,
            ttl_secs: 3600,
        });

        cache.store("What is the capital of France?", "{\"answer\": \"Paris\"}");

        // Completely different query should miss
        let result = cache.lookup("Write a Rust function to sort a list");
        assert!(result.is_none());
    }

    #[test]
    fn test_disabled_cache() {
        let cache = SemanticCache::new(SemanticCacheConfig {
            enabled: false,
            ..Default::default()
        });

        cache.store("hello", "world");
        assert!(cache.lookup("hello").is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_max_entries_eviction() {
        let cache = SemanticCache::new(SemanticCacheConfig {
            enabled: true,
            similarity_threshold: 0.90,
            max_entries: 3,
            ttl_secs: 3600,
        });

        cache.store("first query", "first response");
        cache.store("second query", "second response");
        cache.store("third query", "third response");
        assert_eq!(cache.len(), 3);

        cache.store("fourth query", "fourth response");
        assert_eq!(cache.len(), 3); // oldest evicted
    }
}
