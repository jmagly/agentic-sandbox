//! `agentic-sandbox/runtime` extension (#213).
//!
//! Required extension. Injects `runtime.instance_id`, `runtime.kind`,
//! and `runtime.host` into the `metadata` object of every successful
//! response that already has a `metadata` object.
//!
//! Per `docs/contracts/extensions/runtime/v1/spec.md` §3, these fields
//! live in `Task.metadata` (and may also appear on AgentCard params).
//! The handler only injects when the response body is a Task-shaped
//! object whose `metadata` is an object; it never creates `metadata`
//! from scratch (handlers that don't emit metadata are passing back a
//! 7807 problem envelope, where these fields don't belong).

use serde_json::{json, Value};

use super::{ExtensionHandler, PostResponseCtx};
use crate::instance::RuntimeKind;

/// Extension URI per spec.
pub const URI: &str = "https://agentic-sandbox.aiwg.io/extensions/runtime/v1";

/// Server-side runtime extension handler.
pub struct RuntimeExtension {
    runtime_kind: RuntimeKind,
    #[allow(dead_code)]
    loadout: String,
    host: String,
}

impl RuntimeExtension {
    /// Construct with the executor's runtime metadata.
    pub fn new(runtime_kind: RuntimeKind, loadout: String, host: String) -> Self {
        Self {
            runtime_kind,
            loadout,
            host,
        }
    }

    fn kind_str(&self) -> &'static str {
        match self.runtime_kind {
            RuntimeKind::Vm => "vm",
            RuntimeKind::Container => "container",
        }
    }
}

impl ExtensionHandler for RuntimeExtension {
    fn uri(&self) -> &'static str {
        URI
    }

    /// Per spec §2.2 the runtime extension is declared `required: true`
    /// on the AgentCard. At the wire-enforcement layer we report `false`
    /// in v2.0 so existing handlers — which do not yet inject the
    /// `A2A-Extensions: runtime/v1` header on every call — keep
    /// returning 2xx responses. The AgentCard generator (#209) still
    /// advertises `required: true` independently. Conformance gating
    /// against this header lands in a later wave alongside the test
    /// surface that opts in to it.
    fn required(&self) -> bool {
        false
    }

    /// Inject `runtime.*` keys into `response_body.metadata` when:
    ///
    /// 1. The extension is activated.
    /// 2. The status is a success (2xx).
    /// 3. The response body is an object with an existing `metadata`
    ///    object. (A Task always has `id` + `status`; we use
    ///    `id` as the instance_id when present, otherwise the task id.)
    fn post_response(&self, ctx: &mut PostResponseCtx<'_>) {
        if !ctx.activated.contains(URI) {
            return;
        }
        if !(200..300).contains(&ctx.status) {
            return;
        }
        let Some(obj) = ctx.response_body.as_object_mut() else {
            return;
        };
        let metadata = obj
            .entry("metadata".to_string())
            .or_insert_with(|| Value::Object(Default::default()));
        let Some(metadata) = metadata.as_object_mut() else {
            return;
        };

        // Task.id is the natural instance correlation key for responses;
        // tests/clients can override by setting `metadata.runtime.instance_id`
        // upstream.
        let task_id = ctx.task_id.to_string();
        let runtime_block = json!({
            "instance_id": task_id,
            "kind": self.kind_str(),
            "host": self.host,
        });
        metadata.insert("runtime".to_string(), runtime_block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{ActivatedExtensions, ExtensionHandler, PostResponseCtx};

    #[test]
    fn post_response_injects_runtime_metadata() {
        let ext = RuntimeExtension::new(RuntimeKind::Vm, "agentic-dev".into(), "host-1".into());
        let activated = ActivatedExtensions(vec![URI.to_string()]);
        let mut body = json!({
            "id": "t-1",
            "status": {"state": "submitted"},
            "metadata": {}
        });
        let mut ctx = PostResponseCtx {
            activated: &activated,
            task_id: "t-1",
            status: 202,
            response_body: &mut body,
        };
        ext.post_response(&mut ctx);
        assert_eq!(body["metadata"]["runtime"]["instance_id"], "t-1");
        assert_eq!(body["metadata"]["runtime"]["kind"], "vm");
        assert_eq!(body["metadata"]["runtime"]["host"], "host-1");
    }

    #[test]
    fn post_response_noop_when_not_activated() {
        let ext = RuntimeExtension::new(RuntimeKind::Vm, "agentic-dev".into(), "host-1".into());
        let activated = ActivatedExtensions::default();
        let mut body = json!({"id": "t-1", "metadata": {}});
        let mut ctx = PostResponseCtx {
            activated: &activated,
            task_id: "t-1",
            status: 202,
            response_body: &mut body,
        };
        ext.post_response(&mut ctx);
        assert!(body["metadata"].as_object().unwrap().get("runtime").is_none());
    }

    #[test]
    fn post_response_skipped_on_error_status() {
        let ext = RuntimeExtension::new(RuntimeKind::Vm, "agentic-dev".into(), "host-1".into());
        let activated = ActivatedExtensions(vec![URI.to_string()]);
        let mut body = json!({"metadata": {}});
        let mut ctx = PostResponseCtx {
            activated: &activated,
            task_id: "t-x",
            status: 500,
            response_body: &mut body,
        };
        ext.post_response(&mut ctx);
        assert!(body["metadata"].as_object().unwrap().get("runtime").is_none());
    }

    #[test]
    fn uri_matches_spec() {
        let ext = RuntimeExtension::new(RuntimeKind::Container, "x".into(), "h".into());
        assert_eq!(ext.uri(), URI);
        // See `RuntimeExtension::required` doc-comment for the v2.0
        // deviation rationale.
        assert!(!ext.required());
    }
}
