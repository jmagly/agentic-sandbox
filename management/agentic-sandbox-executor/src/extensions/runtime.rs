//! `agentic-sandbox/runtime` extension (#213).
//!
//! Required extension. Injects `runtime.instance_id`, `runtime.kind`,
//! `runtime.host`, and available provider metadata into the `metadata` object of every successful
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
            RuntimeKind::Host => "host",
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

        let (kind, host, instance_id, provider, capabilities) = match ctx.instance {
            Some(inst) => {
                let kind = match inst.runtime_kind {
                    RuntimeKind::Vm => "vm",
                    RuntimeKind::Container => "container",
                    RuntimeKind::Host => "host",
                };
                (
                    kind,
                    inst.host.clone(),
                    inst.instance_id.clone(),
                    inst.runtime_provider.clone(),
                    inst.runtime_capabilities.clone(),
                )
            }
            None => {
                // Fallback for call sites that don't resolve an instance
                // (tests, server-wide handlers). Preserve the previous
                // task-id-as-instance-id behavior in that case so existing
                // assertions hold.
                (
                    self.kind_str(),
                    self.host.clone(),
                    ctx.task_id.to_string(),
                    None,
                    Vec::new(),
                )
            }
        };
        metadata.insert("runtime.instance_id".to_string(), json!(instance_id));
        metadata.insert("runtime.kind".to_string(), json!(kind));
        metadata.insert("runtime.host".to_string(), json!(host));
        if let Some(provider) = provider {
            metadata.insert("runtime.provider".to_string(), json!(provider));
        }
        if !capabilities.is_empty() {
            metadata.insert("runtime.capabilities".to_string(), json!(capabilities));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{ActivatedExtensions, ExtensionHandler, PostResponseCtx};
    use regex::Regex;

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
        assert_eq!(body["metadata"]["runtime.instance_id"], "t-1");
        assert_eq!(body["metadata"]["runtime.kind"], "vm");
        assert_eq!(body["metadata"]["runtime.host"], "host-1");
    }

    #[test]
    fn post_response_injects_host_runtime_metadata() {
        let ext =
            RuntimeExtension::new(RuntimeKind::Host, "agentic-dev".into(), "host-local".into());
        let activated = ActivatedExtensions(vec![URI.to_string()]);
        let mut body = json!({
            "id": "t-host",
            "status": {"state": "submitted"},
            "metadata": {}
        });
        let mut ctx = PostResponseCtx {
            activated: &activated,
            task_id: "t-host",
            status: 202,
            response_body: &mut body,
            instance: None,
        };
        ext.post_response(&mut ctx);
        assert_eq!(body["metadata"]["runtime.kind"], "host");
        assert_eq!(body["metadata"]["runtime.host"], "host-local");
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
            .get("runtime.kind")
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
            .get("runtime.kind")
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

    #[test]
    fn post_response_injects_instance_provider_and_capabilities() {
        let ext = RuntimeExtension::new(RuntimeKind::Vm, "agentic-dev".into(), "host-1".into());
        let activated = ActivatedExtensions(vec![URI.to_string()]);
        let instance = crate::instance::InstanceContext::new_ephemeral(
            "018fc0a2-7777-7aaa-bbbb-ccccddddeeee",
            RuntimeKind::Vm,
            "agentic-dev",
            None,
            "host-1",
        )
        .with_runtime_metadata(
            Some("cloud-hypervisor".to_string()),
            vec!["instance.fork".to_string(), "future.capability".to_string()],
        );
        let mut body = json!({"id": "t-1", "metadata": {}});
        let mut ctx = PostResponseCtx {
            activated: &activated,
            task_id: "t-1",
            status: 200,
            response_body: &mut body,
            instance: Some(&instance),
        };

        ext.post_response(&mut ctx);

        assert_eq!(body["metadata"]["runtime.provider"], "cloud-hypervisor");
        assert_eq!(
            body["metadata"]["runtime.capabilities"],
            json!(["instance.fork", "future.capability"])
        );
    }

    #[test]
    fn generated_uuidv7_task_metadata_conforms_to_published_schema() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../docs/contracts/extensions/runtime/v1/task-metadata.schema.json"
        ))
        .expect("task metadata schema");
        let instance_id = uuid::Uuid::now_v7();
        assert_eq!(instance_id.get_version(), Some(uuid::Version::SortRand));
        let instance = crate::instance::InstanceContext::new_ephemeral(
            instance_id.to_string(),
            RuntimeKind::Vm,
            "agentic-dev",
            None,
            "host-1",
        )
        .with_runtime_metadata(
            Some("future-vmm".to_string()),
            vec![
                "instance.fork".to_string(),
                "vendor.future:alpha".to_string(),
            ],
        );
        let ext = RuntimeExtension::new(RuntimeKind::Vm, "agentic-dev".into(), "host-1".into());
        let activated = ActivatedExtensions(vec![URI.to_string()]);
        let mut body = json!({"id": "task-1", "metadata": {}});
        let mut ctx = PostResponseCtx {
            activated: &activated,
            task_id: "task-1",
            status: 200,
            response_body: &mut body,
            instance: Some(&instance),
        };
        ext.post_response(&mut ctx);
        let metadata = body["metadata"].as_object().expect("metadata");

        for required in schema["required"].as_array().unwrap() {
            assert!(metadata.contains_key(required.as_str().unwrap()));
        }
        for (field, contract) in schema["properties"].as_object().unwrap() {
            let Some(value) = metadata.get(field) else {
                continue;
            };
            match contract["type"].as_str().unwrap() {
                "string" => {
                    let value = value.as_str().expect("schema string");
                    if let Some(pattern) = contract["pattern"].as_str() {
                        assert!(Regex::new(pattern).unwrap().is_match(value));
                    }
                }
                "array" => {
                    let values = value.as_array().expect("schema array");
                    let item_pattern =
                        Regex::new(contract["items"]["pattern"].as_str().expect("item pattern"))
                            .unwrap();
                    assert!(values.iter().all(|item| item
                        .as_str()
                        .is_some_and(|item| item_pattern.is_match(item))));
                    let unique = values
                        .iter()
                        .map(Value::to_string)
                        .collect::<std::collections::HashSet<_>>();
                    assert_eq!(unique.len(), values.len());
                }
                other => panic!("unhandled task metadata schema type {other}"),
            }
        }
        assert_eq!(
            metadata["runtime.instance_id"],
            Value::String(instance_id.to_string())
        );
        assert_eq!(metadata["runtime.provider"], "future-vmm");
    }
}
