//! A2A `messages:send` handler (#210).
//!
//! Accepts an A2A `Message` envelope, creates a new task in state
//! `submitted`, persists it via [`TaskStore`], and returns the Task JSON
//! with status 202 Accepted. Honors the `idempotency/v1` extension when
//! activated via the `A2A-Extensions` request header.
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

use crate::bindings::rest::{
    activated_extensions_header, error_response, idempotency_activated, AppState,
    EXT_IDEMPOTENCY_URI,
};
use crate::instance::InstanceExt;
use agentic_management::aiwg_serve::idempotency::IdempotencyOutcome;
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

    let idem_active = idempotency_activated(&headers);
    let message_id = body
        .get("message")
        .and_then(|m| m.get("messageId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if idem_active {
        if let Some(mid) = &message_id {
            match state.idem.check(mid, &body) {
                Ok(IdempotencyOutcome::Replay { status, body: cached }) => {
                    return build_response(
                        StatusCode::from_u16(status).unwrap_or(StatusCode::ACCEPTED),
                        cached,
                        idem_active,
                        true,
                        None,
                    );
                }
                Ok(IdempotencyOutcome::Collision) => {
                    return error_response(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "https://agentic-sandbox.aiwg.io/errors/idempotency-collision",
                        "Idempotency key reused with different payload",
                        "The provided messageId was previously used with a different request body",
                        "idempotency.key_reused",
                        None,
                        Some(&instance_id),
                    );
                }
                Ok(IdempotencyOutcome::Fresh) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "idempotency check failed; proceeding fresh");
                }
            }
        }
    }

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

    let task_json = task_row_to_a2a(&row);
    let location = format!("/agents/{}/v1/tasks/{}", instance_id, task_id);

    if idem_active {
        if let Some(mid) = &message_id {
            if let Err(e) = state.idem.record(mid, &body, 202, &task_json) {
                tracing::warn!(error = %e, "failed to record idempotency entry");
            }
        }
    }

    build_response(
        StatusCode::ACCEPTED,
        task_json,
        idem_active,
        false,
        Some(location),
    )
}

fn build_response(
    status: StatusCode,
    body: Value,
    idem_active: bool,
    replayed: bool,
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
    if idem_active {
        if let Some(hv) = activated_extensions_header(&[EXT_IDEMPOTENCY_URI]) {
            resp = resp.header("A2A-Extensions", hv);
        }
    }
    if replayed {
        resp = resp.header("Idempotent-Replayed", HeaderValue::from_static("true"));
    }
    resp.body(Body::from(body_str)).unwrap().into_response()
}
