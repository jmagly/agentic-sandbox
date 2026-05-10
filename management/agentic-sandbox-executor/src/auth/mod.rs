//! Authentication / authorization for inbound A2A requests.
//!
//! Filled in by #210 (bearer-token validation matching the existing
//! executor-contract `dispatch` flow) and #213 (multi-tenant scoping).

/// Auth context attached to each request.
pub struct AuthContext;
