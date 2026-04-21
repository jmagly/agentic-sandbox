//! Server-side session registry.
//!
//! Thread-safe; shared via `Arc<SessionRegistry>` across gRPC, WebSocket,
//! and HTTP handlers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
use dashmap::DashMap;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use super::{ClientId, ReplayBuffer, Role, SessionFrame, SessionId, SessionPayload, StreamKind};

/// Evict a slow subscriber after this many consecutive dropped frames.
///
/// The dropped Sender causes the relay task's Receiver to close, which sends
/// a WebSocket close frame — the client gets a clean disconnect and can
/// reconnect via JoinSession to replay missed frames.
const LAG_EVICT_THRESHOLD: usize = 500;

// ── Registry ──────────────────────────────────────────────────────────────────

pub struct SessionRegistry {
    sessions: DashMap<SessionId, Arc<Mutex<Session>>>,
    /// Reverse index: command_id → session_id for routing raw agent output.
    command_index: DashMap<String, SessionId>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            command_index: DashMap::new(),
        }
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Register a new session when the dispatcher creates one.
    pub fn create(
        &self,
        session_id: SessionId,
        agent_id: String,
        command_id: String,
        name: Option<String>,
    ) {
        self.command_index.insert(command_id.clone(), session_id.clone());
        let session = Session {
            id: session_id.clone(),
            agent_id,
            command_id,
            name,
            created_at: Instant::now(),
            seq: AtomicU64::new(0),
            controller: None,
            attachments: HashMap::new(),
            replay: ReplayBuffer::new(10_000),
        };
        self.sessions
            .insert(session_id.clone(), Arc::new(Mutex::new(session)));
        info!(session_id = %session_id, "Session registered in registry");
    }

    /// Update command_id for an existing session (e.g. after agent reconnect).
    pub async fn update_command_id(&self, session_id: &SessionId, new_command_id: String) {
        if let Some(session_arc) = self.sessions.get(session_id) {
            let mut session = session_arc.lock().await;
            self.command_index.remove(&session.command_id);
            session.command_id = new_command_id.clone();
            self.command_index.insert(new_command_id, session_id.clone());
        }
    }

    /// Look up the session_id that owns a given command_id.
    pub fn session_id_for_command(&self, command_id: &str) -> Option<SessionId> {
        self.command_index.get(command_id).map(|v| v.clone())
    }

    /// Remove session from registry (internal, after close).
    fn remove(&self, session_id: &SessionId) {
        if let Some((_, session_arc)) = self.sessions.remove(session_id) {
            if let Ok(session) = session_arc.try_lock() {
                self.command_index.remove(&session.command_id);
            }
        }
    }

    // ── Attachment ────────────────────────────────────────────────────────────

    /// Attach a client to a session.
    ///
    /// Returns `(receiver, granted_role, current_seq)`.
    ///
    /// - `Controller` is granted if the slot is vacant; otherwise `Observer`.
    /// - If `replay_from` is `Some(n)`, buffered frames from seq `n` onward
    ///   are replayed immediately.  Pass `Some(0)` for a full replay.
    pub async fn attach(
        &self,
        session_id: &SessionId,
        client_id: ClientId,
        requested_role: Role,
        replay_from: Option<u64>,
    ) -> Option<(mpsc::Receiver<Arc<SessionFrame>>, Role, u64)> {
        let session_arc = self.sessions.get(session_id)?.clone();
        let (tx, rx) = mpsc::channel::<Arc<SessionFrame>>(512);

        let mut session = session_arc.lock().await;
        let current_seq = session.seq.load(Ordering::Relaxed);

        let granted_role = if requested_role == Role::Controller && session.controller.is_none() {
            session.controller = Some(client_id.clone());
            Role::Controller
        } else {
            Role::Observer
        };

        // Replay buffered frames before sending the role-assignment frame so
        // the client sees a consistent snapshot first.
        if let Some(from_seq) = replay_from {
            for frame in session.replay.frames_from(from_seq) {
                let _ = tx.try_send(frame.clone());
            }
        }

        // Notify the attaching client of its role.
        let role_frame = Arc::new(session.make_frame(SessionPayload::RoleAssigned {
            role: granted_role,
        }));
        let _ = tx.try_send(role_frame);

        session.attachments.insert(
            client_id.clone(),
            SessionAttachment {
                client_id: client_id.clone(),
                role: granted_role,
                tx,
                lag: 0,
            },
        );

        info!(
            session_id = %session.id,
            client_id = %client_id,
            role = %granted_role,
            "Client attached to session"
        );
        Some((rx, granted_role, current_seq))
    }

    /// Detach a client.  If the client was the controller, broadcasts the vacancy.
    pub async fn detach(&self, session_id: &SessionId, client_id: &ClientId) {
        if let Some(session_arc) = self.sessions.get(session_id) {
            let mut session = session_arc.lock().await;
            session.attachments.remove(client_id);
            if session.controller.as_ref() == Some(client_id) {
                session.controller = None;
                info!(session_id = %session.id, client_id = %client_id, "Controller detached");
                let frame = Arc::new(session.make_frame(SessionPayload::ControllerChanged {
                    controller: None,
                }));
                session.broadcast(frame);
            }
        }
    }

    // ── Floor control ─────────────────────────────────────────────────────────

    /// Request the controller role.  Returns `Some(true)` if granted,
    /// `Some(false)` if already taken, `None` if session not found.
    pub async fn request_control(
        &self,
        session_id: &SessionId,
        client_id: &ClientId,
    ) -> Option<bool> {
        let session_arc = self.sessions.get(session_id)?.clone();
        let mut session = session_arc.lock().await;

        if session.controller.is_none() {
            session.controller = Some(client_id.clone());
            if let Some(att) = session.attachments.get_mut(client_id) {
                att.role = Role::Controller;
            }
            // Broadcast controller change to all.
            let change = Arc::new(session.make_frame(SessionPayload::ControllerChanged {
                controller: Some(client_id.clone()),
            }));
            session.broadcast(change);
            // Send role-assigned specifically to the new controller.
            if let Some(att) = session.attachments.get(client_id) {
                let role = Arc::new(session.make_frame(SessionPayload::RoleAssigned {
                    role: Role::Controller,
                }));
                let _ = att.tx.try_send(role);
            }
            info!(session_id = %session.id, client_id = %client_id, "Control granted");
            Some(true)
        } else {
            Some(false)
        }
    }

    /// Yield the controller role.  The slot becomes open for any observer.
    pub async fn yield_control(&self, session_id: &SessionId, client_id: &ClientId) {
        if let Some(session_arc) = self.sessions.get(session_id) {
            let mut session = session_arc.lock().await;
            if session.controller.as_ref() == Some(client_id) {
                session.controller = None;
                if let Some(att) = session.attachments.get_mut(client_id) {
                    att.role = Role::Observer;
                }
                let change = Arc::new(session.make_frame(SessionPayload::ControllerChanged {
                    controller: None,
                }));
                session.broadcast(change);
                if let Some(att) = session.attachments.get(client_id) {
                    let role = Arc::new(session.make_frame(SessionPayload::RoleAssigned {
                        role: Role::Observer,
                    }));
                    let _ = att.tx.try_send(role);
                }
                info!(session_id = %session.id, client_id = %client_id, "Control yielded");
            }
        }
    }

    /// Returns true if `client_id` holds the controller role.
    pub async fn is_controller(&self, session_id: &SessionId, client_id: &ClientId) -> bool {
        if let Some(session_arc) = self.sessions.get(session_id) {
            session_arc.lock().await.controller.as_ref() == Some(client_id)
        } else {
            false
        }
    }

    // ── Publishing ────────────────────────────────────────────────────────────

    /// Publish PTY output to all attached clients and the replay buffer.
    pub async fn publish_output(
        &self,
        session_id: &SessionId,
        stream: StreamKind,
        data: Vec<u8>,
    ) {
        if let Some(session_arc) = self.sessions.get(session_id) {
            let mut session = session_arc.lock().await;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
            let frame = Arc::new(session.make_frame(SessionPayload::Output {
                stream,
                data: encoded,
            }));
            session.replay.push(frame.clone());
            session.broadcast(frame);
        }
    }

    /// Publish a resize event (broadcast to all clients; also buffered for replay).
    pub async fn publish_resize(&self, session_id: &SessionId, cols: u16, rows: u16) {
        if let Some(session_arc) = self.sessions.get(session_id) {
            let mut session = session_arc.lock().await;
            let frame = Arc::new(session.make_frame(SessionPayload::Resize { cols, rows }));
            session.replay.push(frame.clone());
            session.broadcast(frame);
        }
    }

    /// Close a session: broadcasts `Closed` to all clients, then removes it.
    pub async fn close(&self, session_id: &SessionId, exit_code: Option<i32>) {
        if let Some(session_arc) = self.sessions.get(session_id) {
            let mut session = session_arc.lock().await;
            let frame = Arc::new(session.make_frame(SessionPayload::Closed { exit_code }));
            session.broadcast(frame);
            info!(session_id = %session.id, exit_code, "Session closed");
        }
        self.remove(session_id);
    }

    /// Return all session_ids belonging to an agent (for listing / cleanup).
    pub fn sessions_for_agent(&self, agent_id: &str) -> Vec<SessionId> {
        self.sessions
            .iter()
            .filter(|e| {
                e.value()
                    .try_lock()
                    .map(|s| s.agent_id == agent_id)
                    .unwrap_or(false)
            })
            .map(|e| e.key().clone())
            .collect()
    }

    /// Snapshot of all live sessions (for HTTP listing).
    pub fn list(&self) -> Vec<SessionSummary> {
        self.sessions
            .iter()
            .filter_map(|e| {
                e.value().try_lock().ok().map(|s| {
                    let max_client_lag = s.attachments.values().map(|a| a.lag).max().unwrap_or(0);
                    SessionSummary {
                        session_id: s.id.clone(),
                        agent_id: s.agent_id.clone(),
                        command_id: s.command_id.clone(),
                        name: s.name.clone(),
                        attachment_count: s.attachments.len(),
                        controller: s.controller.clone(),
                        replay_oldest_seq: s.replay.oldest_seq(),
                        replay_newest_seq: s.replay.newest_seq(),
                        replay_len: s.replay.len(),
                        replay_total_bytes: s.replay.total_bytes(),
                        max_client_lag,
                    }
                })
            })
            .collect()
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Summary (for HTTP API) ────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub agent_id: String,
    pub command_id: String,
    pub name: Option<String>,
    pub attachment_count: usize,
    pub controller: Option<ClientId>,
    pub replay_oldest_seq: Option<u64>,
    pub replay_newest_seq: Option<u64>,
    pub replay_len: usize,
    pub replay_total_bytes: usize,
    pub max_client_lag: usize,
}

// ── Session ───────────────────────────────────────────────────────────────────

/// Server-side session state.  Held behind a `Mutex<Session>`.
pub struct Session {
    pub id: SessionId,
    pub agent_id: String,
    pub command_id: String,
    pub name: Option<String>,
    pub created_at: Instant,
    seq: AtomicU64,
    pub controller: Option<ClientId>,
    pub attachments: HashMap<ClientId, SessionAttachment>,
    pub replay: ReplayBuffer,
}

impl Session {
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn make_frame(&self, payload: SessionPayload) -> SessionFrame {
        SessionFrame {
            session_id: self.id.clone(),
            seq: self.next_seq(),
            ts: chrono::Utc::now().timestamp_millis(),
            payload,
        }
    }

    /// Send frame to all attachments.
    ///
    /// Removes closed channels immediately. Increments `lag` on each dropped
    /// frame; evicts the attachment when lag reaches `LAG_EVICT_THRESHOLD`.
    /// The dropped Sender closes the relay Receiver, triggering a WS close
    /// frame — the client can reconnect and replay via JoinSession.
    pub(super) fn broadcast(&mut self, frame: Arc<SessionFrame>) {
        let mut to_remove: Vec<ClientId> = Vec::new();
        for (client_id, att) in self.attachments.iter_mut() {
            if att.tx.is_closed() {
                to_remove.push(client_id.clone());
                continue;
            }
            match att.tx.try_send(frame.clone()) {
                Ok(_) => att.lag = 0,
                Err(_) => {
                    att.lag += 1;
                    if att.lag >= LAG_EVICT_THRESHOLD {
                        warn!(
                            client_id = %client_id,
                            lag = att.lag,
                            "Evicting slow WebSocket subscriber (suicide snail)"
                        );
                        to_remove.push(client_id.clone());
                    } else {
                        debug!(
                            client_id = %client_id,
                            lag = att.lag,
                            "Session frame dropped (slow client)"
                        );
                    }
                }
            }
        }
        for id in to_remove {
            self.attachments.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionPayload, StreamKind};
    use std::sync::atomic::AtomicU64;
    use std::time::Instant;
    use tokio::sync::mpsc;

    fn make_output_frame(seq: u64) -> Arc<SessionFrame> {
        Arc::new(SessionFrame {
            session_id: "test-session".to_string(),
            seq,
            ts: 0,
            payload: SessionPayload::Output {
                stream: StreamKind::Stdout,
                data: "dGVzdA==".to_string(), // "test" base64
            },
        })
    }

    fn make_session_with_attachments(capacity: usize, count: usize) -> Session {
        let mut session = Session {
            id: "test-session".to_string(),
            agent_id: "agent-01".to_string(),
            command_id: "cmd-01".to_string(),
            name: Some("test".to_string()),
            created_at: Instant::now(),
            seq: AtomicU64::new(0),
            controller: None,
            attachments: HashMap::new(),
            replay: ReplayBuffer::new(1_000),
        };
        for i in 0..count {
            let (tx, _rx) = mpsc::channel(capacity);
            session.attachments.insert(
                format!("client-{}", i),
                SessionAttachment {
                    client_id: format!("client-{}", i),
                    role: Role::Observer,
                    tx,
                    lag: 0,
                },
            );
        }
        session
    }

    // ── broadcast: basic delivery ─────────────────────────────────────────────

    #[test]
    fn broadcast_delivers_to_all_clients() {
        let (tx0, mut rx0) = mpsc::channel(10);
        let (tx1, mut rx1) = mpsc::channel(10);
        let mut session = make_session_with_attachments(10, 0);
        session.attachments.insert("c0".to_string(), SessionAttachment { client_id: "c0".to_string(), role: Role::Observer, tx: tx0, lag: 0 });
        session.attachments.insert("c1".to_string(), SessionAttachment { client_id: "c1".to_string(), role: Role::Observer, tx: tx1, lag: 0 });

        let frame = make_output_frame(0);
        session.broadcast(frame);

        assert!(rx0.try_recv().is_ok(), "client 0 should receive frame");
        assert!(rx1.try_recv().is_ok(), "client 1 should receive frame");
    }

    #[test]
    fn broadcast_removes_closed_channels() {
        let (tx, rx) = mpsc::channel::<Arc<SessionFrame>>(10);
        let mut session = make_session_with_attachments(10, 0);
        session.attachments.insert("dead".to_string(), SessionAttachment {
            client_id: "dead".to_string(),
            role: Role::Observer,
            tx,
            lag: 0,
        });
        drop(rx); // close the receiver

        session.broadcast(make_output_frame(0));
        assert!(!session.attachments.contains_key("dead"), "dead attachment should be removed");
    }

    // ── broadcast: lag tracking ───────────────────────────────────────────────

    #[test]
    fn lag_increments_on_dropped_frame() {
        // Channel capacity 1 — will be full after first send
        let (tx, _rx) = mpsc::channel::<Arc<SessionFrame>>(1);
        let mut session = make_session_with_attachments(0, 0);
        session.attachments.insert("slow".to_string(), SessionAttachment {
            client_id: "slow".to_string(),
            role: Role::Observer,
            tx,
            lag: 0,
        });

        // Fill the channel
        session.broadcast(make_output_frame(0));
        // Channel is now full — next send should fail and increment lag
        session.broadcast(make_output_frame(1));

        let lag = session.attachments.get("slow").map(|a| a.lag).unwrap_or(0);
        assert_eq!(lag, 1, "lag should be 1 after one dropped frame");
    }

    #[test]
    fn lag_resets_on_successful_send() {
        // Start with high lag, then drain receiver and send again
        let (tx, mut rx) = mpsc::channel::<Arc<SessionFrame>>(2);
        let mut session = make_session_with_attachments(0, 0);
        session.attachments.insert("client".to_string(), SessionAttachment {
            client_id: "client".to_string(),
            role: Role::Observer,
            tx,
            lag: 99, // pre-set high lag
        });

        // Drain so channel has space
        let _ = rx.try_recv();

        session.broadcast(make_output_frame(0));
        let lag = session.attachments.get("client").map(|a| a.lag).unwrap_or(99);
        assert_eq!(lag, 0, "lag should reset to 0 on successful send");
    }

    // ── broadcast: suicide snail eviction ─────────────────────────────────────

    #[test]
    fn evicts_at_lag_threshold() {
        let (tx, _rx) = mpsc::channel::<Arc<SessionFrame>>(1);
        // Pre-fill the channel so try_send will fail immediately
        tx.try_send(make_output_frame(99)).unwrap();
        let mut session = make_session_with_attachments(0, 0);
        // Pre-set lag to one below threshold so next failure triggers eviction
        session.attachments.insert("snail".to_string(), SessionAttachment {
            client_id: "snail".to_string(),
            role: Role::Observer,
            tx,
            lag: LAG_EVICT_THRESHOLD - 1,
        });

        // Channel is already full — this send fails and pushes lag to threshold
        session.broadcast(make_output_frame(0));

        assert!(
            !session.attachments.contains_key("snail"),
            "snail client should be evicted at lag threshold"
        );
    }

    #[test]
    fn fast_client_not_evicted_while_slow_client_is() {
        let (slow_tx, _slow_rx) = mpsc::channel::<Arc<SessionFrame>>(1);
        let (fast_tx, mut fast_rx) = mpsc::channel::<Arc<SessionFrame>>(1024);

        // Pre-fill slow channel so it drops immediately
        slow_tx.try_send(make_output_frame(99)).unwrap();

        let mut session = make_session_with_attachments(0, 0);
        session.attachments.insert("slow".to_string(), SessionAttachment {
            client_id: "slow".to_string(),
            role: Role::Observer,
            tx: slow_tx,
            lag: LAG_EVICT_THRESHOLD - 1,
        });
        session.attachments.insert("fast".to_string(), SessionAttachment {
            client_id: "fast".to_string(),
            role: Role::Observer,
            tx: fast_tx,
            lag: 0,
        });

        // One broadcast: slow fails (pre-filled) → evicted, fast succeeds
        session.broadcast(make_output_frame(0));

        assert!(!session.attachments.contains_key("slow"), "slow client evicted");
        assert!(session.attachments.contains_key("fast"), "fast client retained");
        assert!(fast_rx.try_recv().is_ok(), "fast client receives frame");
    }

    #[test]
    fn evicted_sender_drop_closes_receiver() {
        let (tx, rx) = mpsc::channel::<Arc<SessionFrame>>(1);
        // Pre-fill so eviction triggers on first broadcast
        tx.try_send(make_output_frame(99)).unwrap();
        let mut session = make_session_with_attachments(0, 0);
        session.attachments.insert("snail".to_string(), SessionAttachment {
            client_id: "snail".to_string(),
            role: Role::Observer,
            tx,
            lag: LAG_EVICT_THRESHOLD - 1,
        });

        session.broadcast(make_output_frame(0));

        // Sender was dropped via removal — receiver should be closed
        assert!(rx.is_closed(), "receiver should be closed after sender eviction");
    }

    // ── SessionRegistry integration ───────────────────────────────────────────

    #[tokio::test]
    async fn registry_attach_sets_lag_zero() {
        let reg = SessionRegistry::new();
        reg.create("sess-1".to_string(), "agent-01".to_string(), "cmd-01".to_string(), Some("main".to_string()));

        let result = reg.attach(&"sess-1".to_string(), "client-1".to_string(), Role::Observer, Some(0)).await;
        assert!(result.is_some());

        // Access internal state to verify lag=0
        let session_arc = reg.sessions.get("sess-1").unwrap().clone();
        let session = session_arc.lock().await;
        let att = session.attachments.get("client-1").unwrap();
        assert_eq!(att.lag, 0, "new attachment should have lag=0");
    }

    #[tokio::test]
    async fn registry_list_exposes_replay_total_bytes() {
        let reg = SessionRegistry::new();
        reg.create("sess-2".to_string(), "agent-01".to_string(), "cmd-02".to_string(), Some("main".to_string()));

        reg.publish_output(&"sess-2".to_string(), StreamKind::Stdout, b"hello world".to_vec()).await;

        let summaries = reg.list();
        let s = summaries.iter().find(|s| s.session_id == "sess-2").unwrap();
        assert!(s.replay_total_bytes > 0, "replay_total_bytes should be non-zero after output");
        assert_eq!(s.max_client_lag, 0, "no clients attached, lag should be 0");
    }
}

// ── Attachment ────────────────────────────────────────────────────────────────

pub struct SessionAttachment {
    pub client_id: ClientId,
    pub role: Role,
    pub tx: mpsc::Sender<Arc<SessionFrame>>,
    /// Consecutive dropped frames. Reset to 0 on success. Eviction at LAG_EVICT_THRESHOLD.
    pub lag: usize,
}
