//! Idempotency key support for VM operations
//!
//! Prevents duplicate execution of mutating operations by caching responses
//! keyed by client-provided idempotency keys.

use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Maximum idempotency key length
const MAX_KEY_LENGTH: usize = 255;

/// TTL for cached responses (24 hours)
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Header name for idempotency key
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// Cached response for idempotent operations
#[derive(Clone, Debug)]
pub struct CachedResponse {
    pub status: StatusCode,
    pub body: Bytes,
    pub created_at: Instant,
}

impl CachedResponse {
    /// Check if the cached response has expired
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > CACHE_TTL
    }
}

/// In-memory store for idempotency keys and responses
#[derive(Clone)]
pub struct IdempotencyStore {
    cache: Arc<DashMap<String, CachedResponse>>,
}

impl IdempotencyStore {
    /// Create a new idempotency store
    pub fn new() -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Extract idempotency key from request headers
    pub fn extract_key(headers: &HeaderMap) -> Option<String> {
        headers
            .get(IDEMPOTENCY_KEY_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty() && s.len() <= MAX_KEY_LENGTH)
            .map(|s| s.to_string())
    }

    /// Get a cached response for the given key
    pub fn get(&self, key: &str) -> Option<CachedResponse> {
        let entry = self.cache.get(key)?;
        let response = entry.value().clone();

        if response.is_expired() {
            drop(entry);
            self.cache.remove(key);
            debug!(key = %key, "Idempotency key expired, removed from cache");
            None
        } else {
            info!(
                key = %key,
                status = %response.status,
                age_secs = response.created_at.elapsed().as_secs(),
                "Returning cached response for idempotency key"
            );
            Some(response)
        }
    }

    /// Store a response for the given key
    pub fn insert(&self, key: String, status: StatusCode, body: Bytes) {
        let response = CachedResponse {
            status,
            body,
            created_at: Instant::now(),
        };

        self.cache.insert(key.clone(), response);
        debug!(key = %key, status = %status, "Cached response for idempotency key");
    }

    /// Remove expired entries (for periodic cleanup)
    pub fn cleanup_expired(&self) {
        let expired_keys: Vec<String> = self
            .cache
            .iter()
            .filter(|entry| entry.value().is_expired())
            .map(|entry| entry.key().clone())
            .collect();

        for key in &expired_keys {
            self.cache.remove(key);
        }

        if !expired_keys.is_empty() {
            info!(count = expired_keys.len(), "Cleaned up expired idempotency keys");
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> IdempotencyStats {
        IdempotencyStats {
            total_entries: self.cache.len(),
        }
    }
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about the idempotency cache
#[derive(Debug, Clone)]
pub struct IdempotencyStats {
    pub total_entries: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn test_extract_key_valid() {
        let mut headers = HeaderMap::new();
        headers.insert(
            IDEMPOTENCY_KEY_HEADER,
            HeaderValue::from_static("test-key-123"),
        );

        let key = IdempotencyStore::extract_key(&headers);
        assert_eq!(key, Some("test-key-123".to_string()));
    }

    #[test]
    fn test_extract_key_missing() {
        let headers = HeaderMap::new();
        let key = IdempotencyStore::extract_key(&headers);
        assert_eq!(key, None);
    }

    #[test]
    fn test_extract_key_empty() {
        let mut headers = HeaderMap::new();
        headers.insert(IDEMPOTENCY_KEY_HEADER, HeaderValue::from_static(""));

        let key = IdempotencyStore::extract_key(&headers);
        assert_eq!(key, None);
    }

    #[test]
    fn test_extract_key_too_long() {
        let mut headers = HeaderMap::new();
        let long_key = "a".repeat(MAX_KEY_LENGTH + 1);
        headers.insert(
            IDEMPOTENCY_KEY_HEADER,
            HeaderValue::from_str(&long_key).unwrap(),
        );

        let key = IdempotencyStore::extract_key(&headers);
        assert_eq!(key, None);
    }

    #[test]
    fn test_insert_and_get() {
        let store = IdempotencyStore::new();
        let key = "test-key".to_string();
        let status = StatusCode::OK;
        let body = Bytes::from("test response");

        store.insert(key.clone(), status, body.clone());

        let cached = store.get(&key).unwrap();
        assert_eq!(cached.status, status);
        assert_eq!(cached.body, body);
    }

    #[test]
    fn test_get_missing_key() {
        let store = IdempotencyStore::new();
        let result = store.get("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_cached_response_not_expired() {
        let response = CachedResponse {
            status: StatusCode::OK,
            body: Bytes::from("test"),
            created_at: Instant::now(),
        };

        assert!(!response.is_expired());
    }

    #[test]
    fn test_cached_response_expired() {
        let response = CachedResponse {
            status: StatusCode::OK,
            body: Bytes::from("test"),
            created_at: Instant::now() - CACHE_TTL - Duration::from_secs(1),
        };

        assert!(response.is_expired());
    }

    #[test]
    fn test_cleanup_expired() {
        let store = IdempotencyStore::new();

        // Insert a fresh entry
        store.insert(
            "fresh".to_string(),
            StatusCode::OK,
            Bytes::from("fresh"),
        );

        // Insert an expired entry manually
        let expired = CachedResponse {
            status: StatusCode::OK,
            body: Bytes::from("expired"),
            created_at: Instant::now() - CACHE_TTL - Duration::from_secs(1),
        };
        store.cache.insert("expired".to_string(), expired);

        assert_eq!(store.cache.len(), 2);

        store.cleanup_expired();

        assert_eq!(store.cache.len(), 1);
        assert!(store.get("fresh").is_some());
        assert!(store.get("expired").is_none());
    }

    #[test]
    fn test_stats() {
        let store = IdempotencyStore::new();
        assert_eq!(store.stats().total_entries, 0);

        store.insert(
            "key1".to_string(),
            StatusCode::OK,
            Bytes::from("response1"),
        );
        store.insert(
            "key2".to_string(),
            StatusCode::CREATED,
            Bytes::from("response2"),
        );

        let stats = store.stats();
        assert_eq!(stats.total_entries, 2);
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(IdempotencyStore::new());
        let mut handles = vec![];

        // Spawn multiple threads inserting and reading
        for i in 0..10 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                let key = format!("key-{}", i);
                store_clone.insert(
                    key.clone(),
                    StatusCode::OK,
                    Bytes::from(format!("response-{}", i)),
                );
                let cached = store_clone.get(&key);
                assert!(cached.is_some());
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(store.stats().total_entries, 10);
    }
}
