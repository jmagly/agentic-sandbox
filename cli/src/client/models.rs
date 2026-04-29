//! Lean DTOs for the management-server REST surface.
//!
//! Kept minimal in #153 — only what `health status` and the audit-log
//! `whoami` path need. Subsequent CLI verb issues (#154+) extend this
//! with vm/agent/session/task/etc. Where a server-side type already
//! exists and is `pub`, we should switch to importing it directly via
//! a workspace dependency rather than redeclaring shapes here.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LivenessResponse {
    pub status: Option<String>,
    pub http: Option<String>,
}
