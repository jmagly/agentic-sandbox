//! Command Dispatcher - tracks pending commands and handles responses

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::http::events::emit_command_started;
use crate::proto::{exec_output, CommandRequest, CommandResult, ExecOutput};
use crate::registry::AgentRegistry;

/// Observer that is notified of every inbound `OutputChunk` from any agent
/// and of agent-disconnect events. Added for the v2 PTY bridge (#243): the
/// existing `pending`-keyed routing in [`handle_stdout`] only delivers to
/// commands the dispatcher itself originated, so v2 PTY sessions that
/// bypass the v1 dispatch path need a parallel delivery channel. The
/// observer is a tee, not a replacement — v1 routing keeps working
/// unchanged when no observer is set.
///
/// Methods are non-blocking and called from inside the dispatcher's lock
/// scope; implementations must not perform `.await` work synchronously
/// and should off-load to a background task if real I/O is required.
pub trait OutputObserver: Send + Sync + 'static {
    /// Called for every inbound stdout/stderr/log chunk on the agent
    /// gRPC stream. `command_id` is the `stream_id` field of the proto
    /// `OutputChunk` (the dispatcher already treats them as the same
    /// thing for v1 routing). Implementations decide whether the chunk
    /// is interesting based on their own routing table.
    fn on_output(&self, command_id: &str, data: &[u8]);

    /// Called when the agent reports final command completion.
    fn on_result(&self, command_id: &str, exit_code: i32, success: bool, error: &str);

    /// Called when an agent disconnects (gRPC stream ends, registry
    /// `unregister`, or `cleanup_agent` fires). The observer should
    /// drop any session-side state keyed on this `agent_id`.
    fn on_agent_disconnect(&self, agent_id: &str);
}

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
    /// Stable session identity (survives command_id changes on reconnect).
    pub session_id: String,
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
    /// Reverse index: command_id → session_id (for routing output to session registry).
    command_to_session: RwLock<HashMap<String, String>>,
    /// Per-WS-connection PTY ownership: ws_id → {command_ids}.
    /// Used to SIGHUP owned PTYs when the WS connection closes, without
    /// disturbing other clients attached to the same tmux session.
    ws_pty_ownership: RwLock<HashMap<String, HashSet<String>>>,
    /// Reference to agent registry for sending
    registry: Arc<AgentRegistry>,
    /// Optional handle to push session events to aiwg serve.
    aiwg: Option<crate::aiwg_serve::AiwgServeHandle>,
    /// Optional mission store — when present, session lifecycle events are
    /// also translated into executor-contract `mission.*` events (#193 pass 2).
    /// Populated by the dispatch route in Pass 3.
    mission_store: Option<crate::aiwg_serve::MissionStore>,
    /// Formal session registry (multicast, replay, roles).
    session_registry: Option<Arc<crate::session::SessionRegistry>>,
    /// Output observer for the v2 PTY bridge (#243). Tee'd into every
    /// `handle_stdout`/`handle_stderr` call so the bridge can route
    /// `OutputChunk` bytes to the right (instance_id, session_id) WS
    /// fanout. Optional — `None` preserves legacy v1 behavior exactly.
    output_observer: RwLock<Option<Arc<dyn OutputObserver>>>,
}

impl CommandDispatcher {
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            pending: RwLock::new(HashMap::new()),
            active_sessions: RwLock::new(HashMap::new()),
            command_to_session: RwLock::new(HashMap::new()),
            ws_pty_ownership: RwLock::new(HashMap::new()),
            registry,
            aiwg: None,
            mission_store: None,
            session_registry: None,
            output_observer: RwLock::new(None),
        }
    }

    /// Install an [`OutputObserver`] (e.g. the v2 PTY bridge) that is
    /// notified of every inbound `OutputChunk` and of agent disconnects.
    /// Calling this replaces any previously installed observer. Pass
    /// `None` (via [`Self::clear_output_observer`]) to detach.
    pub fn set_output_observer(&self, obs: Arc<dyn OutputObserver>) {
        *self.output_observer.write() = Some(obs);
    }

    /// Remove any installed [`OutputObserver`]. Restores pre-#243 dispatch
    /// behavior exactly.
    pub fn clear_output_observer(&self) {
        *self.output_observer.write() = None;
    }

    /// Snapshot of the installed observer, if any. Held across `.await`
    /// points by callers because Arc clone is cheap and the
    /// `OutputObserver` trait is `Send + Sync + 'static`.
    fn output_observer_snapshot(&self) -> Option<Arc<dyn OutputObserver>> {
        self.output_observer.read().clone()
    }

    /// Attach an aiwg serve handle for session lifecycle event push.
    pub fn with_aiwg_serve(mut self, handle: crate::aiwg_serve::AiwgServeHandle) -> Self {
        self.aiwg = Some(handle);
        self
    }

    /// Attach a mission store so session lifecycle events also emit
    /// executor-contract `mission.*` events when the session belongs to
    /// a known mission (#193 pass 2).
    pub fn with_mission_store(mut self, store: crate::aiwg_serve::MissionStore) -> Self {
        self.mission_store = Some(store);
        self
    }

    /// Attach the formal session registry for multicast/replay routing.
    pub fn with_session_registry(mut self, registry: Arc<crate::session::SessionRegistry>) -> Self {
        self.session_registry = Some(registry);
        self
    }

    /// Return the formal session registry configured for this dispatcher.
    ///
    /// Bridge adapters use this to project non-legacy attach surfaces into
    /// the canonical session membership/replay model without reaching into
    /// dispatcher's private fields.
    pub fn formal_session_registry(&self) -> Option<Arc<crate::session::SessionRegistry>> {
        self.session_registry.clone()
    }

    /// Look up the stable session_id for a given command_id.
    pub fn session_id_for_command(&self, command_id: &str) -> Option<String> {
        self.command_to_session.read().get(command_id).cloned()
    }

    fn remove_active_session_by_command(&self, command_id: &str) -> Option<(String, SessionInfo)> {
        let mut sessions = self.active_sessions.write();
        for (agent_id, agent_sessions) in sessions.iter_mut() {
            let session_name = agent_sessions
                .iter()
                .find_map(|(name, info)| (info.command_id == command_id).then(|| name.clone()));
            if let Some(session_name) = session_name {
                let removed = agent_sessions.remove(&session_name)?;
                return Some((agent_id.clone(), removed));
            }
        }
        None
    }

    fn agent_id_for_command(&self, command_id: &str) -> Option<String> {
        if let Some(agent_id) = self
            .pending
            .read()
            .get(command_id)
            .map(|p| p.agent_id.clone())
        {
            return Some(agent_id);
        }

        let sessions = self.active_sessions.read();
        sessions.iter().find_map(|(agent_id, agent_sessions)| {
            agent_sessions
                .values()
                .any(|info| info.command_id == command_id)
                .then(|| agent_id.clone())
        })
    }

    /// Register a PTY session that is owned by an external transport
    /// such as `pty-ws/v1`, rather than by the dispatcher's
    /// `PendingCommand` table. This makes the session visible through
    /// the formal session registry and lets inbound output/result frames
    /// populate formal replay/close state via `command_to_session`.
    pub fn register_external_pty_session(
        &self,
        agent_id: &str,
        session_id: &str,
        command_id: &str,
        session_name: Option<String>,
        command: String,
    ) {
        let session_name = session_name.unwrap_or_else(|| session_id.to_string());
        let session_info = SessionInfo {
            session_name: session_name.clone(),
            session_id: session_id.to_string(),
            command_id: command_id.to_string(),
            session_type: SessionType::Interactive,
            command,
            created_at: Instant::now(),
        };
        self.active_sessions
            .write()
            .entry(agent_id.to_string())
            .or_default()
            .insert(session_name.clone(), session_info);
        self.command_to_session
            .write()
            .insert(command_id.to_string(), session_id.to_string());
        if let Some(ref sr) = self.session_registry {
            sr.create(
                session_id.to_string(),
                agent_id.to_string(),
                command_id.to_string(),
                Some(session_name),
            );
        }
    }

    /// Roll back an external PTY session registration when the start
    /// command fails before the agent owns the process.
    pub fn rollback_external_pty_session(&self, command_id: &str) {
        let session_id = self.command_to_session.write().remove(command_id);
        self.remove_active_session_by_command(command_id);
        if let (Some(ref sr), Some(session_id)) = (&self.session_registry, session_id) {
            sr.forget(&session_id);
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

        // Save command for event emission before it's moved
        let command_for_event = command.clone();

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

        // Emit command started event
        emit_command_started(agent_id, &command_id, &command_for_event).await;

        info!("Dispatched command {} to agent {}", command_id, agent_id);

        Ok((command_id, output_rx))
    }

    /// Handle stdout chunk from agent.
    /// Returns true if the command exists, false if it should be dropped.
    /// Also routes to the formal session registry for multicast/replay.
    pub async fn handle_stdout(&self, command_id: &str, _stream_id: &str, data: Vec<u8>) -> bool {
        // Tee to the v2 PTY bridge (#243) BEFORE the v1 routing tables
        // are consulted. The observer is a fan-out, not a replacement;
        // v1 commands still see their output via the `pending` map below.
        if let Some(obs) = self.output_observer_snapshot() {
            obs.on_output(command_id, &data);
        }

        let tx = {
            let pending = self.pending.read();
            pending.get(command_id).map(|p| p.output_tx.clone())
        };

        // Route to session registry (multicast to all session clients + replay buffer).
        let session_id_opt = self.command_to_session.read().get(command_id).cloned();
        if let (Some(ref sr), Some(session_id)) = (&self.session_registry, session_id_opt) {
            sr.publish_output(
                &session_id,
                crate::session::StreamKind::Stdout,
                data.clone(),
            )
            .await;
        }

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
            true
        } else {
            // Silently drop output for unknown commands (orphaned sessions)
            debug!("Dropping stdout for orphaned command: {}", command_id);
            false
        }
    }

    /// Handle stderr chunk from agent.
    /// Returns true if the command exists, false if it should be dropped.
    pub async fn handle_stderr(&self, command_id: &str, _stream_id: &str, data: Vec<u8>) -> bool {
        // Tee to the v2 PTY bridge (#243). See `handle_stdout` for rationale.
        if let Some(obs) = self.output_observer_snapshot() {
            obs.on_output(command_id, &data);
        }

        let tx = {
            let pending = self.pending.read();
            pending.get(command_id).map(|p| p.output_tx.clone())
        };

        // Route to session registry.
        let session_id_opt = self.command_to_session.read().get(command_id).cloned();
        if let (Some(ref sr), Some(session_id)) = (&self.session_registry, session_id_opt) {
            sr.publish_output(
                &session_id,
                crate::session::StreamKind::Stderr,
                data.clone(),
            )
            .await;
        }

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
            true
        } else {
            // Silently drop output for unknown commands (orphaned sessions)
            debug!("Dropping stderr for orphaned command: {}", command_id);
            false
        }
    }

    /// Handle command completion from agent
    pub fn handle_result(&self, result: CommandResult) {
        let command_id = &result.command_id;

        if let Some(obs) = self.output_observer_snapshot() {
            obs.on_result(
                command_id,
                result.exit_code,
                result.success,
                result.error.as_str(),
            );
        }

        // Close session in registry before removing from pending.
        let session_id = self.command_to_session.write().remove(command_id.as_str());
        if let Some(ref sr) = self.session_registry {
            if let Some(ref sid) = session_id {
                let sr_clone = sr.clone();
                let exit_code = result.exit_code;
                let sid_owned = sid.clone();
                tokio::spawn(async move {
                    sr_clone.close(&sid_owned, Some(exit_code)).await;
                });
            }
        }

        // Mission translation with real exit code (#193 closed gap 1).
        // Natural completion path — was_killed = false. Find the agent_id
        // by scanning active_sessions (the command_id is the lookup key).
        // We do this BEFORE removing from pending so the lookup succeeds
        // even on quick completions. session_id may be None for older
        // command paths that didn't bind into command_to_session — that's
        // fine, the SessionStart hook also no-ops in that case.
        if let Some(ref sid) = session_id {
            let agent_id_opt: Option<String> = {
                let sessions = self.active_sessions.read();
                sessions.iter().find_map(|(agent_id, ssn_map)| {
                    ssn_map
                        .values()
                        .any(|info| &info.command_id == command_id)
                        .then(|| agent_id.clone())
                })
            };
            if let Some(agent_id) = agent_id_opt {
                self.emit_session_end_with_translation(
                    &agent_id,
                    sid,
                    Some(result.exit_code),
                    false,
                );
            }
        }

        self.remove_active_session_by_command(command_id);

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

    /// Clean up all sessions and pending commands for a disconnected agent
    pub fn cleanup_agent(&self, agent_id: &str) {
        // Notify the v2 PTY bridge so it can drop session-side state
        // keyed on this agent (#243). Fires before v1 cleanup so the
        // bridge sees the disconnect even if the rest of cleanup panics.
        if let Some(obs) = self.output_observer_snapshot() {
            obs.on_agent_disconnect(agent_id);
        }

        // Remove all active sessions for this agent
        let removed_sessions = self.active_sessions.write().remove(agent_id);
        if let Some(sessions) = removed_sessions {
            info!(
                "Cleaned up {} sessions for disconnected agent {}",
                sessions.len(),
                agent_id
            );
        }

        // Remove all pending commands for this agent
        let mut pending = self.pending.write();
        let command_ids: Vec<String> = pending
            .iter()
            .filter(|(_, cmd)| cmd.agent_id == agent_id)
            .map(|(id, _)| id.clone())
            .collect();

        for command_id in command_ids {
            if let Some(mut cmd) = pending.remove(&command_id) {
                debug!(
                    "Removing pending command {} for disconnected agent {}",
                    command_id, agent_id
                );
                // Complete with disconnection error
                let result = CommandResult {
                    command_id: command_id.clone(),
                    exit_code: -1,
                    success: false,
                    error: "Agent disconnected".to_string(),
                    duration_ms: cmd.started_at.elapsed().as_millis() as i64,
                };
                cmd.complete(result);
            }
        }
    }

    /// Create a new session of the specified type
    #[allow(clippy::too_many_arguments)]
    pub async fn create_session(
        &self,
        agent_id: &str,
        session_name: String,
        session_type: SessionType,
        command: String,
        args: Vec<String>,
        working_dir: Option<String>,
        cols: u32,
        rows: u32,
    ) -> Result<(String, mpsc::Receiver<ExecOutput>), DispatchError> {
        self.create_session_with_env(
            agent_id,
            session_name,
            session_type,
            command,
            args,
            working_dir,
            HashMap::new(),
            cols,
            rows,
        )
        .await
    }

    /// Create a new session with caller-supplied environment variables.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_session_with_env(
        &self,
        agent_id: &str,
        session_name: String,
        session_type: SessionType,
        command: String,
        args: Vec<String>,
        working_dir: Option<String>,
        env: HashMap<String, String>,
        cols: u32,
        rows: u32,
    ) -> Result<(String, mpsc::Receiver<ExecOutput>), DispatchError> {
        self.create_session_with_env_and_id(
            agent_id,
            session_name,
            session_type,
            command,
            args,
            working_dir,
            env,
            cols,
            rows,
            None,
        )
        .await
    }

    /// Create a session with caller-supplied environment variables and an
    /// optional stable session id. Startup profiles use the preallocated id to
    /// scope credential leases before the command is dispatched.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_session_with_env_and_id(
        &self,
        agent_id: &str,
        session_name: String,
        session_type: SessionType,
        command: String,
        args: Vec<String>,
        working_dir: Option<String>,
        env: HashMap<String, String>,
        cols: u32,
        rows: u32,
        supplied_session_id: Option<String>,
    ) -> Result<(String, mpsc::Receiver<ExecOutput>), DispatchError> {
        // Check agent exists
        if self.registry.get(agent_id).is_none() {
            return Err(DispatchError::AgentNotFound(agent_id.to_string()));
        }

        // Check for an existing session with this name.
        let existing_command_id = {
            let sessions = self.active_sessions.read();
            sessions
                .get(agent_id)
                .and_then(|s| s.get(&session_name))
                .map(|info| info.command_id.clone())
        };

        if let Some(existing_id) = existing_command_id {
            match session_type {
                SessionType::Headless => {
                    // Headless: kill prior run, start fresh.
                    info!(
                        "Killing old headless session {} for agent {}:{}",
                        existing_id, agent_id, session_name
                    );
                    let _ = self.cancel(&existing_id);
                    self.pending.write().remove(&existing_id);
                    // fall through to create a new session below
                }
                SessionType::Interactive | SessionType::Background => {
                    // Reuse the single existing PTY process — do NOT spawn a second one.
                    // All WS clients sharing this session are fed via the OutputAggregator
                    // (which already broadcasts by agent_id to all subscribers), so they
                    // all see identical output. Stdin from any client routes to the same
                    // command_id and thus the same PTY stdin channel on the agent.
                    // The caller (dispatch_shell) registers ws_id → existing_id in
                    // ws_pty_ownership so cleanup_ws_sessions can ref-count correctly.
                    debug!(
                        "Secondary WS attach to existing PTY {} for {}:{}",
                        existing_id, agent_id, session_name
                    );
                    // Return a dead receiver — output reaches all WS clients through
                    // the OutputAggregator, so this rx is not needed.
                    let (_, dead_rx) = mpsc::channel::<ExecOutput>(1);
                    return Ok((existing_id, dead_rx));
                }
            }
        }

        // Stable session identity (UUIDv7, survives command_id changes on reconnect).
        let session_id = supplied_session_id.unwrap_or_else(|| Uuid::now_v7().to_string());
        let command_id = Uuid::new_v4().to_string();
        let (output_tx, output_rx) = mpsc::channel::<ExecOutput>(100);

        // Build command based on session type
        let (final_command, final_args, allocate_pty) = match session_type {
            SessionType::Interactive => {
                let tmux_args = build_interactive_tmux_args(&session_name, &command, &args);
                ("tmux".to_string(), tmux_args, true)
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
        // Default to /home/agent for PTY sessions (agent's home directory)
        // "~" is expanded to the same path
        let effective_working_dir = match working_dir.as_deref() {
            None | Some("") | Some("~") => "/home/agent".to_string(),
            Some(path) => path.to_string(),
        };

        let cmd = CommandRequest {
            command_id: command_id.clone(),
            command: final_command.clone(),
            args: final_args,
            working_dir: effective_working_dir,
            env,
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
            session_id: session_id.clone(),
            command_id: command_id.clone(),
            session_type,
            command,
            created_at: Instant::now(),
        };

        self.active_sessions
            .write()
            .entry(agent_id.to_string())
            .or_default()
            .insert(session_name.clone(), session_info);

        // Register in the formal session registry (multicast + replay).
        self.command_to_session
            .write()
            .insert(command_id.clone(), session_id.clone());
        if let Some(ref sr) = self.session_registry {
            sr.create(
                session_id.clone(),
                agent_id.to_string(),
                command_id.clone(),
                Some(session_name.clone()),
            );
        }

        if let Some(ref h) = self.aiwg {
            h.emit(crate::aiwg_serve::SandboxEvent::SessionStart {
                agent_id: agent_id.to_string(),
                session_id: session_id.clone(),
                command: final_command.clone(),
            });
            // Authoritative inventory re-broadcast (#192) — keeps AIWG's
            // per-agent session cache in sync without needing per-event
            // bookkeeping on its side.
            self.emit_agent_sessions(agent_id);

            // Mission translation (#193 pass 2): if this session belongs to
            // a mission, emit `mission.started` to the executor WS and bump
            // the mission state to Running.
            if let (Some(ref store), Some(executor_id)) =
                (self.mission_store.as_ref(), h.executor_id())
            {
                if let Some(mission_id) = store.find_by_session(&session_id) {
                    info!(
                        mission_id = %mission_id,
                        session_id = %session_id,
                        agent_id = %agent_id,
                        "Mission session started"
                    );
                    h.emit_executor(crate::aiwg_serve::ExecutorEvent::mission_started(
                        &executor_id,
                        &mission_id,
                        Some(&session_id),
                    ));
                    store.update_state(&mission_id, crate::aiwg_serve::MissionState::Running);
                }
            }
        }

        info!(
            "Created {:?} session {} (sid={}) for agent {}:{}",
            session_type, command_id, session_id, agent_id, session_name
        );
        Ok((command_id, output_rx))
    }

    /// Dispatch an interactive shell (PTY) to an agent.
    ///
    /// Multiple WS clients may call this for the same agent+session_name — each
    /// gets its own PTY attach to the underlying tmux session. Pass `ws_id` so
    /// the dispatcher can SIGHUP the PTY when the WS connection closes.
    pub async fn dispatch_shell(
        &self,
        agent_id: &str,
        session_name: Option<String>,
        cols: u32,
        rows: u32,
        ws_id: Option<String>,
    ) -> Result<(String, mpsc::Receiver<ExecOutput>), DispatchError> {
        let session = session_name.unwrap_or_else(|| "main".to_string());
        let result = self
            .create_session(
                agent_id,
                session,
                SessionType::Interactive,
                String::new(),
                Vec::new(),
                None, // default to home directory
                cols,
                rows,
            )
            .await;

        if let Ok((ref command_id, _)) = result {
            if let Some(id) = ws_id {
                self.ws_pty_ownership
                    .write()
                    .entry(id)
                    .or_default()
                    .insert(command_id.clone());
            }
        }

        result
    }

    /// SIGHUP and remove PTY sessions owned by a WS connection, but only when
    /// no other WS connection still references the same command_id.
    ///
    /// Multiple browsers can share one PTY (same command_id). We only tear down
    /// the PTY process when the last subscriber disconnects.
    pub async fn cleanup_ws_sessions(&self, ws_id: &str) {
        let commands: HashSet<String> = self
            .ws_pty_ownership
            .write()
            .remove(ws_id)
            .unwrap_or_default();

        if commands.is_empty() {
            return;
        }

        // Collect every command_id still held by OTHER WS connections.
        let still_held: HashSet<String> = {
            let ownership = self.ws_pty_ownership.read();
            ownership
                .values()
                .flat_map(|cmds| cmds.iter().cloned())
                .collect()
        };

        // Only SIGHUP command_ids that are no longer referenced by any WS client.
        let mut truly_gone: HashSet<String> = HashSet::new();
        for command_id in &commands {
            if still_held.contains(command_id) {
                debug!(
                    ws_id = %ws_id,
                    command_id = %command_id,
                    "WS disconnected; PTY still held by another client — skipping SIGHUP"
                );
            } else {
                // We're the last subscriber — tear down the PTY.
                let _ = self.send_pty_signal(command_id, 1).await;
                self.pending.write().remove(command_id);
                self.command_to_session.write().remove(command_id);
                truly_gone.insert(command_id.clone());
            }
        }

        if !truly_gone.is_empty() {
            let mut sessions = self.active_sessions.write();
            for agent_map in sessions.values_mut() {
                agent_map.retain(|_, info| !truly_gone.contains(&info.command_id));
            }
            info!(
                ws_id = %ws_id,
                count = truly_gone.len(),
                "Cleaned up PTY sessions on last-subscriber WS disconnect"
            );
        }
    }

    /// Get list of active session infos for an agent
    pub fn get_active_sessions(&self, agent_id: &str) -> Vec<SessionInfo> {
        self.active_sessions
            .read()
            .get(agent_id)
            .map(|sessions| sessions.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Adopt sessions reported by a connected agent into the server-side
    /// inventory. This covers host/local-supervisor sessions that were started
    /// outside the HTTP create-session path but are live in the agent process.
    pub fn import_reported_sessions(
        &self,
        agent_id: &str,
        reported: &[crate::proto::ActiveSession],
    ) -> usize {
        let mut imported = 0;
        let mut sessions = self.active_sessions.write();
        let agent_sessions = sessions.entry(agent_id.to_string()).or_default();

        for reported_session in reported {
            let command_id = reported_session.command_id.trim();
            if command_id.is_empty() {
                continue;
            }
            if agent_sessions
                .values()
                .any(|info| info.command_id == command_id)
            {
                continue;
            }

            let session_name = if reported_session.session_name.trim().is_empty() {
                command_id.to_string()
            } else {
                reported_session.session_name.clone()
            };
            let session_type = match reported_session.session_type {
                value if value == crate::proto::SessionType::Headless as i32 => {
                    SessionType::Headless
                }
                value if value == crate::proto::SessionType::Background as i32 => {
                    SessionType::Background
                }
                _ => SessionType::Interactive,
            };
            let command = if reported_session.command.trim().is_empty() {
                "tmux".to_string()
            } else {
                reported_session.command.clone()
            };
            let session_info = SessionInfo {
                session_name: session_name.clone(),
                session_id: command_id.to_string(),
                command_id: command_id.to_string(),
                session_type,
                command,
                created_at: Instant::now(),
            };
            agent_sessions.insert(session_name.clone(), session_info);
            self.command_to_session
                .write()
                .entry(command_id.to_string())
                .or_insert_with(|| command_id.to_string());
            if let Some(ref sr) = self.session_registry {
                if sr.session_id_for_command(command_id).is_none() {
                    sr.create(
                        command_id.to_string(),
                        agent_id.to_string(),
                        command_id.to_string(),
                        Some(session_name),
                    );
                }
            }
            imported += 1;
        }

        imported
    }

    /// Emit `SessionEnd` to AIWG and translate to the executor mission
    /// vocabulary based on how the session ended (#193 closed gap 1).
    /// Centralised so kill_session and handle_result emit the same wire
    /// shape — only the (exit_code, was_killed) inputs differ.
    fn emit_session_end_with_translation(
        &self,
        agent_id: &str,
        session_id: &str,
        exit_code: Option<i32>,
        was_killed: bool,
    ) {
        let Some(ref h) = self.aiwg else { return };
        h.emit(crate::aiwg_serve::SandboxEvent::SessionEnd {
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            exit_code,
        });
        self.emit_agent_sessions(agent_id);

        // Mission translation: pick the right terminal event from
        // (exit_code, was_killed). was_killed always wins (operator
        // intent). Otherwise exit_code 0 → completed, anything else →
        // failed. None exit_code (no info) → completed (existing
        // pre-gap-1 behavior — preserved for kill paths that don't
        // collect a status).
        let (Some(ref store), Some(executor_id)) = (self.mission_store.as_ref(), h.executor_id())
        else {
            return;
        };
        let Some(mission_id) = store.find_by_session(session_id) else {
            return;
        };
        if was_killed {
            info!(
                mission_id = %mission_id,
                session_id = %session_id,
                agent_id = %agent_id,
                "Mission session aborted"
            );
            h.emit_executor(crate::aiwg_serve::ExecutorEvent::mission_aborted(
                &executor_id,
                &mission_id,
                "session killed by operator",
            ));
            store.update_state(&mission_id, crate::aiwg_serve::MissionState::Aborted);
            return;
        }
        match exit_code {
            Some(0) | None => {
                info!(
                    mission_id = %mission_id,
                    session_id = %session_id,
                    agent_id = %agent_id,
                    exit_code = exit_code.unwrap_or(0),
                    "Mission session completed"
                );
                h.emit_executor(crate::aiwg_serve::ExecutorEvent::mission_completed(
                    &executor_id,
                    &mission_id,
                    exit_code.unwrap_or(0),
                    "session ended",
                ));
                store.update_state(&mission_id, crate::aiwg_serve::MissionState::Completed);
            }
            Some(code) => {
                info!(
                    mission_id = %mission_id,
                    session_id = %session_id,
                    agent_id = %agent_id,
                    exit_code = code,
                    "Mission session failed"
                );
                h.emit_executor(crate::aiwg_serve::ExecutorEvent::mission_failed(
                    &executor_id,
                    &mission_id,
                    "non_zero_exit",
                    &format!("session exited with code {code}"),
                    Some(code),
                ));
                store.update_state(&mission_id, crate::aiwg_serve::MissionState::Failed);
            }
        }
    }

    /// Push the authoritative session list for `agent_id` to AIWG (#192).
    /// No-op when AIWG integration is disabled. Cheap to call — already
    /// holding the snapshot via `get_active_sessions`.
    fn emit_agent_sessions(&self, agent_id: &str) {
        let Some(ref h) = self.aiwg else { return };
        let sessions: Vec<crate::aiwg_serve::SessionSummary> = self
            .get_active_sessions(agent_id)
            .into_iter()
            .map(|s| crate::aiwg_serve::SessionSummary {
                session_id: s.session_id,
                session_name: s.session_name,
                session_type: format!("{:?}", s.session_type).to_lowercase(),
                command: s.command,
                created_at_secs: s.created_at.elapsed().as_secs(),
                has_screen: false, // dispatcher doesn't track screen state; AIWG doesn't render it yet
            })
            .collect();
        h.emit(crate::aiwg_serve::SandboxEvent::AgentSessions {
            agent_id: agent_id.to_string(),
            sessions,
        });
    }

    /// Get all known command IDs for an agent (for session reconciliation)
    pub fn get_known_command_ids(&self, agent_id: &str) -> Vec<String> {
        let mut ids = Vec::new();

        // From pending commands
        for (cmd_id, cmd) in self.pending.read().iter() {
            if cmd.agent_id == agent_id {
                ids.push(cmd_id.clone());
            }
        }

        // From active sessions
        if let Some(sessions) = self.active_sessions.read().get(agent_id) {
            for info in sessions.values() {
                if !ids.contains(&info.command_id) {
                    ids.push(info.command_id.clone());
                }
            }
        }

        ids
    }

    /// Reconcile agent sessions after reconnect
    /// Returns (keep_ids, kill_ids, kill_unrecognized)
    pub fn reconcile_sessions(
        &self,
        agent_id: &str,
        reported_command_ids: &[String],
    ) -> (Vec<String>, Vec<String>, bool) {
        let known_ids = self.get_known_command_ids(agent_id);

        let mut keep = Vec::new();
        let mut kill = Vec::new();

        for cmd_id in reported_command_ids {
            if known_ids.contains(cmd_id) {
                keep.push(cmd_id.clone());
            } else {
                kill.push(cmd_id.clone());
            }
        }

        // If server has no knowledge of this agent (e.g., server restarted),
        // tell agent to kill all unrecognized sessions
        let kill_unrecognized = known_ids.is_empty() && !reported_command_ids.is_empty();

        info!(
            agent_id = %agent_id,
            known = known_ids.len(),
            reported = reported_command_ids.len(),
            keep = keep.len(),
            kill = kill.len(),
            kill_unrecognized = kill_unrecognized,
            "Session reconciliation decision"
        );

        (keep, kill, kill_unrecognized)
    }

    /// Handle reconciliation acknowledgment from agent
    pub fn handle_reconcile_ack(
        &self,
        agent_id: &str,
        killed_ids: &[String],
        kept_ids: &[String],
        failed_ids: &[String],
    ) {
        // Remove killed sessions from our tracking
        for killed_id in killed_ids {
            self.pending.write().remove(killed_id);
        }

        // Update active_sessions tracking - remove killed sessions
        if let Some(mut sessions) = self.active_sessions.write().get_mut(agent_id) {
            sessions.retain(|_, info| !killed_ids.contains(&info.command_id));
        }

        info!(
            agent_id = %agent_id,
            killed = ?killed_ids,
            kept = ?kept_ids,
            failed = ?failed_ids,
            "Session reconciliation complete"
        );

        if !failed_ids.is_empty() {
            warn!(
                agent_id = %agent_id,
                failed = ?failed_ids,
                "Some sessions failed to terminate during reconciliation"
            );
        }
    }

    /// Send PTY resize to a running command
    pub async fn send_pty_resize(
        &self,
        command_id: &str,
        cols: u32,
        rows: u32,
    ) -> Result<(), DispatchError> {
        let agent_id = match self.agent_id_for_command(command_id) {
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
            payload: Some(crate::proto::management_message::Payload::PtyControl(
                pty_control,
            )),
        };

        if self.registry.send_command(&agent_id, msg).await {
            debug!("Sent PTY resize to command {}", command_id);
            // Broadcast resize event to all session observers.
            let session_id_opt = self.command_to_session.read().get(command_id).cloned();
            if let (Some(ref sr), Some(session_id)) = (&self.session_registry, session_id_opt) {
                sr.publish_resize(&session_id, cols as u16, rows as u16)
                    .await;
            }
            Ok(())
        } else {
            Err(DispatchError::SendFailed(agent_id))
        }
    }

    /// Send stdin to the command backing a session (by stable session_id).
    pub async fn send_stdin_to_session(
        &self,
        session_id: &str,
        data: Vec<u8>,
    ) -> Result<(), DispatchError> {
        // Resolve in a synchronous block so the lock guard is dropped before await.
        let command_id = {
            let map = self.command_to_session.read();
            map.iter()
                .find(|(_, sid)| sid.as_str() == session_id)
                .map(|(cid, _)| cid.clone())
        };
        match command_id {
            Some(cid) => self.send_stdin(&cid, data).await,
            None => Err(DispatchError::CommandNotFound(session_id.to_string())),
        }
    }

    /// Send PTY resize to the command backing a session (by stable session_id).
    pub async fn send_pty_resize_to_session(
        &self,
        session_id: &str,
        cols: u32,
        rows: u32,
    ) -> Result<(), DispatchError> {
        let command_id = {
            let map = self.command_to_session.read();
            map.iter()
                .find(|(_, sid)| sid.as_str() == session_id)
                .map(|(cid, _)| cid.clone())
        };
        match command_id {
            Some(cid) => self.send_pty_resize(&cid, cols, rows).await,
            None => Err(DispatchError::CommandNotFound(session_id.to_string())),
        }
    }

    /// Signal the command backing a session by stable session_id.
    /// Used by the formal-model `DELETE /api/v1/sessions/{id}` admin verb.
    pub async fn send_pty_signal_to_session(
        &self,
        session_id: &str,
        signal_number: i32,
    ) -> Result<(), DispatchError> {
        let command_id = {
            let map = self.command_to_session.read();
            map.iter()
                .find(|(_, sid)| sid.as_str() == session_id)
                .map(|(cid, _)| cid.clone())
        };
        match command_id {
            Some(cid) => self.send_pty_signal(&cid, signal_number).await,
            None => Err(DispatchError::CommandNotFound(session_id.to_string())),
        }
    }

    /// Kill a session by name (runs tmux kill-session for interactive/background sessions)
    pub async fn kill_session(
        &self,
        agent_id: &str,
        session_name: &str,
    ) -> Result<(), DispatchError> {
        // Find session info (may be None if server was restarted after session creation)
        let session_info = {
            let sessions = self.active_sessions.read();
            sessions
                .get(agent_id)
                .and_then(|s| s.get(session_name).cloned())
        };

        // For tmux sessions (or unknown sessions after restart), run tmux kill-session
        // If session is known and is Headless, use SIGTERM instead
        let is_headless = session_info
            .as_ref()
            .map(|s| s.session_type == SessionType::Headless)
            .unwrap_or(false);

        if is_headless {
            // For headless, just signal the process
            if let Some(ref session) = session_info {
                self.send_pty_signal(&session.command_id, 15).await?; // SIGTERM
            }
        } else {
            // For tmux sessions (or unknown sessions after restart), run tmux kill-session
            let command_id = Uuid::new_v4().to_string();
            let (output_tx, _output_rx) = mpsc::channel::<ExecOutput>(10);

            let request = CommandRequest {
                command_id: command_id.clone(),
                command: "tmux".to_string(),
                args: vec![
                    "kill-session".to_string(),
                    "-t".to_string(),
                    session_name.to_string(),
                ],
                working_dir: String::new(),
                env: std::collections::HashMap::new(),
                timeout_seconds: 10,
                capture_output: false,
                run_as: String::new(),
                allocate_pty: false,
                pty_cols: 0,
                pty_rows: 0,
                pty_term: String::new(),
            };

            let msg = crate::proto::ManagementMessage {
                payload: Some(crate::proto::management_message::Payload::Command(request)),
            };

            // Store in pending briefly for the kill command
            {
                let mut pending = self.pending.write();
                pending.insert(
                    command_id.clone(),
                    PendingCommand::new(
                        command_id.clone(),
                        agent_id.to_string(),
                        "tmux kill-session".to_string(),
                        10,
                        output_tx,
                        None,
                    ),
                );
            }

            if !self.registry.send_command(agent_id, msg).await {
                return Err(DispatchError::SendFailed(agent_id.to_string()));
            }

            // If session was tracked, also send signal to the original PTY command to clean up
            if let Some(ref session) = session_info {
                let _ = self.send_pty_signal(&session.command_id, 9).await; // SIGKILL
            }
        }

        // Remove from active_sessions if present
        let removed_command_id = {
            let mut sessions = self.active_sessions.write();
            sessions
                .get_mut(agent_id)
                .and_then(|s| s.remove(session_name))
                .map(|info| info.command_id)
        };

        if let Some(ref cmd_id) = removed_command_id {
            // Operator-initiated kill → mission.aborted (was_killed = true).
            self.emit_session_end_with_translation(agent_id, cmd_id, None, true);
        }

        // Remove from pending if present
        if let Some(ref session) = session_info {
            let mut pending = self.pending.write();
            pending.remove(&session.command_id);
        }

        info!("Killed session {} on agent {}", session_name, agent_id);
        Ok(())
    }

    /// Send a signal to a PTY session's child process
    pub async fn send_pty_signal(
        &self,
        command_id: &str,
        signal_number: i32,
    ) -> Result<(), DispatchError> {
        let agent_id = match self.agent_id_for_command(command_id) {
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
            payload: Some(crate::proto::management_message::Payload::PtyControl(
                pty_control,
            )),
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
        let agent_id = match self.agent_id_for_command(command_id) {
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
            payload: Some(crate::proto::management_message::Payload::Stdin(
                stdin_chunk,
            )),
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

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn build_shell_command(command: &str, args: &[String]) -> Option<String> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    let mut shell_command = command.to_string();
    for arg in args {
        shell_command.push(' ');
        shell_command.push_str(&shell_quote(arg));
    }
    Some(shell_command)
}

fn build_interactive_tmux_args(session_name: &str, command: &str, args: &[String]) -> Vec<String> {
    let mut tmux_args = vec![
        "new-session".to_string(),
        "-A".to_string(),
        "-s".to_string(),
        session_name.to_string(),
    ];
    if let Some(shell_command) = build_shell_command(command, args) {
        tmux_args.push(shell_command);
    }
    tmux_args.extend([
        ";".to_string(),
        "set-option".to_string(),
        "-g".to_string(),
        "window-size".to_string(),
        "largest".to_string(),
    ]);
    tmux_args
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
            session_id: "test-session-id".to_string(),
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
    fn test_interactive_tmux_args_default_to_shell() {
        let args = build_interactive_tmux_args("main", "", &[]);

        assert_eq!(
            args,
            vec![
                "new-session",
                "-A",
                "-s",
                "main",
                ";",
                "set-option",
                "-g",
                "window-size",
                "largest"
            ]
        );
    }

    #[test]
    fn test_interactive_tmux_args_run_requested_provider_command() {
        let args =
            build_interactive_tmux_args("codex-tui", "sh -lc 'TERM=xterm-256color codex'", &[]);

        assert_eq!(args[0..4], ["new-session", "-A", "-s", "codex-tui"]);
        assert_eq!(args[4], "sh -lc 'TERM=xterm-256color codex'");
        assert_eq!(args[5], ";");
    }

    #[test]
    fn test_interactive_tmux_args_preserve_session_and_quote_command_args() {
        let args = build_interactive_tmux_args(
            "provider's tui",
            "provider",
            &["--label".to_string(), "owner's session".to_string()],
        );

        assert_eq!(args[3], "provider's tui");
        assert_eq!(args[4], "provider '--label' 'owner'\"'\"'s session'");
    }

    #[test]
    fn test_active_sessions_track_session_info() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Manually insert sessions with different types
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);

            agent_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "tmux".to_string(),
                    created_at: Instant::now(),
                },
            );

            agent_sessions.insert(
                "claude".to_string(),
                SessionInfo {
                    session_name: "claude".to_string(),
                    command_id: "cmd-002".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Headless,
                    command: "claude --print".to_string(),
                    created_at: Instant::now(),
                },
            );

            agent_sessions.insert(
                "worker".to_string(),
                SessionInfo {
                    session_name: "worker".to_string(),
                    command_id: "cmd-003".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Background,
                    command: "long-running-job".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        let sessions = dispatcher.get_active_sessions("agent-01");
        assert_eq!(sessions.len(), 3);

        // Find each session and verify its type
        let main_session = sessions.iter().find(|s| s.session_name == "main").unwrap();
        assert_eq!(main_session.session_type, SessionType::Interactive);

        let claude_session = sessions
            .iter()
            .find(|s| s.session_name == "claude")
            .unwrap();
        assert_eq!(claude_session.session_type, SessionType::Headless);

        let worker_session = sessions
            .iter()
            .find(|s| s.session_name == "worker")
            .unwrap();
        assert_eq!(worker_session.session_type, SessionType::Background);
    }

    #[test]
    fn imported_reported_sessions_are_visible_in_active_sessions() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);
        let reported = vec![crate::proto::ActiveSession {
            command_id: "host-cmd-1".to_string(),
            session_name: "cockpit-host-managed-tmux".to_string(),
            session_type: crate::proto::SessionType::Interactive as i32,
            command: "tmux new-session -A -s cockpit-host-managed-tmux bash -l".to_string(),
            started_at_ms: 0,
            pid: 1234,
            is_pty: true,
        }];

        let imported = dispatcher.import_reported_sessions("host-short", &reported);
        let sessions = dispatcher.get_active_sessions("host-short");

        assert_eq!(imported, 1);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "host-cmd-1");
        assert_eq!(sessions[0].command_id, "host-cmd-1");
        assert_eq!(sessions[0].session_name, "cockpit-host-managed-tmux");
        assert_eq!(sessions[0].session_type, SessionType::Interactive);
        assert_eq!(
            dispatcher.session_id_for_command("host-cmd-1").as_deref(),
            Some("host-cmd-1")
        );
    }

    #[test]
    fn test_get_active_sessions_returns_session_info() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Insert a test session
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent_sessions.insert(
                "test".to_string(),
                SessionInfo {
                    session_name: "test".to_string(),
                    command_id: "cmd-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
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
            let agent_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);

            agent_sessions.insert(
                "interactive1".to_string(),
                SessionInfo {
                    session_name: "interactive1".to_string(),
                    command_id: "cmd-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );

            agent_sessions.insert(
                "headless1".to_string(),
                SessionInfo {
                    session_name: "headless1".to_string(),
                    command_id: "cmd-002".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Headless,
                    command: "python script.py".to_string(),
                    created_at: Instant::now(),
                },
            );

            agent_sessions.insert(
                "background1".to_string(),
                SessionInfo {
                    session_name: "background1".to_string(),
                    command_id: "cmd-003".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Background,
                    command: "worker --daemon".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        let sessions = dispatcher.get_active_sessions("agent-01");
        assert_eq!(sessions.len(), 3);

        // Count each type
        let interactive_count = sessions
            .iter()
            .filter(|s| s.session_type == SessionType::Interactive)
            .count();
        let headless_count = sessions
            .iter()
            .filter(|s| s.session_type == SessionType::Headless)
            .count();
        let background_count = sessions
            .iter()
            .filter(|s| s.session_type == SessionType::Background)
            .count();

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

            let agent1_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent1_sessions.insert(
                "work".to_string(),
                SessionInfo {
                    session_name: "work".to_string(),
                    command_id: "cmd-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );

            let agent2_sessions = sessions
                .entry("agent-02".to_string())
                .or_insert_with(HashMap::new);
            agent2_sessions.insert(
                "work".to_string(),
                SessionInfo {
                    session_name: "work".to_string(),
                    command_id: "cmd-002".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Headless,
                    command: "python".to_string(),
                    created_at: Instant::now(),
                },
            );
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
            let agent_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);

            for (name, cmd_id) in [
                ("main", "cmd-001"),
                ("debug", "cmd-002"),
                ("test", "cmd-003"),
            ] {
                agent_sessions.insert(
                    name.to_string(),
                    SessionInfo {
                        session_name: name.to_string(),
                        command_id: cmd_id.to_string(),
                        session_id: "test-session-id".to_string(),
                        session_type: SessionType::Interactive,
                        command: "bash".to_string(),
                        created_at: Instant::now(),
                    },
                );
            }
        }

        let session_infos = dispatcher.get_active_sessions("agent-01");
        assert_eq!(session_infos.len(), 3);

        let names: Vec<String> = session_infos
            .iter()
            .map(|s| s.session_name.clone())
            .collect();
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

            let agent1_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent1_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
            agent1_sessions.insert(
                "debug".to_string(),
                SessionInfo {
                    session_name: "debug".to_string(),
                    command_id: "cmd-002".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );

            let agent2_sessions = sessions
                .entry("agent-02".to_string())
                .or_insert_with(HashMap::new);
            agent2_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-003".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
            agent2_sessions.insert(
                "work".to_string(),
                SessionInfo {
                    session_name: "work".to_string(),
                    command_id: "cmd-004".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        let agent1_sessions = dispatcher.get_active_sessions("agent-01");
        assert_eq!(agent1_sessions.len(), 2);
        let agent1_names: Vec<String> = agent1_sessions
            .iter()
            .map(|s| s.session_name.clone())
            .collect();
        assert!(agent1_names.contains(&"main".to_string()));
        assert!(agent1_names.contains(&"debug".to_string()));

        let agent2_sessions = dispatcher.get_active_sessions("agent-02");
        assert_eq!(agent2_sessions.len(), 2);
        let agent2_names: Vec<String> = agent2_sessions
            .iter()
            .map(|s| s.session_name.clone())
            .collect();
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

            let agent1_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent1_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );

            let agent2_sessions = sessions
                .entry("agent-02".to_string())
                .or_insert_with(HashMap::new);
            agent2_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-002".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        // Verify each agent's "main" session has a different command ID
        let agent1_cmd = {
            let sessions = dispatcher.active_sessions.read();
            sessions
                .get("agent-01")
                .and_then(|s| s.get("main"))
                .map(|info| info.command_id.clone())
        };

        let agent2_cmd = {
            let sessions = dispatcher.active_sessions.read();
            sessions
                .get("agent-02")
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
        assert!(
            pending.stdin_tx.is_some(),
            "stdin_tx should be initialized for non-PTY commands"
        );
    }

    /// Test send_stdin with non-existent command
    #[tokio::test]
    async fn test_send_stdin_command_not_found() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        let result = dispatcher
            .send_stdin("nonexistent-cmd", vec![1, 2, 3])
            .await;

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

        dispatcher
            .pending
            .write()
            .insert(command_id.clone(), pending);

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

        assert!(
            pending.stdin_tx.is_none(),
            "PTY commands should have None stdin_tx"
        );
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
        assert!(
            pending.stdin_tx.is_some(),
            "stdin_tx should still exist after clone"
        );
    }

    // =============================================================================
    // Session Reconciliation Tests
    // =============================================================================

    #[test]
    fn test_get_known_command_ids_from_pending() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Add pending commands
        let (output_tx1, _) = mpsc::channel::<ExecOutput>(100);
        let pending1 = PendingCommand::new(
            "cmd-001".to_string(),
            "agent-01".to_string(),
            "echo".to_string(),
            30,
            output_tx1,
            None,
        );

        let (output_tx2, _) = mpsc::channel::<ExecOutput>(100);
        let pending2 = PendingCommand::new(
            "cmd-002".to_string(),
            "agent-01".to_string(),
            "ls".to_string(),
            30,
            output_tx2,
            None,
        );

        let (output_tx3, _) = mpsc::channel::<ExecOutput>(100);
        let pending3 = PendingCommand::new(
            "cmd-003".to_string(),
            "agent-02".to_string(), // Different agent
            "cat".to_string(),
            30,
            output_tx3,
            None,
        );

        dispatcher
            .pending
            .write()
            .insert("cmd-001".to_string(), pending1);
        dispatcher
            .pending
            .write()
            .insert("cmd-002".to_string(), pending2);
        dispatcher
            .pending
            .write()
            .insert("cmd-003".to_string(), pending3);

        // Get known IDs for agent-01
        let known_ids = dispatcher.get_known_command_ids("agent-01");
        assert_eq!(known_ids.len(), 2);
        assert!(known_ids.contains(&"cmd-001".to_string()));
        assert!(known_ids.contains(&"cmd-002".to_string()));
        assert!(!known_ids.contains(&"cmd-003".to_string())); // Different agent
    }

    #[test]
    fn test_get_known_command_ids_from_active_sessions() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Add active sessions
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-session-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        let known_ids = dispatcher.get_known_command_ids("agent-01");
        assert_eq!(known_ids.len(), 1);
        assert!(known_ids.contains(&"cmd-session-001".to_string()));
    }

    #[test]
    fn test_get_known_command_ids_combined() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Add pending command
        let (output_tx, _) = mpsc::channel::<ExecOutput>(100);
        let pending = PendingCommand::new(
            "cmd-pending".to_string(),
            "agent-01".to_string(),
            "echo".to_string(),
            30,
            output_tx,
            None,
        );
        dispatcher
            .pending
            .write()
            .insert("cmd-pending".to_string(), pending);

        // Add active session
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-session".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        let known_ids = dispatcher.get_known_command_ids("agent-01");
        assert_eq!(known_ids.len(), 2);
        assert!(known_ids.contains(&"cmd-pending".to_string()));
        assert!(known_ids.contains(&"cmd-session".to_string()));
    }

    #[test]
    fn test_reconcile_sessions_all_known() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Add pending commands
        let (output_tx, _) = mpsc::channel::<ExecOutput>(100);
        let pending = PendingCommand::new(
            "cmd-001".to_string(),
            "agent-01".to_string(),
            "echo".to_string(),
            30,
            output_tx,
            None,
        );
        dispatcher
            .pending
            .write()
            .insert("cmd-001".to_string(), pending);

        // Agent reports the same command
        let reported = vec!["cmd-001".to_string()];
        let (keep, kill, kill_unrecognized) = dispatcher.reconcile_sessions("agent-01", &reported);

        assert_eq!(keep.len(), 1);
        assert!(keep.contains(&"cmd-001".to_string()));
        assert!(kill.is_empty());
        assert!(!kill_unrecognized);
    }

    #[test]
    fn test_reconcile_sessions_orphaned() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Server knows cmd-001
        let (output_tx, _) = mpsc::channel::<ExecOutput>(100);
        let pending = PendingCommand::new(
            "cmd-001".to_string(),
            "agent-01".to_string(),
            "echo".to_string(),
            30,
            output_tx,
            None,
        );
        dispatcher
            .pending
            .write()
            .insert("cmd-001".to_string(), pending);

        // Agent reports cmd-001 (known) and cmd-orphan (unknown)
        let reported = vec!["cmd-001".to_string(), "cmd-orphan".to_string()];
        let (keep, kill, kill_unrecognized) = dispatcher.reconcile_sessions("agent-01", &reported);

        assert_eq!(keep.len(), 1);
        assert!(keep.contains(&"cmd-001".to_string()));
        assert_eq!(kill.len(), 1);
        assert!(kill.contains(&"cmd-orphan".to_string()));
        assert!(!kill_unrecognized);
    }

    #[test]
    fn test_reconcile_sessions_server_restart() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Server has no knowledge (just restarted)
        // Agent reports sessions
        let reported = vec!["cmd-old-001".to_string(), "cmd-old-002".to_string()];
        let (keep, kill, kill_unrecognized) = dispatcher.reconcile_sessions("agent-01", &reported);

        // Since server knows nothing, all reported are orphaned
        assert!(keep.is_empty());
        assert_eq!(kill.len(), 2);
        // kill_unrecognized should be true since server has no sessions and agent has some
        assert!(kill_unrecognized);
    }

    #[test]
    fn test_reconcile_sessions_empty_report() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Server knows cmd-001
        let (output_tx, _) = mpsc::channel::<ExecOutput>(100);
        let pending = PendingCommand::new(
            "cmd-001".to_string(),
            "agent-01".to_string(),
            "echo".to_string(),
            30,
            output_tx,
            None,
        );
        dispatcher
            .pending
            .write()
            .insert("cmd-001".to_string(), pending);

        // Agent reports empty (no sessions running)
        let reported: Vec<String> = vec![];
        let (keep, kill, kill_unrecognized) = dispatcher.reconcile_sessions("agent-01", &reported);

        assert!(keep.is_empty());
        assert!(kill.is_empty());
        assert!(!kill_unrecognized);
    }

    #[test]
    fn test_handle_reconcile_ack_clears_pending() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Add pending commands
        let (output_tx1, _) = mpsc::channel::<ExecOutput>(100);
        let pending1 = PendingCommand::new(
            "cmd-killed".to_string(),
            "agent-01".to_string(),
            "echo".to_string(),
            30,
            output_tx1,
            None,
        );

        let (output_tx2, _) = mpsc::channel::<ExecOutput>(100);
        let pending2 = PendingCommand::new(
            "cmd-kept".to_string(),
            "agent-01".to_string(),
            "ls".to_string(),
            30,
            output_tx2,
            None,
        );

        dispatcher
            .pending
            .write()
            .insert("cmd-killed".to_string(), pending1);
        dispatcher
            .pending
            .write()
            .insert("cmd-kept".to_string(), pending2);

        // Process reconciliation ack
        dispatcher.handle_reconcile_ack(
            "agent-01",
            &["cmd-killed".to_string()],
            &["cmd-kept".to_string()],
            &[],
        );

        // cmd-killed should be removed, cmd-kept should remain
        assert!(!dispatcher.pending.read().contains_key("cmd-killed"));
        assert!(dispatcher.pending.read().contains_key("cmd-kept"));
    }

    #[test]
    fn test_handle_reconcile_ack_clears_sessions() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Add active sessions
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent_sessions.insert(
                "killed-session".to_string(),
                SessionInfo {
                    session_name: "killed-session".to_string(),
                    command_id: "cmd-killed".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
            agent_sessions.insert(
                "kept-session".to_string(),
                SessionInfo {
                    session_name: "kept-session".to_string(),
                    command_id: "cmd-kept".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        assert_eq!(dispatcher.get_active_sessions("agent-01").len(), 2);

        // Process reconciliation ack
        dispatcher.handle_reconcile_ack(
            "agent-01",
            &["cmd-killed".to_string()],
            &["cmd-kept".to_string()],
            &[],
        );

        // Only kept-session should remain
        let sessions = dispatcher.get_active_sessions("agent-01");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].command_id, "cmd-kept");
    }

    // Orphaned session handling tests

    /// Test that orphaned output is dropped silently
    #[tokio::test]
    async fn test_handle_stdout_orphaned_command_dropped() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Try to send stdout for non-existent command
        let result = dispatcher
            .handle_stdout("nonexistent-cmd", "stream-1", vec![1, 2, 3])
            .await;

        // Should return false indicating output was dropped
        assert!(!result, "Orphaned output should be dropped");
    }

    /// Test that orphaned stderr is dropped silently
    #[tokio::test]
    async fn test_handle_stderr_orphaned_command_dropped() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Try to send stderr for non-existent command
        let result = dispatcher
            .handle_stderr("nonexistent-cmd", "stream-1", vec![1, 2, 3])
            .await;

        // Should return false indicating output was dropped
        assert!(!result, "Orphaned stderr should be dropped");
    }

    /// Test that valid commands return true for stdout
    #[tokio::test]
    async fn test_handle_stdout_valid_command() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Create a valid pending command
        let command_id = "valid-cmd".to_string();
        let (output_tx, _output_rx) = mpsc::channel::<ExecOutput>(100);

        let pending = PendingCommand::new(
            command_id.clone(),
            "test-agent".to_string(),
            "echo".to_string(),
            30,
            output_tx,
            None,
        );

        dispatcher
            .pending
            .write()
            .insert(command_id.clone(), pending);

        // Send stdout for valid command
        let result = dispatcher
            .handle_stdout(&command_id, "stream-1", vec![1, 2, 3])
            .await;

        // Should return true indicating output was processed
        assert!(result, "Valid command output should be processed");
    }

    /// Test cleanup_agent removes all sessions
    #[test]
    fn test_cleanup_agent_removes_sessions() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Setup sessions for agent
        {
            let mut sessions = dispatcher.active_sessions.write();
            let agent_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
            agent_sessions.insert(
                "debug".to_string(),
                SessionInfo {
                    session_name: "debug".to_string(),
                    command_id: "cmd-002".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        // Verify sessions exist
        assert_eq!(dispatcher.get_active_sessions("agent-01").len(), 2);

        // Cleanup agent
        dispatcher.cleanup_agent("agent-01");

        // Verify sessions were removed
        assert_eq!(dispatcher.get_active_sessions("agent-01").len(), 0);
    }

    /// Test cleanup_agent removes pending commands
    #[test]
    fn test_cleanup_agent_removes_pending_commands() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Create pending commands for agent
        let (output_tx1, _) = mpsc::channel::<ExecOutput>(100);
        let pending1 = PendingCommand::new(
            "cmd-001".to_string(),
            "agent-01".to_string(),
            "echo".to_string(),
            30,
            output_tx1,
            None,
        );

        let (output_tx2, _) = mpsc::channel::<ExecOutput>(100);
        let pending2 = PendingCommand::new(
            "cmd-002".to_string(),
            "agent-01".to_string(),
            "ls".to_string(),
            30,
            output_tx2,
            None,
        );

        dispatcher
            .pending
            .write()
            .insert("cmd-001".to_string(), pending1);
        dispatcher
            .pending
            .write()
            .insert("cmd-002".to_string(), pending2);

        // Verify pending commands exist
        assert_eq!(dispatcher.pending_count(), 2);

        // Cleanup agent
        dispatcher.cleanup_agent("agent-01");

        // Verify pending commands were removed
        assert_eq!(dispatcher.pending_count(), 0);
    }

    /// Test cleanup_agent doesn't affect other agents
    #[test]
    fn test_cleanup_agent_isolation() {
        let registry = MockRegistry::new();
        let dispatcher = CommandDispatcher::new(registry);

        // Setup sessions for multiple agents
        {
            let mut sessions = dispatcher.active_sessions.write();

            let agent1_sessions = sessions
                .entry("agent-01".to_string())
                .or_insert_with(HashMap::new);
            agent1_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-001".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );

            let agent2_sessions = sessions
                .entry("agent-02".to_string())
                .or_insert_with(HashMap::new);
            agent2_sessions.insert(
                "main".to_string(),
                SessionInfo {
                    session_name: "main".to_string(),
                    command_id: "cmd-002".to_string(),
                    session_id: "test-session-id".to_string(),
                    session_type: SessionType::Interactive,
                    command: "bash".to_string(),
                    created_at: Instant::now(),
                },
            );
        }

        // Cleanup agent-01
        dispatcher.cleanup_agent("agent-01");

        // Verify agent-01 sessions removed but agent-02 sessions remain
        assert_eq!(dispatcher.get_active_sessions("agent-01").len(), 0);
        assert_eq!(dispatcher.get_active_sessions("agent-02").len(), 1);
    }
}
