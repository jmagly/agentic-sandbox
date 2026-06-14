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
use agentic_sandbox_executor::bindings::pty_bridge::{SessionBackend, SessionClass};

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
    pub session_backend: SessionBackend,
    pub session_class: SessionClass,
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
    pub session_backend: Option<SessionBackend>,
    pub session_class: Option<SessionClass>,
}

#[derive(Serialize)]
pub struct CreateSessionResponse {
    /// Stable formal session identifier used by session stream/join APIs.
    pub session_id: String,
    /// Routable v2 executor instance id used by AgentCard and pty-ws paths.
    pub instance_id: String,
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
    /// Current v2 pty-ws attach URL template.
    pub pty_ws_url: String,
    /// Required WebSocket subprotocol for `pty_ws_url`.
    pub pty_ws_subprotocol: String,
    /// Observer-first structured screen stream for orchestration.
    pub orchestrator_observer_url: String,
    /// Controller stream for policy-approved input.
    pub orchestrator_controller_url: String,
    /// Safe default role for orchestration clients.
    pub default_role: &'static str,
    /// Human-readable policy hint for Controller use.
    pub controller_policy: &'static str,
    pub session_backend: SessionBackend,
    pub session_class: SessionClass,
    pub supported_session_backends: Vec<SessionBackend>,
    pub supported_session_classes: Vec<SessionClass>,
    pub observe_supported: bool,
    pub drive_supported: bool,
    pub reattach_supported: bool,
}

const PTY_WS_SUBPROTOCOL: &str = "pty-ws.v1";
const DEFAULT_ORCHESTRATOR_ROLE: &str = "observer";
const CONTROLLER_POLICY: &str = "controller input is policy-gated";
const SUPPORTED_SESSION_BACKENDS: &[SessionBackend] = &[SessionBackend::Tmux];
const SUPPORTED_SESSION_CLASSES: &[SessionClass] = &[SessionClass::Managed];

fn validate_session_host_selection(
    backend: Option<SessionBackend>,
    session_class: Option<SessionClass>,
) -> Result<(SessionBackend, SessionClass), serde_json::Value> {
    let backend = backend.unwrap_or(SessionBackend::Tmux);
    let session_class = session_class.unwrap_or(SessionClass::Managed);
    if !SUPPORTED_SESSION_BACKENDS.contains(&backend) {
        return Err(serde_json::json!({
            "error": "session_backend.not_implemented",
            "message": "agent-scoped session creation currently supports only the tmux backend",
            "requested": backend,
            "supported": SUPPORTED_SESSION_BACKENDS,
        }));
    }
    if !SUPPORTED_SESSION_CLASSES.contains(&session_class) {
        return Err(serde_json::json!({
            "error": "session_class.not_implemented",
            "message": "agent-scoped session creation currently supports only managed sessions",
            "requested": session_class,
            "supported": SUPPORTED_SESSION_CLASSES,
        }));
    }
    Ok((backend, session_class))
}

fn build_create_session_response(
    instance_id: String,
    session_id: String,
    command_id: String,
    session_name: String,
    session_backend: SessionBackend,
    session_class: SessionClass,
) -> CreateSessionResponse {
    CreateSessionResponse {
        ws_endpoint: "ws://{host}:8121/".to_string(),
        join_message: serde_json::json!({
            "type": "join_session",
            "session_id": session_id.clone(),
            "role": "controller",
        }),
        pty_ws_url: format!(
            "wss://{{host}}/agents/{}/sessions/{}/attach",
            instance_id, session_id
        ),
        pty_ws_subprotocol: PTY_WS_SUBPROTOCOL.to_string(),
        orchestrator_observer_url: format!("/ws/sessions/{}/orchestrate?role=observer", session_id),
        orchestrator_controller_url: format!(
            "/ws/sessions/{}/orchestrate?role=controller",
            session_id
        ),
        default_role: DEFAULT_ORCHESTRATOR_ROLE,
        controller_policy: CONTROLLER_POLICY,
        session_backend,
        session_class,
        supported_session_backends: SUPPORTED_SESSION_BACKENDS.to_vec(),
        supported_session_classes: SUPPORTED_SESSION_CLASSES.to_vec(),
        observe_supported: true,
        drive_supported: true,
        reattach_supported: true,
        session_id,
        instance_id,
        command_id,
        session_name,
    }
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
                session_backend: SessionBackend::Tmux,
                session_class: SessionClass::Managed,
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
    let instance_id = match state.registry.get(&agent_id) {
        Some(agent) => agent.instance_id.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("agent {} not found", agent_id) })),
            )
                .into_response()
        }
    };

    let command = body.command.unwrap_or_else(|| "bash".to_string());
    let session_name = body
        .session_name
        .unwrap_or_else(|| format!("terminal-{}", &uuid::Uuid::new_v4().to_string()[..8]));
    let (session_backend, session_class) =
        match validate_session_host_selection(body.session_backend, body.session_class) {
            Ok(selection) => selection,
            Err(error) => return (StatusCode::NOT_IMPLEMENTED, Json(error)).into_response(),
        };

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
            // Keep legacy fields for older clients, but also return the v2
            // pty-ws and orchestrator URLs so #321 clients can attach without
            // inferring routes from separate AgentCard metadata.
            Json(build_create_session_response(
                instance_id,
                session_id,
                command_id,
                session_name,
                session_backend,
                session_class,
            ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_managed_tmux_session_host() {
        let (backend, session_class) = validate_session_host_selection(None, None).unwrap();
        assert_eq!(backend, SessionBackend::Tmux);
        assert_eq!(session_class, SessionClass::Managed);
    }

    #[test]
    fn rejects_unimplemented_session_backend() {
        let err = validate_session_host_selection(Some(SessionBackend::Screen), None).unwrap_err();
        assert_eq!(
            err.get("error").and_then(|v| v.as_str()),
            Some("session_backend.not_implemented")
        );
    }

    #[test]
    fn create_session_response_reports_session_host_contract() {
        let response = build_create_session_response(
            "inst-1".to_string(),
            "sess-1".to_string(),
            "cmd-1".to_string(),
            "terminal".to_string(),
            SessionBackend::Tmux,
            SessionClass::Managed,
        );
        assert_eq!(response.session_backend, SessionBackend::Tmux);
        assert_eq!(response.session_class, SessionClass::Managed);
        assert_eq!(
            response.supported_session_backends,
            vec![SessionBackend::Tmux]
        );
        assert_eq!(
            response.supported_session_classes,
            vec![SessionClass::Managed]
        );
        assert!(response.observe_supported);
        assert!(response.drive_supported);
        assert!(response.reattach_supported);
    }
    #[test]
    fn create_session_response_advertises_v2_attach_metadata() {
        let response = build_create_session_response(
            "inst-123".to_string(),
            "sess-456".to_string(),
            "cmd-789".to_string(),
            "codex-tui".to_string(),
            SessionBackend::Tmux,
            SessionClass::Managed,
        );

        assert_eq!(response.session_id, "sess-456");
        assert_eq!(response.instance_id, "inst-123");
        assert_eq!(response.command_id, "cmd-789");
        assert_eq!(response.session_name, "codex-tui");
        assert_eq!(response.pty_ws_subprotocol, "pty-ws.v1");
        assert_eq!(
            response.pty_ws_url,
            "wss://{host}/agents/inst-123/sessions/sess-456/attach"
        );
        assert_eq!(
            response.orchestrator_observer_url,
            "/ws/sessions/sess-456/orchestrate?role=observer"
        );
        assert_eq!(
            response.orchestrator_controller_url,
            "/ws/sessions/sess-456/orchestrate?role=controller"
        );
        assert_eq!(response.default_role, "observer");
        assert!(response.controller_policy.contains("policy-gated"));
    }

    #[test]
    fn create_session_response_keeps_legacy_attach_fields() {
        let response = build_create_session_response(
            "inst-123".to_string(),
            "sess-456".to_string(),
            "cmd-789".to_string(),
            "codex-tui".to_string(),
            SessionBackend::Tmux,
            SessionClass::Managed,
        );

        assert_eq!(response.ws_endpoint, "ws://{host}:8121/");
        assert_eq!(response.join_message["type"], "join_session");
        assert_eq!(response.join_message["session_id"], "sess-456");
        assert_eq!(response.join_message["role"], "controller");
    }
}
