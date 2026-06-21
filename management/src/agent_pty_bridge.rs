//! `AgentPtyBridge` — real [`PtyBridge`] implementation that forwards
//! `pty-ws/v1` session traffic to a connected agent over the existing
//! `agent-rs` gRPC channel (#243).
//!
//! ## Where the wiring sits
//!
//! The [`PtyBridge`](crate::bindings::pty_bridge::PtyBridge) trait lives
//! in this (executor) crate so that the [`pty_ws`](crate::bindings::pty_ws)
//! binding can consume it without crossing crate boundaries. The real
//! implementation lives here too, because the executor crate already
//! depends on [`agentic_management`] (path dep) and therefore has direct
//! access to [`AgentRegistry`](agentic_management::registry::AgentRegistry)
//! and [`CommandDispatcher`](agentic_management::dispatch::CommandDispatcher).
//! Inverting that dep — putting the trait in management and importing it
//! from executor — would cycle (management cannot depend on executor).
//!
//! The bridge is *not* mounted by default. [`crate::bindings::rest::router`]
//! installs a [`NoOpPtyBridge`](crate::bindings::pty_bridge::NoOpPtyBridge)
//! so tests, the executor harness, and any deployment without a connected
//! agent fleet keep their broadcast-only behavior. Production binaries
//! that own an `AgentRegistry` + `CommandDispatcher` construct
//! [`AgentPtyBridge`] and call
//! [`router_with_bridge`](crate::bindings::rest::router_with_bridge) to
//! inject it.
//!
//! ## Identity model
//!
//! - **Inbound from pty_ws.** Sessions are keyed on
//!   `(instance_id, session_id)`. `instance_id` is the per-agent UUIDv7
//!   issued by management at registration (#917); `session_id` is the
//!   pty-ws session identifier from the URL path.
//! - **Outbound to agent-rs.** agent-rs identifies PTY-bearing commands
//!   by `command_id`. We deliberately reuse the WS `session_id` *as*
//!   the agent-side `command_id`: it is a UUID, it never repeats inside
//!   one agent's lifetime, and reusing it removes a layer of indirection
//!   the bridge would otherwise have to keep consistent across reconnects.
//!   No proto change is required.
//! - **Inbound OutputChunk routing.** `OutputChunk.stream_id` is what
//!   agent-rs sets — management's existing gRPC handler already treats
//!   it as the routing key (`handle_stdout(&chunk.stream_id, ...)`).
//!   We install an [`OutputObserver`] on the dispatcher that tees every
//!   chunk through a `(command_id == session_id) → mpsc::Sender<PtyBridgeEvent>`
//!   table. The v1 routing path runs unchanged after the tee — v2 PTY
//!   sessions and v1 missions never share command ids by construction
//!   (different generators, different lifetimes), so the two routes
//!   don't fight.
//!
//! ## Agent-disconnect handling
//!
//! The dispatcher's `cleanup_agent` already fires when the gRPC stream
//! ends or the registry unregisters. We piggyback on that: the bridge's
//! [`OutputObserver::on_agent_disconnect`] impl drops every output route
//! for the affected agent, which closes the corresponding
//! [`mpsc::Receiver`]s downstream — the `pty_ws` reader task observes
//! the close and emits the session's `Closed` frame via its existing
//! path. No new wire format, no separate disconnect channel.
//!
//! ## Command-result handling
//!
//! `CommandDispatcher::handle_result` calls [`OutputObserver::on_result`]
//! before it tears down its own pending-command state. The bridge turns
//! that into a `PtyBridgeEvent::Closed` with the reported exit code, so
//! the `pty-ws/v1` session emits one deterministic retained `closed`
//! frame instead of relying on an unstructured output-channel EOF.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::dispatch::{CommandDispatcher, OutputObserver};
use crate::registry::AgentRegistry;

use agentic_sandbox_executor::bindings::pty_bridge::{
    PtyBridge, PtyBridgeEvent, PtySessionRole, PtyStartCommand, SessionBackend, SessionClass,
    SessionHostCapabilities,
};

/// Output route table entry. We keep both the agent-id (so we can purge
/// the route when the agent disconnects via `on_agent_disconnect`) and
/// the sender (so `forward_output_chunk` can deliver bytes to the
/// `pty_ws` reader task).
struct OutputRoute {
    /// Stable per-process agent identifier (e.g. `agent-01`). Used to
    /// match disconnect notifications coming through the
    /// [`OutputObserver::on_agent_disconnect`] hook.
    agent_id: String,
    /// Channel into the `pty_ws` reader task. Bytes pushed here are
    /// turned into base64-encoded `output` frames on the WS.
    tx: mpsc::Sender<PtyBridgeEvent>,
}

/// Real [`PtyBridge`] backed by the management crate's existing agent
/// registry and dispatcher. See module docs for the full design.
pub struct AgentPtyBridge {
    registry: Arc<AgentRegistry>,
    dispatcher: Arc<CommandDispatcher>,
    /// `command_id (= session_id) → OutputRoute`. Lookups happen in the
    /// hot path of every inbound `OutputChunk`, so this is a
    /// [`parking_lot::RwLock`] (cheap reads, very short writes during
    /// session start/teardown). Keys are the agent-side `command_id`,
    /// which is by construction the WS `session_id`.
    routes: RwLock<HashMap<String, OutputRoute>>,
    /// Formal-registry attachment drain tasks keyed by
    /// `(session_id, pty_ws_client_id)`. Holding and draining the receiver
    /// keeps the canonical registry membership alive without letting its
    /// bounded channel fill and evict the mirrored client.
    formal_attachments: RwLock<HashMap<(String, String), JoinHandle<()>>>,
}

impl AgentPtyBridge {
    /// Construct a new bridge.
    ///
    /// Important: this does *not* install the bridge as an
    /// [`OutputObserver`] on the dispatcher. Call
    /// [`Self::install_as_observer`] (or do it manually via
    /// [`CommandDispatcher::set_output_observer`]) so inbound
    /// `OutputChunk` traffic is teed back through the bridge. The
    /// observer setup is separate from construction so tests can hold
    /// an `Arc<AgentPtyBridge>` directly and assert on its routing
    /// table without going through the dispatcher.
    pub fn new(registry: Arc<AgentRegistry>, dispatcher: Arc<CommandDispatcher>) -> Self {
        Self {
            registry,
            dispatcher,
            routes: RwLock::new(HashMap::new()),
            formal_attachments: RwLock::new(HashMap::new()),
        }
    }

    /// Install this bridge as the dispatcher's [`OutputObserver`]. After
    /// this call, every inbound `OutputChunk` is forwarded to
    /// [`OutputObserver::on_output`] (the tee) in addition to the
    /// existing v1 routing path.
    ///
    /// Production wiring (in `main.rs` or wherever the dispatcher is
    /// constructed) calls this once after the dispatcher is fully
    /// configured but before the gRPC server starts accepting agent
    /// connections.
    pub fn install_as_observer(self: &Arc<Self>) {
        self.dispatcher
            .set_output_observer(Arc::clone(self) as Arc<dyn OutputObserver>);
    }

    /// Look up the WS `output` channel for `(instance_id-implied,
    /// command_id)` and push bytes through it. Called from
    /// [`OutputObserver::on_output`] — the dispatcher already drives
    /// this for every chunk.
    ///
    /// No-op when no route is registered. v1 mission output routes
    /// through `pending.output_tx` (separate path) and uses different
    /// command ids, so the dispatcher's normal v1 path is unaffected
    /// by this miss-case.
    fn forward_output(&self, command_id: &str, data: &[u8]) {
        // Snapshot the sender under the read lock and drop the lock
        // before the (potentially) awaiting `try_send` so we never hold
        // the lock across an await point. We use `try_send` (non-block)
        // because `OutputObserver::on_output` is a non-async trait
        // method called from inside the dispatcher; backpressure on a
        // single WS session must not stall the agent stream.
        let tx = self.routes.read().get(command_id).map(|r| r.tx.clone());
        if let Some(tx) = tx {
            if let Err(e) = tx.try_send(PtyBridgeEvent::output(data.to_vec())) {
                // Channel full or closed. Full = slow WS consumer;
                // closed = session torn down. Either way, drop is fine
                // for v2.0 (best-effort delivery, documented).
                debug!(
                    target: "agent_pty_bridge",
                    "drop output_chunk (cmd={}, bytes={}): {}",
                    command_id,
                    data.len(),
                    e
                );
            }
        }
    }

    /// Notify the pty-ws reader that the agent reported command
    /// completion. This carries the real exit code into the retained
    /// `closed` frame instead of relying on an unstructured channel EOF.
    fn forward_result(&self, command_id: &str, exit_code: i32) {
        let route = self.routes.write().remove(command_id);
        if let Some(route) = route {
            self.drop_formal_attachments_for_session(command_id);
            if let Err(e) = route
                .tx
                .try_send(PtyBridgeEvent::closed(Some(exit_code), "command_result"))
            {
                debug!(
                    target: "agent_pty_bridge",
                    "drop command_result close event (cmd={}, exit={}): {}",
                    command_id,
                    exit_code,
                    e
                );
            }
        }
    }

    /// Drop every output route owned by `agent_id`. Called from
    /// [`OutputObserver::on_agent_disconnect`]; closes the senders so
    /// the `pty_ws` reader tasks see EOF and emit `Closed` frames
    /// through their existing path.
    fn drop_routes_for_agent(&self, agent_id: &str) {
        let removed: Vec<String> = {
            let mut routes = self.routes.write();
            let to_remove: Vec<String> = routes
                .iter()
                .filter(|(_, r)| r.agent_id == agent_id)
                .map(|(k, _)| k.clone())
                .collect();
            for k in &to_remove {
                routes.remove(k);
            }
            to_remove
        };
        if !removed.is_empty() {
            for session_id in &removed {
                self.drop_formal_attachments_for_session(session_id);
            }
            debug!(
                target: "agent_pty_bridge",
                "agent {} disconnected: dropped {} pty session route(s)",
                agent_id,
                removed.len()
            );
        }
    }

    fn drop_formal_attachment(&self, session_id: &str, client_id: &str) {
        if let Some(handle) = self
            .formal_attachments
            .write()
            .remove(&(session_id.to_string(), client_id.to_string()))
        {
            handle.abort();
        }
    }

    fn drop_formal_attachments_for_session(&self, session_id: &str) {
        let removed: Vec<JoinHandle<()>> = {
            let mut attachments = self.formal_attachments.write();
            let keys = attachments
                .keys()
                .filter(|(sid, _)| sid == session_id)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| attachments.remove(&key))
                .collect()
        };
        for handle in removed {
            handle.abort();
        }
    }

    /// Build the proto message that opens a PTY-allocated `CommandRequest`
    /// against the agent. The agent's existing PTY infrastructure
    /// (`allocate_pty: true` branch in `agent-rs/src/main.rs`) takes care
    /// of `openpty`, child supervision, and `OutputChunk` emission.
    fn build_start_command(session_id: &str, cmd: &PtyStartCommand) -> Result<ManagementMessage> {
        // argv[0] is the program; rest are arguments. agent-rs treats the
        // `command` field as the program and `args` as the rest.
        let (program, args) = if cmd.argv.is_empty() {
            ("/bin/bash".to_string(), vec!["-l".to_string()])
        } else {
            (cmd.argv[0].clone(), cmd.argv[1..].to_vec())
        };
        let (program, args) = match (cmd.backend, cmd.session_class) {
            (SessionBackend::Native, SessionClass::Direct) => (program, args),
            (SessionBackend::Screen, SessionClass::Managed) => (
                "screen".to_string(),
                build_managed_screen_args(session_id, &program, &args),
            ),
            (SessionBackend::Zellij, SessionClass::Managed) => (
                "/bin/sh".to_string(),
                build_managed_zellij_args(session_id, &program, &args),
            ),
            (SessionBackend::Tmux, SessionClass::Managed) => (
                "tmux".to_string(),
                build_managed_tmux_args(session_id, &program, &args),
            ),
            (backend, session_class) => {
                return Err(anyhow!(
                    "unsupported PTY session backend/class pair {:?}/{:?}",
                    backend,
                    session_class
                ));
            }
        };

        let env: HashMap<String, String> = cmd.env.iter().cloned().collect::<HashMap<_, _>>();

        let req = CommandRequest {
            command_id: session_id.to_string(),
            command: program,
            args,
            working_dir: cmd.cwd.clone().unwrap_or_default(),
            env,
            timeout_seconds: 0, // no timeout: PTY sessions live until close
            capture_output: true,
            run_as: String::new(),
            allocate_pty: true,
            pty_cols: cmd.initial_cols as u32,
            pty_rows: cmd.initial_rows as u32,
            pty_term: "xterm-256color".to_string(),
        };

        Ok(ManagementMessage {
            payload: Some(management_message::Payload::Command(req)),
        })
    }
}

fn build_managed_tmux_args(session_id: &str, program: &str, args: &[String]) -> Vec<String> {
    let mut tmux_args = vec![
        "new-session".to_string(),
        "-A".to_string(),
        "-s".to_string(),
        session_id.to_string(),
    ];
    if !program.is_empty() {
        tmux_args.push(build_shell_command(program, args));
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

fn build_managed_screen_args(session_id: &str, program: &str, args: &[String]) -> Vec<String> {
    let mut screen_args = vec![
        "-S".to_string(),
        session_id.to_string(),
        "-D".to_string(),
        "-RR".to_string(),
    ];
    if !program.is_empty() {
        screen_args.push(program.to_string());
        screen_args.extend(args.iter().cloned());
    }
    screen_args
}

fn build_managed_zellij_args(session_id: &str, program: &str, args: &[String]) -> Vec<String> {
    let layout = build_zellij_layout(program, args);
    let session = shell_quote(session_id);
    let layout = shell_quote(&layout);
    vec![
        "-lc".to_string(),
        format!(
            "if zellij list-sessions 2>/dev/null | awk '{{print $1}}' | grep -Fx -- {session} >/dev/null; then exec zellij attach {session}; else exec zellij --session {session} --layout-string {layout}; fi"
        ),
    ]
}

fn build_zellij_layout(program: &str, args: &[String]) -> String {
    let mut layout = format!("layout {{ pane command={} {{", kdl_quote(program));
    if !args.is_empty() {
        layout.push_str(" args");
        for arg in args {
            layout.push(' ');
            layout.push_str(&kdl_quote(arg));
        }
    }
    layout.push_str(" } }");
    layout
}

fn build_shell_command(program: &str, args: &[String]) -> String {
    let mut command = shell_quote(program);
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b':' | b'='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn kdl_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[async_trait]
impl PtyBridge for AgentPtyBridge {
    async fn start_session(
        &self,
        instance_id: &str,
        session_id: &str,
        command: PtyStartCommand,
    ) -> Result<mpsc::Receiver<PtyBridgeEvent>> {
        // Resolve `instance_id` → `(agent_id, command_tx)`. v1 dispatch
        // keys on `agent_id`, but pty_ws hands us `instance_id`, so the
        // mapping happens here exactly once at session start.
        let (agent_id, command_tx) = self
            .registry
            .get_by_instance_id(instance_id)
            .ok_or_else(|| anyhow!("agent for instance_id {} is not connected", instance_id))?;

        // Buffer size 64: a reasonable burst window for terminal output
        // (one screen-full of bytes plus headroom). The reader task in
        // pty_ws drains continuously, so we rarely back up here in
        // practice. Documented best-effort: see `forward_output` for
        // drop-on-full behavior.
        let (tx, rx) = mpsc::channel::<PtyBridgeEvent>(64);

        // Insert route BEFORE sending the start command so the first
        // OutputChunk back from the agent never hits an empty table.
        {
            let mut routes = self.routes.write();
            if routes.contains_key(session_id) {
                return Err(anyhow!(
                    "pty session {} already started on instance {}",
                    session_id,
                    instance_id
                ));
            }
            routes.insert(
                session_id.to_string(),
                OutputRoute {
                    agent_id: agent_id.clone(),
                    tx,
                },
            );
        }

        let msg = match Self::build_start_command(session_id, &command) {
            Ok(msg) => msg,
            Err(err) => {
                self.routes.write().remove(session_id);
                return Err(err);
            }
        };
        let command_label = if command.argv.is_empty() {
            "/bin/bash -l".to_string()
        } else {
            command.argv.join(" ")
        };
        self.dispatcher.register_external_pty_session(
            &agent_id,
            session_id,
            session_id,
            Some(session_id.to_string()),
            command_label,
        );
        if command_tx.send(msg).await.is_err() {
            // Couldn't reach the agent: roll back the route so a future
            // retry isn't blocked by a stale entry.
            self.routes.write().remove(session_id);
            self.dispatcher.rollback_external_pty_session(session_id);
            return Err(anyhow!(
                "failed to send pty start command to agent {} (instance={}): channel closed",
                agent_id,
                instance_id
            ));
        }

        Ok(rx)
    }

    async fn write_input(&self, instance_id: &str, session_id: &str, data: &[u8]) -> Result<()> {
        let (agent_id, command_tx) = self
            .registry
            .get_by_instance_id(instance_id)
            .ok_or_else(|| anyhow!("agent for instance_id {} is not connected", instance_id))?;

        let stdin = StdinChunk {
            command_id: session_id.to_string(),
            data: data.to_vec(),
            eof: false,
        };
        let msg = ManagementMessage {
            payload: Some(management_message::Payload::Stdin(stdin)),
        };

        command_tx.send(msg).await.map_err(|_| {
            anyhow!(
                "failed to forward stdin to agent {} (instance={}, session={}): channel closed",
                agent_id,
                instance_id,
                session_id
            )
        })
    }

    async fn resize(
        &self,
        instance_id: &str,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        let (agent_id, command_tx) = self
            .registry
            .get_by_instance_id(instance_id)
            .ok_or_else(|| anyhow!("agent for instance_id {} is not connected", instance_id))?;

        let pty_control = PtyControl {
            command_id: session_id.to_string(),
            action: Some(pty_control::Action::Resize(PtyResize {
                cols: cols as u32,
                rows: rows as u32,
            })),
        };
        let msg = ManagementMessage {
            payload: Some(management_message::Payload::PtyControl(pty_control)),
        };

        command_tx.send(msg).await.map_err(|_| {
            anyhow!(
                "failed to forward resize to agent {} (instance={}, session={}): channel closed",
                agent_id,
                instance_id,
                session_id
            )
        })?;

        if let Some(formal) = self.dispatcher.formal_session_registry() {
            formal
                .publish_resize(&session_id.to_string(), cols, rows)
                .await;
        }

        Ok(())
    }

    async fn close_session(&self, instance_id: &str, session_id: &str) -> Result<()> {
        self.drop_formal_attachments_for_session(session_id);
        if let Some(formal) = self.dispatcher.formal_session_registry() {
            formal.close(&session_id.to_string(), None).await;
        }
        // Drop the output route first so any in-flight OutputChunks
        // race-arriving after SIGTERM are silently discarded instead of
        // racing with the reader task's shutdown path.
        self.routes.write().remove(session_id);

        // Best-effort SIGTERM to the agent-side child. If the agent is
        // already gone (disconnected mid-session) we accept the error:
        // the pty_ws teardown path doesn't depend on this success.
        let Some((agent_id, command_tx)) = self.registry.get_by_instance_id(instance_id) else {
            warn!(
                target: "agent_pty_bridge",
                "close_session: agent for instance_id {} no longer connected (session={})",
                instance_id,
                session_id
            );
            return Ok(());
        };

        let pty_control = PtyControl {
            command_id: session_id.to_string(),
            action: Some(pty_control::Action::Signal(PtySignal {
                signal_number: 15, // SIGTERM
            })),
        };
        let msg = ManagementMessage {
            payload: Some(management_message::Payload::PtyControl(pty_control)),
        };

        if command_tx.send(msg).await.is_err() {
            warn!(
                target: "agent_pty_bridge",
                "close_session: failed to deliver SIGTERM to agent {} (instance={}, session={}): channel closed",
                agent_id,
                instance_id,
                session_id
            );
        }
        Ok(())
    }

    async fn attach_client(
        &self,
        _instance_id: &str,
        session_id: &str,
        client_id: &str,
        requested_role: PtySessionRole,
    ) -> Result<Option<PtySessionRole>> {
        let Some(formal) = self.dispatcher.formal_session_registry() else {
            return Ok(None);
        };

        self.drop_formal_attachment(session_id, client_id);
        let requested = match requested_role {
            PtySessionRole::Controller => crate::session::Role::Controller,
            PtySessionRole::Observer => crate::session::Role::Observer,
        };
        let Some((mut rx, granted, _seq)) = formal
            .attach(
                &session_id.to_string(),
                client_id.to_string(),
                requested,
                None,
            )
            .await
        else {
            return Ok(None);
        };

        let handle = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        self.formal_attachments
            .write()
            .insert((session_id.to_string(), client_id.to_string()), handle);

        Ok(Some(match granted {
            crate::session::Role::Controller => PtySessionRole::Controller,
            crate::session::Role::Observer => PtySessionRole::Observer,
        }))
    }

    async fn detach_client(
        &self,
        _instance_id: &str,
        session_id: &str,
        client_id: &str,
    ) -> Result<()> {
        self.drop_formal_attachment(session_id, client_id);
        if let Some(formal) = self.dispatcher.formal_session_registry() {
            formal
                .detach(&session_id.to_string(), &client_id.to_string())
                .await;
        }
        Ok(())
    }

    fn is_real(&self) -> bool {
        true
    }

    fn capabilities(&self) -> SessionHostCapabilities {
        SessionHostCapabilities {
            supported_backends: vec![
                SessionBackend::Native,
                SessionBackend::Screen,
                SessionBackend::Zellij,
                SessionBackend::Tmux,
            ],
            default_backend: SessionBackend::Native,
            supported_classes: vec![SessionClass::Direct, SessionClass::Managed],
            default_class: SessionClass::Direct,
            observe_supported: true,
            drive_supported: true,
            reattach_supported: true,
        }
    }
}

impl OutputObserver for AgentPtyBridge {
    fn on_output(&self, command_id: &str, data: &[u8]) {
        self.forward_output(command_id, data);
    }

    fn on_result(&self, command_id: &str, exit_code: i32, _success: bool, _error: &str) {
        self.forward_result(command_id, exit_code);
    }

    fn on_agent_disconnect(&self, agent_id: &str) {
        self.drop_routes_for_agent(agent_id);
    }
}

// --- Proto re-exports -------------------------------------------------------
//
// All proto types come from the management crate so the wire format is
// authoritative there. We re-import locally so the impl above reads
// linearly.

use crate::proto::{
    management_message, pty_control, CommandRequest, ManagementMessage, PtyControl, PtyResize,
    PtySignal, StdinChunk,
};

// --- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{management_message::Payload, AgentRegistration, CommandResult};
    use crate::session::{Role, SessionPayload, SessionRegistry};
    use std::time::Duration;
    use tokio::time::timeout;

    /// Spin up an empty registry + dispatcher and register a fake agent
    /// whose outbound channel we own. Returns the captured receiver so
    /// tests can assert on every message the bridge sends.
    ///
    /// Returns `(bridge, agent_id, instance_id, outbound_rx)`.
    async fn mk_bridge_with_agent() -> (
        Arc<AgentPtyBridge>,
        String,
        String,
        mpsc::Receiver<ManagementMessage>,
    ) {
        let registry = Arc::new(AgentRegistry::new());
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));

        // Register a fake agent. The mpsc Receiver is what we use to
        // assert outbound message contents.
        let (cmd_tx, cmd_rx) = mpsc::channel::<ManagementMessage>(32);
        let reg = AgentRegistration {
            agent_id: "agent-test".to_string(),
            hostname: "test.local".to_string(),
            ip_address: "127.0.0.1".to_string(),
            profile: "test".to_string(),
            loadout: "test".to_string(),
            labels: Default::default(),
            system: None,
            aiwg_frameworks: vec![],
            instance_id: String::new(),
        };
        registry.register(reg, cmd_tx);

        // Retrieve instance_id (UUIDv7 assigned at registration).
        let summary = registry
            .list_agents()
            .into_iter()
            .next()
            .expect("registered agent must be listable");
        let instance_id = summary.instance_id.clone();
        let agent_id = summary.id.clone();

        let bridge = Arc::new(AgentPtyBridge::new(registry, dispatcher));
        bridge.install_as_observer();

        (bridge, agent_id, instance_id, cmd_rx)
    }

    async fn mk_bridge_with_agent_and_formal_registry() -> (
        Arc<AgentPtyBridge>,
        Arc<SessionRegistry>,
        String,
        String,
        mpsc::Receiver<ManagementMessage>,
    ) {
        let registry = Arc::new(AgentRegistry::new());
        let session_registry = Arc::new(SessionRegistry::new());
        let dispatcher = Arc::new(
            CommandDispatcher::new(registry.clone())
                .with_session_registry(session_registry.clone()),
        );

        let (cmd_tx, cmd_rx) = mpsc::channel::<ManagementMessage>(32);
        let reg = AgentRegistration {
            agent_id: "agent-test".to_string(),
            hostname: "test.local".to_string(),
            ip_address: "127.0.0.1".to_string(),
            profile: "test".to_string(),
            loadout: "test".to_string(),
            labels: Default::default(),
            system: None,
            aiwg_frameworks: vec![],
            instance_id: String::new(),
        };
        registry.register(reg, cmd_tx);

        let summary = registry
            .list_agents()
            .into_iter()
            .next()
            .expect("registered agent must be listable");
        let instance_id = summary.instance_id.clone();
        let agent_id = summary.id.clone();

        let bridge = Arc::new(AgentPtyBridge::new(registry, dispatcher));
        bridge.install_as_observer();

        (bridge, session_registry, agent_id, instance_id, cmd_rx)
    }

    fn recv_next(rx: &mut mpsc::Receiver<ManagementMessage>) -> ManagementMessage {
        // Use blocking try_recv after yield — tests always send-then-recv
        // serially, so the message is ready immediately.
        for _ in 0..100 {
            match rx.try_recv() {
                Ok(m) => return m,
                Err(mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("rx error: {:?}", e),
            }
        }
        panic!("no message after 100ms");
    }

    #[tokio::test]
    async fn start_session_returns_404_when_agent_offline() {
        let registry = Arc::new(AgentRegistry::new());
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));
        let bridge = Arc::new(AgentPtyBridge::new(registry, dispatcher));

        let result = bridge
            .start_session("no-such-instance", "sess-1", PtyStartCommand::default())
            .await;
        assert!(
            result.is_err(),
            "start_session must fail when instance is unknown"
        );
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("not connected"),
            "error should mention offline agent, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn start_session_sends_pty_command_and_returns_receiver() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let rx = bridge
            .start_session(
                &instance_id,
                "sess-abc",
                PtyStartCommand {
                    argv: vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        "exit 0".to_string(),
                    ],
                    cwd: Some("/tmp".to_string()),
                    env: vec![("FOO".to_string(), "bar".to_string())],
                    initial_cols: 132,
                    initial_rows: 50,
                    ..PtyStartCommand::default()
                },
            )
            .await
            .expect("start_session must succeed for connected agent");

        // Assert the outbound message is a CommandRequest carrying our
        // session_id as command_id and allocate_pty = true.
        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::Command(c)) => {
                assert_eq!(c.command_id, "sess-abc");
                assert!(c.allocate_pty, "PTY sessions must set allocate_pty");
                assert_eq!(c.command, "/bin/sh");
                assert_eq!(c.args, vec!["-c".to_string(), "exit 0".to_string()]);
                assert_eq!(c.working_dir, "/tmp");
                assert_eq!(c.pty_cols, 132);
                assert_eq!(c.pty_rows, 50);
                assert_eq!(c.env.get("FOO"), Some(&"bar".to_string()));
                assert_eq!(c.timeout_seconds, 0);
            }
            other => panic!("expected Command payload, got {:?}", other.is_some()),
        }

        // Hold onto the receiver to keep the route alive for the next test.
        drop(rx);
    }

    #[tokio::test]
    async fn start_session_registers_formal_session_inventory_and_replay() {
        let (bridge, formal, agent_id, instance_id, mut cmd_rx) =
            mk_bridge_with_agent_and_formal_registry().await;

        let _rx = bridge
            .start_session(&instance_id, "sess-formal", PtyStartCommand::default())
            .await
            .unwrap();
        let _start = recv_next(&mut cmd_rx);

        let active = bridge.dispatcher.get_active_sessions(&agent_id);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].session_id, "sess-formal");
        assert_eq!(active[0].command_id, "sess-formal");

        let listed = formal.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, "sess-formal");
        assert_eq!(listed[0].agent_id, agent_id);
        assert_eq!(listed[0].command_id, "sess-formal");

        assert!(
            bridge
                .dispatcher
                .handle_stdout("sess-formal", "sess-formal", b"formal".to_vec())
                .await
                == false,
            "external pty-ws sessions are visible in formal replay but not dispatcher pending commands"
        );
        let listed = formal.list();
        assert_eq!(listed[0].replay_len, 1);

        bridge.dispatcher.handle_result(CommandResult {
            command_id: "sess-formal".to_string(),
            exit_code: 9,
            success: false,
            error: "failed".to_string(),
            duration_ms: 123,
        });

        for _ in 0..20 {
            if formal.list().is_empty()
                && bridge.dispatcher.get_active_sessions(&agent_id).is_empty()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("formal session inventory did not close after command result");
    }

    #[tokio::test]
    async fn pty_ws_attach_projects_membership_into_formal_registry() {
        let (bridge, formal, _agent_id, instance_id, mut cmd_rx) =
            mk_bridge_with_agent_and_formal_registry().await;

        let _rx = bridge
            .start_session(
                &instance_id,
                "sess-formal-members",
                PtyStartCommand::default(),
            )
            .await
            .unwrap();
        let _start = recv_next(&mut cmd_rx);

        let granted = bridge
            .attach_client(
                &instance_id,
                "sess-formal-members",
                "pty-ws-client-1",
                PtySessionRole::Controller,
            )
            .await
            .unwrap();
        assert_eq!(granted, Some(PtySessionRole::Controller));

        let granted = bridge
            .attach_client(
                &instance_id,
                "sess-formal-members",
                "pty-ws-client-2",
                PtySessionRole::Observer,
            )
            .await
            .unwrap();
        assert_eq!(granted, Some(PtySessionRole::Observer));

        let listed = formal.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].controllers, vec!["pty-ws-client-1".to_string()]);
        assert_eq!(listed[0].observers, vec!["pty-ws-client-2".to_string()]);

        bridge
            .detach_client(&instance_id, "sess-formal-members", "pty-ws-client-1")
            .await
            .unwrap();
        let listed = formal.list();
        assert!(listed[0].controllers.is_empty());
        assert_eq!(listed[0].observers, vec!["pty-ws-client-2".to_string()]);

        bridge
            .close_session(&instance_id, "sess-formal-members")
            .await
            .unwrap();
        assert!(
            bridge.formal_attachments.read().is_empty(),
            "close_session must drop formal attachment drain tasks"
        );
    }

    #[tokio::test]
    async fn pty_ws_control_events_publish_into_formal_registry() {
        let (bridge, formal, _agent_id, instance_id, mut cmd_rx) =
            mk_bridge_with_agent_and_formal_registry().await;

        let _rx = bridge
            .start_session(
                &instance_id,
                "sess-formal-controls",
                PtyStartCommand::default(),
            )
            .await
            .unwrap();
        let _start = recv_next(&mut cmd_rx);

        let (mut formal_rx, _role, _seq) = formal
            .attach(
                &"sess-formal-controls".to_string(),
                "formal-observer".to_string(),
                Role::Observer,
                None,
            )
            .await
            .expect("formal observer can attach");
        while let Ok(frame) = formal_rx.try_recv() {
            if matches!(frame.payload, SessionPayload::MembershipChanged { .. }) {
                break;
            }
        }

        bridge
            .resize(&instance_id, "sess-formal-controls", 121, 31)
            .await
            .unwrap();
        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::PtyControl(c)) => match c.action {
                Some(pty_control::Action::Resize(r)) => {
                    assert_eq!(r.cols, 121);
                    assert_eq!(r.rows, 31);
                }
                other => panic!("expected Resize action, got {:?}", other.is_some()),
            },
            other => panic!("expected PtyControl payload, got {:?}", other.is_some()),
        }

        let frame = timeout(Duration::from_secs(1), formal_rx.recv())
            .await
            .expect("formal resize frame timed out")
            .expect("formal observer channel closed before resize");
        match &frame.payload {
            SessionPayload::Resize { cols, rows } => {
                assert_eq!(*cols, 121);
                assert_eq!(*rows, 31);
            }
            other => panic!("expected formal Resize frame, got {:?}", other),
        }

        bridge
            .close_session(&instance_id, "sess-formal-controls")
            .await
            .unwrap();
        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::PtyControl(c)) => {
                assert_eq!(c.command_id, "sess-formal-controls");
                assert!(matches!(
                    c.action,
                    Some(pty_control::Action::Signal(PtySignal { signal_number: 15 }))
                ));
            }
            other => panic!(
                "expected close PtyControl payload, got {:?}",
                other.is_some()
            ),
        }

        let frame = timeout(Duration::from_secs(1), formal_rx.recv())
            .await
            .expect("formal close frame timed out")
            .expect("formal observer channel closed before close");
        assert!(matches!(
            &frame.payload,
            SessionPayload::Closed { exit_code: None }
        ));
        assert!(
            formal.list().is_empty(),
            "explicit pty-ws close must remove formal session"
        );
    }

    #[tokio::test]
    async fn formal_control_paths_route_to_external_pty_ws_session() {
        let (bridge, _formal, _agent_id, instance_id, mut cmd_rx) =
            mk_bridge_with_agent_and_formal_registry().await;

        let _rx = bridge
            .start_session(
                &instance_id,
                "sess-formal-control",
                PtyStartCommand::default(),
            )
            .await
            .unwrap();
        let _start = recv_next(&mut cmd_rx);

        bridge
            .dispatcher
            .send_stdin_to_session("sess-formal-control", b"typed\n".to_vec())
            .await
            .expect("formal session input must route to external pty-ws command");
        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::Stdin(s)) => {
                assert_eq!(s.command_id, "sess-formal-control");
                assert_eq!(s.data, b"typed\n");
                assert!(!s.eof);
            }
            other => panic!("expected formal Stdin payload, got {:?}", other.is_some()),
        }

        bridge
            .dispatcher
            .send_pty_resize_to_session("sess-formal-control", 111, 33)
            .await
            .expect("formal resize must route to external pty-ws command");
        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::PtyControl(c)) => {
                assert_eq!(c.command_id, "sess-formal-control");
                match c.action {
                    Some(pty_control::Action::Resize(r)) => {
                        assert_eq!(r.cols, 111);
                        assert_eq!(r.rows, 33);
                    }
                    other => panic!("expected Resize action, got {:?}", other.is_some()),
                }
            }
            other => panic!(
                "expected resize PtyControl payload, got {:?}",
                other.is_some()
            ),
        }

        bridge
            .dispatcher
            .send_pty_signal_to_session("sess-formal-control", 15)
            .await
            .expect("formal signal must route to external pty-ws command");
        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::PtyControl(c)) => {
                assert_eq!(c.command_id, "sess-formal-control");
                match c.action {
                    Some(pty_control::Action::Signal(s)) => assert_eq!(s.signal_number, 15),
                    other => panic!("expected Signal action, got {:?}", other.is_some()),
                }
            }
            other => panic!(
                "expected signal PtyControl payload, got {:?}",
                other.is_some()
            ),
        }
    }

    #[tokio::test]
    async fn start_session_wraps_managed_tmux_command() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let _rx = bridge
            .start_session(
                &instance_id,
                "sess-managed",
                PtyStartCommand {
                    argv: vec![
                        "/usr/bin/env".to_string(),
                        "bash".to_string(),
                        "-lc".to_string(),
                        "echo 'ready'".to_string(),
                    ],
                    backend: SessionBackend::Tmux,
                    session_class: SessionClass::Managed,
                    initial_cols: 120,
                    initial_rows: 40,
                    ..PtyStartCommand::default()
                },
            )
            .await
            .expect("managed tmux start must succeed for connected agent");

        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::Command(c)) => {
                assert_eq!(c.command_id, "sess-managed");
                assert_eq!(c.command, "tmux");
                assert_eq!(
                    c.args,
                    vec![
                        "new-session".to_string(),
                        "-A".to_string(),
                        "-s".to_string(),
                        "sess-managed".to_string(),
                        "/usr/bin/env bash -lc 'echo '\"'\"'ready'\"'\"''".to_string(),
                        ";".to_string(),
                        "set-option".to_string(),
                        "-g".to_string(),
                        "window-size".to_string(),
                        "largest".to_string(),
                    ]
                );
                assert!(c.allocate_pty);
                assert_eq!(c.pty_cols, 120);
                assert_eq!(c.pty_rows, 40);
                assert_eq!(c.timeout_seconds, 0);
            }
            other => panic!("expected Command payload, got {:?}", other.is_some()),
        }
    }

    #[tokio::test]
    async fn start_session_wraps_managed_screen_command() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let _rx = bridge
            .start_session(
                &instance_id,
                "sess-screen",
                PtyStartCommand {
                    argv: vec![
                        "/usr/bin/env".to_string(),
                        "bash".to_string(),
                        "-lc".to_string(),
                        "echo ready".to_string(),
                    ],
                    backend: SessionBackend::Screen,
                    session_class: SessionClass::Managed,
                    initial_cols: 100,
                    initial_rows: 30,
                    ..PtyStartCommand::default()
                },
            )
            .await
            .expect("managed screen start must succeed for connected agent");

        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::Command(c)) => {
                assert_eq!(c.command_id, "sess-screen");
                assert_eq!(c.command, "screen");
                assert_eq!(
                    c.args,
                    vec![
                        "-S".to_string(),
                        "sess-screen".to_string(),
                        "-D".to_string(),
                        "-RR".to_string(),
                        "/usr/bin/env".to_string(),
                        "bash".to_string(),
                        "-lc".to_string(),
                        "echo ready".to_string(),
                    ]
                );
                assert!(c.allocate_pty);
                assert_eq!(c.pty_cols, 100);
                assert_eq!(c.pty_rows, 30);
                assert_eq!(c.timeout_seconds, 0);
            }
            other => panic!("expected Command payload, got {:?}", other.is_some()),
        }
    }

    #[tokio::test]
    async fn start_session_wraps_managed_zellij_command() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let _rx = bridge
            .start_session(
                &instance_id,
                "sess-zellij",
                PtyStartCommand {
                    argv: vec![
                        "/usr/bin/env".to_string(),
                        "bash".to_string(),
                        "-lc".to_string(),
                        "echo 'ready'".to_string(),
                    ],
                    backend: SessionBackend::Zellij,
                    session_class: SessionClass::Managed,
                    initial_cols: 100,
                    initial_rows: 30,
                    ..PtyStartCommand::default()
                },
            )
            .await
            .expect("managed zellij start must succeed for connected agent");

        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::Command(c)) => {
                assert_eq!(c.command_id, "sess-zellij");
                assert_eq!(c.command, "/bin/sh");
                assert_eq!(c.args.len(), 2);
                assert_eq!(c.args[0], "-lc");
                assert!(
                    c.args[1].contains("zellij attach sess-zellij"),
                    "reattach branch must attach to existing session: {}",
                    c.args[1]
                );
                assert!(
                    c.args[1].contains("zellij --session sess-zellij --layout-string"),
                    "create branch must start zellij with a layout: {}",
                    c.args[1]
                );
                assert!(
                    c.args[1].contains("layout { pane command=\"/usr/bin/env\" {"),
                    "layout must encode command program: {}",
                    c.args[1]
                );
                assert!(
                    c.args[1].contains("args \"bash\" \"-lc\""),
                    "layout must encode command arguments: {}",
                    c.args[1]
                );
                assert!(
                    c.args[1].contains("ready"),
                    "layout must preserve quoted command content: {}",
                    c.args[1]
                );
                assert!(c.allocate_pty);
                assert_eq!(c.pty_cols, 100);
                assert_eq!(c.pty_rows, 30);
                assert_eq!(c.timeout_seconds, 0);
            }
            other => panic!("expected Command payload, got {:?}", other.is_some()),
        }
    }

    #[tokio::test]
    async fn start_session_rejects_unsupported_backend_class_pair() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let result = bridge
            .start_session(
                &instance_id,
                "sess-bad-pair",
                PtyStartCommand {
                    backend: SessionBackend::Tmux,
                    session_class: SessionClass::Direct,
                    ..PtyStartCommand::default()
                },
            )
            .await;

        assert!(result.is_err(), "tmux/direct must fail closed");
        assert!(
            cmd_rx.try_recv().is_err(),
            "invalid backend/class pair must not send a command to the agent"
        );
    }

    #[tokio::test]
    async fn start_session_rejects_screen_direct_pair() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let result = bridge
            .start_session(
                &instance_id,
                "sess-screen-direct",
                PtyStartCommand {
                    backend: SessionBackend::Screen,
                    session_class: SessionClass::Direct,
                    ..PtyStartCommand::default()
                },
            )
            .await;

        assert!(result.is_err(), "screen/direct must fail closed");
        assert!(
            cmd_rx.try_recv().is_err(),
            "invalid backend/class pair must not send a command to the agent"
        );
    }

    #[tokio::test]
    async fn start_session_rejects_zellij_direct_pair() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let result = bridge
            .start_session(
                &instance_id,
                "sess-zellij-direct",
                PtyStartCommand {
                    backend: SessionBackend::Zellij,
                    session_class: SessionClass::Direct,
                    ..PtyStartCommand::default()
                },
            )
            .await;

        assert!(result.is_err(), "zellij/direct must fail closed");
        assert!(
            cmd_rx.try_recv().is_err(),
            "invalid backend/class pair must not send a command to the agent"
        );
    }

    #[test]
    fn agent_bridge_capabilities_include_native_direct_and_managed_multiplexers() {
        let registry = Arc::new(AgentRegistry::new());
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));
        let bridge = AgentPtyBridge::new(registry, dispatcher);

        let caps = bridge.capabilities();
        assert_eq!(
            caps.supported_backends,
            vec![
                SessionBackend::Native,
                SessionBackend::Screen,
                SessionBackend::Zellij,
                SessionBackend::Tmux
            ]
        );
        assert_eq!(caps.default_backend, SessionBackend::Native);
        assert_eq!(
            caps.supported_classes,
            vec![SessionClass::Direct, SessionClass::Managed]
        );
        assert_eq!(caps.default_class, SessionClass::Direct);
        assert!(caps.observe_supported);
        assert!(caps.drive_supported);
        assert!(caps.reattach_supported);
    }

    #[tokio::test]
    async fn forward_output_chunk_delivers_to_receiver() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let mut rx = bridge
            .start_session(&instance_id, "sess-out", PtyStartCommand::default())
            .await
            .unwrap();
        // Drain the outbound start command we don't care about.
        let _ = recv_next(&mut cmd_rx);

        // Simulate an inbound OutputChunk: invoke the observer hook
        // directly the same way `handle_stdout` would.
        bridge.on_output("sess-out", b"hello world");

        let event = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("receiver must produce bytes within 200ms")
            .expect("receiver must not be closed");
        assert_eq!(event, PtyBridgeEvent::output(b"hello world".to_vec()));
    }

    #[tokio::test]
    async fn write_input_sends_stdin_chunk() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        // Start session first so it shows up in routes (not required for
        // write_input itself, but keeps the test scenario realistic).
        let _rx = bridge
            .start_session(&instance_id, "sess-in", PtyStartCommand::default())
            .await
            .unwrap();
        let _start = recv_next(&mut cmd_rx);

        bridge
            .write_input(&instance_id, "sess-in", b"input!")
            .await
            .expect("write_input must succeed");

        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::Stdin(s)) => {
                assert_eq!(s.command_id, "sess-in");
                assert_eq!(s.data, b"input!");
                assert!(!s.eof);
            }
            other => panic!("expected Stdin payload, got {:?}", other.is_some()),
        }
    }

    #[tokio::test]
    async fn resize_sends_pty_resize() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let _rx = bridge
            .start_session(&instance_id, "sess-rz", PtyStartCommand::default())
            .await
            .unwrap();
        let _start = recv_next(&mut cmd_rx);

        bridge
            .resize(&instance_id, "sess-rz", 200, 60)
            .await
            .expect("resize must succeed");

        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::PtyControl(c)) => {
                assert_eq!(c.command_id, "sess-rz");
                match c.action {
                    Some(pty_control::Action::Resize(r)) => {
                        assert_eq!(r.cols, 200);
                        assert_eq!(r.rows, 60);
                    }
                    other => panic!("expected Resize action, got {:?}", other.is_some()),
                }
            }
            other => panic!("expected PtyControl payload, got {:?}", other.is_some()),
        }
    }

    #[tokio::test]
    async fn close_session_sends_sigterm_and_drops_route() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let mut rx = bridge
            .start_session(&instance_id, "sess-cl", PtyStartCommand::default())
            .await
            .unwrap();
        let _start = recv_next(&mut cmd_rx);

        bridge
            .close_session(&instance_id, "sess-cl")
            .await
            .expect("close_session must succeed");

        let msg = recv_next(&mut cmd_rx);
        match msg.payload {
            Some(Payload::PtyControl(c)) => {
                assert_eq!(c.command_id, "sess-cl");
                match c.action {
                    Some(pty_control::Action::Signal(s)) => {
                        assert_eq!(s.signal_number, 15, "must send SIGTERM");
                    }
                    other => panic!("expected Signal action, got {:?}", other.is_some()),
                }
            }
            other => panic!("expected PtyControl payload, got {:?}", other.is_some()),
        }

        // Route was dropped: further OutputChunks must not deliver.
        bridge.on_output("sess-cl", b"post-close should be ignored");
        let result = timeout(Duration::from_millis(50), rx.recv()).await;
        // Either timeout (no message) or recv returns None (sender dropped) —
        // both mean "no delivery". We tolerate either.
        match result {
            Err(_) => { /* timeout — no message produced, as expected */ }
            Ok(None) => { /* sender dropped — also acceptable */ }
            Ok(Some(event)) => panic!("expected no delivery after close, got {:?}", event),
        }
    }

    #[tokio::test]
    async fn command_result_delivers_closed_event_with_exit_code_and_drops_route() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let mut rx = bridge
            .start_session(&instance_id, "sess-result", PtyStartCommand::default())
            .await
            .unwrap();
        let _start = recv_next(&mut cmd_rx);

        bridge.on_result("sess-result", 42, false, "boom");

        let event = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("receiver must produce close within 200ms")
            .expect("receiver must not be closed before close event");
        assert_eq!(event, PtyBridgeEvent::closed(Some(42), "command_result"));

        bridge.on_output("sess-result", b"post-result should be ignored");
        match timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(None) => { /* sender dropped after the close event */ }
            Ok(Some(event)) => panic!("expected EOF after result close, got {:?}", event),
            Err(_) => panic!("expected EOF after result close within 100ms"),
        }
    }

    #[tokio::test]
    async fn output_chunk_for_unknown_session_is_noop() {
        let registry = Arc::new(AgentRegistry::new());
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));
        let bridge = Arc::new(AgentPtyBridge::new(registry, dispatcher));

        // No session started — table is empty. on_output must not panic
        // and must not error: it just drops the chunk.
        bridge.on_output("totally-unknown", b"surprise");
        // If we got here without panicking, the assertion holds.
    }

    #[tokio::test]
    async fn agent_disconnect_drops_all_routes_for_that_agent() {
        let (bridge, agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        // Two sessions on the same agent.
        let mut rx_a = bridge
            .start_session(&instance_id, "sess-a", PtyStartCommand::default())
            .await
            .unwrap();
        let _ = recv_next(&mut cmd_rx);
        let mut rx_b = bridge
            .start_session(&instance_id, "sess-b", PtyStartCommand::default())
            .await
            .unwrap();
        let _ = recv_next(&mut cmd_rx);

        // Both routes are live: a tee delivers.
        bridge.on_output("sess-a", b"a1");
        bridge.on_output("sess-b", b"b1");
        assert_eq!(
            timeout(Duration::from_millis(200), rx_a.recv())
                .await
                .unwrap()
                .unwrap(),
            PtyBridgeEvent::output(b"a1".to_vec())
        );
        assert_eq!(
            timeout(Duration::from_millis(200), rx_b.recv())
                .await
                .unwrap()
                .unwrap(),
            PtyBridgeEvent::output(b"b1".to_vec())
        );

        // Simulate dispatcher firing the disconnect hook.
        bridge.on_agent_disconnect(&agent_id);

        // Both receivers must see EOF (sender dropped) since their
        // routes were keyed on `agent_id`.
        match timeout(Duration::from_millis(100), rx_a.recv()).await {
            Ok(None) => { /* EOF — sender dropped */ }
            Ok(Some(event)) => panic!("expected EOF on rx_a, got {:?}", event),
            Err(_) => panic!("expected EOF on rx_a within 100ms, got timeout"),
        }
        match timeout(Duration::from_millis(100), rx_b.recv()).await {
            Ok(None) => { /* EOF */ }
            Ok(Some(event)) => panic!("expected EOF on rx_b, got {:?}", event),
            Err(_) => panic!("expected EOF on rx_b within 100ms, got timeout"),
        }

        // And subsequent output chunks for those sessions are silently
        // dropped (route gone).
        bridge.on_output("sess-a", b"post-disconnect");
    }

    #[tokio::test]
    async fn duplicate_start_session_rejected() {
        let (bridge, _agent_id, instance_id, mut cmd_rx) = mk_bridge_with_agent().await;

        let _rx = bridge
            .start_session(&instance_id, "sess-dup", PtyStartCommand::default())
            .await
            .unwrap();
        let _ = recv_next(&mut cmd_rx);

        // Second start_session for the same session_id must error per
        // the PtyBridge contract.
        let err = bridge
            .start_session(&instance_id, "sess-dup", PtyStartCommand::default())
            .await;
        assert!(err.is_err(), "duplicate start_session must fail");
    }
}
