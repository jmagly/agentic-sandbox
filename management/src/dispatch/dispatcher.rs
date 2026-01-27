//! Command Dispatcher - tracks pending commands and handles responses

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::proto::{exec_output, CommandRequest, CommandResult, ExecOutput};
use crate::registry::AgentRegistry;

/// Tracks a pending command awaiting response
#[derive(Debug)]
#[allow(dead_code)]
pub struct PendingCommand {
    pub command_id: String,
    pub agent_id: String,
    pub command: String,
    pub started_at: Instant,
    pub timeout: Duration,
    /// Channel to send output chunks
    pub output_tx: mpsc::Sender<ExecOutput>,
    /// Receives final result
    pub result_rx: Option<oneshot::Receiver<CommandResult>>,
    /// Sends final result
    result_tx: Option<oneshot::Sender<CommandResult>>,
    /// Channel to send stdin data to agent
    pub stdin_tx: Option<mpsc::Sender<Vec<u8>>>,
}

#[allow(dead_code)]
impl PendingCommand {
    pub fn new(
        command_id: String,
        agent_id: String,
        command: String,
        timeout_secs: u32,
        output_tx: mpsc::Sender<ExecOutput>,
        stdin_tx: Option<mpsc::Sender<Vec<u8>>>,
    ) -> Self {
        let (result_tx, result_rx) = oneshot::channel();
        Self {
            command_id,
            agent_id,
            command,
            started_at: Instant::now(),
            timeout: Duration::from_secs(timeout_secs as u64),
            output_tx,
            result_rx: Some(result_rx),
            result_tx: Some(result_tx),
            stdin_tx,
        }
    }

    /// Check if command has timed out
    pub fn is_timed_out(&self) -> bool {
        self.started_at.elapsed() > self.timeout
    }

    /// Take the result receiver (can only be called once)
    pub fn take_result_rx(&mut self) -> Option<oneshot::Receiver<CommandResult>> {
        self.result_rx.take()
    }

    /// Complete the command with a result
    pub fn complete(&mut self, result: CommandResult) -> bool {
        if let Some(tx) = self.result_tx.take() {
            tx.send(result).is_ok()
        } else {
            false
        }
    }
}

/// Dispatches commands to agents and tracks responses
pub struct CommandDispatcher {
    /// Pending commands by command_id
    pending: RwLock<HashMap<String, PendingCommand>>,
    /// Reference to agent registry for sending
    registry: Arc<AgentRegistry>,
}

impl CommandDispatcher {
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            pending: RwLock::new(HashMap::new()),
            registry,
        }
    }

    /// Dispatch a command to an agent, returning a stream of output
    pub async fn dispatch(
        &self,
        agent_id: &str,
        command: String,
        args: Vec<String>,
        working_dir: String,
        env: HashMap<String, String>,
        timeout_secs: u32,
    ) -> Result<(String, mpsc::Receiver<ExecOutput>), DispatchError> {
        // Check agent exists
        if self.registry.get(agent_id).is_none() {
            return Err(DispatchError::AgentNotFound(agent_id.to_string()));
        }

        // Generate command ID
        let command_id = Uuid::new_v4().to_string();

        // Create output channel
        let (output_tx, output_rx) = mpsc::channel::<ExecOutput>(100);

        // Create pending command (stdin_tx will be set when stdin support is added)
        let pending = PendingCommand::new(
            command_id.clone(),
            agent_id.to_string(),
            command.clone(),
            timeout_secs,
            output_tx,
            None, // TODO: stdin_tx for interactive commands
        );

        // Store pending command
        self.pending.write().insert(command_id.clone(), pending);

        // Build command request
        let cmd = CommandRequest {
            command_id: command_id.clone(),
            command,
            args,
            working_dir,
            env,
            timeout_seconds: timeout_secs as i32,
            capture_output: true,
            run_as: String::new(),
        };

        // Send to agent
        let msg = crate::proto::ManagementMessage {
            payload: Some(crate::proto::management_message::Payload::Command(cmd)),
        };

        if !self.registry.send_command(agent_id, msg).await {
            // Remove pending on failure
            self.pending.write().remove(&command_id);
            return Err(DispatchError::SendFailed(agent_id.to_string()));
        }

        info!(
            "Dispatched command {} to agent {}",
            command_id, agent_id
        );

        Ok((command_id, output_rx))
    }

    /// Handle stdout chunk from agent
    pub async fn handle_stdout(&self, command_id: &str, _stream_id: &str, data: Vec<u8>) {
        let tx = {
            let pending = self.pending.read();
            pending.get(command_id).map(|p| p.output_tx.clone())
        };

        if let Some(tx) = tx {
            let output = ExecOutput {
                stream: exec_output::Stream::Stdout as i32,
                data,
                exit_code: 0,
                complete: false,
                error: String::new(),
            };
            if tx.send(output).await.is_err() {
                debug!("Output channel closed for command {}", command_id);
            }
        } else {
            warn!("Received stdout for unknown command: {}", command_id);
        }
    }

    /// Handle stderr chunk from agent
    pub async fn handle_stderr(&self, command_id: &str, _stream_id: &str, data: Vec<u8>) {
        let tx = {
            let pending = self.pending.read();
            pending.get(command_id).map(|p| p.output_tx.clone())
        };

        if let Some(tx) = tx {
            let output = ExecOutput {
                stream: exec_output::Stream::Stderr as i32,
                data,
                exit_code: 0,
                complete: false,
                error: String::new(),
            };
            if tx.send(output).await.is_err() {
                debug!("Output channel closed for command {}", command_id);
            }
        } else {
            warn!("Received stderr for unknown command: {}", command_id);
        }
    }

    /// Handle command completion from agent
    pub fn handle_result(&self, result: CommandResult) {
        let command_id = &result.command_id;

        if let Some(mut pending) = self.pending.write().remove(command_id) {
            info!(
                "Command {} completed: exit={}, success={}, duration={}ms",
                command_id, result.exit_code, result.success, result.duration_ms
            );

            // Send final output marker
            let final_output = ExecOutput {
                stream: exec_output::Stream::Unknown as i32,
                data: Vec::new(),
                exit_code: result.exit_code,
                complete: true,
                error: result.error.clone(),
            };

            // Send final marker (ignore error if channel closed)
            let tx = pending.output_tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(final_output).await;
            });

            // Complete with result
            pending.complete(result);
        } else {
            warn!("Received result for unknown command: {}", command_id);
        }
    }

    /// Cancel a pending command
    #[allow(dead_code)]
    pub fn cancel(&self, command_id: &str) -> bool {
        if let Some(mut pending) = self.pending.write().remove(command_id) {
            info!("Cancelled command {}", command_id);

            // Send cancellation result
            let result = CommandResult {
                command_id: command_id.to_string(),
                exit_code: -1,
                success: false,
                error: "Cancelled".to_string(),
                duration_ms: pending.started_at.elapsed().as_millis() as i64,
            };
            pending.complete(result);
            true
        } else {
            false
        }
    }

    /// Get count of pending commands
    #[allow(dead_code)]
    pub fn pending_count(&self) -> usize {
        self.pending.read().len()
    }

    /// Clean up timed out commands
    #[allow(dead_code)]
    pub fn cleanup_timeouts(&self) -> Vec<String> {
        let mut timed_out = Vec::new();
        let mut pending = self.pending.write();

        pending.retain(|id, cmd| {
            if cmd.is_timed_out() {
                timed_out.push(id.clone());
                false
            } else {
                true
            }
        });

        for id in &timed_out {
            warn!("Command {} timed out", id);
        }

        timed_out
    }

    /// Send stdin data to a running command
    pub async fn send_stdin(&self, command_id: &str, data: Vec<u8>) -> Result<(), DispatchError> {
        // Get the agent_id for this command
        let agent_id = {
            let pending = self.pending.read();
            pending.get(command_id).map(|p| p.agent_id.clone())
        };

        let agent_id = match agent_id {
            Some(id) => id,
            None => return Err(DispatchError::CommandNotFound(command_id.to_string())),
        };

        // Build stdin message
        let stdin_chunk = crate::proto::StdinChunk {
            command_id: command_id.to_string(),
            data,
            eof: false,
        };

        let msg = crate::proto::ManagementMessage {
            payload: Some(crate::proto::management_message::Payload::Stdin(stdin_chunk)),
        };

        // Send to agent
        if self.registry.send_command(&agent_id, msg).await {
            debug!("Sent stdin to command {}", command_id);
            Ok(())
        } else {
            Err(DispatchError::SendFailed(agent_id))
        }
    }
}

/// Errors that can occur during command dispatch
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum DispatchError {
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Failed to send command to agent: {0}")]
    SendFailed(String),

    #[error("Command not found: {0}")]
    CommandNotFound(String),

    #[error("Command timed out")]
    Timeout,
}
