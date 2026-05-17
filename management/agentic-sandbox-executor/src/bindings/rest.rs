//! A2A REST HTTP binding (#210).
//!
//! Mounts the canonical A2A endpoint set under `/agents/{instance_id}/v1/...`
//! and shares state — [`TaskStore`] + [`IdempotencyCache`] — with the
//! per-method handlers in [`crate::handlers`]. The [`crate::instance::InstanceLayer`]
//! tower middleware resolves `{instance_id}` to an `Arc<InstanceContext>`
//! before the route handler runs.
//!
//! ## Response shapes
//!
//! - Success bodies are A2A-shaped JSON, currently emitted as
//!   `serde_json::Value`. The wire format is what matters; typed wrappers
//!   around `a2a-rs` are a nice-to-have, not a hard requirement. See
//!   "Deviation" below.
//! - Errors use RFC 7807 `application/problem+json` envelopes built by
//!   [`error_response`]. The shape matches
//!   `docs/contracts/admin-api/error-envelope.schema.json`.
//!
//! ## Deviation from spec
//!
//! The handlers consume / emit `serde_json::Value` directly instead of
//! `a2a::Task` / `a2a::Message` types. Reasons:
//!
//! 1. The `a2a-lf` crate from the Gitea mirror moves quickly; surface-level
//!    type churn would force re-binding every Wave 3 issue.
//! 2. The persistence layer ([`TaskStore`]) already stores Task/Status/
//!    Artifact payloads as `serde_json::Value` blobs (see
//!    `management/src/aiwg_serve/task_store.rs` design note 1) — so the
//!    natural seam is JSON in/JSON out.
//! 3. JCS canonicalization, idempotency hashing, and JWS payloads all
//!    operate on the JSON tree directly.
//!
//! Wire fidelity is asserted by tests that probe the JSON shape (id,
//! `status.state`, etc.) rather than by Rust type checks. When `a2a-lf`
//! stabilizes its public surface we can swap in typed structs without
//! touching callers.

use std::sync::Arc;

use axum::http::header::HeaderValue;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;

use crate::bindings::pty_bridge::{NoOpPtyBridge, PtyBridge};
use crate::bindings::pty_ws::{ws_handler, SessionRegistry};
use crate::extensions::{build_default_registry, require_extensions_middleware, ExtensionRegistry};
use crate::handlers::push_delivery::{DeliveryEvent, PushDelivery};
use crate::instance::{InstanceLayer, InstanceRegistry, RuntimeKind};
use crate::store::idempotency::IdempotencyCache;
use crate::store::task_store::TaskStore;

// --- Shared app state -------------------------------------------------------

/// Shared state for the REST router. Cheaply cloneable.
///
/// Fields are intentionally listed alphabetically to minimize merge
/// conflicts when parallel issues extend this struct.
#[derive(Clone)]
pub struct AppState {
    /// Sender for push-notification deliveries (#211, #235). Handlers
    /// that mutate Task state (`send_message`, `cancel_task`, future
    /// state transitions) enqueue a [`DeliveryEvent`] here; the
    /// [`PushDelivery`] worker spawned by [`router`] consumes events
    /// and dispatches HTTP POSTs to every registered push config.
    pub delivery: tokio::sync::mpsc::Sender<DeliveryEvent>,
    /// Registry of server-side A2A extension handlers (#213).
    pub extensions: Arc<ExtensionRegistry>,
    pub idem: Arc<IdempotencyCache>,
    /// Per-instance routing registry (#253). Surfaced here so the
    /// server-wide JWKS aggregator (`/.well-known/jwks.json`) can iterate
    /// instances without re-plumbing the registry through axum extensions.
    pub instance_registry: InstanceRegistry,
    /// #269: Outbound dispatch seam for `messages:send`. Defaults to
    /// [`crate::bindings::message_dispatch::NoOpMessageDispatch`] when
    /// the management layer hasn't wired a real implementation, so
    /// `send_message` produces a truthful 503 envelope instead of
    /// leaving the task `submitted` indefinitely.
    pub message_dispatch: Arc<dyn crate::bindings::message_dispatch::MessageDispatch>,
    /// Source-of-output bridge for `pty-ws/v1` sessions (#237). The
    /// default is a [`NoOpPtyBridge`] so the executor crate stays
    /// self-contained and existing tests keep their broadcast-echo
    /// behavior; the management crate injects an `AgentPtyBridge`
    /// (see `agentic_management::agent_pty_bridge`) that forwards to
    /// `agent-rs` over gRPC in production builds.
    pub pty_bridge: Arc<dyn PtyBridge>,
    /// Per-`(instance_id, session_id)` shared state for the pty-ws/v1
    /// custom binding (W4.1, #214). Cheaply cloneable.
    pub session_registry: Arc<SessionRegistry>,
    pub store: Arc<TaskStore>,
}

// --- RFC 7807 problem+json envelope ----------------------------------------

/// Build an `application/problem+json` response (RFC 7807).
///
/// The body shape matches `docs/contracts/admin-api/error-envelope.schema.json`:
///
/// ```json
/// {
///   "type":        "<type-uri>",
///   "title":       "<short title>",
///   "status":      <http-status>,
///   "detail":      "<long detail>",
///   "code":        "<machine code>",
///   "trace_id":    "<trace id or empty>",
///   "instance_id": "<instance id or empty>"
/// }
/// ```
pub fn error_response(
    status: StatusCode,
    type_uri: &str,
    title: &str,
    detail: impl Into<String>,
    code: &str,
    trace_id: Option<&str>,
    instance_id: Option<&str>,
) -> Response {
    let body = serde_json::json!({
        "type": type_uri,
        "title": title,
        "status": status.as_u16(),
        "detail": detail.into(),
        "code": code,
        "trace_id": trace_id.unwrap_or(""),
        "instance_id": instance_id.unwrap_or(""),
    });
    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        )],
        body.to_string(),
    )
        .into_response()
}

// --- Extension activation helper -------------------------------------------

/// The `idempotency/v1` extension URI.
pub const EXT_IDEMPOTENCY_URI: &str = "https://agentic-sandbox.aiwg.io/extensions/idempotency/v1";

/// Return `true` if `headers` contains `A2A-Extensions` listing the
/// idempotency extension URI. Multiple values are accepted as either
/// repeated header lines or one comma-separated line.
pub fn idempotency_activated(headers: &HeaderMap) -> bool {
    headers
        .get_all("a2a-extensions")
        .iter()
        .flat_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(|s| s.trim())
        .any(|s| s == EXT_IDEMPOTENCY_URI)
}

/// Build an `A2A-Extensions` header value listing the activated extensions
/// to mirror back to the client per A2A §3.4.
pub fn activated_extensions_header(activated: &[&str]) -> Option<HeaderValue> {
    if activated.is_empty() {
        return None;
    }
    let joined = activated.join(", ");
    HeaderValue::from_str(&joined).ok()
}

// --- Router -----------------------------------------------------------------

/// Build the REST router for the executor.
///
/// Routes (all under `/agents/{instance_id}/v1/...`):
///
/// | Method+Path                       | Handler                  |
/// |-----------------------------------|--------------------------|
/// | POST   `/messages:send`           | `send_message`           |
/// | POST   `/messages:stream`         | `send_streaming_message` (SSE stub) |
/// | GET    `/tasks/{tid}`             | `get_task`               |
/// | GET    `/tasks`                   | `list_tasks` (cursor)    |
/// | POST   `/tasks/{tid}:cancel`      | `cancel_task`            |
/// | GET    `/tasks/{tid}:subscribe`   | `subscribe_to_task` (SSE) |
/// | GET    `/extendedAgentCard`       | `get_extended_agent_card` |
pub fn router(
    registry: InstanceRegistry,
    store: Arc<TaskStore>,
    idem: Arc<IdempotencyCache>,
) -> Router {
    router_with_bridge_and_dispatch(
        registry,
        store,
        idem,
        Arc::new(NoOpPtyBridge),
        crate::bindings::message_dispatch::noop(),
    )
}

/// Build the REST router with a caller-supplied [`PtyBridge`] (#243).
///
/// Production binaries that own an `AgentRegistry` + `CommandDispatcher`
/// construct `AgentPtyBridge` (in the `agentic-management` crate, see
/// `agentic_management::agent_pty_bridge`) and call this variant so
/// `pty-ws/v1` sessions forward to the connected agent fleet instead of
/// falling back to the legacy broadcast-echo path.
///
/// [`router`] delegates here with [`NoOpPtyBridge`] for tests and
/// harness builds.
pub fn router_with_bridge(
    registry: InstanceRegistry,
    store: Arc<TaskStore>,
    idem: Arc<IdempotencyCache>,
    pty_bridge: Arc<dyn PtyBridge>,
) -> Router {
    router_with_bridge_and_dispatch(
        registry,
        store,
        idem,
        pty_bridge,
        crate::bindings::message_dispatch::noop(),
    )
}

/// Build the REST router with caller-supplied [`PtyBridge`] and
/// [`crate::bindings::message_dispatch::MessageDispatch`] (#269).
///
/// Production binaries inject both a real `AgentPtyBridge` (for pty-ws)
/// and a real `AgentMessageDispatch` (for `messages:send` forwarding).
/// Tests inject `accepting()` to exercise the happy path without
/// standing up a full agent connection.
pub fn router_with_bridge_and_dispatch(
    registry: InstanceRegistry,
    store: Arc<TaskStore>,
    idem: Arc<IdempotencyCache>,
    pty_bridge: Arc<dyn PtyBridge>,
    message_dispatch: Arc<dyn crate::bindings::message_dispatch::MessageDispatch>,
) -> Router {
    use crate::handlers;

    // Build the default extension registry. The router-level registry
    // is the executor-wide default; per-instance overrides could be
    // layered in later via the `InstanceContext` if needed.
    let extensions = Arc::new(build_default_registry(
        idem.clone(),
        RuntimeKind::Vm,
        "agentic-dev".to_string(),
        "executor.local".to_string(),
    ));

    // Spawn the push-delivery worker (#211, #235). The sender is plumbed
    // into AppState so handlers can enqueue DeliveryEvents on state
    // transitions; the receiver lives inside the spawned task.
    let delivery = PushDelivery::new(store.clone()).spawn();

    let state = AppState {
        delivery,
        extensions: extensions.clone(),
        idem,
        instance_registry: registry.clone(),
        // #269: dispatch impl from caller. `router` / `router_with_bridge`
        // default to NoOp; production wires a real agent-backed impl
        // via `router_with_bridge_and_dispatch`.
        message_dispatch,
        pty_bridge,
        session_registry: Arc::new(SessionRegistry::new()),
        store,
    };

    // Per #236: mutating routes enforce required A2A extensions
    // (`runtime/v1`) via `RequireA2AExtensions` middleware. Read-only
    // GET routes bypass via separate `Router` composition so callers
    // can fetch tasks / subscribe / extendedAgentCard without
    // negotiating extensions first.
    let mutating = Router::new()
        .route(
            "/agents/{instance_id}/v1/messages:send",
            post(handlers::send_message::handler),
        )
        .route(
            "/agents/{instance_id}/v1/messages:stream",
            post(handlers::send_streaming_message::handler),
        )
        // NOTE: axum 0.8 disallows two parameters in a single path segment
        // and treats `{tid}:cancel` as such. The A2A spec uses
        // `/tasks/{tid}:cancel` / `/tasks/{tid}:subscribe` (colon-suffixed
        // action names). We host the same actions at `/tasks/{tid}/cancel`
        // and `/tasks/{tid}/subscribe`. This is a deviation from §11 wire
        // format that we document explicitly; clients constructing the
        // path from the AgentCard's `supportedInterfaces` should target
        // these URIs. Re-binding to the spec form would require a custom
        // axum matcher or a downgrade of the routing layer.
        .route(
            "/agents/{instance_id}/v1/tasks/{tid}/cancel",
            post(handlers::cancel_task::handler),
        )
        // Push-notification config CRUD (#211). The A2A spec uses
        // `pushNotificationConfigs` (plural noun, camelCase) under the task
        // resource. POST/GET (list)/GET (single)/DELETE all flow through
        // the required-extensions gate per #236.
        .route(
            "/agents/{instance_id}/v1/tasks/{tid}/pushNotificationConfigs",
            post(handlers::push_notification::create_config)
                .get(handlers::push_notification::list_configs),
        )
        .route(
            "/agents/{instance_id}/v1/tasks/{tid}/pushNotificationConfigs/{cid}",
            get(handlers::push_notification::get_config)
                .delete(handlers::push_notification::delete_config),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            extensions.clone(),
            require_extensions_middleware,
        ));

    let readonly = Router::new()
        .route(
            "/agents/{instance_id}/.well-known/jwks.json",
            get(handlers::jwks::single_instance),
        )
        .route(
            "/agents/{instance_id}/v1/tasks",
            get(handlers::list_tasks::handler),
        )
        .route(
            "/agents/{instance_id}/v1/tasks/{tid}",
            get(handlers::get_task::handler),
        )
        .route(
            "/agents/{instance_id}/v1/tasks/{tid}/subscribe",
            get(handlers::subscribe_to_task::handler),
        )
        .route(
            "/agents/{instance_id}/v1/extendedAgentCard",
            get(handlers::get_extended_agent_card::handler),
        )
        // #268: A2A spec discovery path. `agent_card_url` in
        // `/api/v2/admin/instances` advertises the well-known path, but
        // it had 404'd while `/v1/extendedAgentCard` returned a signed
        // card for the same instance. Alias both to the same handler so
        // discovery via the well-known URL matches the published one.
        .route(
            "/agents/{instance_id}/.well-known/agent-card.json",
            get(handlers::get_extended_agent_card::handler),
        )
        // pty-ws/v1 custom binding (W4.1, #214). The WebSocket upgrade
        // shares state with the REST surface so the session registry,
        // TaskStore, and idempotency cache are visible to both transports.
        .route(
            "/agents/{instance_id}/sessions/{session_id}/attach",
            get(ws_handler),
        );

    // Server-wide JWKS aggregate (#253). Mounted OUTSIDE `InstanceLayer`
    // because the path has no `{instance_id}` segment — the handler reads
    // the full `InstanceRegistry` from `AppState` instead.
    let server_wide =
        Router::new().route("/.well-known/jwks.json", get(handlers::jwks::all_instances));

    mutating
        .merge(readonly)
        .layer(InstanceLayer::new(registry))
        .merge(server_wide)
        .with_state(state)
}

// --- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::{InstanceContext, RuntimeKind};

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use serde_json::Value;
    use std::time::Duration;
    use tower::ServiceExt;

    /// `runtime/v1` extension URI; required on mutating routes per #236.
    const EXT_RUNTIME_URI: &str = crate::extensions::runtime::URI;

    /// Build a request::Builder pre-populated with the required
    /// `A2A-Extensions: runtime/v1` header for mutating routes. Tests
    /// that exercise mutating endpoints chain `.method(..).uri(..)` after
    /// this to stay readable.
    fn test_request_with_runtime_ext() -> axum::http::request::Builder {
        Request::builder().header("A2A-Extensions", EXT_RUNTIME_URI)
    }

    fn mk_state() -> (InstanceRegistry, Arc<TaskStore>, Arc<IdempotencyCache>) {
        let reg = InstanceRegistry::new();
        let ctx = Arc::new(InstanceContext::new_ephemeral(
            "inst-1",
            RuntimeKind::Vm,
            "agentic-dev",
            None,
            "inst-1.example.test",
        ));
        reg.insert(ctx);
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        let idem = Arc::new(IdempotencyCache::new(store.clone()));
        (reg, store, idem)
    }

    fn body_json(v: Value) -> Body {
        Body::from(serde_json::to_vec(&v).unwrap())
    }

    async fn read_body(resp: Response) -> Value {
        let (parts, body) = resp.into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        if bytes.is_empty() {
            return Value::Null;
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => v,
            Err(_) => {
                let s = String::from_utf8_lossy(&bytes).to_string();
                panic!("non-JSON body (status={}): {}", parts.status, s);
            }
        }
    }

    fn sample_message() -> Value {
        serde_json::json!({
            "message": {
                "messageId": "00000000-0000-7000-8000-000000000001",
                "role": "user",
                "parts": [{"kind": "text", "text": "ping"}],
            }
        })
    }

    /// #269: helper that wires the test accepting-dispatch so the happy
    /// path tests exercise `submitted → working` instead of getting
    /// 503-failed from the default NoOp.
    fn router_with_accept(
        reg: InstanceRegistry,
        store: Arc<TaskStore>,
        idem: Arc<IdempotencyCache>,
    ) -> Router {
        router_with_bridge_and_dispatch(
            reg,
            store,
            idem,
            Arc::new(NoOpPtyBridge),
            crate::bindings::message_dispatch::accepting(),
        )
    }

    #[tokio::test]
    async fn send_message_creates_task_returns_202() {
        let (reg, store, idem) = mk_state();
        let app = router_with_accept(reg, store.clone(), idem);

        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/messages:send")
            .header("content-type", "application/json")
            .body(body_json(sample_message()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let location = resp
            .headers()
            .get(axum::http::header::LOCATION)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        assert!(
            location.starts_with("/agents/inst-1/v1/tasks/"),
            "Location header missing or wrong: {location}"
        );

        let v = read_body(resp).await;
        assert!(v.get("id").is_some(), "Task body must have id");
        // #269: with a real dispatch wired, the response reflects the
        // post-dispatch transition. The task is `working`, not still
        // `submitted`.
        assert_eq!(v["status"]["state"], "working");

        assert_eq!(store.count_tasks().unwrap(), 1);
    }

    #[tokio::test]
    async fn send_message_503_when_dispatch_unimplemented() {
        // #269: default (NoOp) dispatch must produce a truthful 503
        // envelope and persist the task as failed/infrastructure so
        // callers don't poll a doomed submitted task.
        let (reg, store, idem) = mk_state();
        let app = router(reg, store.clone(), idem);

        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/messages:send")
            .header("content-type", "application/json")
            .body(body_json(sample_message()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let v = read_body(resp).await;
        assert_eq!(v["code"], "dispatch.unimplemented");

        // Exactly one task row, and it is terminal-failed so polling
        // GET /tasks/{id} returns failed instead of perpetual submitted.
        assert_eq!(store.count_tasks().unwrap(), 1);
        let only = store.list_tasks(Default::default()).unwrap();
        // ListFilter default excludes terminal, so include_terminal:true:
        let only = store
            .list_tasks(crate::store::task_store::ListFilter {
                include_terminal: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].state.as_str(), "failed");
    }

    #[tokio::test]
    async fn list_tasks_is_scoped_to_path_instance_id() {
        // #269: tasks from one instance must not leak into another
        // instance's GET /agents/{instance_id}/v1/tasks response.
        let (reg, store, idem) = mk_state();
        // Insert two tasks, one per instance.
        use crate::store::task_store::{TaskRow, TaskState};
        use chrono::Utc;
        let now = Utc::now();
        let row_a = TaskRow {
            task_id: "task-a".into(),
            context_id: None,
            instance_id: Some("inst-1".into()),
            state: TaskState::Submitted,
            fail_kind: None,
            status_json: serde_json::json!({"state": "submitted"}),
            metadata_json: None,
            created_at: now,
            updated_at: now,
            terminal_at: None,
        };
        let row_b = TaskRow {
            task_id: "task-b".into(),
            context_id: None,
            instance_id: Some("inst-2".into()),
            state: TaskState::Submitted,
            fail_kind: None,
            status_json: serde_json::json!({"state": "submitted"}),
            metadata_json: None,
            created_at: now,
            updated_at: now,
            terminal_at: None,
        };
        store.upsert_task(&row_a).unwrap();
        store.upsert_task(&row_b).unwrap();

        let app = router(reg, store, idem);
        let req = Request::builder()
            .method("GET")
            .uri("/agents/inst-1/v1/tasks")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body(resp).await;
        let tasks = v["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1, "inst-1 must only see its own task");
        assert_eq!(tasks[0]["id"], "task-a");
    }

    #[tokio::test]
    async fn send_message_idempotency_replay() {
        let (reg, store, idem) = mk_state();
        let app = router_with_accept(reg, store, idem);

        let body = sample_message();

        let req1 = Request::builder()
            .method("POST")
            .uri("/agents/inst-1/v1/messages:send")
            .header("content-type", "application/json")
            .header("A2A-Extensions", EXT_IDEMPOTENCY_URI)
            .header("A2A-Extensions", EXT_RUNTIME_URI)
            .body(body_json(body.clone()))
            .unwrap();
        let resp1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::ACCEPTED);
        let v1 = read_body(resp1).await;
        let task_id_1 = v1["id"].as_str().unwrap().to_string();

        let req2 = Request::builder()
            .method("POST")
            .uri("/agents/inst-1/v1/messages:send")
            .header("content-type", "application/json")
            .header("A2A-Extensions", EXT_IDEMPOTENCY_URI)
            .header("A2A-Extensions", EXT_RUNTIME_URI)
            .body(body_json(body))
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::ACCEPTED);
        let replayed = resp2
            .headers()
            .get("idempotent-replayed")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(replayed, "true");
        let v2 = read_body(resp2).await;
        assert_eq!(v2["id"].as_str().unwrap(), task_id_1);
    }

    #[tokio::test]
    async fn send_message_idempotency_skipped_when_ext_not_activated() {
        let (reg, store, idem) = mk_state();
        let app = router_with_accept(reg, store.clone(), idem);
        let body = sample_message();

        let req1 = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/messages:send")
            .header("content-type", "application/json")
            .body(body_json(body.clone()))
            .unwrap();
        let resp1 = app.clone().oneshot(req1).await.unwrap();
        let v1 = read_body(resp1).await;
        let tid_1 = v1["id"].as_str().unwrap().to_string();

        let req2 = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/messages:send")
            .header("content-type", "application/json")
            .body(body_json(body))
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        let v2 = read_body(resp2).await;
        let tid_2 = v2["id"].as_str().unwrap().to_string();

        assert_ne!(
            tid_1, tid_2,
            "with no extension activation, each call creates a new task"
        );
        assert_eq!(store.count_tasks().unwrap(), 2);
    }

    #[tokio::test]
    async fn get_task_404_when_unknown() {
        let (reg, store, idem) = mk_state();
        let app = router(reg, store, idem);
        let req = Request::builder()
            .method("GET")
            .uri("/agents/inst-1/v1/tasks/does-not-exist")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "application/problem+json");
        let body = read_body(resp).await;
        assert_eq!(body["status"], 404);
        assert_eq!(body["code"], "task.not_found");
    }

    #[tokio::test]
    async fn get_task_returns_task() {
        let (reg, store, idem) = mk_state();
        // #269: needs a real dispatch to round-trip a non-failed task.
        let app = router_with_accept(reg, store.clone(), idem);

        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/messages:send")
            .header("content-type", "application/json")
            .body(body_json(sample_message()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let v = read_body(resp).await;
        let tid = v["id"].as_str().unwrap().to_string();

        let req = Request::builder()
            .method("GET")
            .uri(format!("/agents/inst-1/v1/tasks/{}", tid))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body(resp).await;
        assert_eq!(v["id"].as_str().unwrap(), tid);
        // After dispatch accept the task is `working`, not `submitted`.
        assert_eq!(v["status"]["state"], "working");
    }

    #[tokio::test]
    async fn list_tasks_pagination() {
        let (reg, store, idem) = mk_state();
        // #269: needs accepting dispatch so each send_message returns 202.
        let app = router_with_accept(reg, store.clone(), idem);

        for i in 0..30 {
            let body = serde_json::json!({
                "message": {
                    "messageId": format!("00000000-0000-7000-8000-{:012}", i),
                    "role": "user",
                    "parts": [{"kind": "text", "text": format!("ping {i}")}],
                }
            });
            let req = test_request_with_runtime_ext()
                .method("POST")
                .uri("/agents/inst-1/v1/messages:send")
                .header("content-type", "application/json")
                .body(body_json(body))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::ACCEPTED);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(store.count_tasks().unwrap(), 30);

        let req = Request::builder()
            .method("GET")
            .uri("/agents/inst-1/v1/tasks?limit=10")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body(resp).await;
        let tasks = v["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 10);
        let cursor = v["next_cursor"].as_str().expect("next_cursor present");

        let req = Request::builder()
            .method("GET")
            .uri(format!(
                "/agents/inst-1/v1/tasks?limit=10&cursor={}",
                cursor
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let v = read_body(resp).await;
        let tasks = v["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 10);
    }

    #[tokio::test]
    async fn list_tasks_state_filter() {
        let (reg, store, idem) = mk_state();

        use crate::store::task_store::{TaskRow, TaskState};
        use chrono::Utc;
        let now = Utc::now();
        store
            .upsert_task(&TaskRow {
                task_id: "t-sub".into(),
                context_id: None,
                instance_id: Some("inst-1".into()),
                state: TaskState::Submitted,
                fail_kind: None,
                status_json: serde_json::json!({"state": "submitted"}),
                metadata_json: None,
                created_at: now,
                updated_at: now,
                terminal_at: None,
            })
            .unwrap();
        store
            .upsert_task(&TaskRow {
                task_id: "t-work".into(),
                context_id: None,
                instance_id: Some("inst-1".into()),
                state: TaskState::Working,
                fail_kind: None,
                status_json: serde_json::json!({"state": "working"}),
                metadata_json: None,
                created_at: now,
                updated_at: now,
                terminal_at: None,
            })
            .unwrap();

        let app = router(reg, store, idem);
        let req = Request::builder()
            .method("GET")
            .uri("/agents/inst-1/v1/tasks?state=working")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body(resp).await;
        let tasks = v["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["status"]["state"], "working");
    }

    #[tokio::test]
    async fn cancel_task_409_when_terminal() {
        let (reg, store, idem) = mk_state();

        use crate::store::task_store::{TaskRow, TaskState};
        use chrono::Utc;
        let now = Utc::now();
        store
            .upsert_task(&TaskRow {
                task_id: "t-done".into(),
                context_id: None,
                instance_id: Some("inst-1".into()),
                state: TaskState::Completed,
                fail_kind: None,
                status_json: serde_json::json!({"state": "completed"}),
                metadata_json: None,
                created_at: now,
                updated_at: now,
                terminal_at: Some(now),
            })
            .unwrap();

        let app = router(reg, store, idem);
        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/tasks/t-done/cancel")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let v = read_body(resp).await;
        assert_eq!(v["code"], "task.not_cancelable");
    }

    #[tokio::test]
    async fn cancel_task_200_transitions() {
        let (reg, store, idem) = mk_state();

        use crate::store::task_store::{TaskRow, TaskState};
        use chrono::Utc;
        let now = Utc::now();
        store
            .upsert_task(&TaskRow {
                task_id: "t-work".into(),
                context_id: None,
                instance_id: Some("inst-1".into()),
                state: TaskState::Working,
                fail_kind: None,
                status_json: serde_json::json!({"state": "working"}),
                metadata_json: None,
                created_at: now,
                updated_at: now,
                terminal_at: None,
            })
            .unwrap();

        let app = router(reg, store.clone(), idem);
        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/tasks/t-work/cancel")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body(resp).await;
        assert_eq!(v["status"]["state"], "canceled");

        let row = store.get_task("t-work").unwrap().unwrap();
        assert_eq!(row.state, TaskState::Canceled);
    }

    #[tokio::test]
    async fn subscribe_to_task_sends_initial_event() {
        let (reg, store, idem) = mk_state();
        use crate::store::task_store::{TaskRow, TaskState};
        use chrono::Utc;
        let now = Utc::now();
        store
            .upsert_task(&TaskRow {
                task_id: "t-done".into(),
                context_id: None,
                instance_id: Some("inst-1".into()),
                state: TaskState::Completed,
                fail_kind: None,
                status_json: serde_json::json!({"state": "completed"}),
                metadata_json: None,
                created_at: now,
                updated_at: now,
                terminal_at: Some(now),
            })
            .unwrap();

        let app = router(reg, store, idem);
        let req = Request::builder()
            .method("GET")
            .uri("/agents/inst-1/v1/tasks/t-done/subscribe")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ct.starts_with("text/event-stream"),
            "expected SSE, got {ct}"
        );
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let body_str = String::from_utf8_lossy(&body);
        assert!(
            body_str.contains("event: task"),
            "missing event: task in {body_str}"
        );
        assert!(body_str.contains("t-done"), "task id missing in SSE body");
    }

    #[tokio::test]
    async fn extended_agent_card_returns_signed() {
        let (reg, store, idem) = mk_state();
        let app = router(reg, store, idem);
        let req = Request::builder()
            .method("GET")
            .uri("/agents/inst-1/v1/extendedAgentCard")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body(resp).await;
        assert_eq!(v["protocolVersion"], "0.3.0");
        let signatures = v["signatures"].as_array().expect("signatures array");
        assert_eq!(signatures.len(), 1);
        assert!(signatures[0]["signature"].is_string());
    }

    #[tokio::test]
    async fn error_envelope_shape() {
        let (reg, store, idem) = mk_state();
        let app = router(reg, store, idem);
        let req = Request::builder()
            .method("GET")
            .uri("/agents/inst-1/v1/tasks/missing-task")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = read_body(resp).await;
        for key in [
            "type",
            "title",
            "status",
            "detail",
            "code",
            "trace_id",
            "instance_id",
        ] {
            assert!(v.get(key).is_some(), "envelope missing field: {key}");
        }
        assert_eq!(v["status"], 404);
        assert_eq!(v["code"], "task.not_found");
    }

    /// #236: mutating routes reject requests that omit the required
    /// `runtime/v1` extension URI in `A2A-Extensions`. The response is
    /// a 400 problem+json with `code: extension.required_not_activated`.
    #[tokio::test]
    async fn send_message_400_when_runtime_ext_missing() {
        let (reg, store, idem) = mk_state();
        let app = router(reg, store, idem);

        let req = Request::builder()
            .method("POST")
            .uri("/agents/inst-1/v1/messages:send")
            .header("content-type", "application/json")
            .body(body_json(sample_message()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "application/problem+json");

        let v = read_body(resp).await;
        assert_eq!(v["status"], 400);
        assert_eq!(v["code"], "extension.required_not_activated");
        assert_eq!(v["title"], "Required extension not activated");
    }
}
