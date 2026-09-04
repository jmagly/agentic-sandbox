//! Idempotency key support for VM operations
//!
//! Prevents duplicate execution of mutating operations by caching responses
//! keyed by client-provided idempotency keys.

use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use dashmap::mapref::entry::Entry;
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
    admin_cache: Arc<DashMap<String, AdminIdempotencyEntry>>,
}

#[derive(Clone, Debug)]
enum AdminIdempotencyEntry {
    Pending {
        request_fingerprint: String,
        created_at: Instant,
    },
    Complete(AdminCachedResponse),
}

impl AdminIdempotencyEntry {
    fn is_expired(&self) -> bool {
        match self {
            Self::Pending { created_at, .. } => created_at.elapsed() > CACHE_TTL,
            Self::Complete(response) => response.created_at.elapsed() > CACHE_TTL,
        }
    }

    fn fingerprint(&self) -> &str {
        match self {
            Self::Pending {
                request_fingerprint,
                ..
            } => request_fingerprint,
            Self::Complete(response) => &response.request_fingerprint,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AdminCachedResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    request_fingerprint: String,
    created_at: Instant,
}

#[derive(Clone, Debug)]
pub enum AdminIdempotencyDecision {
    Execute,
    Replay(AdminCachedResponse),
    InFlight,
    Conflict,
}

impl IdempotencyStore {
    /// Create a new idempotency store
    pub fn new() -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            admin_cache: Arc::new(DashMap::new()),
        }
    }

    /// Extract idempotency key from request headers
    pub fn extract_key(headers: &HeaderMap) -> Option<String> {
        Self::validate_key(headers).ok().flatten()
    }

    /// Distinguish an absent key from a malformed key. Admin mutations must
    /// reject malformed keys instead of silently executing without replay
    /// protection when the caller believes an intent key was accepted.
    pub fn validate_key(headers: &HeaderMap) -> Result<Option<String>, &'static str> {
        let Some(value) = headers.get(IDEMPOTENCY_KEY_HEADER) else {
            return Ok(None);
        };
        let value = value
            .to_str()
            .map_err(|_| "key must contain visible ASCII")?;
        if value.is_empty() {
            return Err("key must not be empty");
        }
        if value.len() > MAX_KEY_LENGTH {
            return Err("key exceeds 255 bytes");
        }
        Ok(Some(value.to_string()))
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

    /// Atomically reserve an admin mutation intent or recover its response.
    /// A key is bound to the method, route/query and body fingerprint supplied
    /// by the HTTP boundary; reuse with any different request is a conflict.
    pub fn begin_admin(&self, key: &str, request_fingerprint: &str) -> AdminIdempotencyDecision {
        loop {
            match self.admin_cache.entry(key.to_string()) {
                Entry::Vacant(entry) => {
                    entry.insert(AdminIdempotencyEntry::Pending {
                        request_fingerprint: request_fingerprint.to_string(),
                        created_at: Instant::now(),
                    });
                    return AdminIdempotencyDecision::Execute;
                }
                Entry::Occupied(entry) if entry.get().is_expired() => {
                    entry.remove();
                }
                Entry::Occupied(entry) if entry.get().fingerprint() != request_fingerprint => {
                    return AdminIdempotencyDecision::Conflict;
                }
                Entry::Occupied(entry) => match entry.get() {
                    AdminIdempotencyEntry::Pending { .. } => {
                        return AdminIdempotencyDecision::InFlight;
                    }
                    AdminIdempotencyEntry::Complete(response) => {
                        return AdminIdempotencyDecision::Replay(response.clone());
                    }
                },
            }
        }
    }

    pub fn complete_admin(
        &self,
        key: &str,
        request_fingerprint: &str,
        status: StatusCode,
        headers: HeaderMap,
        body: Bytes,
    ) {
        let Some(entry) = self.admin_cache.get(key) else {
            return;
        };
        let owns_reservation = matches!(
            entry.value(),
            AdminIdempotencyEntry::Pending { request_fingerprint: current, .. }
                if current == request_fingerprint
        );
        drop(entry);
        if owns_reservation {
            self.admin_cache.insert(
                key.to_string(),
                AdminIdempotencyEntry::Complete(AdminCachedResponse {
                    status,
                    headers,
                    body,
                    request_fingerprint: request_fingerprint.to_string(),
                    created_at: Instant::now(),
                }),
            );
        }
    }

    pub fn abandon_admin(&self, key: &str, request_fingerprint: &str) {
        let should_remove = self.admin_cache.get(key).is_some_and(|entry| {
            matches!(
                entry.value(),
                AdminIdempotencyEntry::Pending { request_fingerprint: current, .. }
                    if current == request_fingerprint
            )
        });
        if should_remove {
            self.admin_cache.remove(key);
        }
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

        self.admin_cache.retain(|_, entry| !entry.is_expired());

        if !expired_keys.is_empty() {
            info!(
                count = expired_keys.len(),
                "Cleaned up expired idempotency keys"
            );
        }
    }

    /// Get cache statistics
    pub fn stats(&self) -> IdempotencyStats {
        IdempotencyStats {
            total_entries: self.cache.len() + self.admin_cache.len(),
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
        assert_eq!(
            IdempotencyStore::validate_key(&headers),
            Err("key exceeds 255 bytes")
        );
    }

    #[test]
    fn invalid_admin_key_is_distinct_from_an_absent_key() {
        let mut headers = HeaderMap::new();
        assert_eq!(IdempotencyStore::validate_key(&headers), Ok(None));
        headers.insert(IDEMPOTENCY_KEY_HEADER, HeaderValue::from_static(""));
        assert_eq!(
            IdempotencyStore::validate_key(&headers),
            Err("key must not be empty")
        );
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
        store.insert("fresh".to_string(), StatusCode::OK, Bytes::from("fresh"));

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

        store.insert("key1".to_string(), StatusCode::OK, Bytes::from("response1"));
        store.insert(
            "key2".to_string(),
            StatusCode::CREATED,
            Bytes::from("response2"),
        );

        let stats = store.stats();
        assert_eq!(stats.total_entries, 2);
    }

    #[test]
    fn admin_request_key_is_bound_to_fingerprint_and_response() {
        let store = IdempotencyStore::new();
        assert!(matches!(
            store.begin_admin("intent-1", "post:/instances:body-a"),
            AdminIdempotencyDecision::Execute
        ));
        assert!(matches!(
            store.begin_admin("intent-1", "post:/instances:body-a"),
            AdminIdempotencyDecision::InFlight
        ));
        assert!(matches!(
            store.begin_admin("intent-1", "post:/instances:body-b"),
            AdminIdempotencyDecision::Conflict
        ));

        store.complete_admin(
            "intent-1",
            "post:/instances:body-a",
            StatusCode::ACCEPTED,
            HeaderMap::new(),
            Bytes::from_static(br#"{"operation_id":"op-1"}"#),
        );
        match store.begin_admin("intent-1", "post:/instances:body-a") {
            AdminIdempotencyDecision::Replay(response) => {
                assert_eq!(response.status, StatusCode::ACCEPTED);
                assert_eq!(
                    response.body,
                    Bytes::from_static(br#"{"operation_id":"op-1"}"#)
                );
            }
            decision => panic!("expected replay, got {decision:?}"),
        }
    }

    #[test]
    fn abandoned_admin_request_can_be_retried() {
        let store = IdempotencyStore::new();
        assert!(matches!(
            store.begin_admin("intent-2", "post:/instances:body"),
            AdminIdempotencyDecision::Execute
        ));
        store.abandon_admin("intent-2", "post:/instances:body");
        assert!(matches!(
            store.begin_admin("intent-2", "post:/instances:body"),
            AdminIdempotencyDecision::Execute
        ));
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
