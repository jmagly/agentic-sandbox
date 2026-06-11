//! `agentic-sandbox/hitl-prompt` extension (#213).
//!
//! Structural validation of the HITL prompt envelope emitted on
//! `input-required` Task states. The envelope shape is defined in
//! `docs/contracts/extensions/hitl-prompt/v1/spec.md` §Prompt envelope.
//!
//! ## What this handler does
//!
//! On `post_response`:
//! - If the response is a Task with `status.state == "input-required"`,
//!   ensure `status.message.metadata` contains the envelope under the
//!   extension URI and that the envelope has the required keys
//!   (`prompt_id`, `prompt`, `response_schema`).
//! - If missing or malformed, log a warning. We DON'T fail the response
//!   here because the agent author owns the envelope; the server's job
//!   is to surface drift, not to block delivery. Conformance tests
//!   (Wave 6) will tighten this if needed.
//!
//! ## Inbound response validation (stub)
//!
//! [`validate_hitl_response`] is a helper for the inbound
//! `messages:send` path that will validate a response payload against
//! the stored envelope's `response_schema`. Wire-up to `send_message`
//! lives behind #210's follow-up work (see TODO in the helper). For
//! now it performs structural checks only.

use serde_json::Value;

use super::{ExtensionHandler, PostResponseCtx};

/// Extension URI per spec.
pub const URI: &str = "https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1";

/// Required envelope keys per spec §Prompt envelope.
const REQUIRED_ENVELOPE_KEYS: &[&str] = &["prompt_id", "prompt", "response_schema"];

/// HITL prompt extension handler.
pub struct HitlPromptExtension {
    _priv: (),
}

impl HitlPromptExtension {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for HitlPromptExtension {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionHandler for HitlPromptExtension {
    fn uri(&self) -> &'static str {
        URI
    }

    fn required(&self) -> bool {
        false
    }

    fn post_response(&self, ctx: &mut PostResponseCtx<'_>) {
        if !ctx.activated.contains(URI) {
            return;
        }
        if !(200..300).contains(&ctx.status) {
            return;
        }
        // Only act on `input-required` Task states.
        let state = ctx
            .response_body
            .get("status")
            .and_then(|s| s.get("state"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if state != "input-required" {
            return;
        }

        let envelope = ctx
            .response_body
            .get("status")
            .and_then(|s| s.get("message"))
            .and_then(|m| m.get("metadata"))
            .and_then(|md| md.get(URI));

        match envelope {
            Some(env) if env.is_object() => {
                for key in REQUIRED_ENVELOPE_KEYS {
                    if env.get(*key).is_none() {
                        tracing::warn!(
                            task_id = %ctx.task_id,
                            missing_key = %key,
                            "hitl-prompt envelope missing required key on input-required state"
                        );
                    }
                }
            }
            _ => {
                tracing::warn!(
                    task_id = %ctx.task_id,
                    "input-required state without hitl-prompt envelope under {}",
                    URI
                );
            }
        }
    }
}

/// Validate a HITL response payload against a stored prompt envelope.
///
/// `stored_envelope` is the envelope previously emitted under the URI
/// key. `response_payload` is the `hitl_response_for` block from the
/// inbound `messages:send` request.
///
/// Returns `Ok(())` when the response shape passes structural checks
/// (object + matching `prompt_id`). v2.0 doesn't validate against the
/// embedded `response_schema` JSON Schema — that's a planned tightening
/// once the conformance suite stabilizes. Wire-up to `send_message`
/// lives behind #210's follow-up work.
pub fn validate_hitl_response(
    stored_envelope: &Value,
    response_payload: &Value,
) -> Result<(), String> {
    let env_pid = stored_envelope
        .get("prompt_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "stored envelope missing prompt_id".to_string())?;
    let resp_obj = response_payload
        .as_object()
        .ok_or_else(|| "response payload must be an object".to_string())?;
    let resp_pid = resp_obj
        .get("prompt_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "response payload missing prompt_id".to_string())?;
    if env_pid != resp_pid {
        return Err(format!(
            "prompt_id mismatch: envelope={env_pid}, response={resp_pid}"
        ));
    }
    let payload = resp_obj
        .get("payload")
        .ok_or_else(|| "response payload missing payload".to_string())?;
    validate_response_schema(
        stored_envelope
            .get("response_schema")
            .ok_or_else(|| "stored envelope missing response_schema".to_string())?,
        payload,
    )?;
    Ok(())
}

fn validate_response_schema(schema: &Value, payload: &Value) -> Result<(), String> {
    if schema.get("type").and_then(|v| v.as_str()) != Some("object") {
        return Err("response_schema type must be object".to_string());
    }
    let payload_obj = payload
        .as_object()
        .ok_or_else(|| "response payload.payload must be an object".to_string())?;
    if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
        for key in required.iter().filter_map(|v| v.as_str()) {
            if !payload_obj.contains_key(key) {
                return Err(format!("response payload missing required property {key}"));
            }
        }
    }
    let properties = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    if schema.get("additionalProperties").and_then(|v| v.as_bool()) == Some(false) {
        for key in payload_obj.keys() {
            if !properties.contains_key(key) {
                return Err(format!("additional property '{key}' is not permitted"));
            }
        }
    }
    for (key, prop_schema) in &properties {
        let Some(value) = payload_obj.get(key) else {
            continue;
        };
        match prop_schema.get("type").and_then(|v| v.as_str()) {
            Some("boolean") if !value.is_boolean() => {
                return Err(format!("expected boolean at /{key}"));
            }
            Some("string") if !value.is_string() => {
                return Err(format!("expected string at /{key}"));
            }
            Some("object") if !value.is_object() => {
                return Err(format!("expected object at /{key}"));
            }
            Some("array") if !value.is_array() => {
                return Err(format!("expected array at /{key}"));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{ActivatedExtensions, PostResponseCtx};
    use serde_json::json;

    fn activated() -> ActivatedExtensions {
        ActivatedExtensions(vec![URI.to_string()])
    }

    #[test]
    fn post_response_envelope_present_passes_check() {
        let ext = HitlPromptExtension::new();
        let act = activated();
        let mut body = json!({
            "id": "t-1",
            "status": {
                "state": "input-required",
                "message": {
                    "metadata": {
                        URI: {
                            "prompt_id": "00000000-0000-0000-0000-000000000001",
                            "prompt": "approve?",
                            "response_schema": {"type": "object"}
                        }
                    }
                }
            }
        });
        let mut ctx = PostResponseCtx {
            activated: &act,
            task_id: "t-1",
            status: 200,
            response_body: &mut body,
            instance: None,
        };
        // No panic = check ran cleanly. We don't assert side-effects
        // because the handler logs warnings rather than mutating.
        ext.post_response(&mut ctx);
    }

    #[test]
    fn post_response_skipped_when_not_input_required() {
        let ext = HitlPromptExtension::new();
        let act = activated();
        let mut body = json!({"status": {"state": "submitted"}});
        let mut ctx = PostResponseCtx {
            activated: &act,
            task_id: "t-1",
            status: 200,
            response_body: &mut body,
            instance: None,
        };
        ext.post_response(&mut ctx);
    }

    #[test]
    fn validate_hitl_response_ok_on_match() {
        let env = json!({
            "prompt_id": "p1",
            "response_schema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["approved"],
                "properties": {
                    "approved": {"type": "boolean"},
                    "comment": {"type": "string"}
                }
            }
        });
        let resp = json!({"prompt_id": "p1", "payload": {"approved": true, "comment": "ok"}});
        assert!(validate_hitl_response(&env, &resp).is_ok());
    }

    #[test]
    fn validate_hitl_response_err_on_mismatch() {
        let env = json!({"prompt_id": "p1", "response_schema": {"type": "object"}});
        let resp = json!({"prompt_id": "p2", "payload": {}});
        assert!(validate_hitl_response(&env, &resp).is_err());
    }

    #[test]
    fn validate_hitl_response_err_on_schema_violation() {
        let env = json!({
            "prompt_id": "p1",
            "response_schema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["approved"],
                "properties": {
                    "approved": {"type": "boolean"}
                }
            }
        });
        let resp = json!({"prompt_id": "p1", "payload": {"approved": "yes"}});
        assert!(validate_hitl_response(&env, &resp).is_err());
    }
}
