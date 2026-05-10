//! `agentic-sandbox/multi-tenant` extension (#213).
//!
//! Declared-only in v2.0. When activated, reads `metadata.tenant_id`
//! from the request body, records it on the current tracing span, and
//! continues. Full tenant scoping (per-tenant AgentCard, authorization
//! checks, quota enforcement) lands in a later wave; this handler
//! exists so the wire surface is consistent with the AgentCard
//! advertisement.
//!
//! Tier: beta (per ADR-019 stability tier).

use super::{ExtensionHandler, ExtensionOutcome, PreRequestCtx};

/// Extension URI per spec.
pub const URI: &str = "https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1";

/// Multi-tenant extension handler.
pub struct MultiTenantExtension {
    _priv: (),
}

impl MultiTenantExtension {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for MultiTenantExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionHandler for MultiTenantExtension {
    fn uri(&self) -> &'static str {
        URI
    }

    fn required(&self) -> bool {
        false
    }

    fn pre_request(&self, ctx: &PreRequestCtx<'_>) -> ExtensionOutcome {
        if !ctx.activated.contains(URI) {
            return ExtensionOutcome::Continue;
        }
        let tenant_id = ctx
            .request_body
            .get("metadata")
            .and_then(|m| m.get("tenant_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        // v2.0: accept any value, record on span only.
        tracing::Span::current().record("tenant_id", tracing::field::display(tenant_id));
        tracing::debug!(tenant_id, "multi-tenant extension activated");
        ExtensionOutcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{ActivatedExtensions, PreRequestCtx};
    use serde_json::json;

    #[test]
    fn pre_request_accepts_any_tenant_id() {
        let ext = MultiTenantExtension::new();
        let act = ActivatedExtensions(vec![URI.to_string()]);
        let body = json!({"metadata": {"tenant_id": "acme-corp"}});
        let ctx = PreRequestCtx {
            activated: &act,
            task_id: None,
            message_id: None,
            request_body: &body,
        };
        assert!(matches!(ext.pre_request(&ctx), ExtensionOutcome::Continue));
    }

    #[test]
    fn pre_request_continues_when_not_activated() {
        let ext = MultiTenantExtension::new();
        let act = ActivatedExtensions::default();
        let body = json!({"metadata": {"tenant_id": "x"}});
        let ctx = PreRequestCtx {
            activated: &act,
            task_id: None,
            message_id: None,
            request_body: &body,
        };
        assert!(matches!(ext.pre_request(&ctx), ExtensionOutcome::Continue));
    }

    #[test]
    fn pre_request_default_tenant_when_missing() {
        let ext = MultiTenantExtension::new();
        let act = ActivatedExtensions(vec![URI.to_string()]);
        let body = json!({});
        let ctx = PreRequestCtx {
            activated: &act,
            task_id: None,
            message_id: None,
            request_body: &body,
        };
        assert!(matches!(ext.pre_request(&ctx), ExtensionOutcome::Continue));
    }
}
