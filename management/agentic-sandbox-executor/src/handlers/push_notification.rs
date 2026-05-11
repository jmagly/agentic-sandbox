//! A2A push-notification config CRUD handlers (#211).
//!
//! Endpoints mounted under `/agents/{instance_id}/v1/tasks/{tid}/`:
//!
//! | Method+Path                              | Handler            |
//! |------------------------------------------|--------------------|
//! | POST   `/pushNotificationConfigs`        | [`create_config`]  |
//! | GET    `/pushNotificationConfigs/{cid}`  | [`get_config`]     |
//! | GET    `/pushNotificationConfigs`        | [`list_configs`]   |
//! | DELETE `/pushNotificationConfigs/{cid}`  | [`delete_config`]  |
//!
//! Persistence is delegated to [`TaskStore::put_push_config`] /
//! `get_push_config` / `list_push_configs` / `delete_push_config` (added in
//! W2.1, issue #205). Config IDs are UUIDv7 so they sort lexicographically
//! by creation time.
//!
//! ## Auth model on the wire
//!
//! Clients register an authentication descriptor with each config:
//!
//! ```json
//! { "type": "bearer|hmac|none", "secret": "<bearer-token | hmac-secret>" }
//! ```
//!
//! Secrets are persisted in `auth_json` but are **never** echoed back on
//! read. Responses replace the secret with `{ "type": "...", "configured": true }`.
//! Tests assert this redaction.
//!
//! ## Error codes
//!
//! - `task.not_found` (404) — the task referenced by `{tid}` does not exist.
//! - `push.config_not_found` (404) — the config referenced by `{cid}` does
//!   not exist (or belongs to a different task — see cross-task isolation).
//! - `push.invalid_url` (400) — the request body is missing `url` or `url`
//!   is empty. (Scheme validation is permissive in tests; production
//!   deployments should enforce HTTPS via deployment policy.)

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::bindings::rest::{error_response, AppState};
use crate::instance::InstanceExt;
use crate::store::task_store::PushNotificationConfigRow;

// ---------- shape helpers ----------

/// Build the wire representation of a persisted config, with `auth.secret`
/// redacted to `{ "type": "...", "configured": true }`.
fn config_to_wire(row: &PushNotificationConfigRow) -> Value {
    let auth_out = match &row.auth_json {
        Some(auth) => {
            let kind = auth
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("none")
                .to_string();
            let has_secret = auth
                .get("secret")
                .map(|v| !v.is_null() && v.as_str().map(|s| !s.is_empty()).unwrap_or(false))
                .unwrap_or(false);
            json!({
                "type": kind,
                "configured": has_secret,
            })
        }
        None => json!({ "type": "none", "configured": false }),
    };
    json!({
        "id": row.config_id,
        "task_id": row.task_id,
        "url": row.url,
        "created_at": row.created_at.to_rfc3339(),
        "auth": auth_out,
    })
}

/// Parse and validate the POST body. Returns `(url, auth_json)` or an error
/// `Response` ready to return to the client.
fn parse_create_body(
    body: &Value,
    instance_id: &str,
) -> Result<(String, Option<Value>), Response> {
    let url = body
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let url = match url {
        Some(u) => u,
        None => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "https://agentic-sandbox.aiwg.io/errors/push-invalid-url",
                "Invalid push subscriber URL",
                "Request body must include a non-empty `url` field",
                "push.invalid_url",
                None,
                Some(instance_id),
            ));
        }
    };

    // `auth` is optional. If present, store as-is — the secret is persisted
    // in auth_json and redacted on read by `config_to_wire`.
    let auth = body.get("auth").cloned();
    Ok((url, auth))
}

// ---------- handlers ----------

/// `POST /agents/{instance_id}/v1/tasks/{tid}/pushNotificationConfigs`
pub async fn create_config(
    Path((instance_id, tid)): Path<(String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
    body: Option<axum::Json<Value>>,
) -> Response {
    // Body is required; treat missing body as invalid_url since `url` is
    // mandatory.
    let body = match body {
        Some(axum::Json(v)) => v,
        None => Value::Null,
    };

    let (url, auth) = match parse_create_body(&body, &instance_id) {
        Ok(parts) => parts,
        Err(resp) => return resp,
    };

    // Verify task exists.
    match state.store.get_task(&tid) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "https://agentic-sandbox.aiwg.io/errors/task-not-found",
                "Task not found",
                format!("Task '{}' not found", tid),
                "task.not_found",
                None,
                Some(&instance_id),
            );
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "https://agentic-sandbox.aiwg.io/errors/internal",
                "Internal server error",
                format!("Failed to read task: {e}"),
                "internal.error",
                None,
                Some(&instance_id),
            );
        }
    }

    let cfg = PushNotificationConfigRow {
        config_id: Uuid::now_v7().to_string(),
        task_id: tid.clone(),
        url,
        auth_json: auth,
        created_at: Utc::now(),
    };

    if let Err(e) = state.store.put_push_config(&cfg) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to persist push config: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        );
    }

    let wire = config_to_wire(&cfg);
    Response::builder()
        .status(StatusCode::CREATED)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(Body::from(wire.to_string()))
        .unwrap()
        .into_response()
}

/// `GET /agents/{instance_id}/v1/tasks/{tid}/pushNotificationConfigs/{cid}`
pub async fn get_config(
    Path((instance_id, tid, cid)): Path<(String, String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    match state.store.get_push_config(&cid) {
        Ok(Some(row)) if row.task_id == tid => {
            let wire = config_to_wire(&row);
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .body(Body::from(wire.to_string()))
                .unwrap()
                .into_response()
        }
        // Config belongs to a different task → treat as not_found from this
        // task's perspective (cross-task isolation).
        Ok(Some(_)) | Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "https://agentic-sandbox.aiwg.io/errors/push-config-not-found",
            "Push notification config not found",
            format!("Config '{}' not found for task '{}'", cid, tid),
            "push.config_not_found",
            None,
            Some(&instance_id),
        ),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to read push config: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        ),
    }
}

/// `GET /agents/{instance_id}/v1/tasks/{tid}/pushNotificationConfigs`
pub async fn list_configs(
    Path((instance_id, tid)): Path<(String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    match state.store.list_push_configs(&tid) {
        Ok(rows) => {
            let items: Vec<Value> = rows.iter().map(config_to_wire).collect();
            let body = json!({ "configs": items });
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .body(Body::from(body.to_string()))
                .unwrap()
                .into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to list push configs: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        ),
    }
}

/// `DELETE /agents/{instance_id}/v1/tasks/{tid}/pushNotificationConfigs/{cid}`
pub async fn delete_config(
    Path((instance_id, tid, cid)): Path<(String, String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    // First check the config exists AND belongs to this task. We do a get
    // to enforce cross-task isolation: a DELETE for a cid under a foreign
    // task must 404, not silently delete.
    match state.store.get_push_config(&cid) {
        Ok(Some(row)) if row.task_id == tid => {
            // fall through and delete
        }
        Ok(Some(_)) | Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "https://agentic-sandbox.aiwg.io/errors/push-config-not-found",
                "Push notification config not found",
                format!("Config '{}' not found for task '{}'", cid, tid),
                "push.config_not_found",
                None,
                Some(&instance_id),
            );
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "https://agentic-sandbox.aiwg.io/errors/internal",
                "Internal server error",
                format!("Failed to read push config: {e}"),
                "internal.error",
                None,
                Some(&instance_id),
            );
        }
    }

    match state.store.delete_push_config(&cid) {
        Ok(_) => Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap()
            .into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to delete push config: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        ),
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::rest::router;
    use crate::instance::{InstanceContext, InstanceRegistry, RuntimeKind};
    use crate::store::idempotency::IdempotencyCache;
    use crate::store::task_store::{TaskRow, TaskState, TaskStore};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use chrono::Utc;
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// `runtime/v1` extension URI; required on mutating routes per #236.
    const EXT_RUNTIME_URI: &str = crate::extensions::runtime::URI;

    /// Build a request::Builder pre-populated with the required
    /// `A2A-Extensions: runtime/v1` header for mutating routes.
    fn test_request_with_runtime_ext() -> axum::http::request::Builder {
        Request::builder().header("A2A-Extensions", EXT_RUNTIME_URI)
    }

    fn mk_state() -> (InstanceRegistry, Arc<TaskStore>, Arc<IdempotencyCache>) {
        let reg = InstanceRegistry::new();
        let ctx = Arc::new(InstanceContext::new(
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

    fn seed_task(store: &TaskStore, tid: &str) {
        let now = Utc::now();
        store
            .upsert_task(&TaskRow {
                task_id: tid.to_string(),
                context_id: None,
                state: TaskState::Submitted,
                fail_kind: None,
                status_json: serde_json::json!({"state": "submitted"}),
                metadata_json: None,
                created_at: now,
                updated_at: now,
                terminal_at: None,
            })
            .unwrap();
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

    fn body_json(v: Value) -> Body {
        Body::from(serde_json::to_vec(&v).unwrap())
    }

    #[tokio::test]
    async fn create_config_persists() {
        let (reg, store, idem) = mk_state();
        seed_task(&store, "t-1");
        let app = router(reg, store.clone(), idem);

        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs")
            .header("content-type", "application/json")
            .body(body_json(serde_json::json!({
                "url": "https://subscriber.example.test/hook",
                "auth": { "type": "bearer", "secret": "super-secret-token" },
            })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let v = read_body(resp).await;
        let cid = v["id"].as_str().expect("id present").to_string();
        assert_eq!(v["task_id"], "t-1");
        assert_eq!(v["url"], "https://subscriber.example.test/hook");
        // Secret MUST be redacted.
        assert_eq!(v["auth"]["type"], "bearer");
        assert_eq!(v["auth"]["configured"], true);
        assert!(
            v["auth"].get("secret").is_none(),
            "secret must not be echoed; got: {}",
            v["auth"]
        );

        // GET round-trip — secret still redacted.
        let req = test_request_with_runtime_ext()
            .method("GET")
            .uri(format!(
                "/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs/{}",
                cid
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body(resp).await;
        assert_eq!(v["id"], cid);
        assert_eq!(v["auth"]["type"], "bearer");
        assert!(v["auth"].get("secret").is_none());

        // Persisted secret IS in TaskStore (so the delivery worker can sign).
        let row = store.get_push_config(&cid).unwrap().unwrap();
        let auth = row.auth_json.unwrap();
        assert_eq!(auth["secret"], "super-secret-token");
    }

    #[tokio::test]
    async fn create_config_404_when_task_missing() {
        let (reg, store, idem) = mk_state();
        let app = router(reg, store, idem);

        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/tasks/no-such-task/pushNotificationConfigs")
            .header("content-type", "application/json")
            .body(body_json(serde_json::json!({
                "url": "https://subscriber.example.test/hook"
            })))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = read_body(resp).await;
        assert_eq!(v["code"], "task.not_found");
    }

    #[tokio::test]
    async fn create_config_400_when_url_missing() {
        let (reg, store, idem) = mk_state();
        seed_task(&store, "t-1");
        let app = router(reg, store, idem);

        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs")
            .header("content-type", "application/json")
            .body(body_json(serde_json::json!({})))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = read_body(resp).await;
        assert_eq!(v["code"], "push.invalid_url");
    }

    #[tokio::test]
    async fn list_configs_for_task() {
        let (reg, store, idem) = mk_state();
        seed_task(&store, "t-1");
        let app = router(reg, store.clone(), idem);

        for i in 0..3 {
            let req = test_request_with_runtime_ext()
                .method("POST")
                .uri("/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs")
                .header("content-type", "application/json")
                .body(body_json(serde_json::json!({
                    "url": format!("https://subscriber.example.test/hook/{}", i),
                })))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
        }

        let req = test_request_with_runtime_ext()
            .method("GET")
            .uri("/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body(resp).await;
        let arr = v["configs"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn delete_config_removes() {
        let (reg, store, idem) = mk_state();
        seed_task(&store, "t-1");
        let app = router(reg, store.clone(), idem);

        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs")
            .header("content-type", "application/json")
            .body(body_json(serde_json::json!({
                "url": "https://subscriber.example.test/hook",
            })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let v = read_body(resp).await;
        let cid = v["id"].as_str().unwrap().to_string();

        let req = test_request_with_runtime_ext()
            .method("DELETE")
            .uri(format!(
                "/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs/{}",
                cid
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let req = test_request_with_runtime_ext()
            .method("GET")
            .uri(format!(
                "/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs/{}",
                cid
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_config_404_when_unknown() {
        let (reg, store, idem) = mk_state();
        seed_task(&store, "t-1");
        let app = router(reg, store, idem);

        let req = test_request_with_runtime_ext()
            .method("DELETE")
            .uri("/agents/inst-1/v1/tasks/t-1/pushNotificationConfigs/does-not-exist")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = read_body(resp).await;
        assert_eq!(v["code"], "push.config_not_found");
    }

    #[tokio::test]
    async fn cross_task_isolation() {
        let (reg, store, idem) = mk_state();
        seed_task(&store, "t-a");
        seed_task(&store, "t-b");
        let app = router(reg, store.clone(), idem);

        // Create config under t-a.
        let req = test_request_with_runtime_ext()
            .method("POST")
            .uri("/agents/inst-1/v1/tasks/t-a/pushNotificationConfigs")
            .header("content-type", "application/json")
            .body(body_json(serde_json::json!({
                "url": "https://subscriber.example.test/hook",
            })))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let v = read_body(resp).await;
        let cid = v["id"].as_str().unwrap().to_string();

        // Listing under t-b shows nothing.
        let req = test_request_with_runtime_ext()
            .method("GET")
            .uri("/agents/inst-1/v1/tasks/t-b/pushNotificationConfigs")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let v = read_body(resp).await;
        assert!(v["configs"].as_array().unwrap().is_empty());

        // GET under wrong task → 404.
        let req = test_request_with_runtime_ext()
            .method("GET")
            .uri(format!(
                "/agents/inst-1/v1/tasks/t-b/pushNotificationConfigs/{}",
                cid
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let v = read_body(resp).await;
        assert_eq!(v["code"], "push.config_not_found");

        // DELETE under wrong task → 404 (no silent deletion).
        let req = test_request_with_runtime_ext()
            .method("DELETE")
            .uri(format!(
                "/agents/inst-1/v1/tasks/t-b/pushNotificationConfigs/{}",
                cid
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Config still exists under t-a.
        assert!(store.get_push_config(&cid).unwrap().is_some());
    }

    /// #235: verify the cancel_task handler emits a DeliveryEvent on the
    /// delivery mpsc when a task transitions to `canceled`. Builds a
    /// minimal AppState with the receiver held in-test (rather than the
    /// spawned worker swallowing it) so we can assert on the channel
    /// directly with `try_recv`.
    #[tokio::test]
    async fn cancel_task_emits_delivery_event() {
        use crate::bindings::pty_ws::SessionRegistry;
        use crate::bindings::rest::AppState;
        use crate::extensions::build_default_registry;
        use crate::instance::InstanceLayer;
        use axum::routing::post;
        use axum::Router;

        let (reg, store, idem) = mk_state();
        seed_task(&store, "t-cancel");

        // Provision a push-notification config so list_push_configs is
        // non-empty (exercises the path even though we intercept at the
        // channel, not at the HTTP subscriber).
        store
            .put_push_config(&PushNotificationConfigRow {
                config_id: "c-1".into(),
                task_id: "t-cancel".into(),
                url: "https://subscriber.example.test/hook".into(),
                auth_json: None,
                created_at: Utc::now(),
            })
            .unwrap();

        let extensions = Arc::new(build_default_registry(
            idem.clone(),
            RuntimeKind::Vm,
            "agentic-dev".into(),
            "executor.local".into(),
        ));
        let (delivery_tx, mut delivery_rx) = tokio::sync::mpsc::channel(16);
        let state = AppState {
            delivery: delivery_tx,
            extensions,
            idem,
            pty_bridge: Arc::new(crate::bindings::pty_bridge::NoOpPtyBridge),
            session_registry: Arc::new(SessionRegistry::new()),
            store: store.clone(),
        };

        let app = Router::new()
            .route(
                "/agents/{instance_id}/v1/tasks/{tid}/cancel",
                post(crate::handlers::cancel_task::handler),
            )
            .layer(InstanceLayer::new(reg))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/agents/inst-1/v1/tasks/t-cancel/cancel")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Yield once so the handler's `try_send` is fully observable on
        // the receiver side. `try_send` is synchronous so this is
        // belt-and-suspenders.
        tokio::task::yield_now().await;

        let ev = delivery_rx
            .try_recv()
            .expect("cancel_task should enqueue a DeliveryEvent");
        assert_eq!(ev.task_id, "t-cancel");
        assert_eq!(ev.status_event["kind"], "task_status");
        assert_eq!(ev.status_event["task_id"], "t-cancel");
        assert_eq!(ev.status_event["status"]["state"], "canceled");

        // Only one event for one transition.
        assert!(delivery_rx.try_recv().is_err());
    }
}
