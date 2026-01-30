//! Command Dispatcher - tracks pending commands and handles responses

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::proto::{exec_output, CommandRequest, CommandResult, ExecOutput};
use crate::registry::AgentRegistry;

/// Session type determines execution behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionType {
    /// User PTY terminal (interactive tmux session)
    Interactive,
    /// Automated agent (headless, no tmux)
    Headless,
    /// Long-running background process (detached tmux)
    Background,
}

/// Information about an active session
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub session_name: String,
    pub command_id: String,
    pub session_type: SessionType,
    pub command: String,
    pub created_at: Instant,
}

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
    /// Active sessions per agent (agent_id -> (session_name -> SessionInfo))
    pub active_sessions: RwLock<HashMap<String, HashMap<String, SessionInfo>>>,
    /// Reference to agent registry for sending
    registry: Arc<AgentRegistry>,
}

impl CommandDispatcher {
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            pending: RwLock::new(HashMap::new()),
            active_sessions: RwLock::new(HashMap::new()),
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

        // Create stdin channel for interactive commands
        let (stdin_tx, _stdin_rx) = mpsc::channel::<Vec<u8>>(100);

        // Create pending command with stdin support
        let pending = PendingCommand::new(
            command_id.clone(),
            agent_id.to_string(),
            command.clone(),
            timeout_secs,
            output_tx,
            Some(stdin_tx),
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
            allocate_pty: false,
            pty_cols: 0,
            pty_rows: 0,
            pty_term: String::new(),
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

    /// Create a new session of the specified type
    pub async fn create_session(
        &self,
        agent_id: &str,
        session_name: String,
        session_type: SessionType,
        command: String,
        args: Vec<String>,
        cols: u32,
        rows: u32,
    ) -> Result<(String, mpsc::Receiver<ExecOutput>), DispatchError> {
        // Check agent exists
        if self.registry.get(agent_id).is_none() {
            return Err(DispatchError::AgentNotFound(agent_id.to_string()));
        }

        // Kill any existing session with this name
        let old_session_id = {
            let sessions = self.active_sessions.read();
            sessions
                .get(agent_id)
                .and_then(|s| s.get(&session_name))
                .map(|info| info.command_id.clone())
        };

        if let Some(old_id) = old_session_id {
            info!("Killing old session {} for agent {}:{}", old_id, agent_id, session_name);
            match session_type {
                SessionType::Interactive | SessionType::Background => {
                    // SIGHUP (1) causes tmux client to detach; session persists
                    let _ = self.send_pty_signal(&old_id, 1).await;
                }
                SessionType::Headless => {
                    // Kill headless sessions directly
                    let _ = self.cancel(&old_id);
                }
            }
            // Remove old pending entry
            self.pending.write().remove(&old_id);
        }

        let command_id = Uuid::new_v4().to_string();
        let (output_tx, output_rx) = mpsc::channel::<ExecOutput>(100);

        // Build command based on session type
        let (final_command, final_args, allocate_pty) = match session_type {
            SessionType::Interactive => {
                // tmux new-session -A -s <session>: creates or attaches
                (
                    "tmux".to_string(),
                    vec![
                        "new-session".to_string(),
                        "-A".to_string(),
                        "-s".to_string(),
                        session_name.clone(),
                    ],
                    true,
                )
            }
            SessionType::Headless => {
                // Run command directly without tmux or PTY
                (command.clone(), args.clone(), false)
            }
            SessionType::Background => {
                // tmux new-session -d -s <session> <command>: detached session
                let mut tmux_args = vec![
                    "new-session".to_string(),
                    "-d".to_string(),
                    "-s".to_string(),
                    session_name.clone(),
                    command.clone(),
                ];
                tmux_args.extend(args.clone());
                ("tmux".to_string(), tmux_args, false)
            }
        };

        let pending = PendingCommand::new(
            command_id.clone(),
            agent_id.to_string(),
            final_command.clone(),
            0, // no timeout for sessions
            output_tx,
            None,
        );

        self.pending.write().insert(command_id.clone(), pending);

        // Build command request
        let cmd = CommandRequest {
            command_id: command_id.clone(),
            command: final_command.clone(),
            args: final_args,
            working_dir: String::new(),
            env: HashMap::new(),
            timeout_seconds: 0,
            capture_output: true,
            run_as: String::new(),
            allocate_pty,
            pty_cols: cols,
            pty_rows: rows,
            pty_term: "xterm-256color".to_string(),
        };

        let msg = crate::proto::ManagementMessage {
            payload: Some(crate::proto::management_message::Payload::Command(cmd)),
        };

        if !self.registry.send_command(agent_id, msg).await {
            self.pending.write().remove(&command_id);
            return Err(DispatchError::SendFailed(agent_id.to_string()));
        }

        // Track session info
        let session_info = SessionInfo {
            session_name: session_name.clone(),
            command_id: command_id.clone(),
            session_type,
            command,
            created_at: Instant::now(),
        };

        self.active_sessions
            .write()
            .entry(agent_id.to_string())
            .or_insert_with(HashMap::new)
            .insert(session_name.clone(), session_info);

        info!(
            "Created {:?} session {} for agent {}:{}",
            session_type, command_id, agent_id, session_name
        );
        Ok((command_id, output_rx))
    }

    /// Dispatch an interactive shell (PTY) to an agent.
    /// Uses tmux for session persistence across reconnects.
    /// Kills any existing shell session for this agent and session name first.
    pub async fn dispatch_shell(
        &self,
        agent_id: &str,
        session_name: Option<String>,
        cols: u32,
        rows: u32,
    ) -> Result<(String, mpsc::Receiver<ExecOutput>), DispatchError> {
        let session = session_name.unwrap_or_else(|| "main".to_string());
        self.create_session(
            agent_id,
            session,
            SessionType::Interactive,
            String::new(),
            Vec::new(),
            cols,
            rows,
        )
        .await
    }

    /// Get list of active session infos for an agent
    pub fn get_active_sessions(&self, agent_id: &str) -> Vec<SessionInfo> {
        self.active_sessions
            .read()
            .get(agent_id)
            .map(|sessions| sessions.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Send PTY resize to a running command
    pub async fn send_pty_resize(
        &self,
        command_id: &str,
        cols: u32,
        rows: u32,
    ) -> Result<(), DispatchError> {
        let agent_id = {
            let pending = self.pending.read();
            pending.get(command_id).map(|p| p.agent_id.clone())
        };

        let agent_id = match agent_id {
            Some(id) => id,
            None => return Err(DispatchError::CommandNotFound(command_id.to_string())),
        };

        let pty_control = crate::proto::PtyControl {
            command_id: command_id.to_string(),
            action: Some(crate::proto::pty_control::Action::Resize(
                crate::proto::PtyResize { cols, rows },
            )),
        };

        let msg = crate::proto::ManagementMessage {
            payload: Some(crate::proto::management_message::Payload::PtyControl(pty_control)),
        };

        if self.registry.send_command(&agent_id, msg).await {
            debug!("Sent PTY resize to command {}", command_id);
            Ok(())
        } else {
            Err(DispatchError::SendFailed(agent_id))
        }
    }

    /// Send a signal to a PTY session's child process
    pub async fn send_pty_signal(
        &self,
        command_id: &str,
        signal_number: i32,
    ) -> Result<(), DispatchError> {
        let agent_id = {
            let pending = self.pending.read();
            pending.get(command_id).map(|p| p.agent_id.clone())
        };

        let agent_id = match agent_id {
            Some(id) => id,
            None => return Err(DispatchError::CommandNotFound(command_id.to_string())),
        };

        let pty_control = crate::proto::PtyControl {
            command_id: command_id.to_string(),
            action: Some(crate::proto::pty_control::Action::Signal(
                crate::proto::PtySignal { signal_number },
            )),
        };

        let msg = crate::proto::ManagementMessage {
            payload: Some(crate::proto::management_message::Payload::PtyControl(pty_control)),
        };

        if self.registry.send_command(&agent_id, msg).await {
            debug!("Sent signal {} to command {}", signal_number, command_id);
            Ok(())
        } else {
            Err(DispatchError::SendFailed(agent_id))
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Mock registry that always returns Some agent and succeeds at sending
    struct MockRegistry;

    impl MockRegistry {
        fn new() -> Arc<AgentRegistry> {
            // This is a simplified mock. In a real test, you'd use a proper mock
            // or test double. For now, we'll test the data structures directly.
            Arc::new(AgentRegistry::new())
        }
    }

    // Session Type Tests

    #[test]
    fn test_session_type_serialization() {
        // Test that SessionType serializes correctly
        assert_eq!(
            serde_json::to_string(&SessionType::Interactive).unwrap(),
            "\"interactive\""
        );
        assert_eq!(
            serde_json::to_string(&SessionType::Headless).unwrap(),
            "\"headless\""
        );
        assert_eq!(
            serde_json::to_string(&SessionType::Background).unwrap(),
            "\"background\""
        );
    }

    #[test]
    fn test_session_info_creation() {
        let session_info = SessionInfo {
            session_name: "test".to_string(),
            command_id: "cmd-123".to_string(),
            session_type: SessionType::Interactive,
            command: "bash".to_string(),
            created_at: Instant::now(),
        };

        assert_eq!(session_info.session_name, "test");
        assert_eq!(session_info.command_id, "cmd-123");
        assert_eq!(session_info.session_type, SessionType::Interactive);
        assert_eq!(session_info.command, "bash");
    }

    #[test]
    fn test_active_sessions_track_session_info() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Manually insert sessions with different types
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions.entry("agent-01".to_string()).or_insert_with(HashMap::new);

            agent_sessions.insert("main".to_string(), SessionInfo {
                session_name: "main".to_string(),
                command_id: "cmd-001".to_string(),
                session_type: SessionType::Interactive,
                command: "tmux".to_string(),
                created_at: Instant::now(),
            });

            agent_sessions.insert("claude".to_string(), SessionInfo {
                session_name: "claude".to_string(),
                command_id: "cmd-002".to_string(),
                session_type: SessionType::Headless,
                command: "claude --print".to_string(),
                created_at: Instant::now(),
            });

            agent_sessions.insert("worker".to_string(), SessionInfo {
                session_name: "worker".to_string(),
                command_id: "cmd-003".to_string(),
                session_type: SessionType::Background,
                command: "long-running-job".to_string(),
                created_at: Instant::now(),
            });
        }

        let sessions = dispatcher.get_active_sessions("agent-01");
        assert_eq!(sessions.len(), 3);

        // Find each session and verify its type
        let main_session = sessions.iter().find(|s| s.session_name == "main").unwrap();
        assert_eq!(main_session.session_type, SessionType::Interactive);

        let claude_session = sessions.iter().find(|s| s.session_name == "claude").unwrap();
        assert_eq!(claude_session.session_type, SessionType::Headless);

        let worker_session = sessions.iter().find(|s| s.session_name == "worker").unwrap();
        assert_eq!(worker_session.session_type, SessionType::Background);
    }

    #[test]
    fn test_get_active_sessions_returns_session_info() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Insert a test session
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions.entry("agent-01".to_string()).or_insert_with(HashMap::new);
            agent_sessions.insert("test".to_string(), SessionInfo {
                session_name: "test".to_string(),
                command_id: "cmd-001".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });
        }

        let sessions = dispatcher.get_active_sessions("agent-01");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_name, "test");
        assert_eq!(sessions[0].command_id, "cmd-001");
        assert_eq!(sessions[0].session_type, SessionType::Interactive);
        assert_eq!(sessions[0].command, "bash");
    }

    #[test]
    fn test_active_sessions_multiple_types() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Setup multiple session types
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions.entry("agent-01".to_string()).or_insert_with(HashMap::new);

            agent_sessions.insert("interactive1".to_string(), SessionInfo {
                session_name: "interactive1".to_string(),
                command_id: "cmd-001".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });

            agent_sessions.insert("headless1".to_string(), SessionInfo {
                session_name: "headless1".to_string(),
                command_id: "cmd-002".to_string(),
                session_type: SessionType::Headless,
                command: "python script.py".to_string(),
                created_at: Instant::now(),
            });

            agent_sessions.insert("background1".to_string(), SessionInfo {
                session_name: "background1".to_string(),
                command_id: "cmd-003".to_string(),
                session_type: SessionType::Background,
                command: "worker --daemon".to_string(),
                created_at: Instant::now(),
            });
        }

        let sessions = dispatcher.get_active_sessions("agent-01");
        assert_eq!(sessions.len(), 3);

        // Count each type
        let interactive_count = sessions.iter().filter(|s| s.session_type == SessionType::Interactive).count();
        let headless_count = sessions.iter().filter(|s| s.session_type == SessionType::Headless).count();
        let background_count = sessions.iter().filter(|s| s.session_type == SessionType::Background).count();

        assert_eq!(interactive_count, 1);
        assert_eq!(headless_count, 1);
        assert_eq!(background_count, 1);
    }

    #[test]
    fn test_session_type_isolation_across_agents() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Setup sessions for multiple agents
        {
            let mut sessions = dispatcher.active_sessions.write();

            let agent1_sessions = sessions.entry("agent-01".to_string()).or_insert_with(HashMap::new);
            agent1_sessions.insert("work".to_string(), SessionInfo {
                session_name: "work".to_string(),
                command_id: "cmd-001".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });

            let agent2_sessions = sessions.entry("agent-02".to_string()).or_insert_with(HashMap::new);
            agent2_sessions.insert("work".to_string(), SessionInfo {
                session_name: "work".to_string(),
                command_id: "cmd-002".to_string(),
                session_type: SessionType::Headless,
                command: "python".to_string(),
                created_at: Instant::now(),
            });
        }

        let agent1_sessions = dispatcher.get_active_sessions("agent-01");
        let agent2_sessions = dispatcher.get_active_sessions("agent-02");

        assert_eq!(agent1_sessions.len(), 1);
        assert_eq!(agent2_sessions.len(), 1);

        assert_eq!(agent1_sessions[0].session_type, SessionType::Interactive);
        assert_eq!(agent2_sessions[0].session_type, SessionType::Headless);
    }

    #[test]
    fn test_empty_sessions_for_unknown_agent() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        let sessions = dispatcher.get_active_sessions("nonexistent-agent");
        assert_eq!(sessions.len(), 0);
    }

    // Legacy tests (updated for new structure)

    #[test]
    fn test_active_shells_multiple_sessions() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Manually insert sessions for testing
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions.entry("agent-01".to_string()).or_insert_with(HashMap::new);

            for (name, cmd_id) in [("main", "cmd-001"), ("debug", "cmd-002"), ("test", "cmd-003")] {
                agent_sessions.insert(name.to_string(), SessionInfo {
                    session_name: name.to_string(),
                    command_id: cmd_id.to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                });
            }
        }

        let session_infos = dispatcher.get_active_sessions("agent-01");
        assert_eq!(session_infos.len(), 3);

        let names: Vec<String> = session_infos.iter().map(|s| s.session_name.clone()).collect();
        assert!(names.contains(&"main".to_string()));
        assert!(names.contains(&"debug".to_string()));
        assert!(names.contains(&"test".to_string()));
    }

    #[test]
    fn test_active_sessions_empty_for_unknown_agent() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        let sessions = dispatcher.get_active_sessions("nonexistent-agent");
        assert_eq!(sessions.len(), 0);
    }

    #[test]
    fn test_active_sessions_multiple_agents() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Setup multiple agents with different sessions
        {
            let mut sessions = dispatcher.active_sessions.write();

            let agent1_sessions = sessions.entry("agent-01".to_string()).or_insert_with(HashMap::new);
            agent1_sessions.insert("main".to_string(), SessionInfo {
                session_name: "main".to_string(),
                command_id: "cmd-001".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });
            agent1_sessions.insert("debug".to_string(), SessionInfo {
                session_name: "debug".to_string(),
                command_id: "cmd-002".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });

            let agent2_sessions = sessions.entry("agent-02".to_string()).or_insert_with(HashMap::new);
            agent2_sessions.insert("main".to_string(), SessionInfo {
                session_name: "main".to_string(),
                command_id: "cmd-003".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });
            agent2_sessions.insert("work".to_string(), SessionInfo {
                session_name: "work".to_string(),
                command_id: "cmd-004".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });
        }

        let agent1_sessions = dispatcher.get_active_sessions("agent-01");
        assert_eq!(agent1_sessions.len(), 2);
        let agent1_names: Vec<String> = agent1_sessions.iter().map(|s| s.session_name.clone()).collect();
        assert!(agent1_names.contains(&"main".to_string()));
        assert!(agent1_names.contains(&"debug".to_string()));

        let agent2_sessions = dispatcher.get_active_sessions("agent-02");
        assert_eq!(agent2_sessions.len(), 2);
        let agent2_names: Vec<String> = agent2_sessions.iter().map(|s| s.session_name.clone()).collect();
        assert!(agent2_names.contains(&"main".to_string()));
        assert!(agent2_names.contains(&"work".to_string()));
    }

    #[test]
    fn test_session_name_isolation() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Same session name across different agents should be isolated
        {
            let mut sessions = dispatcher.active_sessions.write();

            let agent1_sessions = sessions.entry("agent-01".to_string()).or_insert_with(HashMap::new);
            agent1_sessions.insert("main".to_string(), SessionInfo {
                session_name: "main".to_string(),
                command_id: "cmd-001".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });

            let agent2_sessions = sessions.entry("agent-02".to_string()).or_insert_with(HashMap::new);
            agent2_sessions.insert("main".to_string(), SessionInfo {
                session_name: "main".to_string(),
                command_id: "cmd-002".to_string(),
                session_type: SessionType::Interactive,
                command: "bash".to_string(),
                created_at: Instant::now(),
            });
        }

        // Verify each agent's "main" session has a different command ID
        let agent1_cmd = {
            let sessions = dispatcher.active_sessions.read();
            sessions.get("agent-01")
                .and_then(|s| s.get("main"))
                .map(|info| info.command_id.clone())
        };

        let agent2_cmd = {
            let sessions = dispatcher.active_sessions.read();
            sessions.get("agent-02")
                .and_then(|s| s.get("main"))
                .map(|info| info.command_id.clone())
        };

        assert_eq!(agent1_cmd, Some("cmd-001".to_string()));
        assert_eq!(agent2_cmd, Some("cmd-002".to_string()));
    }

    // Stdin support tests

    /// Test that dispatch creates stdin channel for non-PTY commands
    #[test]
    fn test_dispatch_creates_stdin_channel() {
        let command_id = "test-cmd".to_string();
        let (output_tx, _output_rx) = mpsc::channel::<ExecOutput>(100);
        let (stdin_tx, _stdin_rx) = mpsc::channel::<Vec<u8>>(100);

        let pending = PendingCommand::new(
            command_id.clone(),
            "test-agent".to_string(),
            "echo".to_string(),
            30,
            output_tx,
            Some(stdin_tx),
        );

        // Verify stdin_tx is Some
        assert!(pending.stdin_tx.is_some(), "stdin_tx should be initialized for non-PTY commands");
    }

    /// Test send_stdin with non-existent command
    #[tokio::test]
    async fn test_send_stdin_command_not_found() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        let result = dispatcher.send_stdin("nonexistent-cmd", vec![1, 2, 3]).await;

        assert!(result.is_err(), "Should fail for nonexistent command");
        match result.unwrap_err() {
            DispatchError::CommandNotFound(id) => {
                assert_eq!(id, "nonexistent-cmd");
            }
            _ => panic!("Expected CommandNotFound error"),
        }
    }

    /// Test stdin channel cleanup on command complete
    #[tokio::test]
    async fn test_stdin_cleanup_on_completion() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Create a pending command with stdin
        let command_id = "cleanup-test".to_string();
        let (output_tx, _output_rx) = mpsc::channel::<ExecOutput>(100);
        let (stdin_tx, _stdin_rx) = mpsc::channel::<Vec<u8>>(100);

        let pending = PendingCommand::new(
            command_id.clone(),
            "test-agent".to_string(),
            "cat".to_string(),
            30,
            output_tx,
            Some(stdin_tx),
        );

        dispatcher.pending.write().insert(command_id.clone(), pending);

        // Verify command exists
        assert_eq!(dispatcher.pending_count(), 1);

        // Complete the command
        let result = CommandResult {
            command_id: command_id.clone(),
            exit_code: 0,
            success: true,
            error: String::new(),
            duration_ms: 100,
        };
        dispatcher.handle_result(result);

        // Verify command was removed (stdin channel dropped automatically)
        assert_eq!(dispatcher.pending_count(), 0);
    }

    /// Test that PendingCommand can be created with None stdin_tx (for PTY)
    #[test]
    fn test_pending_command_without_stdin() {
        let command_id = "pty-test".to_string();
        let (output_tx, _output_rx) = mpsc::channel::<ExecOutput>(100);

        let pending = PendingCommand::new(
            command_id.clone(),
            "test-agent".to_string(),
            "bash".to_string(),
            0,
            output_tx,
            None, // PTY commands don't need stdin_tx
        );

        assert!(pending.stdin_tx.is_none(), "PTY commands should have None stdin_tx");
    }

    /// Test that stdin_tx can be retrieved from pending command
    #[test]
    fn test_stdin_tx_accessible() {
        let command_id = "stdin-access-test".to_string();
        let (output_tx, _output_rx) = mpsc::channel::<ExecOutput>(100);
        let (stdin_tx, _stdin_rx) = mpsc::channel::<Vec<u8>>(100);

        let pending = PendingCommand::new(
            command_id,
            "test-agent".to_string(),
            "cat".to_string(),
            30,
            output_tx,
            Some(stdin_tx),
        );

        // Verify we can access stdin_tx
        assert!(pending.stdin_tx.is_some());

        // Clone it (simulating what dispatcher.send_stdin would do)
        let _stdin_tx_clone = pending.stdin_tx.clone();
        assert!(pending.stdin_tx.is_some(), "stdin_tx should still exist after clone");
    }
}
