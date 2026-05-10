//! `agentic-sandbox/multi-tenant` extension. Filled in by #213.
//!
//! Adds tenant scoping to AgentCard, task identifiers, and authorization
//! checks for shared executor deployments.

/// Multi-tenant extension marker.
pub struct MultiTenantExtension;
