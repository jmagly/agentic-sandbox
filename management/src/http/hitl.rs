//! HTTP handlers for HITL (Human-in-the-Loop) endpoints.
//!
//! POST /api/v1/agents/:id/hitl     — manually register a HITL request
//! GET  /api/v1/hitl                — list pending requests
//! POST /api/v1/hitl/:id/respond    — inject response text into PTY stdin

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use super::server::AppState;

#[derive(Deserialize)]
pub struct HitlCreateRequest {
    /// The command_id / session_id whose PTY should receive the response.
    pub session_id: String,
    /// The question the agent is asking.
    pub prompt: String,
    /// Optional recent output for context.
    pub context: Option<String>,
}

#[derive(Deserialize)]
pub struct HitlRespondRequest {
    /// The human's response text. A newline is appended before injection.
    pub text: String,
}

#[derive(Serialize)]
struct HitlCreatedResponse {
    hitl_id: String,
}

/// POST /api/v1/agents/:id/hitl — manually register a HITL request.
pub async fn hitl_create(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<HitlCreateRequest>,
) -> impl IntoResponse {
    let store = match &state.hitl_store {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "HITL store unavailable"})),
            )
                .into_response()
        }
    };
    match store.create(
        agent_id,
        body.session_id,
        body.prompt,
        body.context.unwrap_or_default(),
    ) {
        Some(hitl_id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({"hitl_id": hitl_id})),
        )
            .into_response(),
        None => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "A pending HITL request already exists for this session"})),
        )
            .into_response(),
    }
}

/// GET /api/v1/hitl — list all pending HITL requests.
pub async fn hitl_list(State(state): State<AppState>) -> impl IntoResponse {
    let store = match &state.hitl_store {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "HITL store unavailable"})),
            )
                .into_response()
        }
    };
    let requests = store.list();
    Json(serde_json::json!({"requests": requests})).into_response()
}

/// POST /api/v1/hitl/:id/respond — inject response into PTY stdin.
pub async fn hitl_respond(
    State(state): State<AppState>,
    Path(hitl_id): Path<String>,
    Json(body): Json<HitlRespondRequest>,
) -> impl IntoResponse {
    let store = match &state.hitl_store {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "HITL store unavailable"})),
            )
                .into_response()
        }
    };
    let req = match store.resolve(&hitl_id) {
        Some(r) => r,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": format!("HITL request '{}' not found", hitl_id)})),
            )
                .into_response()
        }
    };
    let mut data = body.text.into_bytes();
    data.push(b'\n');
    match state.dispatcher.send_stdin(&req.session_id, data).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
