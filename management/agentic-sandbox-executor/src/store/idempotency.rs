//! Idempotency cache wrapper on top of [`TaskStore`] (issue #206).
//!
//! Implements the `idempotency/v1` A2A extension:
//! `https://agentic-sandbox.aiwg.io/extensions/idempotency/v1`.
//!
//! # Behavior summary (per spec §4.3)
//!
//! 1. **Hash**: `request_hash = lowercase_hex(SHA-256(JCS(params \ {message.messageId})))`.
//!    The A2A `Message.messageId` field at JSON path `params.message.messageId`
//!    MUST be excluded from canonicalization — it IS the lookup key.
//! 2. **Check** returns:
//!    - `Fresh` — no cached entry (or one past TTL).
//!    - `Replay { status, body }` — same `message_id` + same `request_hash`, within TTL.
//!    - `Collision` — same `message_id` + different `request_hash`, within TTL.
//! 3. **Record** stores `(message_id, request_hash, status, response_body, created_at,
//!    expires_at)` atomically. Failed responses (4xx/5xx) are cached identically.
//! 4. **TTL** defaults to 24h. Configurable via [`IdempotencyCache::with_ttl`].
//! 5. **Cap** defaults to 100,000 entries. Configurable via
//!    [`IdempotencyCache::with_max_entries`]. Soft-LRU enforced by
//!    [`IdempotencyCache::evict_to_cap`] (oldest by `created_at`).
//!
//! # Canonicalization
//!
//! Uses [`serde_jcs`] (RFC 8785) for canonicalization. Any deviation from
//! RFC 8785 would be a deviation of the dependency, not this module.
//!
//! # Concurrency
//!
//! `check` followed by `record` is NOT a single atomic step at the
//! application layer — two threads racing on the same `message_id` may
//! both observe `Fresh`. The second `record` overwrites the first via
//! SQLite `ON CONFLICT DO UPDATE`. Spec §4.3 is satisfied because
//! subsequent `check` calls return one durable response. See the
//! `concurrent_check_race` test for the documented race semantics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use super::task_store::{IdempotencyEntry, TaskStore};

/// Default TTL: 24 hours (spec-conformant value, §3.3).
pub const DEFAULT_TTL_SECONDS: i64 = 86_400;

/// Default LRU cap: 100,000 entries (spec default, §3.3).
pub const DEFAULT_MAX_ENTRIES: u64 = 100_000;

/// Outcome of an idempotency cache lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyOutcome {
    /// No matching entry (or the entry is past TTL). Caller proceeds to
    /// execute the operation, then calls [`IdempotencyCache::record`].
    Fresh,
    /// Cached entry with matching `request_hash`. Caller MUST return the
    /// cached `(status, body)` byte-for-byte. The transport layer adds
    /// `Idempotent-Replayed: true` per spec §4.3 step 3.
    Replay {
        status: u16,
        body: serde_json::Value,
    },
    /// Cached entry exists for `message_id` but the canonicalized body
    /// differs. Caller MUST return HTTP 422 with the canonical
    /// `IDEMPOTENCY_KEY_REUSED_WITH_DIFFERENT_PAYLOAD` error per §4.5.
    Collision,
}

/// Atomic counters exposed for observability (Prometheus wiring is #212).
#[derive(Debug, Default)]
pub struct IdempotencyMetrics {
    hits: AtomicU64,
    misses: AtomicU64,
    collisions: AtomicU64,
    evictions: AtomicU64,
    purged_expired: AtomicU64,
}

impl IdempotencyMetrics {
    /// Number of `Replay` outcomes returned by `check`.
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Number of `Fresh` outcomes returned by `check` (includes
    /// past-TTL hits, which are semantically misses).
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Number of `Collision` outcomes returned by `check`.
    pub fn collisions(&self) -> u64 {
        self.collisions.load(Ordering::Relaxed)
    }

    /// Number of entries evicted by `evict_to_cap` (cumulative).
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    /// Number of entries removed by `sweep_expired` (cumulative).
    pub fn purged_expired(&self) -> u64 {
        self.purged_expired.load(Ordering::Relaxed)
    }
}

/// Idempotency cache wrapper on top of [`TaskStore`].
pub struct IdempotencyCache {
    store: Arc<TaskStore>,
    ttl: Duration,
    max_entries: u64,
    metrics: IdempotencyMetrics,
}

impl IdempotencyCache {
    /// Construct with spec defaults: 24h TTL, 100k cap.
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self {
            store,
            ttl: Duration::seconds(DEFAULT_TTL_SECONDS),
            max_entries: DEFAULT_MAX_ENTRIES,
            metrics: IdempotencyMetrics::default(),
        }
    }

    /// Override TTL. Spec-conformant value is 86400s (24h); other values
    /// are permitted by §3.3 as implementation-defined.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Override the soft-LRU cap. Eviction is enforced by
    /// [`Self::evict_to_cap`], not automatically on insert.
    pub fn with_max_entries(mut self, max: u64) -> Self {
        self.max_entries = max;
        self
    }

    /// Observe metrics.
    pub fn metrics(&self) -> &IdempotencyMetrics {
        &self.metrics
    }

    /// Current TTL setting.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Current soft-LRU cap.
    pub fn max_entries(&self) -> u64 {
        self.max_entries
    }

    /// Look up `(message_id, request_body)` in the cache.
    ///
    /// Spec §4.3 step 2:
    /// - Hit, same hash, within TTL → `Replay { status, body }`.
    /// - Hit, different hash, within TTL → `Collision`.
    /// - Hit, expired → treated as `Fresh` (caller proceeds; a subsequent
    ///   `record` overwrites the stale row via `ON CONFLICT DO UPDATE`).
    /// - Miss → `Fresh`.
    pub fn check(
        &self,
        message_id: &str,
        request_body: &serde_json::Value,
    ) -> Result<IdempotencyOutcome> {
        let hash = hash_request(request_body)?;
        match self.store.idempotency_get(message_id)? {
            None => {
                self.metrics.misses.fetch_add(1, Ordering::Relaxed);
                Ok(IdempotencyOutcome::Fresh)
            }
            Some(entry) => {
                // Past-TTL entries MUST be treated as cache miss (§4.3 step 2).
                if entry.expires_at <= Utc::now() {
                    debug!(
                        message_id,
                        expired_at = %entry.expires_at,
                        "idempotency entry past TTL; treating as fresh"
                    );
                    self.metrics.misses.fetch_add(1, Ordering::Relaxed);
                    return Ok(IdempotencyOutcome::Fresh);
                }
                if entry.request_hash == hash {
                    self.metrics.hits.fetch_add(1, Ordering::Relaxed);
                    Ok(IdempotencyOutcome::Replay {
                        status: entry.response_status,
                        body: entry.response_body,
                    })
                } else {
                    self.metrics.collisions.fetch_add(1, Ordering::Relaxed);
                    Ok(IdempotencyOutcome::Collision)
                }
            }
        }
    }

    /// Persist the response for a `(message_id, request_body)` pair.
    ///
    /// Spec §4.3 step 2 (Miss case): both successful and failed responses
    /// are cached. Caller invokes after the operation completes —
    /// `record` does not re-check for collision; that's `check`'s job.
    pub fn record(
        &self,
        message_id: &str,
        request_body: &serde_json::Value,
        status: u16,
        response_body: &serde_json::Value,
    ) -> Result<()> {
        let hash = hash_request(request_body)?;
        let now = Utc::now();
        let entry = IdempotencyEntry {
            message_id: message_id.to_string(),
            request_hash: hash,
            response_status: status,
            response_body: response_body.clone(),
            created_at: now,
            expires_at: now + self.ttl,
        };
        self.store.idempotency_put(&entry)?;
        Ok(())
    }

    /// Sweep expired entries from the underlying store. Returns the
    /// number of rows removed. Safe to call concurrently with `check` /
    /// `record`; SQLite serializes writers.
    pub fn sweep_expired(&self) -> Result<u64> {
        let n = self.store.idempotency_purge_expired()?;
        if n > 0 {
            self.metrics.purged_expired.fetch_add(n, Ordering::Relaxed);
            debug!(removed = n, "idempotency cache sweep");
        }
        Ok(n)
    }

    /// Enforce the soft-LRU cap. If `idempotency_count > max_entries`,
    /// evict the (count - max_entries) oldest rows by `created_at`.
    /// Returns the number of rows evicted.
    pub fn evict_to_cap(&self) -> Result<u64> {
        let count = self.store.idempotency_count()?;
        if count <= self.max_entries {
            return Ok(0);
        }
        let excess = count - self.max_entries;
        let removed = self.store.idempotency_evict_oldest(excess)?;
        if removed > 0 {
            self.metrics.evictions.fetch_add(removed, Ordering::Relaxed);
            debug!(
                removed,
                count_before = count,
                cap = self.max_entries,
                "idempotency LRU eviction"
            );
        }
        Ok(removed)
    }
}

/// Compute `lowercase_hex(SHA-256(JCS(body \ {params.message.messageId})))`.
///
/// The `params.message.messageId` field — when present — is removed before
/// canonicalization per spec §3.3 and §5. All other fields are preserved.
/// The input `body` is treated as the JSON-RPC `params` object (per §4.3
/// step 1: "The canonicalization input is `params` only"), but we also
/// drill through an outer `params` wrapper for callers that pass the full
/// envelope.
///
/// Implementation note: we deep-clone the body so the caller's value is
/// not mutated. The clone is `O(n)` in JSON tree size; for typical
/// A2A `MessageSendParams` payloads this is negligible.
fn hash_request(body: &serde_json::Value) -> Result<String> {
    let mut cloned = body.clone();
    strip_message_id(&mut cloned);
    let canonical = serde_jcs::to_string(&cloned).context("JCS canonicalization (RFC 8785)")?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    Ok(hex::encode(digest))
}

/// Remove `message.messageId` (whether at the top level — when the caller
/// passed `params` already — or nested under `params.message.messageId`
/// when the caller passed a wrapper). Idempotent — no-op if the path is
/// absent.
fn strip_message_id(v: &mut serde_json::Value) {
    if let Some(obj) = v.as_object_mut() {
        if let Some(message) = obj.get_mut("message").and_then(|m| m.as_object_mut()) {
            message.remove("messageId");
        }
        if let Some(params) = obj.get_mut("params").and_then(|p| p.as_object_mut()) {
            if let Some(message) = params.get_mut("message").and_then(|m| m.as_object_mut()) {
                message.remove("messageId");
            }
        }
    } else if !v.is_null() {
        warn!("hash_request: body is not a JSON object; messageId stripping skipped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::thread;
    use std::time::Duration as StdDuration;

    fn fresh_cache() -> IdempotencyCache {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        IdempotencyCache::new(store)
    }

    fn sample_body(text: &str) -> serde_json::Value {
        json!({
            "message": {
                "messageId": "00000000-0000-7000-8000-000000000001",
                "parts": [{"text": text}],
                "role": "user",
            },
            "metadata": {"k": 1},
        })
    }

    #[test]
    fn fresh_then_replay() {
        let cache = fresh_cache();
        let body = sample_body("hello");
        assert_eq!(cache.check("m1", &body).unwrap(), IdempotencyOutcome::Fresh);
        cache
            .record("m1", &body, 202, &json!({"task_id": "t1"}))
            .unwrap();
        match cache.check("m1", &body).unwrap() {
            IdempotencyOutcome::Replay { status, body } => {
                assert_eq!(status, 202);
                assert_eq!(body, json!({"task_id": "t1"}));
            }
            other => panic!("expected Replay, got {other:?}"),
        }
    }

    #[test]
    fn collision_detected() {
        let cache = fresh_cache();
        let body_a = sample_body("hello");
        let body_b = sample_body("goodbye"); // differs in parts[0].text
        cache
            .record("m1", &body_a, 202, &json!({"ok": true}))
            .unwrap();
        assert_eq!(
            cache.check("m1", &body_b).unwrap(),
            IdempotencyOutcome::Collision
        );
        assert_eq!(cache.metrics().collisions(), 1);
    }

    #[test]
    fn expired_entry_treated_as_fresh() {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        let cache = IdempotencyCache::new(store).with_ttl(Duration::milliseconds(50));
        let body = sample_body("hi");
        cache
            .record("m1", &body, 200, &json!({"ok": true}))
            .unwrap();
        thread::sleep(StdDuration::from_millis(120));
        assert_eq!(cache.check("m1", &body).unwrap(), IdempotencyOutcome::Fresh);
    }

    #[test]
    fn failed_response_cached() {
        // Spec §4.3: 4xx/5xx responses are cached identically to success.
        let cache = fresh_cache();
        let body = sample_body("boom");
        cache
            .record("m1", &body, 500, &json!({"error": "kaboom"}))
            .unwrap();
        match cache.check("m1", &body).unwrap() {
            IdempotencyOutcome::Replay { status, body } => {
                assert_eq!(status, 500);
                assert_eq!(body, json!({"error": "kaboom"}));
            }
            other => panic!("expected Replay, got {other:?}"),
        }
    }

    /// **Critical compliance test** (spec §3.3 / §5): bodies differing
    /// ONLY in `message.messageId` MUST produce the same `request_hash`,
    /// so when the cache is keyed under the *same* `message_id` a
    /// replay returns `Replay` — NOT `Collision`.
    #[test]
    fn message_id_excluded_from_hash() {
        let cache = fresh_cache();
        let body_with_id_a = json!({
            "message": {
                "messageId": "id-AAAA",
                "parts": [{"text": "hello"}],
            },
            "metadata": {"k": 1},
        });
        let body_with_id_b = json!({
            "message": {
                "messageId": "id-BBBB", // differs only here
                "parts": [{"text": "hello"}],
            },
            "metadata": {"k": 1},
        });
        let h_a = hash_request(&body_with_id_a).unwrap();
        let h_b = hash_request(&body_with_id_b).unwrap();
        assert_eq!(h_a, h_b, "messageId must be excluded from request_hash");

        cache
            .record("m1", &body_with_id_a, 202, &json!({"ok": true}))
            .unwrap();
        match cache.check("m1", &body_with_id_b).unwrap() {
            IdempotencyOutcome::Replay { status, body } => {
                assert_eq!(status, 202);
                assert_eq!(body, json!({"ok": true}));
            }
            other => panic!("expected Replay (messageId excluded from hash), got {other:?}"),
        }
        assert_eq!(cache.metrics().collisions(), 0);
    }

    #[test]
    fn message_id_excluded_under_params_wrapper() {
        // Callers passing the full JSON-RPC envelope: strip must drill
        // into `params.message.messageId` too.
        let a = json!({
            "params": {
                "message": {"messageId": "id-AAAA", "parts": [{"text": "x"}]},
            }
        });
        let b = json!({
            "params": {
                "message": {"messageId": "id-BBBB", "parts": [{"text": "x"}]},
            }
        });
        assert_eq!(hash_request(&a).unwrap(), hash_request(&b).unwrap());
    }

    #[test]
    fn eviction_at_cap() {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        let cache = IdempotencyCache::new(store.clone()).with_max_entries(10);
        for i in 0..15u32 {
            let body = json!({"message": {"parts": [{"text": format!("t{i}")}]}});
            cache
                .record(&format!("m{i}"), &body, 200, &json!({"i": i}))
                .unwrap();
        }
        let removed = cache.evict_to_cap().unwrap();
        assert_eq!(removed, 5);
        assert!(store.idempotency_count().unwrap() <= 10);
        assert_eq!(cache.metrics().evictions(), 5);
        // No-op at or below cap.
        assert_eq!(cache.evict_to_cap().unwrap(), 0);
    }

    #[test]
    fn metrics_counters() {
        let cache = fresh_cache();
        let body_a = sample_body("a");
        let body_b = sample_body("b");
        // Miss
        assert_eq!(
            cache.check("m1", &body_a).unwrap(),
            IdempotencyOutcome::Fresh
        );
        cache.record("m1", &body_a, 200, &json!({})).unwrap();
        // Two hits
        let _ = cache.check("m1", &body_a).unwrap();
        let _ = cache.check("m1", &body_a).unwrap();
        // Collision
        assert_eq!(
            cache.check("m1", &body_b).unwrap(),
            IdempotencyOutcome::Collision
        );
        let m = cache.metrics();
        assert_eq!(m.hits(), 2);
        assert_eq!(m.misses(), 1);
        assert_eq!(m.collisions(), 1);
    }

    #[test]
    fn sweep_expired_removes_old() {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        let cache = IdempotencyCache::new(store).with_ttl(Duration::milliseconds(30));
        for i in 0..3 {
            cache
                .record(
                    &format!("m{i}"),
                    &json!({"message": {"parts": [{"text": "x"}]}}),
                    200,
                    &json!({}),
                )
                .unwrap();
        }
        thread::sleep(StdDuration::from_millis(80));
        let removed = cache.sweep_expired().unwrap();
        assert_eq!(removed, 3);
        assert_eq!(cache.metrics().purged_expired(), 3);
    }

    /// Documents race semantics for two threads calling `check` for the
    /// same `(message_id, body)` before either has called `record`.
    /// Both observe `Fresh`. The first `record` wins; the second
    /// overwrites via `ON CONFLICT DO UPDATE`. With identical bodies the
    /// overwrite is observationally equivalent — only the response_body
    /// differs in this test to probe last-writer-wins explicitly.
    #[test]
    fn concurrent_check_race() {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        let cache = Arc::new(IdempotencyCache::new(store));
        let body = sample_body("racy");

        let c1 = cache.clone();
        let body1 = body.clone();
        let h1 = thread::spawn(move || c1.check("m-race", &body1).unwrap());
        let c2 = cache.clone();
        let body2 = body.clone();
        let h2 = thread::spawn(move || c2.check("m-race", &body2).unwrap());

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();
        assert_eq!(r1, IdempotencyOutcome::Fresh);
        assert_eq!(r2, IdempotencyOutcome::Fresh);

        cache
            .record("m-race", &body, 200, &json!({"v": 1}))
            .unwrap();
        cache
            .record("m-race", &body, 200, &json!({"v": 2}))
            .unwrap();

        match cache.check("m-race", &body).unwrap() {
            IdempotencyOutcome::Replay { body, .. } => {
                // Spec only requires durable binding to a response.
                // Last writer wins under ON CONFLICT DO UPDATE.
                assert!(body == json!({"v": 1}) || body == json!({"v": 2}));
            }
            other => panic!("expected Replay, got {other:?}"),
        }
    }
}
