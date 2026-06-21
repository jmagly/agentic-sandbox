//! PTY bridge — abstracts the process-side of a `pty-ws/v1` session.
//!
//! The [`pty_ws`](crate::bindings::pty_ws) binding owns the WebSocket
//! transport, replay buffer, role management, and broadcast fan-out for
//! attached controllers/observers. This module defines the *source* side
//! of a session: where the bytes that flow into `output` frames come
//! from, where input bytes from the controller go, and how lifecycle
//! events (resize, close) are propagated to the underlying process.
//!
//! ## Why a trait?
//!
//! v2.0 ships two implementations:
//!
//! - [`NoOpPtyBridge`] — keeps the legacy broadcast-only behavior. The
//!   `pty_ws` binding treats the controller's `pty.session_input` as an
//!   `output` broadcast for tests and demo deployments that do not have a
//!   real agent process behind the session. This is the default for
//!   [`SessionRegistry::new()`](crate::bindings::pty_ws::SessionRegistry::new)
//!   so existing tests and the v2.0 transition keep working unchanged.
//! - `AgentPtyBridge` (in the `agentic-management` crate, follow-up) —
//!   forwards `pty.session_input` / `pty.session_resize` over the existing
//!   agent gRPC channel to the in-VM `agent-rs` PTY infrastructure
//!   (`PtyControlSender`, `RunningCommand`, `nix::pty::openpty`) and pumps
//!   the resulting `OutputChunk` stream back into the session as `output`
//!   frames. This wire-up lives in the management crate (not the executor
//!   crate) because that is where the agent registry and outbound gRPC
//!   client live. See follow-up issue for `AgentPtyBridge` implementation.
//!
//! ## Boundary
//!
//! This crate exposes the trait and the NoOp implementation. The real
//! AgentPtyBridge belongs in `agentic-management` and is injected into the
//! executor via [`AppState::pty_bridge`](crate::bindings::rest::AppState).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Canonical session role understood by the process bridge.
///
/// The `pty_ws` binding has its own wire role enum in `pty_ws.rs`; this
/// bridge-side enum lets a management-backed implementation project pty-ws
/// attachments into the management session registry without making the bridge
/// trait depend on the WebSocket module's private types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtySessionRole {
    Controller,
    Observer,
}

/// Terminal/session backend used to host a PTY session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionBackend {
    /// Native PTY process with no terminal multiplexer.
    Native,
    Screen,
    Zellij,
    Tmux,
}

/// Whether a session is ad-hoc operator-driven or orchestrator managed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionClass {
    Direct,
    Managed,
}

/// Capabilities reported by a PTY bridge implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHostCapabilities {
    pub supported_backends: Vec<SessionBackend>,
    pub default_backend: SessionBackend,
    pub supported_classes: Vec<SessionClass>,
    pub default_class: SessionClass,
    pub observe_supported: bool,
    pub drive_supported: bool,
    pub reattach_supported: bool,
}

impl Default for SessionHostCapabilities {
    fn default() -> Self {
        Self {
            supported_backends: vec![SessionBackend::Native],
            default_backend: SessionBackend::Native,
            supported_classes: vec![SessionClass::Direct],
            default_class: SessionClass::Direct,
            observe_supported: true,
            drive_supported: true,
            reattach_supported: true,
        }
    }
}

/// Command to start a new PTY-backed session inside an agent's runtime.
#[derive(Debug, Clone)]
pub struct PtyStartCommand {
    /// argv to exec inside the PTY. The first element is the program; the
    /// rest are arguments. Example: `["/bin/bash", "-l"]`.
    pub argv: Vec<String>,
    /// Optional working directory inside the sandbox. `None` = agent's
    /// default cwd.
    pub cwd: Option<String>,
    /// Additional environment variables to set in the child. Layered on
    /// top of the agent's baseline env.
    pub env: Vec<(String, String)>,
    /// Requested terminal/session backend. The default is native PTY.
    pub backend: SessionBackend,
    /// Requested session class. The default is ad-hoc direct control.
    pub session_class: SessionClass,
    /// Initial PTY window size. Defaults to 80x24 if the controller does
    /// not specify.
    pub initial_cols: u16,
    pub initial_rows: u16,
}

impl Default for PtyStartCommand {
    fn default() -> Self {
        Self {
            argv: vec!["/bin/bash".to_string(), "-l".to_string()],
            cwd: None,
            env: Vec::new(),
            backend: SessionBackend::Native,
            session_class: SessionClass::Direct,
            initial_cols: 80,
            initial_rows: 24,
        }
    }
}

/// Event emitted by the process-side PTY bridge to the WebSocket binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyBridgeEvent {
    /// Raw PTY output bytes.
    Output {
        data: Vec<u8>,
        /// Sequence number from a canonical session bus, when available.
        seq: Option<u64>,
    },
    /// Keyframe/full repaint bytes from the canonical session bus.
    Keyframe { data: Vec<u8>, seq: Option<u64> },
    /// Terminal size changed.
    Resize {
        cols: u16,
        rows: u16,
        seq: Option<u64>,
    },
    /// Terminal lifecycle event. `exit_code` is populated when the agent
    /// reported a command result; otherwise `reason` explains the close.
    Closed {
        exit_code: Option<i32>,
        reason: String,
        seq: Option<u64>,
    },
}

impl PtyBridgeEvent {
    pub fn output(data: impl Into<Vec<u8>>) -> Self {
        Self::Output {
            data: data.into(),
            seq: None,
        }
    }

    pub fn canonical_output(seq: u64, data: impl Into<Vec<u8>>) -> Self {
        Self::Output {
            data: data.into(),
            seq: Some(seq),
        }
    }

    pub fn canonical_keyframe(seq: u64, data: impl Into<Vec<u8>>) -> Self {
        Self::Keyframe {
            data: data.into(),
            seq: Some(seq),
        }
    }

    pub fn canonical_resize(seq: u64, cols: u16, rows: u16) -> Self {
        Self::Resize {
            cols,
            rows,
            seq: Some(seq),
        }
    }

    pub fn closed(exit_code: Option<i32>, reason: impl Into<String>) -> Self {
        Self::Closed {
            exit_code,
            reason: reason.into(),
            seq: None,
        }
    }

    pub fn canonical_closed(seq: u64, exit_code: Option<i32>, reason: impl Into<String>) -> Self {
        Self::Closed {
            exit_code,
            reason: reason.into(),
            seq: Some(seq),
        }
    }
}

/// Result of attaching a pty-ws client to a bridge-owned canonical bus.
pub struct PtyClientAttachment {
    pub role: PtySessionRole,
    pub events: Option<mpsc::Receiver<PtyBridgeEvent>>,
}

/// Source-of-output side of a `pty-ws/v1` session.
///
/// All methods are keyed on `(instance_id, session_id)` so a single
/// bridge implementation can multiplex many concurrent sessions across
/// many agents.
///
/// ## Lifecycle
///
/// 1. The `pty_ws` binding calls [`start_session`](Self::start_session)
///    when the first controller joins. The bridge spawns the process
///    inside the addressed agent and returns a receiver carrying
///    [`PtyBridgeEvent`]s. The binding spawns a tokio task that reads from
///    that receiver, turns output events into `output` frames, and turns
///    close events into retained `closed` frames.
/// 2. Subsequent controller `pty.session_input` frames arrive at
///    [`write_input`](Self::write_input). For the NoOp bridge this is a
///    no-op (the binding falls back to the broadcast-echo path); for a
///    real bridge it forwards the bytes to the agent's PTY master fd.
/// 3. `pty.session_resize` arrives at [`resize`](Self::resize) and is
///    forwarded to the agent's `ioctl(TIOCSWINSZ)` path.
/// 4. When the last member leaves the session the binding calls
///    [`close_session`](Self::close_session) so the bridge can signal
///    the process and reap it.
///
/// ## Errors
///
/// Implementations should return `Err` on:
/// - Agent for `instance_id` is offline / not registered.
/// - `session_id` already exists for this agent (re-starts must come
///   through reconnect-with-replay, not a fresh `start_session`).
/// - Underlying gRPC transport error.
///
/// The binding logs the error but does not abort the WS connection: the
/// session continues with no process, and any later `start_session` for
/// the same session id is treated as a no-op.
#[async_trait]
pub trait PtyBridge: Send + Sync + 'static {
    /// Start a process for `(instance_id, session_id)`. Returns a
    /// receiver yielding PTY output/lifecycle events. The receiver closes
    /// when the process exits or the bridge tears the session down.
    async fn start_session(
        &self,
        instance_id: &str,
        session_id: &str,
        command: PtyStartCommand,
    ) -> anyhow::Result<mpsc::Receiver<PtyBridgeEvent>>;

    /// Write `data` to the session's PTY master. The controller's
    /// `pty.session_input.data` field is base64-decoded by the binding
    /// before this call.
    async fn write_input(
        &self,
        instance_id: &str,
        session_id: &str,
        data: &[u8],
    ) -> anyhow::Result<()>;

    /// Resize the session's PTY window. Maps to `ioctl(TIOCSWINSZ)` on
    /// the agent side.
    async fn resize(
        &self,
        instance_id: &str,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<()>;

    /// Best-effort close: signal the child, reap, drop the bridge's
    /// session-side state. Called when the last member leaves.
    async fn close_session(&self, instance_id: &str, session_id: &str) -> anyhow::Result<()>;

    /// Register a pty-ws client attachment with the canonical session bus.
    ///
    /// Implementations that do not own a canonical bus return `Ok(None)` and
    /// let the pty-ws binding use its local compatibility registry. Real
    /// management-backed bridges return `Ok(Some(role))`, where `role` is the
    /// authoritative role granted by the canonical registry.
    async fn attach_client(
        &self,
        _instance_id: &str,
        _session_id: &str,
        _client_id: &str,
        _requested_role: PtySessionRole,
    ) -> anyhow::Result<Option<PtySessionRole>> {
        Ok(None)
    }

    /// Register a pty-ws client and, when supported, return that client's
    /// canonical event stream. Real management-backed bridges use this to let
    /// pty-ws consume formal replay/fanout without owning a second event log.
    async fn attach_client_stream(
        &self,
        instance_id: &str,
        session_id: &str,
        client_id: &str,
        requested_role: PtySessionRole,
        _replay_from: Option<u64>,
    ) -> anyhow::Result<Option<PtyClientAttachment>> {
        Ok(self
            .attach_client(instance_id, session_id, client_id, requested_role)
            .await?
            .map(|role| PtyClientAttachment { role, events: None }))
    }

    /// Remove a pty-ws client attachment from the canonical session bus.
    async fn detach_client(
        &self,
        _instance_id: &str,
        _session_id: &str,
        _client_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Returns `true` if this is a real bridge that actually delivers
    /// process output. The `NoOp` implementation returns `false`; the
    /// binding uses this signal to decide whether to fall back to the
    /// legacy `pty.session_input` → `output` echo path. Real bridges
    /// should return `true` so the binding suppresses the echo path
    /// (otherwise observers would see input echoed AND the real process
    /// output, which is doubled).
    fn is_real(&self) -> bool {
        true
    }

    /// Returns `true` when [`attach_client_stream`](Self::attach_client_stream)
    /// supplies the authoritative per-client event stream. In that mode pty-ws
    /// starts the process but does not mirror the raw bridge receiver into its
    /// local compatibility replay/broadcast ring.
    fn supports_canonical_client_events(&self) -> bool {
        false
    }

    /// Report the backends/classes this bridge can host.
    fn capabilities(&self) -> SessionHostCapabilities {
        SessionHostCapabilities::default()
    }
}

/// No-op bridge: legacy broadcast-only behavior.
///
/// This is the default [`AppState::pty_bridge`](crate::bindings::rest::AppState::pty_bridge)
/// for tests, the executor-only crate harness, and any deployment that
/// has not yet wired in a real `AgentPtyBridge`.
///
/// All methods succeed without side effects. `start_session` returns a
/// receiver whose sender is dropped immediately, so the binding's reader
/// task observes a closed channel and exits — no `output` frames are
/// produced from this side, and the controller's `pty.session_input`
/// continues to broadcast through the legacy echo path.
pub struct NoOpPtyBridge;

#[async_trait]
impl PtyBridge for NoOpPtyBridge {
    async fn start_session(
        &self,
        _instance_id: &str,
        _session_id: &str,
        _command: PtyStartCommand,
    ) -> anyhow::Result<mpsc::Receiver<PtyBridgeEvent>> {
        // Channel with the sender dropped immediately → receiver returns
        // None on first recv(). The binding's reader task notices the
        // closed channel and exits cleanly.
        let (_tx, rx) = mpsc::channel::<PtyBridgeEvent>(1);
        Ok(rx)
    }

    async fn write_input(
        &self,
        _instance_id: &str,
        _session_id: &str,
        _data: &[u8],
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn resize(
        &self,
        _instance_id: &str,
        _session_id: &str,
        _cols: u16,
        _rows: u16,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn close_session(&self, _instance_id: &str, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn is_real(&self) -> bool {
        false
    }

    fn capabilities(&self) -> SessionHostCapabilities {
        SessionHostCapabilities::default()
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Test helpers: a recording mock that lets tests assert call args
    //! and inject simulated PTY output bytes.

    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// One recorded call against the mock bridge. Variants carry the
    /// arguments tests want to assert on.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum BridgeCall {
        Start {
            instance_id: String,
            session_id: String,
            argv: Vec<String>,
            backend: SessionBackend,
            session_class: SessionClass,
        },
        Input {
            instance_id: String,
            session_id: String,
            data: Vec<u8>,
        },
        Resize {
            instance_id: String,
            session_id: String,
            cols: u16,
            rows: u16,
        },
        Close {
            instance_id: String,
            session_id: String,
        },
        Attach {
            instance_id: String,
            session_id: String,
            client_id: String,
            requested_role: PtySessionRole,
            replay_from: Option<u64>,
        },
        Detach {
            instance_id: String,
            session_id: String,
            client_id: String,
        },
    }

    /// Recording bridge. `calls` is a Mutex-guarded vec of every call
    /// (start/input/resize/close) in invocation order; tests assert
    /// against it.
    ///
    /// Each `start_session` returns the matching receiver; the test can
    /// push bytes into the corresponding sender via [`Self::sender_for`]
    /// to simulate real PTY output flowing back through the bridge.
    pub struct MockPtyBridge {
        pub calls: Mutex<Vec<BridgeCall>>,
        senders: Mutex<HashMap<(String, String), mpsc::Sender<PtyBridgeEvent>>>,
        attach_role: Mutex<Option<PtySessionRole>>,
        canonical_events: Mutex<Option<Vec<PtyBridgeEvent>>>,
    }

    impl Default for MockPtyBridge {
        fn default() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                senders: Mutex::new(HashMap::new()),
                attach_role: Mutex::new(None),
                canonical_events: Mutex::new(None),
            }
        }
    }

    impl MockPtyBridge {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// Look up the sender for an already-started session so the test
        /// can pump bytes through it.
        pub fn sender_for(
            &self,
            instance_id: &str,
            session_id: &str,
        ) -> Option<mpsc::Sender<PtyBridgeEvent>> {
            self.senders
                .lock()
                .get(&(instance_id.to_string(), session_id.to_string()))
                .cloned()
        }

        /// Drop the mock output sender without recording a close call,
        /// simulating process EOF from the bridge side.
        pub fn close_output(&self, instance_id: &str, session_id: &str) {
            self.senders
                .lock()
                .remove(&(instance_id.to_string(), session_id.to_string()));
        }

        pub fn calls(&self) -> Vec<BridgeCall> {
            self.calls.lock().clone()
        }

        /// Force the next and subsequent `attach_client` calls to return a
        /// canonical role instead of echoing the requested role.
        pub fn set_attach_role(&self, role: PtySessionRole) {
            *self.attach_role.lock() = Some(role);
        }

        pub fn set_canonical_events(&self, events: Vec<PtyBridgeEvent>) {
            *self.canonical_events.lock() = Some(events);
        }
    }

    #[async_trait]
    impl PtyBridge for MockPtyBridge {
        async fn start_session(
            &self,
            instance_id: &str,
            session_id: &str,
            command: PtyStartCommand,
        ) -> anyhow::Result<mpsc::Receiver<PtyBridgeEvent>> {
            self.calls.lock().push(BridgeCall::Start {
                instance_id: instance_id.to_string(),
                session_id: session_id.to_string(),
                argv: command.argv,
                backend: command.backend,
                session_class: command.session_class,
            });
            let (tx, rx) = mpsc::channel::<PtyBridgeEvent>(16);
            self.senders
                .lock()
                .insert((instance_id.to_string(), session_id.to_string()), tx);
            Ok(rx)
        }

        async fn write_input(
            &self,
            instance_id: &str,
            session_id: &str,
            data: &[u8],
        ) -> anyhow::Result<()> {
            self.calls.lock().push(BridgeCall::Input {
                instance_id: instance_id.to_string(),
                session_id: session_id.to_string(),
                data: data.to_vec(),
            });
            Ok(())
        }

        async fn resize(
            &self,
            instance_id: &str,
            session_id: &str,
            cols: u16,
            rows: u16,
        ) -> anyhow::Result<()> {
            self.calls.lock().push(BridgeCall::Resize {
                instance_id: instance_id.to_string(),
                session_id: session_id.to_string(),
                cols,
                rows,
            });
            Ok(())
        }

        async fn close_session(&self, instance_id: &str, session_id: &str) -> anyhow::Result<()> {
            self.calls.lock().push(BridgeCall::Close {
                instance_id: instance_id.to_string(),
                session_id: session_id.to_string(),
            });
            self.senders
                .lock()
                .remove(&(instance_id.to_string(), session_id.to_string()));
            Ok(())
        }

        async fn attach_client(
            &self,
            instance_id: &str,
            session_id: &str,
            client_id: &str,
            requested_role: PtySessionRole,
        ) -> anyhow::Result<Option<PtySessionRole>> {
            self.calls.lock().push(BridgeCall::Attach {
                instance_id: instance_id.to_string(),
                session_id: session_id.to_string(),
                client_id: client_id.to_string(),
                requested_role,
                replay_from: None,
            });
            Ok(Some((*self.attach_role.lock()).unwrap_or(requested_role)))
        }

        async fn attach_client_stream(
            &self,
            instance_id: &str,
            session_id: &str,
            client_id: &str,
            requested_role: PtySessionRole,
            replay_from: Option<u64>,
        ) -> anyhow::Result<Option<PtyClientAttachment>> {
            self.calls.lock().push(BridgeCall::Attach {
                instance_id: instance_id.to_string(),
                session_id: session_id.to_string(),
                client_id: client_id.to_string(),
                requested_role,
                replay_from,
            });
            let role = (*self.attach_role.lock()).unwrap_or(requested_role);
            let Some(events) = self.canonical_events.lock().clone() else {
                return Ok(Some(PtyClientAttachment { role, events: None }));
            };
            let (tx, rx) = mpsc::channel::<PtyBridgeEvent>(events.len().max(1));
            for event in events {
                let _ = tx.try_send(event);
            }
            Ok(Some(PtyClientAttachment {
                role,
                events: Some(rx),
            }))
        }

        async fn detach_client(
            &self,
            instance_id: &str,
            session_id: &str,
            client_id: &str,
        ) -> anyhow::Result<()> {
            self.calls.lock().push(BridgeCall::Detach {
                instance_id: instance_id.to_string(),
                session_id: session_id.to_string(),
                client_id: client_id.to_string(),
            });
            Ok(())
        }

        fn is_real(&self) -> bool {
            // Real-bridge semantics: tests want to assert that input is
            // forwarded (not echoed back through the legacy path), so the
            // mock advertises itself as real.
            true
        }

        fn supports_canonical_client_events(&self) -> bool {
            self.canonical_events.lock().is_some()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_bridge_start_returns_closed_receiver() {
        let b = NoOpPtyBridge;
        let mut rx = b
            .start_session("i", "s", PtyStartCommand::default())
            .await
            .unwrap();
        // Sender was dropped inside start_session → recv() returns None.
        assert!(rx.recv().await.is_none(), "NoOp must yield closed stream");
    }

    #[tokio::test]
    async fn noop_bridge_other_methods_are_ok() {
        let b = NoOpPtyBridge;
        assert!(b.write_input("i", "s", b"x").await.is_ok());
        assert!(b.resize("i", "s", 100, 30).await.is_ok());
        assert!(b.close_session("i", "s").await.is_ok());
        assert!(!b.is_real());
    }

    #[tokio::test]
    async fn mock_bridge_records_calls_in_order() {
        let m = test_support::MockPtyBridge::new();
        let _rx = m
            .start_session("inst", "sess", PtyStartCommand::default())
            .await
            .unwrap();
        m.write_input("inst", "sess", b"hello").await.unwrap();
        m.resize("inst", "sess", 120, 40).await.unwrap();
        m.close_session("inst", "sess").await.unwrap();

        let calls = m.calls();
        assert_eq!(calls.len(), 4);
        assert!(matches!(
            &calls[0],
            test_support::BridgeCall::Start { argv, backend, session_class, .. }
                if argv == &vec!["/bin/bash".to_string(), "-l".to_string()]
                    && *backend == SessionBackend::Native
                    && *session_class == SessionClass::Direct
        ));
        assert!(matches!(
            &calls[1],
            test_support::BridgeCall::Input { data, .. } if data == b"hello"
        ));
        assert!(matches!(
            calls[2],
            test_support::BridgeCall::Resize {
                cols: 120,
                rows: 40,
                ..
            }
        ));
        assert!(matches!(
            &calls[3],
            test_support::BridgeCall::Close { session_id, .. } if session_id == "sess"
        ));
    }

    #[test]
    fn default_capabilities_are_native_direct() {
        let caps = NoOpPtyBridge.capabilities();
        assert_eq!(caps.supported_backends, vec![SessionBackend::Native]);
        assert_eq!(caps.default_backend, SessionBackend::Native);
        assert_eq!(caps.supported_classes, vec![SessionClass::Direct]);
        assert_eq!(caps.default_class, SessionClass::Direct);
        assert!(caps.observe_supported);
        assert!(caps.drive_supported);
        assert!(caps.reattach_supported);
    }
}
