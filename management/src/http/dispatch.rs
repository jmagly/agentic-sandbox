//! POST /api/v1/sessions/:id/dispatch — AIWG executor-contract dispatch
//! route (#193 pass 3).
//!
//! When `aiwg serve` routes a mission to this sandbox (because we
//! registered as an executor — see aiwg_serve.rs), it calls this route
//! on the sandbox's `transport_endpoints.rest`. The handler:
//!
//! 1. Validates the bearer token issued at executor registration.
//! 2. Picks a target VM/agent based on `executor_filter` (or operator
//!    default when filter omitted).
//! 3. Generates a `mission_id` and inserts a record into `MissionStore`.
//! 4. Calls `dispatcher.create_session` with the mission's objective as
//!    the command, binds the resulting `pty_session_id` to the mission.
//! 5. Emits `mission.assigned` immediately. The dispatcher's
//!    `SessionStart` hook (Pass 2) emits `mission.started` once the agent
//!    process actually begins.
//! 6. Returns `202 Accepted` with the spec response shape.
//!
//! The `:id` path component is the **session_id** the AIWG side wants
//! to associate with this mission — but the spec lets us return our own
//! `mission_id` and AIWG correlates by request-response. We accept the
//! path param for spec compliance but use our own UUID generation.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

use super::server::AppState;
use crate::aiwg_serve::{ExecutorEvent, MissionRecord, MissionState};
use crate::dispatch::{DispatchError, SessionType};

/// Shape of `POST /api/v1/sessions/:id/dispatch` request body
/// per executor.v1.md §"Dispatch payload".
#[derive(Debug, Deserialize)]
pub struct DispatchRequest {
    pub mission_id: String,
    pub objective: String,
    #[serde(default)]
    pub completion: String,
    #[serde(default)]
    pub long_running: bool,
    #[serde(default)]
    pub executor_filter: Option<ExecutorFilter>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ExecutorFilter {
    pub executor_id: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Optional `agent_id` hint — when present, picks that agent directly
    /// instead of using the operator default.
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DispatchResponse {
    pub mission_id: String,
    pub executor_id: String,
    pub status: &'static str,
    pub estimated_start: String,
}

/// `POST /api/v1/sessions/:id/dispatch`
pub async fn dispatch_mission(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(_session_id_hint): Path<String>,
    Json(req): Json<DispatchRequest>,
) -> impl IntoResponse {
    // ── 1. Bearer auth ───────────────────────────────────────────────────
    let Some(ref aiwg) = state.aiwg_handle else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "aiwg integration not configured"})),
        )
            .into_response();
    };
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if !aiwg.verify_bearer(token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid bearer token"})),
        )
            .into_response();
    }

    let Some(executor_id) = aiwg.executor_id() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "executor not registered with aiwg serve"})),
        )
            .into_response();
    };

    // ── 2. Pick target agent ─────────────────────────────────────────────
    // Filter precedence: explicit agent_id hint > operator default (first
    // available agent). `executor_id` and `capabilities` filters are honoured
    // by the AIWG-side router before it reaches this sandbox, so we don't
    // need to re-validate them here.
    let target_agent = match req
        .executor_filter
        .as_ref()
        .and_then(|f| f.agent_id.as_ref())
    {
        Some(id) => {
            if state.registry.get(id).is_none() {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": format!("agent {id} not found")})),
                )
                    .into_response();
            }
            id.clone()
        }
        None => {
            // Pick the first ready agent. In a multi-agent deployment the
            // operator can supply `executor_filter.agent_id` to target a
            // specific one.
            match state.registry.list_agents().into_iter().next() {
                Some(a) => a.id,
                None => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(serde_json::json!({
                            "error": "no agents available to receive mission"
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    // ── 3. Pre-flight mission record ─────────────────────────────────────
    let mission_store = match state.mission_store.as_ref() {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "mission store not available"})),
            )
                .into_response();
        }
    };
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    mission_store.insert(MissionRecord {
        mission_id: req.mission_id.clone(),
        objective: req.objective.clone(),
        completion: req.completion.clone(),
        state: MissionState::Assigned,
        pty_session_id: None,
        checkpoint_id: None,
        crash_loop: crate::aiwg_serve::MissionCrashLoopStatus::default(),
        created_at: now.clone(),
        updated_at: now.clone(),
    });

    // Emit mission.assigned immediately — independent of session start time.
    aiwg.emit_executor(ExecutorEvent::mission_assigned(
        &executor_id,
        &req.mission_id,
        &now,
    ));

    // ── 4. Start the agent session ───────────────────────────────────────
    let session_name = format!("mission-{}", &req.mission_id[..req.mission_id.len().min(8)]);
    let mut env = HashMap::new();
    env.insert("AIWG_MISSION_ID".to_string(), req.mission_id.clone());
    env.insert("AIWG_SESSION_HINT".to_string(), _session_id_hint);
    match state
        .dispatcher
        .create_session_with_env(
            &target_agent,
            session_name.clone(),
            SessionType::Background,
            req.objective.clone(),
            vec![],
            None,
            env,
            220,
            50,
        )
        .await
    {
        Ok((pty_session_id, _rx)) => {
            // Bind the session to the mission so the SessionStart/SessionEnd
            // hooks (Pass 2) can translate downstream events.
            mission_store.set_pty_session(&req.mission_id, &pty_session_id);
            info!(
                mission_id = %req.mission_id,
                agent = %target_agent,
                pty_session_id = %pty_session_id,
                "Dispatched mission"
            );
            (
                StatusCode::ACCEPTED,
                Json(DispatchResponse {
                    mission_id: req.mission_id,
                    executor_id,
                    status: "assigned",
                    estimated_start: now,
                }),
            )
                .into_response()
        }
        Err(DispatchError::AgentNotFound(_)) => {
            // Roll back the mission record on dispatch failure so AIWG can
            // re-route. Emit mission.failed so the dashboard reflects reality.
            mission_store.update_state(&req.mission_id, MissionState::Failed);
            aiwg.emit_executor(ExecutorEvent::mission_failed(
                &executor_id,
                &req.mission_id,
                "agent_not_connected",
                "agent disconnected before session could start",
                None,
            ));
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "agent not connected"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!(error = %e, "Mission dispatch failed");
            mission_store.update_state(&req.mission_id, MissionState::Failed);
            aiwg.emit_executor(ExecutorEvent::mission_failed(
                &executor_id,
                &req.mission_id,
                "dispatch_error",
                &e.to_string(),
                None,
            ));
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}
