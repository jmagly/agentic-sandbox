//! Orchestrator WebSocket endpoint — structured screen state for AI agents.
//!
//! GET /ws/sessions/:id/orchestrate
//!     Upgrades to WebSocket. Sends JSON `screen_update` and `prompt_detected`
//!     frames as the PTY output changes, debounced to ~100 ms.
//!
//! GET /api/v1/sessions/:id/screen
//!     REST snapshot of the current screen state (no streaming).

use axum::{
    extract::{ws, Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, warn};

use crate::output::StreamType;

use super::server::AppState;

// ─── JSON frame types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestratorFrame {
    ScreenUpdate {
        session_id: String,
        timestamp: i64,
        screen: ScreenPayload,
        prompt_detected: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_text: Option<String>,
    },
    PromptDetected {
        session_id: String,
        prompt_text: String,
        confidence: f32,
    },
    SessionStart {
        session_id: String,
        role: String,
        can_write: bool,
    },
    SessionEnd {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize)]
pub struct ScreenPayload {
    pub rows: u16,
    pub cols: u16,
    pub text: String,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub scrollback_tail: String,
}

/// Client → server messages (write-back)
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestratorInput {
    Write { text: String },
    Signal { signal: String },
    Resize { rows: u16, cols: u16 },
}

#[derive(Debug, Deserialize)]
pub struct OrchestrateQuery {
    role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrchestratorRole {
    Observer,
    Controller,
}

impl OrchestratorRole {
    fn parse(role: Option<&str>) -> Result<Self, String> {
        match role.unwrap_or("observer") {
            "observer" => Ok(Self::Observer),
            "controller" => Ok(Self::Controller),
            other => Err(format!(
                "invalid role {}; expected observer or controller",
                other
            )),
        }
    }

    fn as_str(self) -> String {
        match self {
            Self::Observer => "observer".to_string(),
            Self::Controller => "controller".to_string(),
        }
    }

    fn can_write(self) -> bool {
        self == Self::Controller
    }
}

fn resolve_session_identity(state: &AppState, requested_id: &str) -> (String, String) {
    if let Some(session_id) = state.dispatcher.session_id_for_command(requested_id) {
        return (session_id, requested_id.to_string());
    }

    if let Some(session_registry) = state.session_registry.as_ref() {
        if let Some(summary) = session_registry
            .list()
            .into_iter()
            .find(|session| session.session_id == requested_id)
        {
            return (summary.session_id, summary.command_id);
        }
    }

    (requested_id.to_string(), requested_id.to_string())
}

// ─── REST snapshot endpoint ───────────────────────────────────────────────────

/// GET /api/v1/sessions/:id/screen
pub async fn get_screen_snapshot(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let registry = match state.screen_registry.as_ref() {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "screen registry not initialised"})),
            )
                .into_response()
        }
    };

    let (public_session_id, command_id) = resolve_session_identity(&state, &session_id);

    match registry.get(&command_id) {
        Some(state_arc) => {
            let snap = state_arc
                .lock()
                .map(|s| s.snapshot())
                .unwrap_or_else(|_| crate::screen_state::ScreenState::new(24, 80).snapshot());
            Json(serde_json::json!({
                "session_id": public_session_id,
                "rows": snap.rows,
                "cols": snap.cols,
                "text": snap.text,
                "cursor": { "row": snap.cursor_row, "col": snap.cursor_col },
                "scrollback_tail": snap.scrollback_tail,
                "prompt_detected": snap.prompt.is_some(),
                "prompt_text": snap.prompt.map(|p| p.text),
            }))
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("session '{}' not tracked", session_id)})),
        )
            .into_response(),
    }
}

// ─── WebSocket upgrade handler ────────────────────────────────────────────────

/// GET /ws/sessions/:id/orchestrate
pub async fn orchestrate_ws(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<OrchestrateQuery>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let role = match OrchestratorRole::parse(query.role.as_deref()) {
        Ok(role) => role,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": message })),
            )
                .into_response()
        }
    };

    upgrade
        .on_upgrade(move |socket| handle_orchestrate(socket, session_id, role, state))
        .into_response()
}

async fn handle_orchestrate(
    socket: ws::WebSocket,
    session_id: String,
    role: OrchestratorRole,
    state: AppState,
) {
    let registry = match state.screen_registry.as_ref() {
        Some(r) => r.clone(),
        None => {
            warn!("Screen registry not initialised — orchestrator WS rejected");
            return;
        }
    };

    let output_agg = state.output_agg.clone();
    let (public_session_id, command_id) = resolve_session_identity(&state, &session_id);

    // Split socket
    let (mut sender, mut receiver) = socket.split();

    // Send session_start
    send_frame(
        &mut sender,
        &OrchestratorFrame::SessionStart {
            session_id: public_session_id.clone(),
            role: role.as_str(),
            can_write: role.can_write(),
        },
    )
    .await;

    // Subscribe to output for this session
    let mut sub = output_agg.subscribe(None, Some(StreamType::Stdout));

    // Debounce timer — send at most one screen_update per 100 ms
    let debounce = Duration::from_millis(100);
    let mut pending_update = false;
    let mut debounce_deadline = tokio::time::Instant::now() + debounce;

    // Ensure screen state exists
    let _ = registry.get_or_create(&command_id, 24, 80);

    let sid = public_session_id.clone();
    let command_id_for_io = command_id.clone();
    let reg_clone = registry.clone();

    // Client to server write-back is controller-only. Observer is the safe
    // default for orchestrators that only need screen state.
    let dispatcher = state.dispatcher.clone();
    let sid_write = command_id.clone();

    // Main loop — receive output, update screen, debounce, emit frames
    loop {
        tokio::select! {
            msg = receiver.next() => {
                let Some(Ok(msg)) = msg else { break };
                let text = match msg {
                    ws::Message::Text(t) => t.to_string(),
                    ws::Message::Close(_) => break,
                    _ => continue,
                };
                let input: OrchestratorInput = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(error = %e, "Unparseable orchestrator input");
                        send_frame(
                            &mut sender,
                            &OrchestratorFrame::Error {
                                message: "invalid orchestrator input".to_string(),
                            },
                        )
                        .await;
                        continue;
                    }
                };

                if !role.can_write() {
                    warn!(
                        session_id = %public_session_id,
                        "observer orchestrator attempted write-capable input"
                    );
                    send_frame(
                        &mut sender,
                        &OrchestratorFrame::Error {
                            message: "orchestrator role observer cannot write; reconnect with ?role=controller".to_string(),
                        },
                    )
                    .await;
                    continue;
                }

                match input {
                    OrchestratorInput::Write { text } => {
                        let _ = dispatcher
                            .send_stdin(&sid_write, text.as_bytes().to_vec())
                            .await;
                    }
                    OrchestratorInput::Resize { rows, cols } => {
                        let _ = dispatcher
                            .send_pty_resize(&sid_write, cols as u32, rows as u32)
                            .await;
                    }
                    OrchestratorInput::Signal { signal } => {
                        let sig_num: i32 = match signal.as_str() {
                            "SIGINT" => 2,
                            "SIGTERM" => 15,
                            "SIGKILL" => 9,
                            _ => {
                                send_frame(
                                    &mut sender,
                                    &OrchestratorFrame::Error {
                                        message: format!("unsupported signal {}", signal),
                                    },
                                )
                                .await;
                                continue;
                            }
                        };
                        let _ = dispatcher.send_pty_signal(&sid_write, sig_num).await;
                    }
                }
            }
            msg = sub.recv_with_policy() => {
                match msg {
                    Ok(Some(output)) if output.command_id == command_id_for_io => {
                        // ScreenRegistry is fed centrally from OutputAggregator
                        // in main.rs. Do not process bytes here as well: doing
                        // so double-applies PTY output for every connected
                        // orchestrator observer and corrupts high-redraw TUIs.
                        pending_update = true;
                        // Reset debounce deadline
                        debounce_deadline = tokio::time::Instant::now() + debounce;
                    }
                    Ok(Some(_)) => {} // Different command, ignore
                    Ok(None) => break, // Aggregator closed
                    Err(crate::output::OutputRecvError::SlowSubscriber { subscriber_id, dropped }) => {
                        warn!(
                            session_id = %public_session_id,
                            subscriber_id = %subscriber_id,
                            dropped,
                            "closing orchestrator output subscriber after bounded broadcast lag"
                        );
                        break;
                    }
                }
            }
            _ = tokio::time::sleep_until(debounce_deadline), if pending_update => {
                pending_update = false;
                // Take snapshot while holding lock, drop guard before any await
                let snap_opt = reg_clone
                    .get(&command_id_for_io)
                    .and_then(|state_arc| state_arc.lock().ok().map(|s| s.snapshot()));

                if let Some(snap) = snap_opt {
                    // Emit prompt_detected fast-path if confidence is high enough
                    if let Some(ref p) = snap.prompt {
                        if p.confidence >= 0.80 {
                            send_frame(
                                &mut sender,
                                &OrchestratorFrame::PromptDetected {
                                    session_id: sid.clone(),
                                    prompt_text: p.text.clone(),
                                    confidence: p.confidence,
                                },
                            )
                            .await;
                        }
                    }

                    send_frame(
                        &mut sender,
                        &OrchestratorFrame::ScreenUpdate {
                            session_id: sid.clone(),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                            screen: ScreenPayload {
                                rows: snap.rows,
                                cols: snap.cols,
                                text: snap.text,
                                cursor_row: snap.cursor_row,
                                cursor_col: snap.cursor_col,
                                scrollback_tail: snap.scrollback_tail,
                            },
                            prompt_detected: snap.prompt.is_some(),
                            prompt_text: snap.prompt.map(|p| p.text),
                        },
                    )
                    .await;
                }
            }
        }
    }

    send_frame(
        &mut sender,
        &OrchestratorFrame::SessionEnd {
            session_id: public_session_id.clone(),
            exit_code: None,
        },
    )
    .await;
}

async fn send_frame(
    sender: &mut futures_util::stream::SplitSink<ws::WebSocket, ws::Message>,
    frame: &OrchestratorFrame,
) {
    use futures_util::SinkExt;
    if let Ok(json) = serde_json::to_string(frame) {
        let _ = sender.send(ws::Message::Text(json.into())).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_role_defaults_to_observer() {
        let role = OrchestratorRole::parse(None).expect("default role");
        assert_eq!(role, OrchestratorRole::Observer);
        assert_eq!(role.as_str(), "observer");
        assert!(!role.can_write());
    }

    #[test]
    fn orchestrator_role_controller_can_write() {
        let role = OrchestratorRole::parse(Some("controller")).expect("controller role");
        assert_eq!(role, OrchestratorRole::Controller);
        assert_eq!(role.as_str(), "controller");
        assert!(role.can_write());
    }

    #[test]
    fn orchestrator_role_rejects_unknown_values() {
        let err = OrchestratorRole::parse(Some("writer")).expect_err("invalid role");
        assert!(err.contains("invalid role"));
    }
}
