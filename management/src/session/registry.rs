//! Server-side session registry.
//!
//! Thread-safe; shared via `Arc<SessionRegistry>` across gRPC, WebSocket,
//! and HTTP handlers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
use dashmap::DashMap;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use super::replay::DEFAULT_MAX_FRAMES;
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
        self.command_index
            .insert(command_id.clone(), session_id.clone());
        let session = Session {
            id: session_id.clone(),
            agent_id,
            command_id,
            name,
            created_at: Instant::now(),
            seq: AtomicU64::new(0),
            attachments: HashMap::new(),
            replay: ReplayBuffer::new(DEFAULT_MAX_FRAMES),
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
            self.command_index
                .insert(new_command_id, session_id.clone());
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
    /// - The requested role is granted verbatim: `Controller` (write) or
    ///   `Observer` (read-only). Multi-controller is allowed by design —
    ///   input is serialized by the dispatcher mpsc downstream.
    /// - If `replay_from` is `Some(n)`, buffered frames from seq `n` onward
    ///   are replayed immediately.  Pass `Some(0)` for a full replay.
    /// - After attach, a `MembershipChanged` frame is broadcast to every
    ///   attached client (including the new one) so UIs can render the
    ///   participant list.
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
        let granted_role = requested_role;

        // Replay buffered frames before the role-assignment frame so the
        // client sees a consistent snapshot first. Output entries are
        // base64-encoded fresh per-replay (#147) — the ring stores raw
        // bytes, materializing wire frames here only.
        //
        // Replay-floor policy (#145):
        // - `replay_from = Some(n)` → start at `n` (filter clamps to
        //   what's actually in the ring; missing seqs are simply skipped).
        // - `replay_from = None` → default to the most recent keyframe,
        //   so a fresh joiner gets a safe full-repaint start without
        //   needing the entire ring. If there's no keyframe yet, no
        //   replay is sent (matches pre-#145 behaviour for fresh joins).
        let effective_from = replay_from.or_else(|| session.replay.last_keyframe_seq());
        if let Some(from_seq) = effective_from {
            let session_id_for_replay = session.id.clone();
            for entry in session.replay.frames_from(from_seq) {
                let frame = Arc::new(entry.to_wire(&session_id_for_replay));
                let _ = tx.try_send(frame);
            }
        }

        // Notify the attaching client of its role.
        let role_frame =
            Arc::new(session.make_frame(SessionPayload::RoleAssigned { role: granted_role }));
        let _ = tx.try_send(role_frame);

        session.attachments.insert(
            client_id.clone(),
            SessionAttachment {
                client_id: client_id.clone(),
                role: granted_role,
                tx,
                lag: Arc::new(AtomicUsize::new(0)),
            },
        );

        info!(
            session_id = %session.id,
            client_id = %client_id,
            role = %granted_role,
            "Client attached to session"
        );

        // Broadcast new membership snapshot to everyone (including new client).
        let frame = Arc::new(session.make_frame(session.membership_payload()));
        session.broadcast(frame);

        Some((rx, granted_role, current_seq))
    }

    /// Detach a client. Broadcasts `MembershipChanged` to remaining attachments.
    pub async fn detach(&self, session_id: &SessionId, client_id: &ClientId) {
        if let Some(session_arc) = self.sessions.get(session_id) {
            let mut session = session_arc.lock().await;
            let removed = session.attachments.remove(client_id).is_some();
            if removed {
                info!(session_id = %session.id, client_id = %client_id, "Client detached");
                let frame = Arc::new(session.make_frame(session.membership_payload()));
                session.broadcast(frame);
            }
        }
    }

    /// Returns true if `client_id` is attached with `Role::Controller`.
    /// Used to gate write operations (`SessionInput`, `SessionResize`).
    pub async fn is_controller(&self, session_id: &SessionId, client_id: &ClientId) -> bool {
        if let Some(session_arc) = self.sessions.get(session_id) {
            let session = session_arc.lock().await;
            session
                .attachments
                .get(client_id)
                .map(|a| a.role == Role::Controller)
                .unwrap_or(false)
        } else {
            false
        }
    }

    // ── Publishing ────────────────────────────────────────────────────────────

    /// Publish PTY output to all attached clients and the replay buffer.
    ///
    /// Lock discipline (#146): the `Mutex<Session>` is held only long
    /// enough to seq-number the frame, push to the replay buffer, and
    /// snapshot the live sender list. Fan-out to N WebSocket clients
    /// happens **outside** the lock, so the PTY producer task is never
    /// blocked by N×channel-send while a single slow client is filling.
    pub async fn publish_output(&self, session_id: &SessionId, stream: StreamKind, data: Vec<u8>) {
        let Some(session_arc) = self.sessions.get(session_id).map(|e| e.clone()) else {
            return;
        };
        // Wrap the Vec<u8> in zero-copy Bytes; cheap to clone (Arc-backed).
        let raw = bytes::Bytes::from(data);
        // Encode once for live fan-out. Per #147 we no longer store the
        // encoded copy in the ring — that String is shared via Arc<SessionFrame>
        // among live mpsc receivers and dropped once they all consume it.
        let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
        let (frame, senders) = {
            let mut session = session_arc.lock().await;
            let seq = session.next_seq_pub();
            let ts = chrono::Utc::now().timestamp_millis();
            // Ring stores raw bytes; replay re-encodes per attaching client.
            session.replay.push_output(seq, ts, stream, raw);
            let frame = Arc::new(SessionFrame {
                session_id: session.id.clone(),
                seq,
                ts,
                payload: SessionPayload::Output {
                    stream,
                    data: encoded,
                },
            });
            (frame, session.snapshot_senders())
        };
        fan_out(&session_arc, frame, senders).await;
    }

    /// Publish a periodic keyframe (#145). Same shape as `publish_output`
    /// but the wire payload is `SessionPayload::Keyframe` so smart
    /// clients can recognize it as a safe replay starting point. The
    /// ring also tracks the seq so future fresh joiners can replay
    /// from this point forward.
    pub async fn publish_keyframe(
        &self,
        session_id: &SessionId,
        stream: StreamKind,
        data: Vec<u8>,
    ) {
        let Some(session_arc) = self.sessions.get(session_id).map(|e| e.clone()) else {
            return;
        };
        if data.is_empty() {
            return; // nothing to repaint; skip
        }
        let raw = bytes::Bytes::from(data);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
        let (frame, senders) = {
            let mut session = session_arc.lock().await;
            let seq = session.next_seq_pub();
            let ts = chrono::Utc::now().timestamp_millis();
            session.replay.push_keyframe(seq, ts, stream, raw);
            let frame = Arc::new(SessionFrame {
                session_id: session.id.clone(),
                seq,
                ts,
                payload: SessionPayload::Keyframe {
                    stream,
                    data: encoded,
                },
            });
            (frame, session.snapshot_senders())
        };
        fan_out(&session_arc, frame, senders).await;
    }

    /// Publish a resize event (broadcast to all clients; also buffered for replay).
    /// Same lock discipline as `publish_output` — fan-out is lockless.
    pub async fn publish_resize(&self, session_id: &SessionId, cols: u16, rows: u16) {
        let Some(session_arc) = self.sessions.get(session_id).map(|e| e.clone()) else {
            return;
        };
        let (frame, senders) = {
            let mut session = session_arc.lock().await;
            let payload = SessionPayload::Resize { cols, rows };
            let seq = session.next_seq_pub();
            let ts = chrono::Utc::now().timestamp_millis();
            session.replay.push_control(seq, ts, payload.clone());
            let frame = Arc::new(SessionFrame {
                session_id: session.id.clone(),
                seq,
                ts,
                payload,
            });
            (frame, session.snapshot_senders())
        };
        fan_out(&session_arc, frame, senders).await;
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

    /// Aggregate replay/memory counters for Prometheus exposition.
    pub fn metrics_snapshot(&self) -> SessionMetricsSnapshot {
        let mut snapshot = SessionMetricsSnapshot::default();
        for entry in self.sessions.iter() {
            if let Ok(session) = entry.value().try_lock() {
                snapshot.active_sessions += 1;
                snapshot.hot_frames += session.replay.len() as u64;
                snapshot.hot_bytes += session.replay.total_bytes() as u64;
                snapshot.max_hot_frames += session.replay.max_frames() as u64;
                snapshot.max_hot_bytes += session.replay.max_bytes() as u64;
                snapshot.evicted_frames_total += session.replay.evicted_frames_total();
                snapshot.evicted_bytes_total += session.replay.evicted_bytes_total();
                let session_max_lag = session
                    .attachments
                    .values()
                    .map(|a| a.lag.load(Ordering::Relaxed) as u64)
                    .max()
                    .unwrap_or(0);
                snapshot.max_client_lag = snapshot.max_client_lag.max(session_max_lag);
            }
        }
        snapshot
    }

    /// Snapshot of all live sessions (for HTTP listing).
    pub fn list(&self) -> Vec<SessionSummary> {
        self.sessions
            .iter()
            .filter_map(|e| {
                e.value().try_lock().ok().map(|s| {
                    let max_client_lag = s
                        .attachments
                        .values()
                        .map(|a| a.lag.load(Ordering::Relaxed))
                        .max()
                        .unwrap_or(0);
                    let mut controllers: Vec<ClientId> = Vec::new();
                    let mut observers: Vec<ClientId> = Vec::new();
                    for att in s.attachments.values() {
                        match att.role {
                            Role::Controller => controllers.push(att.client_id.clone()),
                            Role::Observer => observers.push(att.client_id.clone()),
                        }
                    }
                    controllers.sort();
                    observers.sort();
                    SessionSummary {
                        session_id: s.id.clone(),
                        agent_id: s.agent_id.clone(),
                        command_id: s.command_id.clone(),
                        name: s.name.clone(),
                        attachment_count: s.attachments.len(),
                        controllers,
                        observers,
                        replay_oldest_seq: s.replay.oldest_seq(),
                        replay_newest_seq: s.replay.newest_seq(),
                        replay_len: s.replay.len(),
                        replay_total_bytes: s.replay.total_bytes(),
                        replay_max_frames: s.replay.max_frames(),
                        replay_max_bytes: s.replay.max_bytes(),
                        replay_evicted_frames_total: s.replay.evicted_frames_total(),
                        replay_evicted_bytes_total: s.replay.evicted_bytes_total(),
                        max_client_lag,
                    }
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionMetricsSnapshot {
    pub active_sessions: u64,
    pub hot_frames: u64,
    pub hot_bytes: u64,
    pub max_hot_frames: u64,
    pub max_hot_bytes: u64,
    pub evicted_frames_total: u64,
    pub evicted_bytes_total: u64,
    pub max_client_lag: u64,
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
    /// All attached clients holding `Role::Controller` (may be empty or
    /// contain multiple IDs; the old singleton `controller` field is gone).
    pub controllers: Vec<ClientId>,
    /// All attached clients holding `Role::Observer`.
    pub observers: Vec<ClientId>,
    pub replay_oldest_seq: Option<u64>,
    pub replay_newest_seq: Option<u64>,
    pub replay_len: usize,
    pub replay_total_bytes: usize,
    pub replay_max_frames: usize,
    pub replay_max_bytes: usize,
    pub replay_evicted_frames_total: u64,
    pub replay_evicted_bytes_total: u64,
    pub max_client_lag: usize,
}

// ── Session ───────────────────────────────────────────────────────────────────

/// Server-side session state.  Held behind a `Mutex<Session>`.
///
/// Multi-writer: any attachment whose role is `Controller` may send input.
/// There is no singleton controller field — participant set is derived
/// from `attachments`.
pub struct Session {
    pub id: SessionId,
    pub agent_id: String,
    pub command_id: String,
    pub name: Option<String>,
    pub created_at: Instant,
    seq: AtomicU64,
    pub attachments: HashMap<ClientId, SessionAttachment>,
    pub replay: ReplayBuffer,
}

impl Session {
    /// Mint and return the next monotonic sequence number for this
    /// session. Used by the publish path which builds wire frames and
    /// pushes ring entries with matching seq.
    pub(super) fn next_seq_pub(&self) -> u64 {
        self.next_seq()
    }

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

    /// Build a `MembershipChanged` payload snapshot of current attachments.
    /// Lists are sorted by `client_id` for stable output; callers can diff.
    pub(super) fn membership_payload(&self) -> SessionPayload {
        let mut controllers: Vec<ClientId> = Vec::new();
        let mut observers: Vec<ClientId> = Vec::new();
        for att in self.attachments.values() {
            match att.role {
                Role::Controller => controllers.push(att.client_id.clone()),
                Role::Observer => observers.push(att.client_id.clone()),
            }
        }
        controllers.sort();
        observers.sort();
        SessionPayload::MembershipChanged {
            controllers,
            observers,
        }
    }

    /// Send frame to all attachments — lock-held variant.
    ///
    /// Used by control-plane events (attach/detach/close) where the
    /// session lock is already held to mutate membership, so collecting
    /// senders for an outside-the-lock fan-out would just be ceremony.
    /// The hot publish path goes through `snapshot_senders` + free
    /// `fan_out` instead — see #146.
    ///
    /// Removes closed channels immediately. Increments `lag` on each
    /// dropped frame; evicts the attachment when lag reaches
    /// `LAG_EVICT_THRESHOLD`. The dropped `Sender` closes the relay
    /// `Receiver`, triggering a WS close frame — the client can
    /// reconnect and replay via `JoinSession`.
    pub(super) fn broadcast(&mut self, frame: Arc<SessionFrame>) {
        let mut to_remove: Vec<ClientId> = Vec::new();
        for (client_id, att) in self.attachments.iter_mut() {
            if att.tx.is_closed() {
                to_remove.push(client_id.clone());
                continue;
            }
            match att.tx.try_send(frame.clone()) {
                Ok(_) => att.lag.store(0, Ordering::Relaxed),
                Err(_) => {
                    let n = att.lag.fetch_add(1, Ordering::Relaxed) + 1;
                    if n >= LAG_EVICT_THRESHOLD {
                        warn!(
                            client_id = %client_id,
                            lag = n,
                            "Evicting slow WebSocket subscriber (suicide snail)"
                        );
                        to_remove.push(client_id.clone());
                    } else {
                        debug!(
                            client_id = %client_id,
                            lag = n,
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

    /// Snapshot the live senders for lockless fan-out.
    ///
    /// Returns `(client_id, tx, lag)` triples. `lag` is shared via
    /// `Arc<AtomicUsize>` so the fan-out path can update it from
    /// outside the session lock. Closed channels are filtered here so
    /// we don't waste a try_send on them.
    pub(super) fn snapshot_senders(&self) -> Vec<SenderRef> {
        self.attachments
            .values()
            .filter(|a| !a.tx.is_closed())
            .map(|a| SenderRef {
                client_id: a.client_id.clone(),
                tx: a.tx.clone(),
                lag: a.lag.clone(),
            })
            .collect()
    }
}

/// Per-attachment data needed for lockless fan-out.
pub(super) struct SenderRef {
    pub client_id: ClientId,
    pub tx: mpsc::Sender<Arc<SessionFrame>>,
    pub lag: Arc<AtomicUsize>,
}

/// Lock-free fan-out used by `publish_output` / `publish_resize`.
///
/// `try_send` per sender; on failure we bump the per-attachment lag
/// counter atomically. If lag crosses `LAG_EVICT_THRESHOLD` we briefly
/// re-acquire the session lock and prune the offender. This is the
/// only place the lock is taken on the publish path post-fan-out, and
/// only when there's something to evict — under healthy load there is
/// nothing to do here at all.
async fn fan_out(
    session_arc: &Arc<Mutex<Session>>,
    frame: Arc<SessionFrame>,
    senders: Vec<SenderRef>,
) {
    if senders.is_empty() {
        return;
    }
    let mut to_evict: Vec<ClientId> = Vec::new();
    for s in senders {
        if s.tx.is_closed() {
            to_evict.push(s.client_id);
            continue;
        }
        match s.tx.try_send(frame.clone()) {
            Ok(_) => {
                s.lag.store(0, Ordering::Relaxed);
            }
            Err(_) => {
                let n = s.lag.fetch_add(1, Ordering::Relaxed) + 1;
                if n >= LAG_EVICT_THRESHOLD {
                    warn!(
                        client_id = %s.client_id,
                        lag = n,
                        "Evicting slow WebSocket subscriber (suicide snail)"
                    );
                    to_evict.push(s.client_id);
                } else {
                    debug!(
                        client_id = %s.client_id,
                        lag = n,
                        "Session frame dropped (slow client)"
                    );
                }
            }
        }
    }
    if !to_evict.is_empty() {
        let mut session = session_arc.lock().await;
        for id in to_evict {
            session.attachments.remove(&id);
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
                    lag: Arc::new(AtomicUsize::new(0)),
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
        session.attachments.insert(
            "c0".to_string(),
            SessionAttachment {
                client_id: "c0".to_string(),
                role: Role::Observer,
                tx: tx0,
                lag: Arc::new(AtomicUsize::new(0)),
            },
        );
        session.attachments.insert(
            "c1".to_string(),
            SessionAttachment {
                client_id: "c1".to_string(),
                role: Role::Observer,
                tx: tx1,
                lag: Arc::new(AtomicUsize::new(0)),
            },
        );

        let frame = make_output_frame(0);
        session.broadcast(frame);

        assert!(rx0.try_recv().is_ok(), "client 0 should receive frame");
        assert!(rx1.try_recv().is_ok(), "client 1 should receive frame");
    }

    #[test]
    fn broadcast_removes_closed_channels() {
        let (tx, rx) = mpsc::channel::<Arc<SessionFrame>>(10);
        let mut session = make_session_with_attachments(10, 0);
        session.attachments.insert(
            "dead".to_string(),
            SessionAttachment {
                client_id: "dead".to_string(),
                role: Role::Observer,
                tx,
                lag: Arc::new(AtomicUsize::new(0)),
            },
        );
        drop(rx); // close the receiver

        session.broadcast(make_output_frame(0));
        assert!(
            !session.attachments.contains_key("dead"),
            "dead attachment should be removed"
        );
    }

    // ── broadcast: lag tracking ───────────────────────────────────────────────

    #[test]
    fn lag_increments_on_dropped_frame() {
        // Channel capacity 1 — will be full after first send
        let (tx, _rx) = mpsc::channel::<Arc<SessionFrame>>(1);
        let mut session = make_session_with_attachments(0, 0);
        session.attachments.insert(
            "slow".to_string(),
            SessionAttachment {
                client_id: "slow".to_string(),
                role: Role::Observer,
                tx,
                lag: Arc::new(AtomicUsize::new(0)),
            },
        );

        // Fill the channel
        session.broadcast(make_output_frame(0));
        // Channel is now full — next send should fail and increment lag
        session.broadcast(make_output_frame(1));

        let lag = session
            .attachments
            .get("slow")
            .map(|a| a.lag.load(Ordering::Relaxed))
            .unwrap_or(0);
        assert_eq!(lag, 1, "lag should be 1 after one dropped frame");
    }

    #[test]
    fn lag_resets_on_successful_send() {
        // Start with high lag, then drain receiver and send again
        let (tx, mut rx) = mpsc::channel::<Arc<SessionFrame>>(2);
        let mut session = make_session_with_attachments(0, 0);
        session.attachments.insert(
            "client".to_string(),
            SessionAttachment {
                client_id: "client".to_string(),
                role: Role::Observer,
                tx,
                lag: Arc::new(AtomicUsize::new(99)), // pre-set high lag
            },
        );

        // Drain so channel has space
        let _ = rx.try_recv();

        session.broadcast(make_output_frame(0));
        let lag = session
            .attachments
            .get("client")
            .map(|a| a.lag.load(Ordering::Relaxed))
            .unwrap_or(99);
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
        session.attachments.insert(
            "snail".to_string(),
            SessionAttachment {
                client_id: "snail".to_string(),
                role: Role::Observer,
                tx,
                lag: Arc::new(AtomicUsize::new(LAG_EVICT_THRESHOLD - 1)),
            },
        );

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
        session.attachments.insert(
            "slow".to_string(),
            SessionAttachment {
                client_id: "slow".to_string(),
                role: Role::Observer,
                tx: slow_tx,
                lag: Arc::new(AtomicUsize::new(LAG_EVICT_THRESHOLD - 1)),
            },
        );
        session.attachments.insert(
            "fast".to_string(),
            SessionAttachment {
                client_id: "fast".to_string(),
                role: Role::Observer,
                tx: fast_tx,
                lag: Arc::new(AtomicUsize::new(0)),
            },
        );

        // One broadcast: slow fails (pre-filled) → evicted, fast succeeds
        session.broadcast(make_output_frame(0));

        assert!(
            !session.attachments.contains_key("slow"),
            "slow client evicted"
        );
        assert!(
            session.attachments.contains_key("fast"),
            "fast client retained"
        );
        assert!(fast_rx.try_recv().is_ok(), "fast client receives frame");
    }

    #[test]
    fn evicted_sender_drop_closes_receiver() {
        let (tx, rx) = mpsc::channel::<Arc<SessionFrame>>(1);
        // Pre-fill so eviction triggers on first broadcast
        tx.try_send(make_output_frame(99)).unwrap();
        let mut session = make_session_with_attachments(0, 0);
        session.attachments.insert(
            "snail".to_string(),
            SessionAttachment {
                client_id: "snail".to_string(),
                role: Role::Observer,
                tx,
                lag: Arc::new(AtomicUsize::new(LAG_EVICT_THRESHOLD - 1)),
            },
        );

        session.broadcast(make_output_frame(0));

        // Sender was dropped via removal — receiver should be closed
        assert!(
            rx.is_closed(),
            "receiver should be closed after sender eviction"
        );
    }

    // ── SessionRegistry integration ───────────────────────────────────────────

    #[tokio::test]
    async fn registry_attach_sets_lag_zero() {
        let reg = SessionRegistry::new();
        reg.create(
            "sess-1".to_string(),
            "agent-01".to_string(),
            "cmd-01".to_string(),
            Some("main".to_string()),
        );

        let result = reg
            .attach(
                &"sess-1".to_string(),
                "client-1".to_string(),
                Role::Observer,
                Some(0),
            )
            .await;
        assert!(result.is_some());

        // Access internal state to verify lag=0
        let session_arc = reg.sessions.get("sess-1").unwrap().clone();
        let session = session_arc.lock().await;
        let att = session.attachments.get("client-1").unwrap();
        assert_eq!(
            att.lag.load(Ordering::Relaxed),
            0,
            "new attachment should have lag=0"
        );
    }

    /// Regression for #146: a slow client whose channel is full must not
    /// stall publishing. With the fast publish path doing `try_send`
    /// outside the session lock, a publisher can keep producing frames
    /// at full speed while the slow client's lag counter climbs (and
    /// eventually evicts).
    #[tokio::test]
    async fn slow_client_does_not_stall_publisher() {
        let reg = SessionRegistry::new();
        reg.create(
            "slow-test".to_string(),
            "agent-01".to_string(),
            "cmd-slow".to_string(),
            None,
        );

        // Attach a client and immediately drop the receiver side so the
        // mpsc Sender reports closed. This is the "channel full /
        // consumer gone" extreme case: every try_send fails.
        let (rx, _, _) = reg
            .attach(
                &"slow-test".to_string(),
                "stuck".to_string(),
                Role::Observer,
                None,
            )
            .await
            .unwrap();
        drop(rx);

        // Publish many frames; this must complete in bounded time.
        // Pre-fix, even though try_send is non-blocking, the lock was
        // held across N attachments × per-client-overhead each call.
        // Post-fix, the lock is released before fan-out so the publish
        // path is straight-line per call.
        let started = std::time::Instant::now();
        for i in 0..2_000usize {
            reg.publish_output(
                &"slow-test".to_string(),
                StreamKind::Stdout,
                format!("chunk {}", i).into_bytes(),
            )
            .await;
        }
        let elapsed = started.elapsed();
        // Generous bound; on a healthy machine this completes in <100ms.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "2000 publishes against a stuck client took {:?} — fan-out should not stall",
            elapsed
        );
    }

    #[tokio::test]
    async fn registry_list_exposes_replay_total_bytes() {
        let reg = SessionRegistry::new();
        reg.create(
            "sess-2".to_string(),
            "agent-01".to_string(),
            "cmd-02".to_string(),
            Some("main".to_string()),
        );

        reg.publish_output(
            &"sess-2".to_string(),
            StreamKind::Stdout,
            b"hello world".to_vec(),
        )
        .await;

        let summaries = reg.list();
        let s = summaries.iter().find(|s| s.session_id == "sess-2").unwrap();
        assert!(
            s.replay_total_bytes > 0,
            "replay_total_bytes should be non-zero after output"
        );
        assert_eq!(s.max_client_lag, 0, "no clients attached, lag should be 0");
    }

    // ── Multi-controller semantics ────────────────────────────────────────────

    #[tokio::test]
    async fn multiple_clients_can_attach_as_controllers() {
        let reg = SessionRegistry::new();
        reg.create(
            "mc-1".to_string(),
            "agent-01".to_string(),
            "cmd-mc".to_string(),
            None,
        );

        let (_rx_a, role_a, _) = reg
            .attach(
                &"mc-1".to_string(),
                "alice".to_string(),
                Role::Controller,
                None,
            )
            .await
            .unwrap();
        let (_rx_b, role_b, _) = reg
            .attach(
                &"mc-1".to_string(),
                "bob".to_string(),
                Role::Controller,
                None,
            )
            .await
            .unwrap();

        assert_eq!(role_a, Role::Controller, "first controller grant");
        assert_eq!(
            role_b,
            Role::Controller,
            "second controller grant (multi-writer)"
        );

        assert!(
            reg.is_controller(&"mc-1".to_string(), &"alice".to_string())
                .await
        );
        assert!(
            reg.is_controller(&"mc-1".to_string(), &"bob".to_string())
                .await
        );
    }

    #[tokio::test]
    async fn observer_role_is_locked_readonly() {
        let reg = SessionRegistry::new();
        reg.create(
            "ro-1".to_string(),
            "agent-01".to_string(),
            "cmd-ro".to_string(),
            None,
        );

        let (_rx, granted, _) = reg
            .attach(
                &"ro-1".to_string(),
                "watcher".to_string(),
                Role::Observer,
                None,
            )
            .await
            .unwrap();
        assert_eq!(granted, Role::Observer);
        assert!(
            !reg.is_controller(&"ro-1".to_string(), &"watcher".to_string())
                .await,
            "observer must not pass the is_controller gate"
        );
    }

    #[tokio::test]
    async fn membership_changed_frame_broadcast_on_attach_and_detach() {
        let reg = SessionRegistry::new();
        reg.create(
            "mb-1".to_string(),
            "agent-01".to_string(),
            "cmd-mb".to_string(),
            None,
        );

        // First client attaches as controller — will receive its own RoleAssigned
        // plus MembershipChanged for itself, and MembershipChanged when bob joins.
        let (mut rx_alice, _, _) = reg
            .attach(
                &"mb-1".to_string(),
                "alice".to_string(),
                Role::Controller,
                None,
            )
            .await
            .unwrap();

        // Drain alice's startup frames so the next membership event is easy to find.
        while rx_alice.try_recv().is_ok() {}

        // Bob attaches as observer.
        let (_rx_bob, _, _) = reg
            .attach(&"mb-1".to_string(), "bob".to_string(), Role::Observer, None)
            .await
            .unwrap();

        // Alice should have observed the membership update.
        let mut saw_membership = false;
        while let Ok(f) = rx_alice.try_recv() {
            if let SessionPayload::MembershipChanged {
                ref controllers,
                ref observers,
            } = f.payload
            {
                assert!(controllers.contains(&"alice".to_string()));
                assert!(observers.contains(&"bob".to_string()));
                saw_membership = true;
                break;
            }
        }
        assert!(saw_membership, "MembershipChanged must broadcast on attach");

        // Detach bob; alice should see a follow-up MembershipChanged that
        // omits bob.
        while rx_alice.try_recv().is_ok() {}
        reg.detach(&"mb-1".to_string(), &"bob".to_string()).await;
        let mut saw_detach_event = false;
        while let Ok(f) = rx_alice.try_recv() {
            if let SessionPayload::MembershipChanged { ref observers, .. } = f.payload {
                assert!(!observers.contains(&"bob".to_string()));
                saw_detach_event = true;
            }
        }
        assert!(
            saw_detach_event,
            "MembershipChanged must broadcast on detach"
        );
    }

    #[tokio::test]
    async fn new_sessions_use_three_screen_hot_replay_window() {
        let reg = SessionRegistry::new();
        reg.create(
            "hot-1".to_string(),
            "agent-01".to_string(),
            "cmd-hot".to_string(),
            None,
        );
        for _ in 0..(DEFAULT_MAX_FRAMES as u64 + 8) {
            reg.publish_output(&"hot-1".to_string(), StreamKind::Stdout, vec![b'x'])
                .await;
        }
        let summaries = reg.list();
        let s = summaries.iter().find(|s| s.session_id == "hot-1").unwrap();
        assert_eq!(s.replay_max_frames, DEFAULT_MAX_FRAMES);
        assert_eq!(s.replay_len, DEFAULT_MAX_FRAMES);
        assert_eq!(s.replay_evicted_frames_total, 8);
        let metrics = reg.metrics_snapshot();
        assert_eq!(metrics.active_sessions, 1);
        assert_eq!(metrics.hot_frames, DEFAULT_MAX_FRAMES as u64);
        assert_eq!(metrics.evicted_frames_total, 8);
    }
    #[tokio::test]
    async fn session_summary_reflects_multi_controller_lists() {
        let reg = SessionRegistry::new();
        reg.create(
            "sum-1".to_string(),
            "agent-01".to_string(),
            "cmd-sum".to_string(),
            None,
        );
        // Hold the receivers — a dropped rx closes its tx, and the next
        // attach's MembershipChanged broadcast would evict the prior
        // attachment on the closed-channel check in `broadcast`.
        let _keep_a = reg
            .attach(
                &"sum-1".to_string(),
                "a".to_string(),
                Role::Controller,
                None,
            )
            .await
            .unwrap();
        let _keep_b = reg
            .attach(
                &"sum-1".to_string(),
                "b".to_string(),
                Role::Controller,
                None,
            )
            .await
            .unwrap();
        let _keep_c = reg
            .attach(&"sum-1".to_string(), "c".to_string(), Role::Observer, None)
            .await
            .unwrap();

        let summaries = reg.list();
        let s = summaries.iter().find(|s| s.session_id == "sum-1").unwrap();
        assert_eq!(s.controllers.len(), 2);
        assert_eq!(s.observers.len(), 1);
        assert_eq!(s.attachment_count, 3);
    }
}

// ── Attachment ────────────────────────────────────────────────────────────────

pub struct SessionAttachment {
    pub client_id: ClientId,
    pub role: Role,
    pub tx: mpsc::Sender<Arc<SessionFrame>>,
    /// Consecutive dropped frames. Reset to 0 on success. Eviction at
    /// LAG_EVICT_THRESHOLD. `Arc<AtomicUsize>` so the fast publish path
    /// (`SessionRegistry::publish_*`) can mutate lag from outside the
    /// `Mutex<Session>` it cloned the value from — fan-out happens
    /// without holding the session lock. See #146.
    pub lag: Arc<AtomicUsize>,
}
