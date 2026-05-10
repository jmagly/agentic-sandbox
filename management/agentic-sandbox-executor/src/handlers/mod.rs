//! A2A method handlers.
//!
//! One module per A2A method. Filled in by #210 (most) and #211
//! (push_notification). Handlers are axum endpoints: they take the
//! [`crate::instance::InstanceExt`] extractor (resolved by the
//! [`crate::instance::InstanceLayer`] middleware), the shared
//! [`crate::bindings::rest::AppState`], and the request body, then return
//! either an A2A-shaped JSON response or an RFC 7807 problem+json envelope.

pub mod cancel_task;
pub mod get_extended_agent_card;
pub mod get_task;
pub mod list_tasks;
pub mod push_notification;
pub mod send_message;
pub mod send_streaming_message;
pub mod subscribe_to_task;

use agentic_management::aiwg_serve::task_store::{TaskRow, TaskState};
use serde_json::{json, Value};

/// Convert a stored [`TaskRow`] into the A2A Task wire shape.
///
/// The persisted `status_json` is the authoritative status; we merge in
/// the canonical `state` string to ensure consistency even if upstream
/// callers forgot to populate the JSON.
pub(crate) fn task_row_to_a2a(row: &TaskRow) -> Value {
    let state_str = row.state.as_str();
    let mut status = row.status_json.clone();
    if let Some(obj) = status.as_object_mut() {
        obj.insert("state".to_string(), Value::String(state_str.to_string()));
        obj.entry("timestamp")
            .or_insert_with(|| Value::String(row.updated_at.to_rfc3339()));
    } else {
        status = json!({
            "state": state_str,
            "timestamp": row.updated_at.to_rfc3339(),
        });
    }

    let mut task = json!({
        "id": row.task_id,
        "kind": "task",
        "status": status,
    });
    if let Some(ctx) = &row.context_id {
        task["contextId"] = Value::String(ctx.clone());
    }
    if let Some(meta) = &row.metadata_json {
        task["metadata"] = meta.clone();
    }
    task
}

/// Parse a `state=` query string value into [`TaskState`].
pub(crate) fn parse_state(s: &str) -> Option<TaskState> {
    use std::str::FromStr;
    TaskState::from_str(s).ok()
}
