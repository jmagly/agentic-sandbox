//! A2A task artifact retrieval handlers.
//!
//! These routes expose artifacts persisted in the executor TaskStore by
//! `messages:send` observers. They are distinct from the legacy
//! management `/api/v1/tasks/{id}/artifacts` filesystem artifact routes.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::bindings::rest::{error_response, AppState};
use crate::instance::InstanceExt;
use crate::store::task_store::ArtifactRow;

pub async fn list(
    Path((instance_id, tid)): Path<(String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    if let Some(resp) = ensure_task_visible(&state, &instance_id, &tid) {
        return resp;
    }

    match state.store.list_artifacts(&tid) {
        Ok(rows) => json_response(json!({
            "task_id": tid,
            "artifacts": rows.into_iter().map(artifact_to_json).collect::<Vec<_>>(),
        })),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to list task artifacts: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        ),
    }
}

pub async fn get(
    Path((instance_id, tid, artifact_id)): Path<(String, String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
) -> Response {
    if let Some(resp) = ensure_task_visible(&state, &instance_id, &tid) {
        return resp;
    }

    match state.store.get_artifact(&tid, &artifact_id) {
        Ok(Some(row)) => json_response(artifact_to_json(row)),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "https://agentic-sandbox.aiwg.io/errors/artifact-not-found",
            "Artifact not found",
            format!("Artifact {artifact_id} not found for task {tid}"),
            "artifact.not_found",
            None,
            Some(&instance_id),
        ),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to read task artifact: {e}"),
            "internal.error",
            None,
            Some(&instance_id),
        ),
    }
}

fn ensure_task_visible(state: &AppState, instance_id: &str, tid: &str) -> Option<Response> {
    match state.store.get_task(tid) {
        Ok(Some(row)) if row.instance_id.as_deref() == Some(instance_id) => None,
        Ok(Some(_)) | Ok(None) => Some(error_response(
            StatusCode::NOT_FOUND,
            "https://agentic-sandbox.aiwg.io/errors/task-not-found",
            "Task not found",
            format!("Task {tid} not found"),
            "task.not_found",
            None,
            Some(instance_id),
        )),
        Err(e) => Some(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to read task: {e}"),
            "internal.error",
            None,
            Some(instance_id),
        )),
    }
}

fn artifact_to_json(row: ArtifactRow) -> Value {
    json!({
        "artifact_id": row.artifact_id,
        "task_id": row.task_id,
        "created_at": row.created_at.to_rfc3339(),
        "artifact": row.artifact_json,
    })
}

fn json_response(body: Value) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(Body::from(body.to_string()))
        .unwrap()
        .into_response()
}
