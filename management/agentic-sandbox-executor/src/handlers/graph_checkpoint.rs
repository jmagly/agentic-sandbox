//! Flow graph checkpoint lifecycle recording.
//!
//! The backing runtime performs checkpoint/restore. This endpoint records the
//! durable, digest-bound result on the owning task so graph orchestrators can
//! distinguish creation, restoration, and restore failure.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::{json, Value};

use crate::bindings::rest::{error_response, AppState};
use crate::extensions::{flow_graph, ActivatedExtensions};
use crate::instance::InstanceExt;

use super::task_row_to_a2a;

pub async fn handler(
    Path((instance_id, tid)): Path<(String, String)>,
    State(state): State<AppState>,
    InstanceExt(_ctx): InstanceExt,
    headers: HeaderMap,
    body_bytes: axum::body::Bytes,
) -> Response {
    if !ActivatedExtensions::from_headers(&headers).contains(flow_graph::URI) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "https://agentic-sandbox.aiwg.io/errors/extension-required",
            "Flow graph extension not activated",
            "checkpoint events require flow-graph/v1 activation",
            "flow_graph.extension_required",
            None,
            Some(&instance_id),
        );
    }
    let body: Value = match serde_json::from_slice(&body_bytes) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "https://agentic-sandbox.aiwg.io/errors/invalid-json",
                "Invalid JSON body",
                error.to_string(),
                "request.invalid_json",
                None,
                Some(&instance_id),
            )
        }
    };
    let event = match checkpoint_event(&body) {
        Ok(value) => value,
        Err(detail) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "https://agentic-sandbox.aiwg.io/errors/flow-graph-checkpoint",
                "Invalid checkpoint event",
                detail,
                "flow_graph.invalid_checkpoint",
                None,
                Some(&instance_id),
            )
        }
    };
    let mut row = match state.store.get_task(&tid) {
        Ok(Some(row)) if row.instance_id.as_deref() == Some(&instance_id) => row,
        Ok(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "https://agentic-sandbox.aiwg.io/errors/task-not-found",
                "Task not found",
                format!("Task {tid} not found"),
                "task.not_found",
                None,
                Some(&instance_id),
            )
        }
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "https://agentic-sandbox.aiwg.io/errors/internal",
                "Internal server error",
                error.to_string(),
                "internal.error",
                None,
                Some(&instance_id),
            )
        }
    };
    if row
        .metadata_json
        .as_ref()
        .and_then(|m| m.get(flow_graph::URI))
        .is_none()
    {
        return error_response(
            StatusCode::CONFLICT,
            "https://agentic-sandbox.aiwg.io/errors/not-a-graph-task",
            "Task is not a Flow graph task",
            "checkpoint events cannot be attached to an ordinary A2A task",
            "flow_graph.not_graph_task",
            None,
            Some(&instance_id),
        );
    }
    flow_graph::latest_event(&mut row.metadata_json, event);
    row.updated_at = Utc::now();
    if let Err(error) = state.store.upsert_task(&row) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            error.to_string(),
            "internal.error",
            None,
            Some(&instance_id),
        );
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(Body::from(task_row_to_a2a(&row).to_string()))
        .unwrap()
        .into_response()
}

fn checkpoint_event(body: &Value) -> Result<Value, String> {
    let state = body
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| "state is required".to_string())?;
    if !["created", "restored", "restore_failed"].contains(&state) {
        return Err("state must be created, restored, or restore_failed".to_string());
    }
    let resumability = body
        .get("resumability")
        .and_then(Value::as_str)
        .ok_or_else(|| "resumability is required".to_string())?;
    if !["resumable", "non_resumable", "unknown"].contains(&resumability) {
        return Err("invalid resumability".to_string());
    }
    let mut event = json!({
        "type": "checkpoint",
        "state": state,
        "observed_at": Utc::now().to_rfc3339(),
        "resumability": resumability,
    });
    if state == "restore_failed" {
        let reason = body
            .get("reason")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "restore_failed requires reason".to_string())?;
        event["reason"] = Value::String(reason.to_string());
    } else {
        let checkpoint_id = body
            .get("checkpoint_id")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "checkpoint_id is required".to_string())?;
        let digest = body
            .get("checkpoint_digest")
            .and_then(Value::as_str)
            .ok_or_else(|| "checkpoint_digest is required".to_string())?;
        if digest.len() != 71
            || !digest.starts_with("sha256:")
            || !digest[7..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err("checkpoint_digest must be lowercase sha256:<64 hex>".to_string());
        }
        event["checkpoint_id"] = Value::String(checkpoint_id.to_string());
        event["checkpoint_digest"] = Value::String(digest.to_string());
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_checkpoint_and_restore_failure_shapes() {
        let digest = format!("sha256:{}", "a".repeat(64));
        assert!(checkpoint_event(&json!({
            "state": "created", "resumability": "resumable",
            "checkpoint_id": "cp-1", "checkpoint_digest": digest
        }))
        .is_ok());
        assert!(checkpoint_event(&json!({
            "state": "restore_failed", "resumability": "unknown", "reason": "digest mismatch"
        }))
        .is_ok());
        assert!(checkpoint_event(&json!({
            "state": "restored", "resumability": "resumable", "checkpoint_id": "cp-1"
        }))
        .is_err());
    }
}
