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
//!   chunk through a `(command_id == session_id) → mpsc::Sender<Vec<u8>>`
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

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use agentic_management::dispatch::{CommandDispatcher, OutputObserver};
use agentic_management::registry::AgentRegistry;

use crate::bindings::pty_bridge::{PtyBridge, PtyStartCommand};

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
    tx: mpsc::Sender<Vec<u8>>,
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
        let tx = self
            .routes
            .read()
            .get(command_id)
            .map(|r| r.tx.clone());
        if let Some(tx) = tx {
            if let Err(e) = tx.try_send(data.to_vec()) {
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
            debug!(
                target: "agent_pty_bridge",
                "agent {} disconnected: dropped {} pty session route(s)",
                agent_id,
                removed.len()
            );
        }
    }

    /// Build the proto message that opens a PTY-allocated `CommandRequest`
    /// against the agent. The agent's existing PTY infrastructure
    /// (`allocate_pty: true` branch in `agent-rs/src/main.rs`) takes care
    /// of `openpty`, child supervision, and `OutputChunk` emission.
    fn build_start_command(session_id: &str, cmd: &PtyStartCommand) -> ManagementMessage {
        // argv[0] is the program; rest are arguments. agent-rs treats the
        // `command` field as the program and `args` as the rest.
        let (program, args) = if cmd.argv.is_empty() {
            ("/bin/bash".to_string(), vec!["-l".to_string()])
        } else {
            (cmd.argv[0].clone(), cmd.argv[1..].to_vec())
        };

        let env: HashMap<String, String> =
            cmd.env.iter().cloned().collect::<HashMap<_, _>>();

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

        ManagementMessage {
            payload: Some(management_message::Payload::Command(req)),
        }
    }
}

#[async_trait]
impl PtyBridge for AgentPtyBridge {
    async fn start_session(
        &self,
        instance_id: &str,
        session_id: &str,
        command: PtyStartCommand,
    ) -> Result<mpsc::Receiver<Vec<u8>>> {
        // Resolve `instance_id` → `(agent_id, command_tx)`. v1 dispatch
        // keys on `agent_id`, but pty_ws hands us `instance_id`, so the
        // mapping happens here exactly once at session start.
        let (agent_id, command_tx) = self
            .registry
            .get_by_instance_id(instance_id)
            .ok_or_else(|| {
                anyhow!("agent for instance_id {} is not connected", instance_id)
            })?;

        // Buffer size 64: a reasonable burst window for terminal output
        // (one screen-full of bytes plus headroom). The reader task in
        // pty_ws drains continuously, so we rarely back up here in
        // practice. Documented best-effort: see `forward_output` for
        // drop-on-full behavior.
        let (tx, rx) = mpsc::channel::<Vec<u8>>(64);

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

        let msg = Self::build_start_command(session_id, &command);
        if command_tx.send(msg).await.is_err() {
            // Couldn't reach the agent: roll back the route so a future
            // retry isn't blocked by a stale entry.
            self.routes.write().remove(session_id);
            return Err(anyhow!(
                "failed to send pty start command to agent {} (instance={}): channel closed",
                agent_id,
                instance_id
            ));
        }

        Ok(rx)
    }

    async fn write_input(
        &self,
        instance_id: &str,
        session_id: &str,
        data: &[u8],
    ) -> Result<()> {
        let (agent_id, command_tx) = self
            .registry
            .get_by_instance_id(instance_id)
            .ok_or_else(|| {
                anyhow!("agent for instance_id {} is not connected", instance_id)
            })?;

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
            .ok_or_else(|| {
                anyhow!("agent for instance_id {} is not connected", instance_id)
            })?;

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
        })
    }

    async fn close_session(&self, instance_id: &str, session_id: &str) -> Result<()> {
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

    fn is_real(&self) -> bool {
        true
    }
}

impl OutputObserver for AgentPtyBridge {
    fn on_output(&self, command_id: &str, data: &[u8]) {
        self.forward_output(command_id, data);
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

use agentic_management::proto::{
    management_message, pty_control, CommandRequest, ManagementMessage, PtyControl,
    PtyResize, PtySignal, StdinChunk,
};

// --- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_management::proto::{management_message::Payload, AgentRegistration};
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

    fn recv_next(
        rx: &mut mpsc::Receiver<ManagementMessage>,
    ) -> ManagementMessage {
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
                    argv: vec!["/bin/sh".to_string(), "-c".to_string(), "exit 0".to_string()],
                    cwd: Some("/tmp".to_string()),
                    env: vec![("FOO".to_string(), "bar".to_string())],
                    initial_cols: 132,
                    initial_rows: 50,
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

        let bytes = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("receiver must produce bytes within 200ms")
            .expect("receiver must not be closed");
        assert_eq!(bytes, b"hello world");
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
            Ok(Some(b)) => panic!("expected no delivery after close, got {:?}", b),
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
            b"a1"
        );
        assert_eq!(
            timeout(Duration::from_millis(200), rx_b.recv())
                .await
                .unwrap()
                .unwrap(),
            b"b1"
        );

        // Simulate dispatcher firing the disconnect hook.
        bridge.on_agent_disconnect(&agent_id);

        // Both receivers must see EOF (sender dropped) since their
        // routes were keyed on `agent_id`.
        match timeout(Duration::from_millis(100), rx_a.recv()).await {
            Ok(None) => { /* EOF — sender dropped */ }
            Ok(Some(b)) => panic!("expected EOF on rx_a, got {:?}", b),
            Err(_) => panic!("expected EOF on rx_a within 100ms, got timeout"),
        }
        match timeout(Duration::from_millis(100), rx_b.recv()).await {
            Ok(None) => { /* EOF */ }
            Ok(Some(b)) => panic!("expected EOF on rx_b, got {:?}", b),
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
