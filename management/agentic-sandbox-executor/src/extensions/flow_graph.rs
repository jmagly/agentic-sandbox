//! Optional AIWG Flow graph-node extension.
//!
//! The Sandbox executes one node; it never interprets graph routes.  This
//! module validates and preserves the AIWG-owned identity namespace and emits
//! schema-shaped lifecycle records without copying prompts or raw output into
//! metadata.

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use super::{ExtensionHandler, ExtensionOutcome, PreRequestCtx};

pub const URI: &str = "https://aiwg.io/extensions/flow-graph/v1";
pub const EVENT_KEY: &str = "flow_graph.event";
pub const RESUME_KEY: &str = "flow_graph.resume";

const REQUIRED_IDS: &[&str] = &[
    "graph_id",
    "graph_version",
    "run_id",
    "node_id",
    "node_run_id",
];
const OPTIONAL_IDS: &[&str] = &["edge_id"];
const FORBIDDEN_KEYS: &[&str] = &[
    "prompt",
    "content",
    "reasoning",
    "credential",
    "credentials",
    "secret",
    "token",
    "raw_output",
];

pub struct FlowGraphExtension;

impl FlowGraphExtension {
    pub fn new() -> Self {
        Self
    }
}

impl ExtensionHandler for FlowGraphExtension {
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
        match graph_metadata(ctx.request_body) {
            Ok(Some(_)) => ExtensionOutcome::Continue,
            Ok(None) => reject("activated flow-graph/v1 requires message.metadata namespace"),
            Err(reason) => reject(&reason),
        }
    }
}

fn reject(detail: &str) -> ExtensionOutcome {
    ExtensionOutcome::Reject {
        status: 422,
        body: json!({
            "type": "https://agentic-sandbox.aiwg.io/errors/flow-graph-metadata",
            "title": "Invalid Flow graph metadata",
            "status": 422,
            "code": "flow_graph.invalid_metadata",
            "detail": detail,
        }),
    }
}

/// Return a validated, minimized graph identity object from an A2A request.
pub fn graph_metadata(body: &Value) -> Result<Option<Value>, String> {
    let Some(value) = body
        .get("message")
        .and_then(|v| v.get("metadata"))
        .and_then(|v| v.get(URI))
    else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| "flow-graph/v1 metadata must be an object".to_string())?;
    for key in REQUIRED_IDS {
        validate_id(object, key, true)?;
    }
    for key in OPTIONAL_IDS {
        validate_id(object, key, false)?;
    }
    for key in object.keys() {
        if !REQUIRED_IDS.contains(&key.as_str()) && !OPTIONAL_IDS.contains(&key.as_str()) {
            return Err(format!("unsupported flow-graph/v1 field: {key}"));
        }
        if FORBIDDEN_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "sensitive field is forbidden in graph metadata: {key}"
            ));
        }
    }
    Ok(Some(Value::Object(object.clone())))
}

fn validate_id(object: &Map<String, Value>, key: &str, required: bool) -> Result<(), String> {
    let Some(value) = object.get(key) else {
        return if required {
            Err(format!("missing required flow-graph/v1 field: {key}"))
        } else {
            Ok(())
        };
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("flow-graph/v1 field {key} must be a string"))?;
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(format!(
            "flow-graph/v1 field {key} must be 1..256 printable characters"
        ));
    }
    Ok(())
}

pub fn task_metadata(
    graph: Value,
    task_id: &str,
    session_id: Option<&str>,
    idempotency_key: &str,
    event: Value,
) -> Value {
    task_metadata_with_lineage(graph, task_id, session_id, idempotency_key, event, None)
}

pub fn task_metadata_with_lineage(
    graph: Value,
    task_id: &str,
    session_id: Option<&str>,
    idempotency_key: &str,
    event: Value,
    lineage: Option<Value>,
) -> Value {
    let mut event_graph = graph.clone();
    event_graph["namespace"] = Value::String(URI.to_string());
    let mut task = json!({
        "task_id": task_id,
        "runtime_binding": "a2a-sandbox",
        "idempotency_key": idempotency_key,
    });
    if let Some(session_id) = session_id {
        task["session_id"] = Value::String(session_id.to_string());
    }
    if let Some(lineage) = lineage {
        if let Some(value) = lineage.get("replay_of_task_id") {
            task["replay_of_task_id"] = value.clone();
        }
        if let Some(value) = lineage.get("checkpoint_id") {
            task["checkpoint_id"] = value.clone();
        }
    }
    json!({
        URI: graph,
        EVENT_KEY: {
            "api_version": "flow.aiwg.io/v1alpha1",
            "kind": "GraphSandboxNodeEvent",
            "metadata": event_graph,
            "task": task,
            "event": event,
        }
    })
}

pub fn resume_lineage(body: &Value) -> Result<Option<Value>, String> {
    let Some(value) = body
        .get("message")
        .and_then(|v| v.get("metadata"))
        .and_then(|v| v.get(RESUME_KEY))
    else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| "flow_graph.resume must be an object".to_string())?;
    if object.len() != 2 {
        return Err(
            "flow_graph.resume requires only replay_of_task_id and checkpoint_id".to_string(),
        );
    }
    validate_id(object, "replay_of_task_id", true)?;
    validate_id(object, "checkpoint_id", true)?;
    Ok(Some(Value::Object(object.clone())))
}

pub fn lifecycle(state: &str, observed_at: DateTime<Utc>) -> Value {
    json!({"type": "lifecycle", "state": state, "observed_at": observed_at.to_rfc3339()})
}

pub fn terminal(
    state: &str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    exit: Value,
    evidence: Value,
    termination_reason: Option<&str>,
) -> Value {
    let duration_ms = (ended_at - started_at).num_milliseconds().max(0);
    let mut event = json!({
        "type": "terminal",
        "state": state,
        "started_at": started_at.to_rfc3339(),
        "ended_at": ended_at.to_rfc3339(),
        "duration_ms": duration_ms,
        "exit": exit,
        "evidence": evidence,
    });
    if let Some(reason) = termination_reason {
        event["termination_reason"] = Value::String(reason.to_string());
    }
    event
}

pub fn latest_event(metadata: &mut Option<Value>, event: Value) {
    if let Some(Value::Object(object)) = metadata {
        let Some(mut graph) = object.get(URI).cloned() else {
            return;
        };
        graph["namespace"] = Value::String(URI.to_string());
        let task = object
            .get(EVENT_KEY)
            .and_then(|v| v.get("task"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        object.insert(
            EVENT_KEY.to_string(),
            json!({
                "api_version": "flow.aiwg.io/v1alpha1",
                "kind": "GraphSandboxNodeEvent",
                "metadata": graph,
                "task": task,
                "event": event,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(metadata: Value) -> Value {
        json!({"message": {"metadata": {URI: metadata}}})
    }

    #[test]
    fn validates_exact_identity_and_rejects_extra_content() {
        let valid = json!({
            "graph_id": "g", "graph_version": "1", "run_id": "r",
            "node_id": "n", "node_run_id": "nr", "edge_id": "e"
        });
        assert_eq!(
            graph_metadata(&request(valid.clone())).unwrap(),
            Some(valid)
        );
        let invalid = json!({
            "graph_id": "g", "graph_version": "1", "run_id": "r",
            "node_id": "n", "node_run_id": "nr", "prompt": "do work"
        });
        assert!(graph_metadata(&request(invalid))
            .unwrap_err()
            .contains("unsupported"));
    }

    #[test]
    fn terminal_duration_is_non_negative() {
        let now = Utc::now();
        let event = terminal(
            "unknown",
            now,
            now,
            json!({"status": "unknown", "reason": "disconnect"}),
            json!([]),
            Some("disconnect"),
        );
        assert_eq!(event["duration_ms"], 0);
    }
}
