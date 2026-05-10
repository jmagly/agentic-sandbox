//! PTY-over-WebSocket binding. Filled in by Wave 4 W4.1.
//!
//! Streams structured PTY frames (output, exit, resize, input) on a
//! WebSocket scoped to a single task. Multiplexes with the existing
//! v1 PTY pipeline in `agentic_management::ws`.

use axum::Router;

/// Build the PTY-WS router for an instance.
pub fn router() -> Router {
    Router::new()
}
