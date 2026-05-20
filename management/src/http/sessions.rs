//! Session management HTTP endpoints.
//!
//! GET    /api/v1/agents/:id/sessions           — list active sessions
//! POST   /api/v1/agents/:id/sessions           — create a new session
//! DELETE /api/v1/agents/:id/sessions/:session  — kill a session

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use super::server::AppState;
use crate::dispatch::{DispatchError, SessionType};

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SessionEntry {
    /// Stable formal session identifier used by session stream/join APIs.
    pub session_id: String,
    /// Ephemeral process/PTY command handle used internally by the agent bridge.
    pub command_id: String,
    pub session_name: String,
    pub session_type: &'static str,
    pub command: String,
    pub created_at_secs: u64,
    pub has_screen: bool,
}

#[derive(Serialize)]
pub struct SessionListResponse {
    pub agent_id: String,
    pub sessions: Vec<SessionEntry>,
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub command: Option<String>,
    pub session_name: Option<String>,
}

#[derive(Serialize)]
pub struct CreateSessionResponse {
    /// Stable formal session identifier used by session stream/join APIs.
    pub session_id: String,
    /// Ephemeral process/PTY command handle. Exposed so clients can correlate
    /// lower-level agent output without treating it as the session identity.
    pub command_id: String,
    pub session_name: String,
    /// The bare WS endpoint to dial (e.g. `ws://host:8121/`). The server
    /// has no path-based routing per session — connect to this URL, then
    /// send `join_message` as the first frame to attach. See #191.
    pub ws_endpoint: String,
    /// Pre-baked `join_session` envelope the client should send on its
    /// freshly-opened WS as the first frame. Keeps the contract
    /// self-describing — no out-of-band protocol knowledge needed.
    pub join_message: serde_json::Value,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /api/v1/agents/:id/sessions
pub async fn list_sessions(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    if state.registry.get(&agent_id).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("agent {} not found", agent_id) })),
        )
            .into_response();
    }

    let raw = state.dispatcher.get_active_sessions(&agent_id);
    let sessions = raw
        .into_iter()
        .map(|s| {
            let has_screen = state
                .screen_registry
                .as_ref()
                .map(|sr| sr.get(&s.command_id).is_some())
                .unwrap_or(false);
            SessionEntry {
                session_id: s.session_id,
                command_id: s.command_id,
                session_name: s.session_name,
                session_type: match s.session_type {
                    SessionType::Interactive => "interactive",
                    SessionType::Headless => "headless",
                    SessionType::Background => "background",
                },
                command: s.command,
                created_at_secs: s.created_at.elapsed().as_secs(),
                has_screen,
            }
        })
        .collect();

    Json(SessionListResponse { agent_id, sessions }).into_response()
}

/// POST /api/v1/agents/:id/sessions
pub async fn create_session(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    if state.registry.get(&agent_id).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("agent {} not found", agent_id) })),
        )
            .into_response();
    }

    let command = body.command.unwrap_or_else(|| "bash".to_string());
    let session_name = body
        .session_name
        .unwrap_or_else(|| format!("terminal-{}", &uuid::Uuid::new_v4().to_string()[..8]));

    // 409 if session already exists
    let exists = state
        .dispatcher
        .get_active_sessions(&agent_id)
        .into_iter()
        .any(|s| s.session_name == session_name);
    if exists {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("session '{}' already exists on agent {}", session_name, agent_id)
            })),
        )
            .into_response();
    }

    match state
        .dispatcher
        .create_session(
            &agent_id,
            session_name.clone(),
            SessionType::Interactive,
            command,
            vec![],
            None,
            220,
            50,
        )
        .await
    {
        Ok((command_id, _rx)) => {
            let session_id = state
                .dispatcher
                .session_id_for_command(&command_id)
                .unwrap_or_else(|| command_id.clone());
            let join_message = serde_json::json!({
                "type": "join_session",
                "session_id": session_id.clone(),
                "role": "controller",
            });
            // The WS server is path-agnostic; clients connect to the bare
            // endpoint and send `join_message` as the first frame. The old
            // `ws_url: "/ws/sessions/<id>/orchestrate"` shape advertised a
            // route that doesn't exist (#191).
            Json(CreateSessionResponse {
                ws_endpoint: "ws://{host}:8121/".to_string(),
                join_message,
                session_id,
                command_id,
                session_name,
            })
            .into_response()
        }
        Err(DispatchError::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "agent not connected" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /api/v1/agents/:id/sessions/:session
pub async fn delete_session(
    State(state): State<AppState>,
    Path((agent_id, session_name)): Path<(String, String)>,
) -> impl IntoResponse {
    match state
        .dispatcher
        .kill_session(&agent_id, &session_name)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(DispatchError::AgentNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("agent {} not found", agent_id) })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
