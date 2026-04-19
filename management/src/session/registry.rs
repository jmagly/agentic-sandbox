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
                e.value().try_lock().ok().map(|s| SessionSummary {
                    session_id: s.id.clone(),
                    agent_id: s.agent_id.clone(),
                    command_id: s.command_id.clone(),
                    name: s.name.clone(),
                    attachment_count: s.attachments.len(),
                    controller: s.controller.clone(),
                    replay_oldest_seq: s.replay.oldest_seq(),
                    replay_newest_seq: s.replay.newest_seq(),
                    replay_len: s.replay.len(),
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

    /// Send frame to all attachments.  Removes closed channels.
    pub(super) fn broadcast(&mut self, frame: Arc<SessionFrame>) {
        let mut dead: Vec<ClientId> = Vec::new();
        for (client_id, att) in &self.attachments {
            if att.tx.is_closed() {
                dead.push(client_id.clone());
            } else if att.tx.try_send(frame.clone()).is_err() {
                debug!(client_id = %client_id, "Session frame dropped (slow client)");
            }
        }
        for id in dead {
            warn!(client_id = %id, "Removing dead session attachment");
            self.attachments.remove(&id);
        }
    }
}

// ── Attachment ────────────────────────────────────────────────────────────────

pub struct SessionAttachment {
    pub client_id: ClientId,
    pub role: Role,
    pub tx: mpsc::Sender<Arc<SessionFrame>>,
}
