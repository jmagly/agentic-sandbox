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
//!    fields and dispatch a [`CommandRequest`] carrying that text
//!    through `printf` so the agent's process supervision captures
//!    it as a normal command output stream.
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
//! ids for correlation). File / data parts are ignored. The agent
//! inside the container decides what to do with the message —
//! container-specific entrypoints can read those env vars and route
//! the prompt to whichever CLI tool they wrap (claude, codex, etc.).
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
                        "summary": summary,
                        "exit_code": chunk.exit_code,
                    });
                    row.updated_at = now;
                    row.terminal_at = Some(now);
                    if let Err(e) = store.upsert_task(&row) {
                        tracing::warn!(
                            error = %e,
                            task_id = %task_id,
                            "observer: failed to record terminal transition"
                        );
                    } else {
                        tracing::info!(
                            task_id = %task_id,
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
                });
                if let Err(e) = store.append_artifact(&task_id, &artifact_id, &artifact) {
                    // Artifact persistence is best-effort: log and keep
                    // draining the channel so the terminal frame still
                    // closes out the task.
                    tracing::warn!(
                        error = %e,
                        task_id = %task_id,
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

        // Forward via a tiny shell that prints the message text. The
        // agent's process supervision treats this as a normal command
        // — its captured output flows back through the dispatcher's
        // output_rx, which the observer below drains into TaskStore.
        let command = "sh".to_string();
        let args = vec![
            "-c".to_string(),
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
            Ok((command_id, output_rx)) => {
                tracing::info!(
                    instance_id = %instance.instance_id,
                    agent_id = %agent_id,
                    task_id = %task_id,
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
        assert_eq!(artifacts.len(), 1, "stdout chunk persisted as artifact");
        assert_eq!(artifacts[0].artifact_json["stream"], "stdout");
        assert_eq!(artifacts[0].artifact_json["data"], "hello\n");
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
