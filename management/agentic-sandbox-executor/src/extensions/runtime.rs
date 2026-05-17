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
    /// on the AgentCard. As of #236 the wire-enforcement layer also
    /// returns `true` so the `RequireA2AExtensions` middleware rejects
    /// mutating requests that omit the `A2A-Extensions: runtime/v1`
    /// header. GET-only routes (get_task, list_tasks, subscribe_to_task,
    /// extendedAgentCard) bypass the middleware via route-scoped
    /// layering in `bindings::rest::router`.
    fn required(&self) -> bool {
        true
    }

    /// Inject `runtime.*` keys into `response_body.metadata` when:
    ///
    /// 1. The extension is activated.
    /// 2. The status is a success (2xx).
    /// 3. The response body is an object with an existing `metadata`
    ///    object. (A Task always has `id` + `status`.)
    ///
    /// #268: prefer the per-instance `InstanceContext` from
    /// [`PostResponseCtx::instance`] when the layer has resolved one.
    /// The handler previously reported the extension's globally-configured
    /// defaults (`kind: "vm"`, the static host) for every response,
    /// which contradicted the AgentCard for container-backed instances.
    /// `runtime.instance_id` now carries the canonical instance id
    /// instead of the task id, matching the published AgentCard.
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

        let (kind, host, instance_id) = match ctx.instance {
            Some(inst) => {
                let kind = match inst.runtime_kind {
                    RuntimeKind::Vm => "vm",
                    RuntimeKind::Container => "container",
                };
                (kind, inst.host.clone(), inst.instance_id.clone())
            }
            None => {
                // Fallback for call sites that don't resolve an instance
                // (tests, server-wide handlers). Preserve the previous
                // task-id-as-instance-id behavior in that case so existing
                // assertions hold.
                (self.kind_str(), self.host.clone(), ctx.task_id.to_string())
            }
        };

        let runtime_block = json!({
            "instance_id": instance_id,
            "kind": kind,
            "host": host,
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
            instance: None,
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
            instance: None,
        };
        ext.post_response(&mut ctx);
        assert!(body["metadata"]
            .as_object()
            .unwrap()
            .get("runtime")
            .is_none());
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
            instance: None,
        };
        ext.post_response(&mut ctx);
        assert!(body["metadata"]
            .as_object()
            .unwrap()
            .get("runtime")
            .is_none());
    }

    #[test]
    fn uri_matches_spec() {
        let ext = RuntimeExtension::new(RuntimeKind::Container, "x".into(), "h".into());
        assert_eq!(ext.uri(), URI);
        // Per #236 the runtime extension is required at wire level
        // (matches AgentCard `required: true`).
        assert!(ext.required());
    }
}
