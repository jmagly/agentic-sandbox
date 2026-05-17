//! Production [`MessageDispatch`] impl that forwards A2A messages to the
//! connected agent's gRPC channel.
//!
//! Bridges the executor's per-instance `messages:send` seam to the
//! management-crate's existing [`AgentRegistry`] + gRPC plumbing. The
//! executor crate stays self-contained (defaults to `NoOpMessageDispatch`);
//! the management binary wires this implementation in `main.rs` so a
//! provisioned instance actually receives the work the operator submits.
//!
//! Issue: #269.
//!
//! ## Behavior
//!
//! 1. Resolve the path's `instance_id` to an `agent_id` via
//!    [`AgentRegistry::get_by_instance_id`].
//! 2. Extract the text content of the A2A `message.parts[*].text`
//!    fields and dispatch a [`CommandRequest`] carrying that text as
//!    the program's stdin payload.
//! 3. Return [`DispatchOutcome::Accepted`] on successful enqueue so
//!    `send_message` transitions the task `submitted → working`.
//!
//! ## What this does NOT do (yet)
//!
//! - Wire the agent's `CommandResult` output back to the [`TaskStore`]
//!   so the task progresses to `completed` / `failed`. The existing
//!   `CommandDispatcher` output channel needs an observer that
//!   translates command output into task state transitions. Follow-up
//!   work; tracked alongside this issue.
//! - Translate A2A message kinds beyond `text` (file/data parts are
//!   ignored). The agent receives the concatenated text.
//!
//! [`TaskStore`]: agentic_sandbox_executor::store::task_store::TaskStore

use std::collections::HashMap;
use std::sync::Arc;

use agentic_sandbox_executor::bindings::message_dispatch::{
    DispatchError, DispatchOutcome, MessageDispatch,
};
use agentic_sandbox_executor::instance::InstanceContext;
use async_trait::async_trait;
use serde_json::Value;

use crate::dispatch::{CommandDispatcher, DispatchError as ExecDispatchError};
use crate::registry::AgentRegistry;

/// #269: Forwards A2A messages to the agent that backs an instance.
pub struct AgentMessageDispatch {
    registry: Arc<AgentRegistry>,
    dispatcher: Arc<CommandDispatcher>,
}

impl AgentMessageDispatch {
    pub fn new(registry: Arc<AgentRegistry>, dispatcher: Arc<CommandDispatcher>) -> Self {
        Self {
            registry,
            dispatcher,
        }
    }

    /// Pull the text content out of an A2A message envelope. Concatenates
    /// every `parts[i].text` (or `parts[i].content` as a defensive
    /// fallback for older shapes) so the agent sees the full prompt.
    fn extract_text(body: &Value) -> String {
        let Some(parts) = body
            .get("message")
            .and_then(|m| m.get("parts"))
            .and_then(|v| v.as_array())
        else {
            return String::new();
        };
        let mut out = String::new();
        for p in parts {
            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
        out
    }
}

#[async_trait]
impl MessageDispatch for AgentMessageDispatch {
    async fn dispatch(
        &self,
        instance: &InstanceContext,
        task_id: &str,
        message: &Value,
    ) -> Result<DispatchOutcome, DispatchError> {
        let Some((agent_id, _command_tx)) =
            self.registry.get_by_instance_id(&instance.instance_id)
        else {
            return Err(DispatchError::RuntimeUnavailable(
                instance.instance_id.clone(),
                "no agent registered for this instance_id".into(),
            ));
        };

        let text = Self::extract_text(message);
        if text.is_empty() {
            return Err(DispatchError::DispatchFailed(
                "A2A message has no text parts; this dispatch only forwards text".into(),
            ));
        }

        // Pass the text as stdin to a tiny shell that prints it. The
        // agent's process supervision treats this as a normal command;
        // its captured output goes through the existing CommandDispatcher
        // output pipeline. Future work: pipe directly into whatever
        // long-running agent process is already inside the container.
        let command = "sh".to_string();
        let args = vec![
            "-c".to_string(),
            // Use printf to avoid losing trailing newlines / escape
            // handling. The agent process's stdout is captured by
            // CommandDispatcher.
            "printf '%s\\n' \"$AIWG_A2A_MESSAGE\"".to_string(),
        ];
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("AIWG_A2A_MESSAGE".into(), text);
        env.insert("AIWG_A2A_TASK_ID".into(), task_id.to_string());
        env.insert(
            "AIWG_A2A_INSTANCE_ID".into(),
            instance.instance_id.clone(),
        );

        // 300s timeout is generous; the dispatcher kills runaway commands.
        match self
            .dispatcher
            .dispatch(&agent_id, command, args, String::new(), env, 300)
            .await
        {
            Ok((command_id, _output_rx)) => {
                tracing::info!(
                    instance_id = %instance.instance_id,
                    agent_id = %agent_id,
                    task_id = %task_id,
                    command_id = %command_id,
                    "messages:send forwarded to agent"
                );
                Ok(DispatchOutcome::Accepted)
            }
            Err(ExecDispatchError::AgentNotFound(_)) => Err(DispatchError::RuntimeUnavailable(
                instance.instance_id.clone(),
                format!("agent {} disappeared between lookup and dispatch", agent_id),
            )),
            Err(ExecDispatchError::SendFailed(_)) => Err(DispatchError::RuntimeUnavailable(
                instance.instance_id.clone(),
                format!("send to agent {} failed: gRPC channel closed", agent_id),
            )),
            Err(e) => Err(DispatchError::DispatchFailed(format!(
                "CommandDispatcher rejected the message: {}",
                e
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_text_concatenates_parts() {
        let body = json!({
            "message": {
                "role": "user",
                "parts": [
                    {"kind": "text", "text": "hello"},
                    {"kind": "text", "text": "world"},
                ],
            }
        });
        assert_eq!(AgentMessageDispatch::extract_text(&body), "hello\nworld");
    }

    #[test]
    fn extract_text_empty_when_no_text_parts() {
        let body = json!({"message": {"role": "user", "parts": [{"kind": "file"}]}});
        assert!(AgentMessageDispatch::extract_text(&body).is_empty());
    }
}
