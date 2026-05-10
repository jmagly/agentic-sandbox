//! A2A REST + JSON-RPC HTTP binding. Filled in by W3.3 (#210).
//!
//! Exposes the canonical A2A endpoint set:
//! - `POST /` — JSON-RPC (`message/send`, `tasks/get`, ...).
//! - `GET  /tasks/{id}` — REST convenience.
//! - `GET  /.well-known/agent-card.json` — AgentCard.

use axum::Router;

/// Build the REST router for an instance. Real implementation in #210.
pub fn router() -> Router {
    Router::new()
}
