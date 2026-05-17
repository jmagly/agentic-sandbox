//! A2A `messages:stream` handler (#210 — **SSE stub**).
//!
//! `POST /agents/{instance_id}/v1/messages:stream`
//!
//! Creates a new task identically to [`super::send_message`] and returns
//! an `text/event-stream` containing exactly one `event: task` carrying
//! the Task JSON, then closes.
//!
//! **Stub**: real per-event streaming (artifacts, progress, status
//! transitions) wires in #213 once the in-process event source lands.
//! Documented here so callers know not to expect more than one event
//! from the v2.0 implementation.

use std::convert::Infallible;

use async_stream::stream;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use futures_util::Stream;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::bindings::rest::{error_response, AppState};
use crate::instance::InstanceExt;
use crate::store::task_store::{TaskRow, TaskState};

use super::task_row_to_a2a;

/// Axum handler for `POST /agents/{instance_id}/v1/messages:stream`.
pub async fn handler(
    Path((instance_id,)): Path<(String,)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
    _headers: HeaderMap,
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
    let stream = stub_stream(task_json);
    Sse::new(stream).into_response()
}

fn stub_stream(task_json: Value) -> impl Stream<Item = Result<Event, Infallible>> {
    stream! {
        yield Ok(Event::default().event("task").data(task_json.to_string()));
    }
}
