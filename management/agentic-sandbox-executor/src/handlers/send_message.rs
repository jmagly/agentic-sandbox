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
use axum::extract::{Extension, Path, State};
use axum::http::header::{HeaderValue, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::bindings::rest::{error_response, AppState};
use crate::extensions::{
    flow_graph,
    hitl_prompt::{validate_hitl_response, URI as HITL_PROMPT_URI},
    idempotency::{versioned_request_body, URI as IDEMPOTENCY_URI},
    ActivatedExtensions, ExtensionOutcome, PostResponseCtx, PreRequestCtx,
};
use crate::handlers::push_delivery::DeliveryEvent;
use crate::instance::InstanceExt;
use crate::protocol::ProtocolVersion;
use crate::store::task_store::{TaskRow, TaskState};

use super::task_row_to_a2a;

/// Axum handler for `POST /agents/{instance_id}/v1/messages:send`.
pub async fn handler(
    Path((instance_id,)): Path<(String,)>,
    State(state): State<AppState>,
    InstanceExt(inst_ctx): InstanceExt,
    Extension(protocol_version): Extension<ProtocolVersion>,
    headers: HeaderMap,
    body_bytes: axum::body::Bytes,
) -> Response {
    // Parse the request body manually so that malformed JSON returns an
    // application/problem+json envelope instead of axum's default
    // text/plain 400. Empty body → empty object (back-compat with
    // earlier `Option<Json<Value>>` signature).
    let body: Value = if body_bytes.is_empty() {
        Value::Object(Default::default())
    } else {
        match serde_json::from_slice(&body_bytes) {
            Ok(v) => v,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "https://agentic-sandbox.aiwg.io/errors/invalid-json",
                    "Invalid JSON body",
                    format!("Failed to parse request body as JSON: {e}"),
                    "request.invalid_json",
                    None,
                    Some(&instance_id),
                );
            }
        }
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

    if let Some(response) = hitl_response_payload(&body) {
        return handle_hitl_response(&state, &instance_id, &body, response).await;
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
        protocol_version: protocol_version.as_header_value(),
    };
    match state.extensions.pre_request(&pre_ctx) {
        ExtensionOutcome::Continue => {}
        ExtensionOutcome::Replay {
            status,
            body: cached,
        } => {
            // Replay path: idempotent re-send. Honor the cached status,
            // tag with `Idempotent-Replayed: true`, mirror activated
            // extensions back per A2A §3.4. Bump the labeled hit counter
            // surfaced as aiwg_idempotency_hit_total{operation="messages:send"}
            // (issue #206 follow-up).
            state.idem.metrics().record_hit_for_op("messages:send");
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

    // --- main handler: create or continue the task ---
    let now = Utc::now();
    let requested_task_id = body
        .get("message")
        .and_then(|message| message.get("taskId"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty());
    let existing = if let Some(task_id) = requested_task_id {
        match state.store.get_task_for_instance(&instance_id, task_id) {
            Ok(Some(row)) => Some(row),
            Ok(None) => {
                return error_response(
                    StatusCode::NOT_FOUND,
                    "https://agentic-sandbox.aiwg.io/errors/task-not-found",
                    "Task not found",
                    "The referenced task does not exist".to_string(),
                    "task.not_found",
                    None,
                    Some(&instance_id),
                );
            }
            Err(error) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "https://agentic-sandbox.aiwg.io/errors/internal",
                    "Internal server error",
                    format!("Failed to read referenced task: {error}"),
                    "internal.error",
                    None,
                    Some(&instance_id),
                );
            }
        }
    } else {
        None
    };
    if existing.as_ref().is_some_and(|row| row.state.is_terminal()) {
        return error_response(
            StatusCode::CONFLICT,
            "https://agentic-sandbox.aiwg.io/errors/task-terminal",
            "Task is terminal",
            "Messages cannot be sent to a task that has reached a terminal state".to_string(),
            "task.terminal",
            None,
            Some(&instance_id),
        );
    }
    let supplied_context_id = body
        .get("message")
        .and_then(|message| message.get("contextId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    if let (Some(existing), Some(supplied)) = (existing.as_ref(), supplied_context_id) {
        if existing.context_id.as_deref() != Some(supplied) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "https://agentic-sandbox.aiwg.io/errors/invalid-params",
                "Invalid params",
                "message.contextId does not match the referenced task".to_string(),
                "request.context_task_mismatch",
                None,
                Some(&instance_id),
            );
        }
    }
    let task_id = existing
        .as_ref()
        .map(|row| row.task_id.clone())
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let context_id = existing
        .as_ref()
        .and_then(|row| row.context_id.clone())
        .or_else(|| supplied_context_id.map(str::to_string))
        .or_else(|| Some(Uuid::now_v7().to_string()));

    let status_json = json!({
        "state": TaskState::Submitted.as_str(),
        "timestamp": now.to_rfc3339(),
    });

    let graph_identity = if activated.contains(flow_graph::URI) {
        // The extension pre-request hook has already validated this payload.
        flow_graph::graph_metadata(&body).ok().flatten()
    } else {
        None
    };
    let resume_lineage = match graph_identity
        .as_ref()
        .map(|_| flow_graph::resume_lineage(&body))
        .transpose()
    {
        Ok(Some(value)) => value,
        Ok(None) => None,
        Err(detail) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "https://agentic-sandbox.aiwg.io/errors/flow-graph-resume",
                "Invalid Flow graph resume lineage",
                detail,
                "flow_graph.invalid_resume",
                None,
                Some(&instance_id),
            );
        }
    };
    let mut task_metadata = graph_identity.map(|graph| {
        flow_graph::task_metadata_with_lineage(
            graph,
            &task_id,
            context_id.as_deref(),
            message_id.as_deref().unwrap_or(&task_id),
            flow_graph::lifecycle("queued", now),
            resume_lineage,
        )
    });
    if task_metadata.is_none() {
        task_metadata = existing.as_ref().and_then(|row| row.metadata_json.clone());
    }
    append_protocol_history(
        &mut task_metadata,
        body.get("message").cloned().unwrap_or(Value::Null),
    );

    let row = TaskRow {
        task_id: task_id.clone(),
        context_id,
        // #269: persist owning instance so list_tasks can scope by path id.
        instance_id: Some(instance_id.clone()),
        state: TaskState::Submitted,
        fail_kind: None,
        status_json,
        metadata_json: task_metadata,
        created_at: existing.as_ref().map_or(now, |row| row.created_at),
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

    if let Some(config) = body
        .get("configuration")
        .and_then(|configuration| configuration.get("taskPushNotificationConfig"))
    {
        if let Err(response) =
            super::push_notification::persist_inline_config(&state, &instance_id, &task_id, config)
        {
            return response;
        }
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
            let current = state
                .store
                .get_task_for_instance(&instance_id, &task_id)
                .ok()
                .flatten()
                .unwrap_or_else(|| row_after.clone());
            if current.state == TaskState::Submitted {
                row_after.state = TaskState::Working;
                row_after.status_json = json!({
                    "state": TaskState::Working.as_str(),
                    "timestamp": updated.to_rfc3339(),
                });
                flow_graph::latest_event(
                    &mut row_after.metadata_json,
                    flow_graph::lifecycle("running", updated),
                );
                row_after.updated_at = updated;
                if let Err(e) = state.store.upsert_task(&row_after) {
                    tracing::warn!(error = %e, task_id, "could not record dispatch transition");
                }
            } else {
                row_after = current;
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
            flow_graph::latest_event(
                &mut row_after.metadata_json,
                flow_graph::terminal(
                    "failed",
                    row_after.created_at,
                    updated,
                    json!({"status": "unknown", "reason": "dispatch did not start"}),
                    json!([]),
                    Some(&err.to_string()),
                ),
            );
            row_after.updated_at = updated;
            row_after.terminal_at = Some(updated);
            if let Err(e) = state.store.upsert_task(&row_after) {
                tracing::warn!(error = %e, task_id, "could not record dispatch failure");
            }
            Some(err)
        }
    };

    let mut task_json = task_row_to_a2a(&row_after);
    if let Some(message) = internal_response_message(&row_after) {
        task_json = json!({"message": message});
    }
    if let Some(limit) = body
        .get("configuration")
        .and_then(|configuration| configuration.get("historyLength"))
        .and_then(Value::as_u64)
        .and_then(|limit| usize::try_from(limit).ok())
    {
        if let Some(history) = task_json.get_mut("history").and_then(Value::as_array_mut) {
            let remove = history.len().saturating_sub(limit);
            history.drain(..remove);
        }
        if limit == 0 {
            task_json
                .as_object_mut()
                .map(|object| object.remove("history"));
        }
    }

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
            let versioned = versioned_request_body(protocol_version.as_header_value(), &body);
            if let Err(e) = state
                .idem
                .record(mid, &versioned, status.as_u16(), &task_json)
            {
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
        "context_id": row_after.context_id,
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
            crate::bindings::message_dispatch::DispatchError::NotImplemented => {
                ("dispatch.unimplemented", "Runtime dispatch unimplemented")
            }
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

fn append_protocol_history(metadata: &mut Option<Value>, message: Value) {
    if metadata.is_none() {
        *metadata = Some(json!({}));
    }
    let Some(root) = metadata.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    let internal = root.entry("_a2a").or_insert_with(|| json!({}));
    let Some(internal) = internal.as_object_mut() else {
        return;
    };
    let history = internal
        .entry("history")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(history) = history.as_array_mut() {
        history.push(message);
    }
}

fn internal_response_message(row: &TaskRow) -> Option<Value> {
    row.metadata_json
        .as_ref()?
        .get("_a2a")?
        .get("responseMessage")
        .cloned()
}

fn hitl_response_payload(body: &Value) -> Option<&Value> {
    body.get("message")
        .and_then(|m| m.get("metadata"))
        .and_then(|m| m.get("hitl_response_for"))
}

async fn handle_hitl_response(
    state: &AppState,
    instance_id: &str,
    body: &Value,
    response: &Value,
) -> Response {
    let Some(task_id) = body
        .get("message")
        .and_then(|m| m.get("taskId"))
        .and_then(|v| v.as_str())
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "https://agentic-sandbox.aiwg.io/errors/invalid-params",
            "Invalid HITL response",
            "HITL response messages must include message.taskId",
            "hitl_response.missing_task_id",
            None,
            Some(instance_id),
        );
    };

    let mut row = match state.store.get_task_for_instance(instance_id, task_id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error_response(
                StatusCode::CONFLICT,
                "https://agentic-sandbox.aiwg.io/errors/hitl-response",
                "HITL response rejected",
                "HITL prompt is unknown for this instance",
                "hitl_already_answered_or_unknown",
                None,
                Some(instance_id),
            );
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "https://agentic-sandbox.aiwg.io/errors/internal",
                "Internal server error",
                format!("Failed to read HITL task: {e}"),
                "internal.error",
                None,
                Some(instance_id),
            );
        }
    };

    if row.state != TaskState::InputRequired {
        return error_response(
            StatusCode::CONFLICT,
            "https://agentic-sandbox.aiwg.io/errors/hitl-response",
            "HITL response rejected",
            format!("Task '{}' is not waiting for HITL input", task_id),
            "hitl_already_answered_or_unknown",
            None,
            Some(instance_id),
        );
    }

    let stored_envelope = row
        .status_json
        .get("message")
        .and_then(|m| m.get("metadata"))
        .and_then(|m| m.get(HITL_PROMPT_URI));
    let Some(stored_envelope) = stored_envelope else {
        return error_response(
            StatusCode::CONFLICT,
            "https://agentic-sandbox.aiwg.io/errors/hitl-response",
            "HITL response rejected",
            "Task is input-required but has no stored hitl-prompt envelope",
            "hitl_already_answered_or_unknown",
            None,
            Some(instance_id),
        );
    };

    if let Err(detail) = validate_hitl_response(stored_envelope, response) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "https://agentic-sandbox.aiwg.io/errors/hitl-response-invalid",
            "HITL response invalid",
            detail,
            "hitl_response_invalid",
            None,
            Some(instance_id),
        );
    }

    let now = Utc::now();
    row.state = TaskState::Working;
    row.updated_at = now;
    row.terminal_at = None;
    row.status_json = json!({
        "state": TaskState::Working.as_str(),
        "timestamp": now.to_rfc3339(),
        "message": body["message"].clone(),
    });
    if let Err(e) = state.store.upsert_task(&row) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to persist HITL response: {e}"),
            "internal.error",
            None,
            Some(instance_id),
        );
    }

    let task_json = super::task_row_to_a2a(&row);
    build_fresh_response(
        StatusCode::ACCEPTED,
        task_json,
        &ActivatedExtensions::default(),
        None,
    )
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
