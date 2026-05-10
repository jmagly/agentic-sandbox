//! A2A `tasks/cancel` handler (#210).
//!
//! `POST /agents/{instance_id}/v1/tasks/{tid}:cancel`
//!
//! - 404 `task.not_found` if the task does not exist.
//! - 409 `task.not_cancelable` if the task is already in a terminal state
//!   (`completed`, `failed`, `canceled`, `rejected`).
//! - 200 with the updated Task JSON otherwise; the task is upserted with
//!   state = `canceled`.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::json;

use crate::bindings::rest::{error_response, AppState};
use crate::handlers::push_delivery::DeliveryEvent;
use crate::instance::InstanceExt;
use agentic_management::aiwg_serve::task_store::TaskState;

use super::task_row_to_a2a;

/// Axum handler for `POST /agents/{instance_id}/v1/tasks/{tid}:cancel`.
pub async fn handler(
    Path((instance_id, tid)): Path<(String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    let mut row = match state.store.get_task(&tid) {
        Ok(Some(r)) => r,
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
    };

    if row.state.is_terminal() {
        return error_response(
            StatusCode::CONFLICT,
            "https://agentic-sandbox.aiwg.io/errors/task-not-cancelable",
            "Task not cancelable",
            format!(
                "Task '{}' is in terminal state '{}'",
                tid,
                row.state.as_str()
            ),
            "task.not_cancelable",
            None,
            Some(&instance_id),
        );
    }

    let now = Utc::now();
    row.state = TaskState::Canceled;
    row.updated_at = now;
    row.terminal_at = Some(now);
    row.status_json = json!({
        "state": TaskState::Canceled.as_str(),
        "timestamp": now.to_rfc3339(),
    });

    if let Err(e) = state.store.upsert_task(&row) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to persist canceled task: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        );
    }

    let task_json = task_row_to_a2a(&row);

    // Enqueue a push-notification delivery for the canceled state (#235).
    // The body shape matches the StreamResponse-style envelope used by the
    // delivery worker: { task_id, status_event: { kind, task_id, status } }.
    let status_event = json!({
        "kind": "task_status",
        "task_id": row.task_id,
        "status": task_json["status"].clone(),
    });
    // `try_send` is non-blocking; if the channel is full or closed we
    // log and continue rather than failing the HTTP response.
    if let Err(e) = state.delivery.try_send(DeliveryEvent {
        task_id: row.task_id.clone(),
        status_event,
    }) {
        tracing::warn!(error = %e, task_id = %row.task_id, "cancel_task: push delivery enqueue failed");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(Body::from(task_json.to_string()))
        .unwrap()
        .into_response()
}
