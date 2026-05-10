//! AgentCard generation, JCS canonicalization (RFC 8785), and JWS signing.
//!
//! Filled in by W3.2 (#209). Each instance publishes a signed AgentCard at
//! `/.well-known/agent-card.json` describing its capabilities, supported
//! extensions, and bindings (REST, SSE, WebSocket). This module owns the
//! build pipeline and signature verification helpers.

use serde::{Deserialize, Serialize};

/// Minimal placeholder AgentCard type. The real schema (per A2A spec +
/// agentic-sandbox extensions) is materialized in #209.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    /// Card protocol version.
    pub version: String,
    /// Instance display name.
    pub name: String,
}

impl AgentCard {
    /// Build a stub card with the given name. #209 replaces this with a
    /// builder that consumes [`crate::instance::InstanceContext`] and emits
    /// the full A2A schema.
    pub fn stub(name: impl Into<String>) -> Self {
        Self {
            version: "0.0.0-skeleton".to_string(),
            name: name.into(),
        }
    }
}
