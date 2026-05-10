//! Axum server skeleton for the A2A per-instance executor.
//!
//! Filled in by W3.3 (#210) — REST/JSON-RPC bindings, middleware stack, and
//! TLS termination. For now this module exposes a placeholder
//! [`ExecutorServer`] type so the rest of the crate can reference it.

use std::net::SocketAddr;

/// Top-level executor HTTP server.
///
/// Wraps an [`axum::Router`] and the per-instance routing table. Construction
/// and route wiring land in #210.
pub struct ExecutorServer {
    /// Bind address. `None` until the server is configured.
    pub bind: Option<SocketAddr>,
}

impl ExecutorServer {
    /// Create an unconfigured server. Real builders arrive in #210.
    pub fn new() -> Self {
        Self { bind: None }
    }
}

impl Default for ExecutorServer {
    fn default() -> Self {
        Self::new()
    }
}
