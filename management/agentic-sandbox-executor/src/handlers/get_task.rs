//! A2A `tasks/get` handler (#210).
//!
//! `GET /agents/{instance_id}/v1/tasks/{tid}` → returns the A2A Task JSON
//! or 404 with `application/problem+json` if the task is unknown.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::bindings::rest::{error_response, AppState};
use crate::instance::InstanceExt;

use super::task_row_to_a2a;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTaskQuery {
    history_length: Option<usize>,
}

/// Axum handler for `GET /agents/{instance_id}/v1/tasks/{tid}`.
pub async fn handler(
    Path((instance_id, tid)): Path<(String, String)>,
    Query(query): Query<GetTaskQuery>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    match state.store.get_task_for_instance(&instance_id, &tid) {
        Ok(Some(row)) => {
            let mut task_json = task_row_to_a2a(&row);
            if let Some(limit) = query.history_length {
                if let Some(history) = task_json
                    .get_mut("history")
                    .and_then(|value| value.as_array_mut())
                {
                    let remove = history.len().saturating_sub(limit);
                    history.drain(..remove);
                }
                if limit == 0 {
                    task_json
                        .as_object_mut()
                        .map(|object| object.remove("history"));
                }
            }
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .body(Body::from(task_json.to_string()))
                .unwrap()
                .into_response()
        }
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "https://agentic-sandbox.aiwg.io/errors/task-not-found",
            "Task not found",
            format!("Task '{}' not found", tid),
            "task.not_found",
            None,
            Some(&instance_id),
        ),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to read task: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        ),
    }
}
