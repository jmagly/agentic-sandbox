//! Orchestrator WebSocket endpoint — structured screen state for AI agents.
//!
//! GET /ws/sessions/:id/orchestrate
//!     Upgrades to WebSocket. Sends JSON `screen_update` and `prompt_detected`
//!     frames as the PTY output changes, debounced to ~100 ms.
//!
//! GET /api/v1/sessions/:id/screen
//!     REST snapshot of the current screen state (no streaming).

use axum::{
    extract::{ws, Path, State, WebSocketUpgrade},
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

    match registry.get(&session_id) {
        Some(state_arc) => {
            let snap = state_arc
                .lock()
                .map(|s| s.snapshot())
                .unwrap_or_else(|_| crate::screen_state::ScreenState::new(24, 80).snapshot());
            Json(serde_json::json!({
                "session_id": session_id,
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
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| handle_orchestrate(socket, session_id, state))
}

async fn handle_orchestrate(socket: ws::WebSocket, session_id: String, state: AppState) {
    let registry = match state.screen_registry.as_ref() {
        Some(r) => r.clone(),
        None => {
            warn!("Screen registry not initialised — orchestrator WS rejected");
            return;
        }
    };

    let output_agg = state.output_agg.clone();

    // Split socket
    let (mut sender, mut receiver) = socket.split();

    // Send session_start
    send_frame(
        &mut sender,
        &OrchestratorFrame::SessionStart {
            session_id: session_id.clone(),
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
    let _ = registry.get_or_create(&session_id, 24, 80);

    let sid = session_id.clone();
    let reg_clone = registry.clone();

    // Spawn read task (client → server write-back)
    let dispatcher = state.dispatcher.clone();
    let sid_write = session_id.clone();
    let mut write_task = tokio::spawn(async move {
        use axum::extract::ws::Message;
        while let Some(Ok(msg)) = receiver.next().await as Option<Result<ws::Message, _>> {
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            let input: OrchestratorInput = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    debug!(error = %e, "Unparseable orchestrator input");
                    continue;
                }
            };
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
                        _ => continue,
                    };
                    let _ = dispatcher.send_pty_signal(&sid_write, sig_num).await;
                }
            }
        }
    });

    // Main loop — receive output, update screen, debounce, emit frames
    loop {
        tokio::select! {
            msg = sub.recv() => {
                match msg {
                    Some(output) if output.command_id == sid => {
                        reg_clone.process(&sid, &output.data);
                        pending_update = true;
                        // Reset debounce deadline
                        debounce_deadline = tokio::time::Instant::now() + debounce;
                    }
                    Some(_) => {} // Different command, ignore
                    None => break, // Aggregator closed
                }
            }
            _ = tokio::time::sleep_until(debounce_deadline), if pending_update => {
                pending_update = false;
                // Take snapshot while holding lock, drop guard before any await
                let snap_opt = reg_clone
                    .get(&sid)
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
            _ = &mut write_task => break,
        }
    }

    send_frame(
        &mut sender,
        &OrchestratorFrame::SessionEnd {
            session_id: session_id.clone(),
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
