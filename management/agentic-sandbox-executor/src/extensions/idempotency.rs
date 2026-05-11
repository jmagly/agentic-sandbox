//! `agentic-sandbox/idempotency` extension (#213).
//!
//! Wraps the v2 [`IdempotencyCache`] (Wave 2 W2.2 / #206) and applies
//! `messageId` + JCS-canonical-payload deduplication to `message/send`
//! and `tasks/cancel`.
//!
//! ## Wire behavior
//!
//! - `pre_request`:
//!   - If activated AND `request_body` has `message.messageId`, run
//!     [`IdempotencyCache::check`].
//!   - On `Replay { status, body }`, short-circuit with
//!     [`ExtensionOutcome::Replay`].
//!   - On `Collision`, short-circuit with [`ExtensionOutcome::Reject`]
//!     (HTTP 422 problem+json `idempotency.key_reused`).
//!   - Otherwise, continue.
//! - `post_response`:
//!   - If activated AND the request had a `messageId` AND the response
//!     was not a replay, record `(message_id, body, status, response)`
//!     via [`IdempotencyCache::record`].
//!
//! The `post_response` path needs the original request body and the
//! status code, which it gets via `PostResponseCtx`. The request body
//! is re-extracted from the inbound `messageId` lookup chain. This
//! intentionally mirrors the inline behavior previously in
//! `bindings/rest.rs::send_message` so that tests asserting end-to-end
//! status codes + headers continue to pass.

use std::sync::Arc;

use serde_json::json;

use super::{ExtensionHandler, ExtensionOutcome, PostResponseCtx, PreRequestCtx};
use crate::store::idempotency::{IdempotencyCache, IdempotencyOutcome};

/// Extension URI per spec.
pub const URI: &str = "https://agentic-sandbox.aiwg.io/extensions/idempotency/v1";

/// Server-side idempotency extension handler.
pub struct IdempotencyExtension {
    cache: Arc<IdempotencyCache>,
}

impl IdempotencyExtension {
    pub fn new(cache: Arc<IdempotencyCache>) -> Self {
        Self { cache }
    }

    /// Public accessor for the wrapped cache; used by `post_response`
    /// wiring in the REST binding to record after the main handler
    /// produces a fresh response.
    pub fn cache(&self) -> Arc<IdempotencyCache> {
        Arc::clone(&self.cache)
    }
}

impl ExtensionHandler for IdempotencyExtension {
    fn uri(&self) -> &'static str {
        URI
    }

    fn required(&self) -> bool {
        false
    }

    fn pre_request(&self, ctx: &PreRequestCtx<'_>) -> ExtensionOutcome {
        if !ctx.activated.contains(URI) {
            return ExtensionOutcome::Continue;
        }
        let Some(mid) = ctx.message_id else {
            return ExtensionOutcome::Continue;
        };
        match self.cache.check(mid, ctx.request_body) {
            Ok(IdempotencyOutcome::Replay { status, body }) => {
                ExtensionOutcome::Replay { status, body }
            }
            Ok(IdempotencyOutcome::Collision) => ExtensionOutcome::Reject {
                status: 422,
                body: json!({
                    "type": "https://agentic-sandbox.aiwg.io/errors/idempotency-collision",
                    "title": "Idempotency key reused with different payload",
                    "status": 422,
                    "detail": "The provided messageId was previously used with a different request body",
                    "code": "idempotency.key_reused",
                }),
            },
            Ok(IdempotencyOutcome::Fresh) => ExtensionOutcome::Continue,
            Err(e) => {
                tracing::warn!(error = %e, "idempotency check failed; proceeding fresh");
                ExtensionOutcome::Continue
            }
        }
    }

    fn post_response(&self, _ctx: &mut PostResponseCtx<'_>) {
        // Recording happens via the explicit `cache()` accessor in the
        // REST binding, where the original request body is still in
        // scope. Keeping the recording out of `post_response` lets us
        // skip the record on replay/reject paths without threading the
        // outcome through PostResponseCtx.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{
        ActivatedExtensions, ExtensionHandler, ExtensionRegistry, PreRequestCtx,
    };
    use crate::store::task_store::TaskStore;
    use serde_json::Value;

    fn mk_cache() -> Arc<IdempotencyCache> {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        Arc::new(IdempotencyCache::new(store))
    }

    fn activated() -> ActivatedExtensions {
        ActivatedExtensions(vec![URI.to_string()])
    }

    fn sample_body() -> Value {
        json!({
            "message": {
                "messageId": "00000000-0000-7000-8000-000000000001",
                "role": "user",
                "parts": [{"kind": "text", "text": "ping"}]
            }
        })
    }

    #[test]
    fn idempotency_replay_via_extension() {
        let cache = mk_cache();
        let ext = IdempotencyExtension::new(cache.clone());

        // Seed cache with a recorded response.
        let body = sample_body();
        let mid = "00000000-0000-7000-8000-000000000001";
        let stored = json!({"id": "task-1", "kind": "task"});
        cache.record(mid, &body, 202, &stored).unwrap();

        let act = activated();
        let ctx = PreRequestCtx {
            activated: &act,
            task_id: None,
            message_id: Some(mid),
            request_body: &body,
        };
        match ext.pre_request(&ctx) {
            ExtensionOutcome::Replay {
                status,
                body: cached,
            } => {
                assert_eq!(status, 202);
                assert_eq!(cached["id"], "task-1");
            }
            other => panic!("expected Replay, got {other:?}"),
        }
    }

    #[test]
    fn idempotency_collision_via_extension() {
        let cache = mk_cache();
        let ext = IdempotencyExtension::new(cache.clone());

        let mid = "00000000-0000-7000-8000-000000000002";
        let body1 = json!({
            "message": {"messageId": mid, "role": "user", "parts": [{"kind": "text", "text": "a"}]}
        });
        let body2 = json!({
            "message": {"messageId": mid, "role": "user", "parts": [{"kind": "text", "text": "b"}]}
        });
        cache
            .record(mid, &body1, 202, &json!({"id": "task-2"}))
            .unwrap();

        let act = activated();
        let ctx = PreRequestCtx {
            activated: &act,
            task_id: None,
            message_id: Some(mid),
            request_body: &body2,
        };
        match ext.pre_request(&ctx) {
            ExtensionOutcome::Reject { status, body } => {
                assert_eq!(status, 422);
                assert_eq!(body["code"], "idempotency.key_reused");
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn idempotency_skipped_when_not_activated() {
        let cache = mk_cache();
        let ext = IdempotencyExtension::new(cache.clone());
        let mid = "00000000-0000-7000-8000-000000000003";
        let body = sample_body();
        // Even if cached, no activation means continue.
        cache.record(mid, &body, 202, &json!({"id": "x"})).unwrap();

        let act = ActivatedExtensions::default();
        let ctx = PreRequestCtx {
            activated: &act,
            task_id: None,
            message_id: Some(mid),
            request_body: &body,
        };
        assert!(matches!(ext.pre_request(&ctx), ExtensionOutcome::Continue));
    }

    #[test]
    fn idempotency_through_registry() {
        let cache = mk_cache();
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(IdempotencyExtension::new(cache.clone())));

        let mid = "00000000-0000-7000-8000-000000000004";
        let body = sample_body_with_mid(mid);
        cache
            .record(mid, &body, 202, &json!({"id": "task-r"}))
            .unwrap();

        let act = activated();
        let ctx = PreRequestCtx {
            activated: &act,
            task_id: None,
            message_id: Some(mid),
            request_body: &body,
        };
        match reg.pre_request(&ctx) {
            ExtensionOutcome::Replay { body: cached, .. } => {
                assert_eq!(cached["id"], "task-r");
            }
            other => panic!("expected Replay via registry, got {other:?}"),
        }
    }

    fn sample_body_with_mid(mid: &str) -> Value {
        json!({
            "message": {
                "messageId": mid,
                "role": "user",
                "parts": [{"kind": "text", "text": "ping"}]
            }
        })
    }
}
