//! A2A `messages:send` handler (#210, refactored in #213).
//!
//! Accepts an A2A `Message` envelope, creates a new task in state
//! `submitted`, persists it via [`TaskStore`], and returns the Task JSON
//! with status 202 Accepted.
//!
//! Idempotency, runtime metadata injection, multi-tenant span tagging,
//! and HITL envelope checks all run through the
//! [`crate::extensions::ExtensionRegistry`] (replaces the inline
//! idempotency logic that previously lived here). The wire response
//! shape — status code, `Location` header, `A2A-Extensions` echo,
//! `Idempotent-Replayed` on replay — is unchanged.
//!
//! [`TaskStore`]: crate::store::task_store::TaskStore

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{HeaderValue, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::bindings::rest::{error_response, AppState};
use crate::extensions::{
    idempotency::URI as IDEMPOTENCY_URI, ActivatedExtensions, ExtensionOutcome, PostResponseCtx,
    PreRequestCtx,
};
use crate::handlers::push_delivery::DeliveryEvent;
use crate::instance::InstanceExt;
use crate::store::task_store::{TaskRow, TaskState};

use super::task_row_to_a2a;

/// Axum handler for `POST /agents/{instance_id}/v1/messages:send`.
pub async fn handler(
    Path((instance_id,)): Path<(String,)>,
    State(state): State<AppState>,
    InstanceExt(inst_ctx): InstanceExt,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let body = match body {
        Some(Json(v)) => v,
        None => Value::Object(Default::default()),
    };

    if !body.is_object() || !body.get("message").map(|m| m.is_object()).unwrap_or(false) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "https://agentic-sandbox.aiwg.io/errors/invalid-params",
            "Invalid params",
            "Request body must be a JSON object with a `message` object",
            "request.invalid_params",
            None,
            Some(&instance_id),
        );
    }

    // #268: fail fast when the backing runtime can't service work.
    // The previous behavior accepted the message, persisted a task in
    // `submitted` state, and left it stalled because no agent was
    // connected (e.g. container exited at provision time). 503 lets
    // orchestrators retry or surface degraded state instead of polling
    // a phantom task forever.
    if !inst_ctx.is_ready() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "https://agentic-sandbox.aiwg.io/errors/runtime-unavailable",
            "Runtime not ready",
            "The backing runtime for this instance is not currently \
             servicing requests. Check the instance state in \
             /api/v2/admin/instances; the runtime may have failed to \
             start or has dropped its management connection."
                .to_string(),
            "runtime.not_ready",
            None,
            Some(&instance_id),
        );
    }

    let activated = ActivatedExtensions::from_headers(&headers);
    let echoed = state.extensions.echo_activated(&activated);

    let message_id = body
        .get("message")
        .and_then(|m| m.get("messageId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // --- pre_request: extensions may short-circuit ---
    let pre_ctx = PreRequestCtx {
        activated: &activated,
        task_id: None,
        message_id: message_id.as_deref(),
        request_body: &body,
    };
    match state.extensions.pre_request(&pre_ctx) {
        ExtensionOutcome::Continue => {}
        ExtensionOutcome::Replay {
            status,
            body: cached,
        } => {
            // Replay path: idempotent re-send. Honor the cached status,
            // tag with `Idempotent-Replayed: true`, mirror activated
            // extensions back per A2A §3.4.
            return build_replay_response(
                StatusCode::from_u16(status).unwrap_or(StatusCode::ACCEPTED),
                cached,
                &echoed,
            );
        }
        ExtensionOutcome::Reject {
            status,
            body: err_body,
        } => {
            return error_response(
                StatusCode::from_u16(status).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY),
                err_body
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("https://agentic-sandbox.aiwg.io/errors/extension"),
                err_body
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Extension rejected request"),
                err_body
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                err_body
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("extension.rejected"),
                None,
                Some(&instance_id),
            );
        }
    }

    // --- main handler: create the task ---
    let now = Utc::now();
    let task_id = Uuid::now_v7().to_string();
    let context_id = body
        .get("message")
        .and_then(|m| m.get("contextId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let status_json = json!({
        "state": TaskState::Submitted.as_str(),
        "timestamp": now.to_rfc3339(),
    });

    let row = TaskRow {
        task_id: task_id.clone(),
        context_id,
        // #269: persist owning instance so list_tasks can scope by path id.
        instance_id: Some(instance_id.clone()),
        state: TaskState::Submitted,
        fail_kind: None,
        status_json,
        metadata_json: None,
        created_at: now,
        updated_at: now,
        terminal_at: None,
    };

    if let Err(e) = state.store.upsert_task(&row) {
        tracing::error!(error = %e, task_id, "failed to persist new task");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to persist task: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        );
    }

    // #269: hand the message off to the dispatch seam. The previous
    // path stopped here, returning 202 + submitted while nothing
    // forwarded work to the runtime — tasks sat in `submitted`
    // indefinitely.
    //
    // - Real dispatch impl + agent connected → task transitions to
    //   `working` and we return 202; observers drive further progress.
    // - No dispatch impl (executor-only / NoOp) → task transitions to
    //   `failed/infrastructure` with `dispatch.unimplemented` and we
    //   return 503. Truthful degraded response per acceptance criteria.
    // - Runtime unreachable → task transitions to `failed/infrastructure`
    //   with `runtime.unavailable`, 503.
    let mut row_after = row.clone();
    let dispatch_outcome = state
        .message_dispatch
        .dispatch(inst_ctx.as_ref(), &task_id, &body)
        .await;
    let dispatch_error = match dispatch_outcome {
        Ok(crate::bindings::message_dispatch::DispatchOutcome::Accepted) => {
            let updated = Utc::now();
            row_after.state = TaskState::Working;
            row_after.status_json = json!({
                "state": TaskState::Working.as_str(),
                "timestamp": updated.to_rfc3339(),
            });
            row_after.updated_at = updated;
            if let Err(e) = state.store.upsert_task(&row_after) {
                tracing::warn!(error = %e, task_id, "could not record dispatch transition");
            }
            None
        }
        Err(err) => {
            let updated = Utc::now();
            row_after.state = TaskState::Failed;
            row_after.fail_kind = Some(crate::store::task_store::FailKind::Infrastructure);
            row_after.status_json = json!({
                "state": TaskState::Failed.as_str(),
                "timestamp": updated.to_rfc3339(),
                "error": err.to_string(),
            });
            row_after.updated_at = updated;
            row_after.terminal_at = Some(updated);
            if let Err(e) = state.store.upsert_task(&row_after) {
                tracing::warn!(error = %e, task_id, "could not record dispatch failure");
            }
            Some(err)
        }
    };

    let mut task_json = task_row_to_a2a(&row_after);

    // --- post_response: extensions may mutate the body ---
    let status = match &dispatch_error {
        None => StatusCode::ACCEPTED,
        Some(crate::bindings::message_dispatch::DispatchError::DispatchFailed(_)) => {
            StatusCode::BAD_GATEWAY
        }
        Some(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    let mut post_ctx = PostResponseCtx {
        activated: &activated,
        task_id: &task_id,
        status: status.as_u16(),
        response_body: &mut task_json,
        // #268: thread the per-instance context so the runtime extension
        // reports the actual runtime kind/host/instance_id instead of
        // the registry-wide defaults.
        instance: Some(inst_ctx.as_ref()),
    };
    state.extensions.post_response(&mut post_ctx);

    // Record into idempotency cache AFTER post_response mutated the
    // body, so a replay returns the same body the original client saw.
    if activated.contains(IDEMPOTENCY_URI) {
        if let Some(mid) = &message_id {
            if let Err(e) = state.idem.record(mid, &body, status.as_u16(), &task_json) {
                tracing::warn!(error = %e, "failed to record idempotency entry");
            }
        }
    }

    // Enqueue a push-notification delivery for the initial submission
    // (#235). Subscribers registered against this task (if any are added
    // out-of-band) will see this first state transition.
    let status_event = json!({
        "kind": "task_status",
        "task_id": task_id,
        "status": task_json["status"].clone(),
    });
    if let Err(e) = state.delivery.try_send(DeliveryEvent {
        task_id: task_id.clone(),
        status_event,
    }) {
        tracing::warn!(error = %e, task_id = %task_id, "send_message: push delivery enqueue failed");
    }

    // #269: if dispatch failed, return a 7807 problem+json envelope
    // (instead of a task body) so callers don't poll a doomed task.
    if let Some(err) = dispatch_error {
        let (code, title) = match &err {
            crate::bindings::message_dispatch::DispatchError::NotImplemented => (
                "dispatch.unimplemented",
                "Runtime dispatch unimplemented",
            ),
            crate::bindings::message_dispatch::DispatchError::RuntimeUnavailable(_, _) => {
                ("runtime.unavailable", "Runtime unavailable")
            }
            crate::bindings::message_dispatch::DispatchError::DispatchFailed(_) => {
                ("dispatch.failed", "Dispatch failed")
            }
        };
        return error_response(
            status,
            "https://agentic-sandbox.aiwg.io/errors/dispatch",
            title,
            err.to_string(),
            code,
            None,
            Some(&instance_id),
        );
    }

    let location = format!("/agents/{}/v1/tasks/{}", instance_id, task_id);
    build_fresh_response(status, task_json, &echoed, Some(location))
}

fn build_fresh_response(
    status: StatusCode,
    body: Value,
    echoed: &ActivatedExtensions,
    location: Option<String>,
) -> Response {
    let body_str = body.to_string();
    let mut resp = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    if let Some(loc) = location {
        if let Ok(hv) = HeaderValue::from_str(&loc) {
            resp = resp.header(LOCATION, hv);
        }
    }
    if !echoed.as_slice().is_empty() {
        resp = resp.header("A2A-Extensions", echoed.to_response_header());
    }
    resp.body(Body::from(body_str)).unwrap().into_response()
}

fn build_replay_response(
    status: StatusCode,
    body: Value,
    echoed: &ActivatedExtensions,
) -> Response {
    let body_str = body.to_string();
    let mut resp = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .header("Idempotent-Replayed", HeaderValue::from_static("true"));

    if !echoed.as_slice().is_empty() {
        resp = resp.header("A2A-Extensions", echoed.to_response_header());
    }
    resp.body(Body::from(body_str)).unwrap().into_response()
}
