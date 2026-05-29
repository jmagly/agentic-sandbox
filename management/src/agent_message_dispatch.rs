//! Production [`MessageDispatch`] impl that forwards A2A messages to the
//! connected agent's gRPC channel and drives the resulting task through
//! its full lifecycle (`submitted → working → completed/failed`).
//!
//! Bridges the executor's per-instance `messages:send` seam to the
//! management-crate's existing [`AgentRegistry`] + [`CommandDispatcher`]
//! plumbing. The executor crate stays self-contained (defaults to
//! `NoOpMessageDispatch`); the management binary wires this
//! implementation in `main.rs` so a provisioned instance actually
//! receives the work the operator submits AND the operator sees real
//! state transitions reflecting that work.
//!
//! Issue: #269 (initial seam + dispatch); the lifecycle observer that
//! transitions `working → completed/failed` lives in `spawn_observer`
//! below.
//!
//! ## Behavior
//!
//! 1. Resolve the path's `instance_id` to an `agent_id` via
//!    [`AgentRegistry::get_by_instance_id`].
//! 2. Extract the text content of the A2A `message.parts[*].text`
//!    fields. By default, dispatch a [`CommandRequest`] carrying that
//!    text through `printf` so the agent's process supervision captures
//!    it as a normal command output stream. If the message carries an
//!    explicit `adapter-command/v1` metadata envelope, dispatch the
//!    allowlisted adapter command instead.
//! 3. Spawn a background observer that drains the dispatcher's
//!    `output_rx` for this command_id: stream chunks are appended as
//!    [`task_artifacts`] rows; the final `ExecOutput { complete:true }`
//!    transitions the task to `completed` (exit code 0) or `failed`
//!    (non-zero / error).
//! 4. Return [`DispatchOutcome::Accepted`] so `send_message` transitions
//!    the task `submitted → working` on the wire.
//!
//! ## Translating A2A messages
//!
//! Today the dispatch concatenates `parts[*].text` and exposes it to
//! the agent via the `AIWG_A2A_MESSAGE` env var (plus task and instance
//! ids for correlation). File / data parts are ignored. The
//! `adapter-command/v1` metadata envelope is intentionally narrow: it
//! accepts only supported `sandbox-agent-runner` modes using the wrapper
//! command.
//!
//! [`task_artifacts`]: agentic_sandbox_executor::store::task_store::TaskStore::append_artifact

use std::collections::HashMap;
use std::sync::Arc;

use agentic_sandbox_executor::bindings::message_dispatch::{
    DispatchError, DispatchOutcome, MessageDispatch,
};
use agentic_sandbox_executor::instance::InstanceContext;
use agentic_sandbox_executor::store::task_store::{FailKind, TaskState, TaskStore};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::dispatch::{CommandDispatcher, DispatchError as ExecDispatchError};
use crate::proto::{exec_output, ExecOutput};
use crate::registry::AgentRegistry;

const ADAPTER_COMMAND_URI: &str = "https://agentic-sandbox.aiwg.io/extensions/adapter-command/v1";
const SANDBOX_AGENT_RUNNER: &str = ".aiwg/ops/adapters/sandbox-agent-runner/runner.mjs";
const SANDBOX_AGENT_RUNNER_MODES: &[&str] = &["plan", "assess"];

#[derive(Debug, PartialEq)]
struct DispatchCommand {
    command: String,
    args: Vec<String>,
    working_dir: String,
    env: HashMap<String, String>,
    timeout_secs: u32,
}

/// #269: Forwards A2A messages to the agent that backs an instance and
/// drives the resulting task through its full lifecycle.
pub struct AgentMessageDispatch {
    registry: Arc<AgentRegistry>,
    dispatcher: Arc<CommandDispatcher>,
    store: Arc<TaskStore>,
}

impl AgentMessageDispatch {
    pub fn new(
        registry: Arc<AgentRegistry>,
        dispatcher: Arc<CommandDispatcher>,
        store: Arc<TaskStore>,
    ) -> Self {
        Self {
            registry,
            dispatcher,
            store,
        }
    }

    /// Pull the text content out of an A2A message envelope. Concatenates
    /// every `parts[i].text` so the agent sees the full prompt.
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

    fn adapter_command_envelope(body: &Value) -> Option<&Value> {
        body.get("message")
            .and_then(|m| m.get("metadata"))
            .and_then(|m| m.get(ADAPTER_COMMAND_URI))
            .or_else(|| {
                body.get("metadata")
                    .and_then(|m| m.get(ADAPTER_COMMAND_URI))
            })
    }

    fn build_dispatch_command(
        message: &Value,
        task_id: &str,
        instance_id: &str,
    ) -> Result<DispatchCommand, DispatchError> {
        let text = Self::extract_text(message);
        if text.is_empty() {
            return Err(DispatchError::DispatchFailed(
                "A2A message has no text parts; this dispatch only forwards text".into(),
            ));
        }

        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("AIWG_A2A_MESSAGE".into(), text);
        env.insert("AIWG_A2A_TASK_ID".into(), task_id.to_string());
        env.insert("AIWG_A2A_INSTANCE_ID".into(), instance_id.to_string());
        env.insert("AIWG_MISSION_ID".into(), task_id.to_string());

        let Some(envelope) = Self::adapter_command_envelope(message) else {
            return Ok(DispatchCommand {
                command: "sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "printf '%s\\n' \"$AIWG_A2A_MESSAGE\"".to_string(),
                ],
                working_dir: String::new(),
                env,
                timeout_secs: 300,
            });
        };

        Self::build_adapter_command(envelope, env)
    }

    fn build_adapter_command(
        envelope: &Value,
        mut env: HashMap<String, String>,
    ) -> Result<DispatchCommand, DispatchError> {
        let Some(obj) = envelope.as_object() else {
            return Err(Self::bad_adapter_command(
                "adapter-command/v1 metadata must be an object",
            ));
        };

        let adapter = obj
            .get("adapter")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let mode = obj.get("mode").and_then(|v| v.as_str()).unwrap_or_default();
        if adapter != "sandbox-agent-runner" || !SANDBOX_AGENT_RUNNER_MODES.contains(&mode) {
            return Err(Self::bad_adapter_command(
                "only adapter=sandbox-agent-runner modes plan, assess are supported",
            ));
        }

        let command_values = obj
            .get("command")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Self::bad_adapter_command("command must be an array of strings"))?;
        let command_line: Vec<String> = command_values
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| Self::bad_adapter_command("command entries must be strings"))
            })
            .collect::<Result<_, _>>()?;
        Self::validate_runner_command(&command_line)?;

        let timeout_secs = match obj.get("timeout_seconds").and_then(|v| v.as_u64()) {
            Some(n @ 1..=900) => n as u32,
            Some(_) => {
                return Err(Self::bad_adapter_command(
                    "timeout_seconds must be between 1 and 900",
                ));
            }
            None => 300,
        };

        let working_dir = obj
            .get("working_dir")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if working_dir.contains('\0') {
            return Err(Self::bad_adapter_command("working_dir contains NUL"));
        }

        env.insert("AIWG_A2A_ADAPTER".into(), adapter.to_string());
        env.insert("AIWG_A2A_ADAPTER_MODE".into(), mode.to_string());

        let mut parts = command_line.into_iter();
        let command = parts.next().expect("validated command is non-empty");
        let args = parts.collect();
        Ok(DispatchCommand {
            command,
            args,
            working_dir,
            env,
            timeout_secs,
        })
    }

    fn validate_runner_command(command: &[String]) -> Result<(), DispatchError> {
        if command.len() != 4 {
            return Err(Self::bad_adapter_command(
                "sandbox-agent-runner command must be: node <runner> --request <path>",
            ));
        }
        if command[0] != "node" || command[1] != SANDBOX_AGENT_RUNNER || command[2] != "--request" {
            return Err(Self::bad_adapter_command(
                "unsupported sandbox-agent-runner command",
            ));
        }
        Self::validate_relative_path(&command[3], "request path")
    }

    fn validate_relative_path(path: &str, label: &str) -> Result<(), DispatchError> {
        if path.is_empty()
            || path.starts_with('/')
            || path.contains('\\')
            || path.split('/').any(|part| part == "..")
            || path.contains('\0')
        {
            return Err(Self::bad_adapter_command(format!("{label} is not allowed")));
        }
        if !path.starts_with(".aiwg/ops/adapters/sandbox-agent-runner/")
            && !path.starts_with(".aiwg/ops/runs/")
        {
            return Err(Self::bad_adapter_command(format!(
                "{label} must stay under .aiwg/ops/adapters/sandbox-agent-runner/ or .aiwg/ops/runs/"
            )));
        }
        Ok(())
    }

    fn bad_adapter_command(message: impl Into<String>) -> DispatchError {
        DispatchError::DispatchFailed(format!("adapter-command/v1 rejected: {}", message.into()))
    }

    /// Spawn the lifecycle observer that translates the dispatcher's
    /// `ExecOutput` stream into TaskStore state transitions. Runs until
    /// the channel closes (typically on the final `complete:true`
    /// chunk). Failures are logged but never block dispatch — the worst
    /// case is a task that stays in `working` rather than transitioning
    /// to `completed`/`failed`, which is still better than the
    /// pre-#269 silent-submit failure mode.
    fn spawn_observer(
        store: Arc<TaskStore>,
        task_id: String,
        command_id: String,
        mut output_rx: mpsc::Receiver<ExecOutput>,
    ) {
        tokio::spawn(async move {
            let mission_id = task_id.clone();
            let mut artifact_seq: u64 = 0;
            while let Some(chunk) = output_rx.recv().await {
                // Final frame from the dispatcher: drive terminal state.
                if chunk.complete {
                    let now = Utc::now();
                    let row = match store.get_task(&task_id) {
                        Ok(Some(r)) => r,
                        Ok(None) => {
                            tracing::warn!(
                                task_id = %task_id,
                                command_id = %command_id,
                                "observer: task disappeared before completion"
                            );
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                task_id = %task_id,
                                "observer: failed to load task on completion"
                            );
                            break;
                        }
                    };

                    let (state, fail_kind, summary) = if !chunk.error.is_empty() {
                        (
                            TaskState::Failed,
                            Some(FailKind::Infrastructure),
                            chunk.error.clone(),
                        )
                    } else if chunk.exit_code == 0 {
                        (
                            TaskState::Completed,
                            None,
                            format!("command {} exited 0", command_id),
                        )
                    } else {
                        (
                            TaskState::Failed,
                            Some(FailKind::Application),
                            format!("command {} exited {}", command_id, chunk.exit_code),
                        )
                    };

                    let mut row = row;
                    row.state = state;
                    row.fail_kind = fail_kind;
                    row.status_json = json!({
                        "state": state.as_str(),
                        "timestamp": now.to_rfc3339(),
                        "mission_id": mission_id,
                        "task_id": task_id,
                        "summary": summary,
                        "exit_code": chunk.exit_code,
                    });
                    row.updated_at = now;
                    row.terminal_at = Some(now);
                    if let Err(e) = store.upsert_task(&row) {
                        tracing::warn!(
                            mission_id = %mission_id,
                            error = %e,
                            task_id = %task_id,
                            "observer: failed to record terminal transition"
                        );
                    } else {
                        tracing::info!(
                            task_id = %task_id,
                            mission_id = %mission_id,
                            command_id = %command_id,
                            state = state.as_str(),
                            exit_code = chunk.exit_code,
                            "messages:send task reached terminal state"
                        );
                    }
                    break;
                }

                // Mid-stream output chunk: append as a task artifact so
                // GET /tasks/{tid}/... surfaces what the agent emitted.
                // Empty `data` is skipped — it carries no information
                // and the dispatcher occasionally sends keep-alives.
                if chunk.data.is_empty() {
                    continue;
                }
                artifact_seq = artifact_seq.saturating_add(1);
                let stream = match exec_output::Stream::try_from(chunk.stream)
                    .unwrap_or(exec_output::Stream::Unknown)
                {
                    exec_output::Stream::Stdout => "stdout",
                    exec_output::Stream::Stderr => "stderr",
                    _ => "unknown",
                };
                let artifact_id = format!("{}-{}-{:04}", task_id, stream, artifact_seq);
                let text = String::from_utf8_lossy(&chunk.data).to_string();
                let artifact = json!({
                    "kind": "output_chunk",
                    "stream": stream,
                    "data": text,
                    "command_id": command_id,
                    "seq": artifact_seq,
                    "mission_id": mission_id,
                    "task_id": task_id,
                });
                if let Err(e) = store.append_artifact(&task_id, &artifact_id, &artifact) {
                    // Artifact persistence is best-effort: log and keep
                    // draining the channel so the terminal frame still
                    // closes out the task.
                    tracing::warn!(
                        error = %e,
                        task_id = %task_id,
                        mission_id = %mission_id,
                        artifact_id = %artifact_id,
                        "observer: failed to append output artifact"
                    );
                }
            }
        });
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
        let Some((agent_id, _command_tx)) = self.registry.get_by_instance_id(&instance.instance_id)
        else {
            return Err(DispatchError::RuntimeUnavailable(
                instance.instance_id.clone(),
                "no agent registered for this instance_id".into(),
            ));
        };

        let dispatch_command =
            Self::build_dispatch_command(message, task_id, &instance.instance_id)?;

        // The dispatcher kills runaway commands at the selected timeout.
        match self
            .dispatcher
            .dispatch(
                &agent_id,
                dispatch_command.command,
                dispatch_command.args,
                dispatch_command.working_dir,
                dispatch_command.env,
                dispatch_command.timeout_secs,
            )
            .await
        {
            Ok((command_id, output_rx)) => {
                tracing::info!(
                    instance_id = %instance.instance_id,
                    agent_id = %agent_id,
                    task_id = %task_id,
                    mission_id = %task_id,
                    command_id = %command_id,
                    "messages:send forwarded to agent"
                );
                Self::spawn_observer(
                    self.store.clone(),
                    task_id.to_string(),
                    command_id,
                    output_rx,
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
    use agentic_sandbox_executor::store::task_store::TaskRow;

    fn seed_working_task(store: &TaskStore, task_id: &str, instance_id: &str) {
        let now = Utc::now();
        let row = TaskRow {
            task_id: task_id.to_string(),
            context_id: None,
            instance_id: Some(instance_id.to_string()),
            state: TaskState::Working,
            fail_kind: None,
            status_json: json!({"state": "working", "timestamp": now.to_rfc3339()}),
            metadata_json: None,
            created_at: now,
            updated_at: now,
            terminal_at: None,
        };
        store.upsert_task(&row).unwrap();
    }

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

    #[test]
    fn build_dispatch_command_defaults_to_echo() {
        let body = json!({
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "hello"}],
            }
        });

        let cmd = AgentMessageDispatch::build_dispatch_command(&body, "task-1", "inst-1")
            .expect("echo dispatch should build");

        assert_eq!(cmd.command, "sh");
        assert_eq!(
            cmd.args,
            vec![
                "-c".to_string(),
                "printf '%s\\n' \"$AIWG_A2A_MESSAGE\"".to_string()
            ]
        );
        assert_eq!(cmd.timeout_secs, 300);
        assert_eq!(cmd.env["AIWG_A2A_MESSAGE"], "hello");
        assert_eq!(cmd.env["AIWG_A2A_TASK_ID"], "task-1");
        assert_eq!(cmd.env["AIWG_A2A_INSTANCE_ID"], "inst-1");
        assert_eq!(cmd.env["AIWG_MISSION_ID"], "task-1");
    }

    #[test]
    fn build_dispatch_command_accepts_allowlisted_plan_adapter() {
        let mut body = json!({
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "run adapter"}],
                "metadata": {}
            }
        });
        body["message"]["metadata"][ADAPTER_COMMAND_URI] = json!({
            "adapter": "sandbox-agent-runner",
            "mode": "plan",
            "command": [
                "node",
                ".aiwg/ops/adapters/sandbox-agent-runner/runner.mjs",
                "--request",
                ".aiwg/ops/adapters/sandbox-agent-runner/examples/cycle-005-request.json"
            ],
            "working_dir": "/workspace",
            "timeout_seconds": 120
        });

        let cmd = AgentMessageDispatch::build_dispatch_command(&body, "task-2", "inst-2")
            .expect("adapter dispatch should build");

        assert_eq!(cmd.command, "node");
        assert_eq!(
            cmd.args,
            vec![
                ".aiwg/ops/adapters/sandbox-agent-runner/runner.mjs".to_string(),
                "--request".to_string(),
                ".aiwg/ops/adapters/sandbox-agent-runner/examples/cycle-005-request.json"
                    .to_string()
            ]
        );
        assert_eq!(cmd.working_dir, "/workspace");
        assert_eq!(cmd.timeout_secs, 120);
        assert_eq!(cmd.env["AIWG_A2A_ADAPTER"], "sandbox-agent-runner");
        assert_eq!(cmd.env["AIWG_A2A_ADAPTER_MODE"], "plan");
    }

    #[test]
    fn build_dispatch_command_accepts_allowlisted_assess_adapter() {
        let mut body = json!({
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "assess adapter"}],
                "metadata": {}
            }
        });
        body["message"]["metadata"][ADAPTER_COMMAND_URI] = json!({
            "adapter": "sandbox-agent-runner",
            "mode": "assess",
            "command": [
                "node",
                ".aiwg/ops/adapters/sandbox-agent-runner/runner.mjs",
                "--request",
                ".aiwg/ops/runs/M011/cycle-012/deterministic-assess-docker-001/request.json"
            ],
            "working_dir": "/workspace",
            "timeout_seconds": 300
        });

        let cmd = AgentMessageDispatch::build_dispatch_command(&body, "task-assess", "inst-assess")
            .expect("assess adapter dispatch should build");

        assert_eq!(cmd.command, "node");
        assert_eq!(cmd.working_dir, "/workspace");
        assert_eq!(cmd.timeout_secs, 300);
        assert_eq!(cmd.env["AIWG_A2A_ADAPTER"], "sandbox-agent-runner");
        assert_eq!(cmd.env["AIWG_A2A_ADAPTER_MODE"], "assess");
    }

    #[test]
    fn build_dispatch_command_rejects_unsupported_runner_mode() {
        let mut body = json!({
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "run adapter"}],
                "metadata": {}
            }
        });
        body["message"]["metadata"][ADAPTER_COMMAND_URI] = json!({
            "adapter": "sandbox-agent-runner",
            "mode": "apply",
            "command": [
                "node",
                ".aiwg/ops/adapters/sandbox-agent-runner/runner.mjs",
                "--request",
                ".aiwg/ops/adapters/sandbox-agent-runner/examples/cycle-005-request.json"
            ]
        });

        let err = AgentMessageDispatch::build_dispatch_command(&body, "task-mode", "inst-mode")
            .expect_err("unsupported mode should be rejected");

        assert!(err
            .to_string()
            .contains("only adapter=sandbox-agent-runner modes plan, assess are supported"));
    }

    #[test]
    fn build_dispatch_command_rejects_non_allowlisted_adapter_command() {
        let mut body = json!({
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "run adapter"}],
                "metadata": {}
            }
        });
        body["message"]["metadata"][ADAPTER_COMMAND_URI] = json!({
            "adapter": "sandbox-agent-runner",
            "mode": "plan",
            "command": ["sh", "-c", "echo unsafe"]
        });

        let err = AgentMessageDispatch::build_dispatch_command(&body, "task-3", "inst-3")
            .expect_err("unsupported command should be rejected");

        assert!(err.to_string().contains("adapter-command/v1 rejected"));
    }

    #[test]
    fn build_dispatch_command_rejects_request_path_escape() {
        let mut body = json!({
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "run adapter"}],
                "metadata": {}
            }
        });
        body["message"]["metadata"][ADAPTER_COMMAND_URI] = json!({
            "adapter": "sandbox-agent-runner",
            "mode": "plan",
            "command": [
                "node",
                ".aiwg/ops/adapters/sandbox-agent-runner/runner.mjs",
                "--request",
                "../secrets.json"
            ]
        });

        let err = AgentMessageDispatch::build_dispatch_command(&body, "task-4", "inst-4")
            .expect_err("path escape should be rejected");

        assert!(err.to_string().contains("request path is not allowed"));
    }

    #[tokio::test]
    async fn observer_transitions_to_completed_on_exit_zero() {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        seed_working_task(&store, "task-ok", "inst-1");

        let (tx, rx) = mpsc::channel::<ExecOutput>(16);
        AgentMessageDispatch::spawn_observer(
            store.clone(),
            "task-ok".to_string(),
            "cmd-1".to_string(),
            rx,
        );

        // One stdout chunk, then the terminal complete frame.
        tx.send(ExecOutput {
            stream: exec_output::Stream::Stdout as i32,
            data: b"hello\n".to_vec(),
            exit_code: 0,
            complete: false,
            error: String::new(),
        })
        .await
        .unwrap();
        tx.send(ExecOutput {
            stream: exec_output::Stream::Unknown as i32,
            data: Vec::new(),
            exit_code: 0,
            complete: true,
            error: String::new(),
        })
        .await
        .unwrap();
        drop(tx);

        // Give the observer a chance to drain.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let row = store.get_task("task-ok").unwrap().unwrap();
        assert_eq!(row.state, TaskState::Completed);
        assert!(row.terminal_at.is_some());
        let artifacts = store.list_artifacts("task-ok").unwrap();
        assert_eq!(row.status_json["mission_id"], "task-ok");
        assert_eq!(row.status_json["task_id"], "task-ok");
        assert_eq!(artifacts.len(), 1, "stdout chunk persisted as artifact");
        assert_eq!(artifacts[0].artifact_json["stream"], "stdout");
        assert_eq!(artifacts[0].artifact_json["data"], "hello\n");
        assert_eq!(artifacts[0].artifact_json["mission_id"], "task-ok");
    }

    #[tokio::test]
    async fn observer_transitions_to_failed_on_nonzero_exit() {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        seed_working_task(&store, "task-fail", "inst-1");

        let (tx, rx) = mpsc::channel::<ExecOutput>(16);
        AgentMessageDispatch::spawn_observer(
            store.clone(),
            "task-fail".to_string(),
            "cmd-2".to_string(),
            rx,
        );

        tx.send(ExecOutput {
            stream: exec_output::Stream::Stderr as i32,
            data: b"boom\n".to_vec(),
            exit_code: 0,
            complete: false,
            error: String::new(),
        })
        .await
        .unwrap();
        tx.send(ExecOutput {
            stream: exec_output::Stream::Unknown as i32,
            data: Vec::new(),
            exit_code: 1,
            complete: true,
            error: String::new(),
        })
        .await
        .unwrap();
        drop(tx);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let row = store.get_task("task-fail").unwrap().unwrap();
        assert_eq!(row.state, TaskState::Failed);
        assert_eq!(row.fail_kind, Some(FailKind::Application));
        assert!(row.terminal_at.is_some());
        let artifacts = store.list_artifacts("task-fail").unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact_json["stream"], "stderr");
    }

    #[tokio::test]
    async fn observer_transitions_to_failed_on_error_field() {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        seed_working_task(&store, "task-err", "inst-1");

        let (tx, rx) = mpsc::channel::<ExecOutput>(16);
        AgentMessageDispatch::spawn_observer(
            store.clone(),
            "task-err".to_string(),
            "cmd-3".to_string(),
            rx,
        );

        // Non-empty `error` overrides exit_code 0 — the dispatcher uses
        // this when the agent itself reports an execution failure
        // before the command produced an exit code.
        tx.send(ExecOutput {
            stream: exec_output::Stream::Unknown as i32,
            data: Vec::new(),
            exit_code: 0,
            complete: true,
            error: "agent timed out".into(),
        })
        .await
        .unwrap();
        drop(tx);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let row = store.get_task("task-err").unwrap().unwrap();
        assert_eq!(row.state, TaskState::Failed);
        assert_eq!(row.fail_kind, Some(FailKind::Infrastructure));
        assert_eq!(row.status_json["summary"], "agent timed out");
    }

    #[tokio::test]
    async fn observer_silently_exits_when_task_disappears() {
        // Defensive: a task that's been deleted between dispatch and
        // the observer's terminal frame should not panic the runtime.
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        // Note: no seed — task does not exist.
        let (tx, rx) = mpsc::channel::<ExecOutput>(16);
        AgentMessageDispatch::spawn_observer(
            store.clone(),
            "ghost".to_string(),
            "cmd-4".to_string(),
            rx,
        );
        tx.send(ExecOutput {
            stream: exec_output::Stream::Unknown as i32,
            data: Vec::new(),
            exit_code: 0,
            complete: true,
            error: String::new(),
        })
        .await
        .unwrap();
        drop(tx);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(store.get_task("ghost").unwrap().is_none());
    }
}
