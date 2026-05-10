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
//! [`TaskStore`]: agentic_management::aiwg_serve::task_store::TaskStore

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
use crate::instance::InstanceExt;
use agentic_management::aiwg_serve::task_store::{TaskRow, TaskState};

use super::task_row_to_a2a;

/// Axum handler for `POST /agents/{instance_id}/v1/messages:send`.
pub async fn handler(
    Path((instance_id,)): Path<(String,)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
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

    let mut task_json = task_row_to_a2a(&row);

    // --- post_response: extensions may mutate the body ---
    let status = StatusCode::ACCEPTED;
    let mut post_ctx = PostResponseCtx {
        activated: &activated,
        task_id: &task_id,
        status: status.as_u16(),
        response_body: &mut task_json,
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
