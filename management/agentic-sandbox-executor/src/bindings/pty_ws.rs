//! `pty-ws/v1` custom WebSocket binding (W4.1, #214).
//!
//! Implements the per-instance, per-session WebSocket attach endpoint
//! defined in `docs/contracts/bindings/pty-ws/v1/spec.md` and pairs it
//! with the verb set advertised by `pty-extensions/v1`
//! (`docs/contracts/extensions/pty-extensions/v1/spec.md`).
//!
//! Endpoint: `GET wss://host/agents/{instance_id}/sessions/{session_id}/attach`
//!
//! ## Surface implemented
//!
//! - All six A2A core operations as request/response frame pairs
//!   (`message/send`, `message/stream`, `tasks/get`, `tasks/list`,
//!   `tasks/cancel`, `tasks/subscribe`).
//! - PTY verbs from the extension: `pty.join_session`,
//!   `pty.session_input`, `pty.session_resize`, `pty.request_keyframe`,
//!   `pty.request_role`, `pty.release_role`, and `pty.leave_session`.
//! - Server-initiated frames: `binding_hello`, `output`, `resize`,
//!   `role_assigned`, `membership_changed`, `keyframe`, `closed`,
//!   `error`, `task_list`, plus the A2A `task` response frame.
//! - Replay buffer + `replay_from=<seq>` query parameter handling with
//!   the documented out-of-range error.
//!
//! ## Deviations from spec
//!
//! The on-the-wire envelope follows the simpler `{op, seq, payload}`
//! shape called out in the issue brief rather than the longer
//! `{op, id, ts, sequence, ...}` shape in the full v1 spec. The brief
//! is the authoritative work item for this issue; bridging the two
//! shapes (request `id` echo, RFC 3339 `ts`, etc.) is tracked separately.
//! Behavioral contracts — replay, role assignment, error vocabulary —
//! match the spec.
//!
//! Authentication (Bearer token / mTLS) is intentionally **not enforced**
//! at the WS upgrade in this issue. The existing
//! [`crate::instance::InstanceLayer`] resolves `{instance_id}` to a
//! registered context (404 on miss); per-token validation is deferred
//! to a follow-up patch.
//!
//! Real PTY process plumbing lands behind the
//! [`PtyBridge`](crate::bindings::pty_bridge::PtyBridge) trait (#237).
//! When [`AppState::pty_bridge`](crate::bindings::rest::AppState::pty_bridge)
//! is a real bridge (`is_real() == true`), `pty.session_input` and
//! `pty.session_resize` are forwarded to the bridge instead of broadcast
//! as echo frames, and the bridge's output stream feeds `output` frames
//! into the session. The default [`NoOpPtyBridge`](crate::bindings::pty_bridge::NoOpPtyBridge)
//! preserves the legacy broadcast-echo behavior for tests and harness
//! deployments without a real agent.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::bindings::pty_bridge::PtyStartCommand;
use crate::bindings::rest::AppState;
use crate::instance::InstanceExt;
use crate::store::task_store::{ListFilter, TaskRow, TaskState};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of frames retained per session for replay.
///
/// Spec §8.2: at least the larger of 1000 frames or 24h retention.
pub const REPLAY_MAX_FRAMES: usize = 1000;

/// Maximum age of frames retained for replay.
///
/// Spec §8.2: at least 24 hours.
pub const REPLAY_MAX_AGE_HOURS: i64 = 24;

/// Broadcast channel buffer for server-initiated frames. Slow clients
/// that lag this many frames behind get dropped (`broadcast::error::RecvError::Lagged`).
const BROADCAST_BUFFER: usize = 256;

/// Bindings URI advertised in `binding_hello.activated_extensions`.
pub const BINDING_URI: &str = "https://agentic-sandbox.aiwg.io/bindings/pty-ws/v1";

/// Required WebSocket subprotocol token per `pty-ws/v1` spec §2.1.
///
/// Clients SHOULD send `Sec-WebSocket-Protocol: pty-ws.v1` on the
/// upgrade request; the server echoes it on accept. If the client
/// sends a `Sec-WebSocket-Protocol` header that does NOT include this
/// token, the upgrade is rejected with HTTP 400. If the header is
/// absent entirely, the upgrade is accepted in lenient mode for the
/// v2.0 transition window (a warning is logged).
pub const SUBPROTOCOL: &str = "pty-ws.v1";

/// Companion extension URI; see `pty-extensions/v1/spec.md`.
pub const PTY_EXTENSION_URI: &str = "https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1";

// ---------------------------------------------------------------------------
// Roles + members
// ---------------------------------------------------------------------------

/// PTY session role. First joiner becomes Controller; subsequent
/// joiners become Observer. `pty.request_role { role: "controller" }`
/// is granted only when no Controller is present (v2.0 simple model;
/// richer authority transfer is future work).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Controller,
    Observer,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Controller => "controller",
            Role::Observer => "observer",
        }
    }
}

/// One attached client. The `client_id` is generated on connect; we
/// surface it back to the client in the `role_assigned` frame.
#[derive(Clone, Debug)]
pub struct Member {
    pub client_id: String,
    pub role: Role,
    pub joined_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-`(instance_id, session_id)` shared state.
///
/// Tracks the monotonic frame counter, replay ring buffer, attached
/// members, and a tokio broadcast channel that fans frames out to all
/// connected clients.
pub struct SessionState {
    pub instance_id: String,
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    /// Ring buffer of (seq, frame). Oldest dropped on overflow.
    pub replay: RwLock<Vec<(u64, Value)>>,
    /// Monotonic per-session frame counter.
    pub seq: AtomicU64,
    /// Currently attached members. Authority gating uses this list.
    pub members: RwLock<Vec<Member>>,
    /// Broadcast channel feeding all attached WS connections.
    pub broadcast_tx: broadcast::Sender<(u64, Value)>,
    pub max_frames: usize,
    pub retention: ChronoDuration,
    /// `true` once the bridge's `start_session` has been invoked for this
    /// session. Guards against double-spawn when multiple controllers
    /// rapidly join. Only meaningful when a real bridge is configured.
    bridge_started: std::sync::atomic::AtomicBool,
}

impl SessionState {
    fn new(instance_id: String, session_id: String) -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_BUFFER);
        Self {
            instance_id,
            session_id,
            created_at: Utc::now(),
            replay: RwLock::new(Vec::with_capacity(REPLAY_MAX_FRAMES)),
            seq: AtomicU64::new(0),
            members: RwLock::new(Vec::new()),
            broadcast_tx: tx,
            max_frames: REPLAY_MAX_FRAMES,
            retention: ChronoDuration::hours(REPLAY_MAX_AGE_HOURS),
            bridge_started: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Atomically claim the right to start the bridge for this session.
    /// Returns `true` if this caller should call `bridge.start_session`;
    /// returns `false` if another caller already started it.
    pub fn try_mark_bridge_started(&self) -> bool {
        !self
            .bridge_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    /// Assign a sequence number, stamp the frame envelope, append to the
    /// replay buffer (evicting oldest beyond `max_frames`), and broadcast
    /// to all attached connections.
    ///
    /// The `op` field of the returned envelope is set to `kind`; any
    /// pre-existing `seq`/`op` on the supplied `payload_envelope` is
    /// overwritten. Callers pass either a payload object (preferred) or
    /// a fully built envelope from which `payload` is harvested.
    pub fn append_frame(&self, kind: &str, payload: Value) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let envelope = json!({
            "op": kind,
            "seq": seq,
            "ts": Utc::now().to_rfc3339(),
            "payload": payload,
        });

        // Evict oldest frames beyond capacity and the retention window.
        {
            let mut buf = self.replay.write();
            buf.push((seq, envelope.clone()));
            let cutoff = Utc::now() - self.retention;
            // Drop frames older than retention, keep the latest at most
            // `max_frames`. We approximate age by re-reading `ts` from
            // each envelope; on parse failure we keep the frame
            // (conservative).
            while buf.len() > self.max_frames {
                buf.remove(0);
            }
            let mut i = 0;
            while i < buf.len() {
                let too_old = buf[i]
                    .1
                    .get("ts")
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc) < cutoff)
                    .unwrap_or(false);
                if too_old {
                    buf.remove(i);
                } else {
                    i += 1;
                }
            }
        }

        // Best-effort broadcast — ignore "no receivers" errors.
        let _ = self.broadcast_tx.send((seq, envelope));
        seq
    }

    /// Return all frames with `seq > since`, in order.
    pub fn replay_from(&self, since: u64) -> Vec<(u64, Value)> {
        self.replay
            .read()
            .iter()
            .filter(|(s, _)| *s > since)
            .cloned()
            .collect()
    }

    /// Lowest seq still held in the replay buffer (or `0` if empty).
    pub fn oldest_seq(&self) -> u64 {
        self.replay.read().first().map(|(s, _)| *s).unwrap_or(0)
    }

    /// Assign a role to a new joining client. First member becomes
    /// Controller; everyone else becomes Observer.
    pub fn assign_role(&self, client_id: &str) -> Role {
        let mut members = self.members.write();
        let role = if members.iter().any(|m| m.role == Role::Controller) {
            Role::Observer
        } else {
            Role::Controller
        };
        members.push(Member {
            client_id: client_id.to_string(),
            role,
            joined_at: Utc::now(),
        });
        role
    }

    /// Drop a member from the roster. Used on WS disconnect or
    /// `pty.leave_session`.
    pub fn drop_member(&self, client_id: &str) {
        let mut members = self.members.write();
        members.retain(|m| m.client_id != client_id);
    }

    /// Returns true if the named client currently holds the Controller
    /// role.
    pub fn is_controller(&self, client_id: &str) -> bool {
        self.members
            .read()
            .iter()
            .any(|m| m.client_id == client_id && m.role == Role::Controller)
    }

    /// Promote `client_id` to Controller iff no Controller is currently
    /// present. Returns the role the client now holds.
    pub fn try_promote_to_controller(&self, client_id: &str) -> Role {
        let mut members = self.members.write();
        let has_controller = members.iter().any(|m| m.role == Role::Controller);
        if !has_controller {
            for m in members.iter_mut() {
                if m.client_id == client_id {
                    m.role = Role::Controller;
                    return Role::Controller;
                }
            }
        }
        members
            .iter()
            .find(|m| m.client_id == client_id)
            .map(|m| m.role)
            .unwrap_or(Role::Observer)
    }

    /// Demote the named client from Controller to Observer. Used by
    /// `pty.release_role` so the next requester can take authority.
    pub fn demote_controller(&self, client_id: &str) {
        let mut members = self.members.write();
        for m in members.iter_mut() {
            if m.client_id == client_id && m.role == Role::Controller {
                m.role = Role::Observer;
            }
        }
    }

    /// Snapshot of the current membership for `membership_changed`
    /// frames.
    pub fn members_snapshot(&self) -> Vec<Member> {
        self.members.read().clone()
    }
}

// ---------------------------------------------------------------------------
// Session registry
// ---------------------------------------------------------------------------

/// In-memory `(instance_id, session_id) → SessionState` map.
///
/// Stored in [`AppState::session_registry`] so the WS handler and any
/// host-side PTY producers share the same fan-out point.
#[derive(Default)]
pub struct SessionRegistry {
    inner: RwLock<HashMap<(String, String), Arc<SessionState>>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Look up an existing session.
    pub fn get(&self, instance_id: &str, session_id: &str) -> Option<Arc<SessionState>> {
        self.inner
            .read()
            .get(&(instance_id.to_string(), session_id.to_string()))
            .cloned()
    }

    /// Look up or insert a session.
    pub fn get_or_create(&self, instance_id: &str, session_id: &str) -> Arc<SessionState> {
        let key = (instance_id.to_string(), session_id.to_string());
        if let Some(s) = self.inner.read().get(&key).cloned() {
            return s;
        }
        let mut w = self.inner.write();
        w.entry(key)
            .or_insert_with(|| {
                Arc::new(SessionState::new(
                    instance_id.to_string(),
                    session_id.to_string(),
                ))
            })
            .clone()
    }

    /// Remove a session from the registry. Existing `Arc<SessionState>`
    /// holders keep functioning; future attaches go to a fresh state.
    pub fn close(&self, instance_id: &str, session_id: &str) {
        self.inner
            .write()
            .remove(&(instance_id.to_string(), session_id.to_string()));
    }

    /// Number of registered sessions (test helper).
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

/// Query parameters supported on the WS upgrade URL. Currently just
/// `replay_from=<seq>` for reconnect-resume per spec §8.3.
#[derive(Debug, Deserialize, Default)]
pub struct AttachQuery {
    pub replay_from: Option<u64>,
}

/// Axum WebSocket handler.
///
/// Mounted at `GET /agents/{instance_id}/sessions/{session_id}/attach`.
/// The [`InstanceLayer`](crate::instance::InstanceLayer) tower middleware
/// resolves `{instance_id}` ahead of this handler; unknown instances 404
/// before the upgrade.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    InstanceExt(ctx): InstanceExt,
    Path((instance_id, session_id)): Path<(String, String)>,
    Query(query): Query<AttachQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> axum::response::Response {
    // ---- Sec-WebSocket-Protocol negotiation (spec §2.1) ----
    //
    // RFC 6455 §1.9 specifies the header as a comma-separated list of
    // protocol tokens; matching is case-sensitive. Three cases:
    //   1. Header absent → accept in lenient mode (log warn).
    //   2. Header present, contains "pty-ws.v1" → echo via .protocols().
    //   3. Header present, does NOT contain "pty-ws.v1" → 400.
    let subprotocol_header = headers.get(header::SEC_WEBSOCKET_PROTOCOL);
    let client_offers_pty_ws = match subprotocol_header {
        None => None,
        Some(value) => match value.to_str() {
            Ok(s) => {
                let offered = s.split(',').any(|tok| tok.trim() == SUBPROTOCOL);
                Some(offered)
            }
            Err(_) => {
                // Non-ASCII header value — treat as malformed offer.
                Some(false)
            }
        },
    };
    match client_offers_pty_ws {
        None => {
            tracing::warn!(
                "WS upgrade without subprotocol header — accepting in lenient mode for v2.0 transition"
            );
        }
        Some(true) => {
            // Will echo via ws.protocols(...) below.
        }
        Some(false) => {
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::to_string(&json!({
                    "error": "unsupported_subprotocol",
                    "supported": [SUBPROTOCOL],
                }))
                .unwrap_or_else(|_| {
                    String::from(r#"{"error":"unsupported_subprotocol","supported":["pty-ws.v1"]}"#)
                }),
            )
                .into_response();
        }
    }

    let session = state
        .session_registry
        .get_or_create(&instance_id, &session_id);
    let _ = ctx; // future: per-instance auth + audit
    let ws = ws.protocols([SUBPROTOCOL]);
    ws.on_upgrade(move |socket| {
        connection_loop(
            socket,
            state.clone(),
            session,
            instance_id,
            session_id,
            query.replay_from,
        )
    })
}

/// One WS connection. Runs until the client disconnects, the server
/// sends `closed`, or the underlying socket errors.
async fn connection_loop(
    socket: WebSocket,
    state: AppState,
    session: Arc<SessionState>,
    instance_id: String,
    session_id: String,
    replay_from: Option<u64>,
) {
    let client_id = Uuid::now_v7().to_string();
    let (mut sink, mut stream) = socket.split();

    // Subscribe before sending hello so we don't miss frames produced
    // mid-handshake.
    let mut rx = session.broadcast_tx.subscribe();

    // ---- binding_hello (server → client, very first frame) ----
    let hello = json!({
        "op": "binding_hello",
        "seq": 0,
        "ts": Utc::now().to_rfc3339(),
        "payload": {
            "binding_uri": BINDING_URI,
            "binding_version": "1.0.0",
            "activated_extensions": [PTY_EXTENSION_URI],
            "supported_operations": [
                "message/send",
                "message/stream",
                "tasks/get",
                "tasks/list",
                "tasks/cancel",
                "tasks/subscribe",
                "pty.join_session",
                "pty.session_input",
                "pty.session_resize",
                "pty.request_keyframe",
                "pty.request_role",
                "pty.release_role",
                "pty.leave_session",
            ],
            "session": {
                "instance_id": instance_id,
                "session_id": session_id,
                "current_sequence": session.seq.load(Ordering::SeqCst),
            },
            "server_info": {
                "name": "agentic-sandbox-executor",
                "version": env!("CARGO_PKG_VERSION"),
            }
        }
    });
    if sink
        .send(WsMessage::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // ---- Optional replay-from-seq on connect ----
    if let Some(since) = replay_from {
        let oldest = session.oldest_seq();
        let current = session.seq.load(Ordering::SeqCst);
        if since < oldest && current > 0 && oldest > 0 {
            // Spec §8.3: out-of-range → error frame + fresh keyframe.
            let err = json!({
                "op": "error",
                "seq": null,
                "ts": Utc::now().to_rfc3339(),
                "payload": {
                    "code": "replay.out_of_range",
                    "message": format!(
                        "replay_from={} precedes oldest retained seq {}",
                        since, oldest
                    ),
                    "oldest": oldest,
                }
            });
            if sink
                .send(WsMessage::Text(err.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
            let kf = build_keyframe(&session);
            if sink
                .send(WsMessage::Text(kf.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        } else {
            // In-range: emit a fresh keyframe so the client has a coherent
            // baseline, then deliver the buffered delta.
            let kf = build_keyframe(&session);
            if sink
                .send(WsMessage::Text(kf.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
            for (_seq, frame) in session.replay_from(since) {
                if sink
                    .send(WsMessage::Text(frame.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }

    // ---- Main bidirectional loop ----
    //
    // Two concurrent producers feed the sink: (a) server-initiated
    // broadcasts on `rx`, and (b) responses to client frames coming in
    // on `stream`. We multiplex with `tokio::select!` so neither side
    // can starve the other.
    loop {
        tokio::select! {
            biased;

            // Server-initiated frames (from append_frame on any thread).
            recv = rx.recv() => match recv {
                Ok((_seq, frame)) => {
                    if sink.send(WsMessage::Text(frame.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Skipped frames are recoverable via replay on
                    // reconnect; surface a non-fatal error frame.
                    let warn = json!({
                        "op": "error",
                        "seq": null,
                        "ts": Utc::now().to_rfc3339(),
                        "payload": {
                            "code": "broadcast.lagged",
                            "message": "client lagged broadcast channel; reconnect with replay_from",
                            "status": 200,
                        }
                    });
                    let _ = sink.send(WsMessage::Text(warn.to_string().into())).await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },

            // Client → server messages.
            msg = stream.next() => match msg {
                None => break,
                Some(Err(_)) => break,
                Some(Ok(WsMessage::Close(_))) => break,
                Some(Ok(WsMessage::Ping(_))) | Some(Ok(WsMessage::Pong(_))) => {
                    // Axum auto-replies to Ping; nothing to do.
                    continue;
                }
                Some(Ok(WsMessage::Binary(_))) => {
                    // Spec §2.3: binary frames reserved; reject.
                    let err = build_error_frame(
                        "request.invalid_params",
                        "Binary frames are not supported; send UTF-8 JSON",
                        400,
                    );
                    if sink.send(WsMessage::Text(err.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Some(Ok(WsMessage::Text(text))) => {
                    let parsed: Result<Value, _> = serde_json::from_str(&text);
                    let envelope = match parsed {
                        Ok(v) if v.is_object() => v,
                        _ => {
                            let err = build_error_frame(
                                "request.invalid_params",
                                "Frame must be a JSON object",
                                400,
                            );
                            let _ = sink.send(WsMessage::Text(err.to_string().into())).await;
                            continue;
                        }
                    };
                    let op = envelope.get("op").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let payload = envelope.get("payload").cloned().unwrap_or(Value::Null);

                    let response = dispatch_op(
                        &op,
                        payload,
                        &client_id,
                        &session,
                        &state,
                        &instance_id,
                    ).await;

                    if let Some(resp) = response {
                        if sink.send(WsMessage::Text(resp.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                }
            },
        }
    }

    // ---- Cleanup ----
    let was_controller = session.is_controller(&client_id);
    session.drop_member(&client_id);
    let remaining = session.members_snapshot();
    if was_controller || !remaining.is_empty() {
        // Notify remaining attachees that membership changed.
        session.append_frame(
            "membership_changed",
            json!({
                "members": remaining
                    .iter()
                    .map(|m| json!({
                        "client_id": m.client_id,
                        "role": m.role.as_str(),
                    }))
                    .collect::<Vec<_>>(),
            }),
        );
    }
    if remaining.is_empty() {
        // Last member out: ask the bridge to reap any backing process,
        // and forget this session in the registry so a future attach
        // gets a fresh `SessionState`.
        let bridge = state.pty_bridge.clone();
        let inst = instance_id.clone();
        let sid = session_id.clone();
        tokio::spawn(async move {
            if let Err(e) = bridge.close_session(&inst, &sid).await {
                tracing::warn!(
                    "pty bridge close_session failed: {} (instance={}, session={})",
                    e,
                    inst,
                    sid
                );
            }
        });
        state.session_registry.close(&instance_id, &session_id);
    }
}

/// Spawn a tokio task that drains the bridge's `start_session` receiver
/// and turns each chunk into an `output` frame on `session`. Logs and
/// exits cleanly when the bridge closes the channel (process exit, agent
/// disconnect, bridge teardown).
fn spawn_bridge_reader(
    session: Arc<SessionState>,
    bridge: Arc<dyn crate::bindings::pty_bridge::PtyBridge>,
    instance_id: &str,
) {
    let inst = instance_id.to_string();
    let sid = session.session_id.clone();
    tokio::spawn(async move {
        let mut rx = match bridge
            .start_session(&inst, &sid, PtyStartCommand::default())
            .await
        {
            Ok(rx) => rx,
            Err(e) => {
                tracing::warn!(
                    "pty bridge start_session failed: {} (instance={}, session={})",
                    e,
                    inst,
                    sid
                );
                return;
            }
        };
        while let Some(chunk) = rx.recv().await {
            if chunk.is_empty() {
                continue;
            }
            let encoded = B64.encode(&chunk);
            session.append_frame("output", json!({ "data": encoded }));
        }
    });
}

// ---------------------------------------------------------------------------
// Op dispatcher
// ---------------------------------------------------------------------------

/// Dispatch one client frame. Returns `Some(response_envelope)` when the
/// op produces a unicast reply (A2A core ops), and `None` for PTY verbs
/// whose effect is a broadcast to all attached clients (which the sender
/// will also receive through its own `rx` subscription).
async fn dispatch_op(
    op: &str,
    payload: Value,
    client_id: &str,
    session: &Arc<SessionState>,
    state: &AppState,
    instance_id: &str,
) -> Option<Value> {
    match op {
        // ----- A2A core ops -----
        "message/send" => Some(handle_message_send(payload, state, instance_id).await),
        "message/stream" => {
            // Mirror REST behavior: persist the task, ack immediately.
            // Subsequent task updates flow through subscribe semantics
            // which we approximate with the existing broadcast channel.
            Some(handle_message_send(payload, state, instance_id).await)
        }
        "tasks/get" => Some(handle_tasks_get(payload, state).await),
        "tasks/list" => Some(handle_tasks_list(payload, state).await),
        "tasks/cancel" => Some(handle_tasks_cancel(payload, state).await),
        "tasks/subscribe" => Some(handle_tasks_subscribe(payload, state).await),

        // ----- PTY extension verbs -----
        "pty.join_session" => {
            let role = session.assign_role(client_id);

            // First controller arrival → ask the bridge to spawn the
            // real PTY process and pipe its output back into this
            // session as `output` frames. Skipped for the NoOp bridge:
            // there's no process to spawn, and the legacy echo path
            // continues to handle `pty.session_input`.
            if role == Role::Controller
                && state.pty_bridge.is_real()
                && session.try_mark_bridge_started()
            {
                spawn_bridge_reader(session.clone(), state.pty_bridge.clone(), instance_id);
            }

            // Direct ack to the joiner.
            let role_assigned = json!({
                "op": "role_assigned",
                "seq": session.seq.load(Ordering::SeqCst),
                "ts": Utc::now().to_rfc3339(),
                "payload": {
                    "client_id": client_id,
                    "role": role.as_str(),
                }
            });
            // Broadcast membership update to everyone else.
            session.append_frame(
                "membership_changed",
                json!({
                    "members": session
                        .members_snapshot()
                        .iter()
                        .map(|m| json!({
                            "client_id": m.client_id,
                            "role": m.role.as_str(),
                        }))
                        .collect::<Vec<_>>(),
                }),
            );
            Some(role_assigned)
        }

        "pty.session_input" => {
            if !session.is_controller(client_id) {
                return Some(build_error_frame(
                    "pty.permission_denied",
                    "Only the controller may send PTY input",
                    403,
                ));
            }
            let data = payload.get("data").cloned().unwrap_or(Value::Null);
            let terminal_size = payload.get("terminal_size").cloned();

            if state.pty_bridge.is_real() {
                // Forward bytes to the real PTY master. The bridge's
                // output stream is what produces `output` frames; we
                // deliberately do NOT echo input here (that would double
                // up with real process output).
                if let Some(s) = data.as_str() {
                    match B64.decode(s) {
                        Ok(bytes) => {
                            if let Err(e) = state
                                .pty_bridge
                                .write_input(instance_id, &session.session_id, &bytes)
                                .await
                            {
                                tracing::warn!(
                                    "pty bridge write_input failed: {} (instance={}, session={})",
                                    e,
                                    instance_id,
                                    session.session_id
                                );
                            }
                        }
                        Err(e) => {
                            return Some(build_error_frame(
                                "request.invalid_params",
                                &format!("pty.session_input.data must be base64: {e}"),
                                400,
                            ));
                        }
                    }
                }
                // Optional terminal_size piggybacks a resize hint.
                if let Some(ts) = terminal_size.as_ref() {
                    let cols = ts.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                    let rows = ts.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                    let _ = state
                        .pty_bridge
                        .resize(instance_id, &session.session_id, cols, rows)
                        .await;
                }
                None
            } else {
                // Legacy NoOp behavior: echo input back as Output so
                // observers (and existing tests) see fan-out without a
                // real process behind the session.
                let mut out = json!({ "data": data });
                if let Some(ts) = terminal_size {
                    out["terminal_size"] = ts;
                }
                session.append_frame("output", out);
                None
            }
        }

        "pty.session_resize" => {
            if !session.is_controller(client_id) {
                return Some(build_error_frame(
                    "pty.permission_denied",
                    "Only the controller may resize the PTY",
                    403,
                ));
            }
            let cols = payload.get("cols").and_then(|v| v.as_u64()).unwrap_or(80);
            let rows = payload.get("rows").and_then(|v| v.as_u64()).unwrap_or(24);

            // Forward to the bridge (best-effort) for real-process
            // resizes; either way, broadcast the Resize frame so UI
            // observers stay in sync.
            if state.pty_bridge.is_real() {
                let _ = state
                    .pty_bridge
                    .resize(instance_id, &session.session_id, cols as u16, rows as u16)
                    .await;
            }
            session.append_frame("resize", json!({ "cols": cols, "rows": rows }));
            None
        }

        "pty.request_keyframe" => Some(build_keyframe(session)),

        "pty.request_role" => {
            let requested = payload
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("observer");
            let role = if requested == "controller" {
                session.try_promote_to_controller(client_id)
            } else {
                Role::Observer
            };
            // Always broadcast membership_changed so peers see the
            // authority transition.
            session.append_frame(
                "membership_changed",
                json!({
                    "members": session
                        .members_snapshot()
                        .iter()
                        .map(|m| json!({
                            "client_id": m.client_id,
                            "role": m.role.as_str(),
                        }))
                        .collect::<Vec<_>>(),
                }),
            );
            Some(json!({
                "op": "role_assigned",
                "seq": session.seq.load(Ordering::SeqCst),
                "ts": Utc::now().to_rfc3339(),
                "payload": {
                    "client_id": client_id,
                    "role": role.as_str(),
                }
            }))
        }

        "pty.release_role" => {
            session.demote_controller(client_id);
            session.append_frame(
                "membership_changed",
                json!({
                    "members": session
                        .members_snapshot()
                        .iter()
                        .map(|m| json!({
                            "client_id": m.client_id,
                            "role": m.role.as_str(),
                        }))
                        .collect::<Vec<_>>(),
                }),
            );
            Some(json!({
                "op": "role_assigned",
                "seq": session.seq.load(Ordering::SeqCst),
                "ts": Utc::now().to_rfc3339(),
                "payload": {
                    "client_id": client_id,
                    "role": Role::Observer.as_str(),
                }
            }))
        }

        "pty.leave_session" => {
            session.drop_member(client_id);
            None
        }

        // ----- Unknown / unsupported -----
        _ => Some(build_error_frame(
            "request.unsupported_operation",
            &format!("Unsupported op '{}'", op),
            400,
        )),
    }
}

// ---------------------------------------------------------------------------
// A2A core op helpers
//
// These inline the same TaskStore logic the REST handlers use. We don't
// reuse the axum handler bodies directly because they take axum
// extractors (Path/State/InstanceExt) and emit `axum::Response`. The
// WS path needs to surface JSON frames instead. The behavior — task
// persistence, terminal-state gating on cancel, pagination via
// ListFilter — is intentionally identical.
// ---------------------------------------------------------------------------

async fn handle_message_send(payload: Value, state: &AppState, _instance_id: &str) -> Value {
    let message_obj = match payload.get("message") {
        Some(m) if m.is_object() => m,
        _ => {
            return build_error_frame(
                "request.invalid_params",
                "payload.message object required",
                400,
            );
        }
    };

    let now = Utc::now();
    let task_id = Uuid::now_v7().to_string();
    let context_id = message_obj
        .get("contextId")
        .and_then(|v| v.as_str())
        .map(String::from);
    let status_json = json!({
        "state": TaskState::Submitted.as_str(),
        "timestamp": now.to_rfc3339(),
    });
    let row = TaskRow {
        task_id: task_id.clone(),
        context_id,
        state: TaskState::Submitted,
        fail_kind: None,
        status_json,
        metadata_json: None,
        created_at: now,
        updated_at: now,
        terminal_at: None,
    };
    if let Err(e) = state.store.upsert_task(&row) {
        return build_error_frame(
            "internal.error",
            &format!("Failed to persist task: {e}"),
            500,
        );
    }
    let task = crate::handlers::task_row_to_a2a(&row);
    json!({
        "op": "task",
        "seq": null,
        "ts": Utc::now().to_rfc3339(),
        "payload": task,
    })
}

async fn handle_tasks_get(payload: Value, state: &AppState) -> Value {
    let tid = match payload.get("task_id").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return build_error_frame("request.invalid_params", "payload.task_id required", 400);
        }
    };
    match state.store.get_task(tid) {
        Ok(Some(row)) => {
            let task = crate::handlers::task_row_to_a2a(&row);
            json!({
                "op": "task",
                "seq": null,
                "ts": Utc::now().to_rfc3339(),
                "payload": task,
            })
        }
        Ok(None) => build_error_frame("task.not_found", &format!("Task '{}' not found", tid), 404),
        Err(e) => build_error_frame("internal.error", &format!("Failed to read task: {e}"), 500),
    }
}

async fn handle_tasks_list(payload: Value, state: &AppState) -> Value {
    let limit = payload
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(25)
        .clamp(1, 100);
    let state_str = payload.get("state").and_then(|v| v.as_str());
    let state_filter = match state_str {
        None => None,
        Some(s) => match crate::handlers::parse_state(s) {
            Some(ts) => Some(ts),
            None => {
                return build_error_frame(
                    "request.invalid_params",
                    &format!("Unknown task state: {s}"),
                    400,
                );
            }
        },
    };
    let filter = ListFilter {
        state: state_filter,
        limit: Some(limit),
        include_terminal: true,
    };
    match state.store.list_tasks(filter) {
        Ok(rows) => {
            let tasks: Vec<Value> = rows.iter().map(crate::handlers::task_row_to_a2a).collect();
            json!({
                "op": "task_list",
                "seq": null,
                "ts": Utc::now().to_rfc3339(),
                "payload": {
                    "tasks": tasks,
                    "next_cursor": Value::Null,
                }
            })
        }
        Err(e) => build_error_frame("internal.error", &format!("Failed to list tasks: {e}"), 500),
    }
}

async fn handle_tasks_cancel(payload: Value, state: &AppState) -> Value {
    let tid = match payload.get("task_id").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            return build_error_frame("request.invalid_params", "payload.task_id required", 400);
        }
    };
    let mut row = match state.store.get_task(&tid) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return build_error_frame("task.not_found", &format!("Task '{}' not found", tid), 404);
        }
        Err(e) => {
            return build_error_frame("internal.error", &format!("Failed to read task: {e}"), 500);
        }
    };
    if row.state.is_terminal() {
        return build_error_frame(
            "task.not_cancelable",
            &format!(
                "Task '{}' is in terminal state '{}'",
                tid,
                row.state.as_str()
            ),
            409,
        );
    }
    let now = Utc::now();
    row.state = TaskState::Canceled;
    row.updated_at = now;
    row.terminal_at = Some(now);
    row.status_json = json!({
        "state": TaskState::Canceled.as_str(),
        "timestamp": now.to_rfc3339(),
    });
    if let Err(e) = state.store.upsert_task(&row) {
        return build_error_frame(
            "internal.error",
            &format!("Failed to persist canceled task: {e}"),
            500,
        );
    }
    let task = crate::handlers::task_row_to_a2a(&row);
    json!({
        "op": "task",
        "seq": null,
        "ts": Utc::now().to_rfc3339(),
        "payload": task,
    })
}

async fn handle_tasks_subscribe(payload: Value, state: &AppState) -> Value {
    let tid = match payload.get("task_id").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return build_error_frame("request.invalid_params", "payload.task_id required", 400);
        }
    };
    match state.store.get_task(tid) {
        Ok(Some(row)) => {
            // Emit the current snapshot. Live updates ride the session
            // broadcast channel and are forwarded by the connection loop
            // through the existing `rx` subscription; per-task fan-out
            // is a follow-up patch.
            let task = crate::handlers::task_row_to_a2a(&row);
            json!({
                "op": "task",
                "seq": null,
                "ts": Utc::now().to_rfc3339(),
                "payload": task,
            })
        }
        Ok(None) => build_error_frame("task.not_found", &format!("Task '{}' not found", tid), 404),
        Err(e) => build_error_frame("internal.error", &format!("Failed to read task: {e}"), 500),
    }
}

// ---------------------------------------------------------------------------
// Frame builders
// ---------------------------------------------------------------------------

fn build_error_frame(code: &str, message: &str, status: u16) -> Value {
    json!({
        "op": "error",
        "seq": null,
        "ts": Utc::now().to_rfc3339(),
        "payload": {
            "code": code,
            "message": message,
            "status": status,
        }
    })
}

fn build_keyframe(session: &SessionState) -> Value {
    let cursor = session.seq.load(Ordering::SeqCst);
    let frames: Vec<Value> = session
        .replay
        .read()
        .iter()
        .map(|(_seq, f)| f.clone())
        .collect();
    json!({
        "op": "keyframe",
        "seq": cursor,
        "ts": Utc::now().to_rfc3339(),
        "payload": {
            "cursor": cursor,
            "frames": frames,
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::rest::AppState;
    use crate::extensions::build_default_registry;
    use crate::instance::{InstanceContext, InstanceLayer, InstanceRegistry, RuntimeKind};
    use crate::store::idempotency::IdempotencyCache;
    use crate::store::task_store::TaskStore;
    use axum::routing::get;
    use axum::Router;
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TgMessage;

    // ---- Unit tests on the registry / state ----

    fn mk_app_state() -> AppState {
        mk_app_state_with_bridge(Arc::new(crate::bindings::pty_bridge::NoOpPtyBridge))
    }

    fn mk_app_state_with_bridge(
        bridge: Arc<dyn crate::bindings::pty_bridge::PtyBridge>,
    ) -> AppState {
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        let idem = Arc::new(IdempotencyCache::new(store.clone()));
        let extensions = Arc::new(build_default_registry(
            idem.clone(),
            RuntimeKind::Vm,
            "agentic-dev".into(),
            "host.local".into(),
        ));
        // Test-only: discard the receiver. The delivery worker is not
        // exercised by pty_ws tests; the channel is required only to
        // satisfy the AppState shape.
        let (delivery, _rx) = tokio::sync::mpsc::channel(16);
        AppState {
            delivery,
            extensions,
            idem,
            instance_registry: crate::instance::InstanceRegistry::new(),
            pty_bridge: bridge,
            store,
            session_registry: Arc::new(SessionRegistry::new()),
        }
    }

    #[test]
    fn session_registry_get_or_create_returns_same_arc() {
        let reg = SessionRegistry::new();
        let a = reg.get_or_create("i-1", "s-1");
        let b = reg.get_or_create("i-1", "s-1");
        assert!(
            Arc::ptr_eq(&a, &b),
            "second lookup must return the same Arc"
        );
        assert_eq!(reg.len(), 1);

        let c = reg.get_or_create("i-1", "s-2");
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(reg.len(), 2);

        let d = reg.get("i-1", "s-1").unwrap();
        assert!(Arc::ptr_eq(&a, &d));
        reg.close("i-1", "s-1");
        assert!(reg.get("i-1", "s-1").is_none());
    }

    #[test]
    fn role_assignment_first_is_controller_rest_observer() {
        let s = SessionState::new("i".into(), "s".into());
        assert_eq!(s.assign_role("client-A"), Role::Controller);
        assert_eq!(s.assign_role("client-B"), Role::Observer);
        assert_eq!(s.assign_role("client-C"), Role::Observer);
        assert!(s.is_controller("client-A"));
        assert!(!s.is_controller("client-B"));

        // Demote then promote a different observer to controller.
        s.demote_controller("client-A");
        assert!(!s.is_controller("client-A"));
        assert_eq!(s.try_promote_to_controller("client-B"), Role::Controller);
        assert!(s.is_controller("client-B"));

        // Cannot promote a third client while controller present.
        assert_eq!(s.try_promote_to_controller("client-C"), Role::Observer);
    }

    #[test]
    fn append_frame_increments_seq_and_buffers() {
        let s = SessionState::new("i".into(), "s".into());
        let a = s.append_frame("output", json!({"data": "a"}));
        let b = s.append_frame("output", json!({"data": "b"}));
        let c = s.append_frame("resize", json!({"cols": 80, "rows": 24}));
        assert_eq!(a, 1);
        assert_eq!(b, 2);
        assert_eq!(c, 3);
        let buf = s.replay.read();
        assert_eq!(buf.len(), 3);
        assert_eq!(buf[0].1["op"], "output");
        assert_eq!(buf[2].1["op"], "resize");
        assert_eq!(buf[2].1["seq"], 3);
    }

    #[test]
    fn replay_from_seq_returns_delta() {
        let s = SessionState::new("i".into(), "s".into());
        for i in 0..5 {
            s.append_frame("output", json!({ "data": format!("{i}") }));
        }
        let delta = s.replay_from(2);
        assert_eq!(delta.len(), 3);
        assert_eq!(delta[0].0, 3);
        assert_eq!(delta[2].0, 5);
        assert_eq!(s.replay_from(0).len(), 5);
        assert_eq!(s.replay_from(5).len(), 0);
    }

    #[test]
    fn replay_buffer_evicts_oldest_beyond_capacity() {
        let mut s = SessionState::new("i".into(), "s".into());
        s.max_frames = 3;
        for i in 0..7 {
            s.append_frame("output", json!({ "data": i }));
        }
        let buf = s.replay.read();
        assert_eq!(buf.len(), 3, "ring buffer cap = 3");
        let seqs: Vec<u64> = buf.iter().map(|(s, _)| *s).collect();
        assert_eq!(seqs, vec![5, 6, 7]);
        assert_eq!(s.oldest_seq(), 5);
    }

    // ---- WS integration tests against an in-process axum server ----

    async fn spawn_server(instance_id: &str) -> (String, Arc<AppState>) {
        spawn_server_with_bridge(
            instance_id,
            Arc::new(crate::bindings::pty_bridge::NoOpPtyBridge),
        )
        .await
    }

    async fn spawn_server_with_bridge(
        instance_id: &str,
        bridge: Arc<dyn crate::bindings::pty_bridge::PtyBridge>,
    ) -> (String, Arc<AppState>) {
        let state = Arc::new(mk_app_state_with_bridge(bridge));
        let registry = InstanceRegistry::new();
        registry.insert(Arc::new(InstanceContext::new_ephemeral(
            instance_id,
            RuntimeKind::Vm,
            "agentic-dev",
            None,
            "127.0.0.1",
        )));

        // Minimal router that mounts the WS endpoint + InstanceLayer.
        let st: AppState = (*state).clone();
        let app = Router::new()
            .route(
                "/agents/{instance_id}/sessions/{session_id}/attach",
                get(ws_handler),
            )
            .layer(InstanceLayer::new(registry))
            .with_state(st);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });
        (format!("ws://{}", addr), state)
    }

    async fn connect(
        base: &str,
        instance_id: &str,
        session_id: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let url = format!(
            "{}/agents/{}/sessions/{}/attach",
            base, instance_id, session_id
        );
        let (ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();
        ws
    }

    /// Connect with an explicit `Sec-WebSocket-Protocol` header. Returns
    /// the upgrade response so tests can inspect the negotiated protocol
    /// and status code.
    async fn connect_with_subprotocol(
        base: &str,
        instance_id: &str,
        session_id: &str,
        subprotocol: &str,
    ) -> Result<
        (
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
        ),
        tokio_tungstenite::tungstenite::Error,
    > {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let url = format!(
            "{}/agents/{}/sessions/{}/attach",
            base, instance_id, session_id
        );
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("Sec-WebSocket-Protocol", subprotocol.parse().unwrap());
        tokio_tungstenite::connect_async(req).await
    }

    async fn connect_with_replay(
        base: &str,
        instance_id: &str,
        session_id: &str,
        replay_from: u64,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let url = format!(
            "{}/agents/{}/sessions/{}/attach?replay_from={}",
            base, instance_id, session_id, replay_from
        );
        let (ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();
        ws
    }

    async fn recv_json(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Value {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("recv timed out")
            .expect("stream ended")
            .expect("ws error");
        let text = match msg {
            TgMessage::Text(t) => t.to_string(),
            other => panic!("expected text frame, got {:?}", other),
        };
        serde_json::from_str(&text).unwrap()
    }

    async fn send_op(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        op: &str,
        payload: Value,
    ) {
        let frame = json!({ "op": op, "payload": payload });
        ws.send(TgMessage::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ws_handshake_emits_binding_hello() {
        let (base, _state) = spawn_server("inst-1").await;
        let mut ws = connect(&base, "inst-1", "sess-1").await;
        let hello = recv_json(&mut ws).await;
        assert_eq!(hello["op"], "binding_hello");
        assert_eq!(hello["seq"], 0);
        let payload = &hello["payload"];
        assert_eq!(payload["binding_uri"], BINDING_URI);
        let activated = payload["activated_extensions"].as_array().unwrap();
        assert!(activated
            .iter()
            .any(|v| v.as_str() == Some(PTY_EXTENSION_URI)));
        let supported = payload["supported_operations"].as_array().unwrap();
        for required in [
            "message/send",
            "message/stream",
            "tasks/get",
            "tasks/list",
            "tasks/cancel",
            "tasks/subscribe",
        ] {
            assert!(
                supported.iter().any(|v| v.as_str() == Some(required)),
                "missing core op {required}"
            );
        }
        assert!(payload["server_info"]["name"].is_string());
    }

    #[tokio::test]
    async fn ws_a2a_send_message_via_ws() {
        let (base, state) = spawn_server("inst-1").await;
        let mut ws = connect(&base, "inst-1", "sess-1").await;
        let _hello = recv_json(&mut ws).await;

        send_op(
            &mut ws,
            "message/send",
            json!({
                "message": {
                    "messageId": "00000000-0000-7000-8000-000000000001",
                    "role": "user",
                    "parts": [{"kind": "text", "text": "hi"}]
                }
            }),
        )
        .await;
        let resp = recv_json(&mut ws).await;
        assert_eq!(resp["op"], "task");
        let task = &resp["payload"];
        assert!(task["id"].is_string());
        assert_eq!(task["status"]["state"], "submitted");
        assert_eq!(state.store.count_tasks().unwrap(), 1);
    }

    #[tokio::test]
    async fn ws_a2a_get_task() {
        let (base, _state) = spawn_server("inst-1").await;
        let mut ws = connect(&base, "inst-1", "sess-1").await;
        let _hello = recv_json(&mut ws).await;

        send_op(
            &mut ws,
            "message/send",
            json!({
                "message": {
                    "messageId": "00000000-0000-7000-8000-000000000002",
                    "role": "user",
                    "parts": [{"kind": "text", "text": "x"}]
                }
            }),
        )
        .await;
        let task = recv_json(&mut ws).await;
        let tid = task["payload"]["id"].as_str().unwrap().to_string();

        send_op(&mut ws, "tasks/get", json!({ "task_id": tid })).await;
        let got = recv_json(&mut ws).await;
        assert_eq!(got["op"], "task");
        assert_eq!(got["payload"]["id"], tid);
    }

    #[tokio::test]
    async fn ws_a2a_list_tasks() {
        let (base, _state) = spawn_server("inst-1").await;
        let mut ws = connect(&base, "inst-1", "sess-1").await;
        let _hello = recv_json(&mut ws).await;

        for i in 0..3 {
            send_op(
                &mut ws,
                "message/send",
                json!({
                    "message": {
                        "messageId": format!("00000000-0000-7000-8000-{:012}", i),
                        "role": "user",
                        "parts": [{"kind": "text", "text": format!("{i}")}]
                    }
                }),
            )
            .await;
            let _ = recv_json(&mut ws).await;
        }

        send_op(&mut ws, "tasks/list", json!({ "limit": 10 })).await;
        let resp = recv_json(&mut ws).await;
        assert_eq!(resp["op"], "task_list");
        let tasks = resp["payload"]["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 3);
    }

    #[tokio::test]
    async fn ws_a2a_cancel_task() {
        let (base, _state) = spawn_server("inst-1").await;
        let mut ws = connect(&base, "inst-1", "sess-1").await;
        let _hello = recv_json(&mut ws).await;

        send_op(
            &mut ws,
            "message/send",
            json!({
                "message": {
                    "messageId": "00000000-0000-7000-8000-000000000010",
                    "role": "user",
                    "parts": [{"kind": "text", "text": "x"}]
                }
            }),
        )
        .await;
        let task = recv_json(&mut ws).await;
        let tid = task["payload"]["id"].as_str().unwrap().to_string();

        send_op(&mut ws, "tasks/cancel", json!({ "task_id": tid })).await;
        let resp = recv_json(&mut ws).await;
        assert_eq!(resp["op"], "task");
        assert_eq!(resp["payload"]["status"]["state"], "canceled");

        // Re-cancel: terminal state → error frame.
        send_op(&mut ws, "tasks/cancel", json!({ "task_id": tid })).await;
        let resp2 = recv_json(&mut ws).await;
        assert_eq!(resp2["op"], "error");
        assert_eq!(resp2["payload"]["code"], "task.not_cancelable");
    }

    #[tokio::test]
    async fn ws_a2a_subscribe_emits_current_task() {
        let (base, _state) = spawn_server("inst-1").await;
        let mut ws = connect(&base, "inst-1", "sess-1").await;
        let _hello = recv_json(&mut ws).await;

        send_op(
            &mut ws,
            "message/send",
            json!({
                "message": {
                    "messageId": "00000000-0000-7000-8000-000000000020",
                    "role": "user",
                    "parts": [{"kind": "text", "text": "x"}]
                }
            }),
        )
        .await;
        let t = recv_json(&mut ws).await;
        let tid = t["payload"]["id"].as_str().unwrap().to_string();

        send_op(&mut ws, "tasks/subscribe", json!({ "task_id": tid })).await;
        let resp = recv_json(&mut ws).await;
        assert_eq!(resp["op"], "task");
        assert_eq!(resp["payload"]["id"], tid);
    }

    #[tokio::test]
    async fn pty_join_assigns_role_and_broadcasts_membership_change() {
        let (base, state) = spawn_server("inst-1").await;

        // First joiner → controller.
        let mut c1 = connect(&base, "inst-1", "sess-pty").await;
        let _ = recv_json(&mut c1).await; // hello
        send_op(&mut c1, "pty.join_session", json!({})).await;
        // c1 receives both the role_assigned ack and (via broadcast)
        // the membership_changed frame. Order is implementation-defined.
        let mut seen_role = false;
        let mut seen_membership = false;
        for _ in 0..2 {
            let f = recv_json(&mut c1).await;
            match f["op"].as_str().unwrap_or("") {
                "role_assigned" => {
                    assert_eq!(f["payload"]["role"], "controller");
                    seen_role = true;
                }
                "membership_changed" => seen_membership = true,
                other => panic!("unexpected op {other}"),
            }
        }
        assert!(seen_role && seen_membership);

        // Second joiner → observer.
        let mut c2 = connect(&base, "inst-1", "sess-pty").await;
        let _ = recv_json(&mut c2).await; // hello
        send_op(&mut c2, "pty.join_session", json!({})).await;
        let mut role_for_c2 = None;
        for _ in 0..2 {
            let f = recv_json(&mut c2).await;
            if f["op"] == "role_assigned" {
                role_for_c2 = f["payload"]["role"].as_str().map(String::from);
            }
        }
        assert_eq!(role_for_c2.as_deref(), Some("observer"));

        // The session has 2 members in registry state.
        let s = state.session_registry.get("inst-1", "sess-pty").unwrap();
        assert_eq!(s.members_snapshot().len(), 2);
    }

    #[tokio::test]
    async fn pty_session_input_only_for_controller() {
        let (base, _state) = spawn_server("inst-1").await;

        let mut ctrl = connect(&base, "inst-1", "sess-x").await;
        let _ = recv_json(&mut ctrl).await; // hello
        send_op(&mut ctrl, "pty.join_session", json!({})).await;
        // drain join responses
        let _ = recv_json(&mut ctrl).await;
        let _ = recv_json(&mut ctrl).await;

        let mut obs = connect(&base, "inst-1", "sess-x").await;
        let _ = recv_json(&mut obs).await; // hello
        send_op(&mut obs, "pty.join_session", json!({})).await;
        // Drain join responses: role_assigned (direct ack) +
        // membership_changed (broadcast). Past broadcasts are not
        // delivered to late subscribers, so the ctrl-side join is not
        // re-emitted here.
        for _ in 0..2 {
            let _ = recv_json(&mut obs).await;
        }

        // Observer attempts input → error.
        send_op(&mut obs, "pty.session_input", json!({ "data": "ZGF0YQ==" })).await;
        let err = recv_json(&mut obs).await;
        assert_eq!(err["op"], "error");
        assert_eq!(err["payload"]["code"], "pty.permission_denied");
    }

    #[tokio::test]
    async fn pty_request_keyframe_returns_buffered_frames() {
        let (base, state) = spawn_server("inst-1").await;
        let s = state.session_registry.get_or_create("inst-1", "sess-kf");
        s.append_frame("output", json!({"data": "AA=="}));
        s.append_frame("output", json!({"data": "BB=="}));
        s.append_frame("resize", json!({"cols": 100, "rows": 30}));

        let mut ws = connect(&base, "inst-1", "sess-kf").await;
        let _ = recv_json(&mut ws).await; // hello
        send_op(&mut ws, "pty.request_keyframe", json!({})).await;
        let kf = recv_json(&mut ws).await;
        assert_eq!(kf["op"], "keyframe");
        let frames = kf["payload"]["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(kf["payload"]["cursor"], 3);
    }

    #[tokio::test]
    async fn replay_from_out_of_range_returns_error_keyframe() {
        let (base, state) = spawn_server("inst-1").await;
        // Force a small ring buffer and append enough frames that seq=1
        // is evicted.
        {
            let s = state.session_registry.get_or_create("inst-1", "sess-rep");
            // SessionState already exists with default max_frames; we can't
            // mutate it through Arc. Instead, populate many frames so the
            // oldest seq advances past 1 (we test the "since < oldest"
            // branch indirectly by replay_from=0 against a fresh session
            // which has no frames yet).
            for i in 0..5 {
                s.append_frame("output", json!({ "data": i }));
            }
            assert_eq!(s.oldest_seq(), 1);
        }

        // Connect with replay_from larger than oldest (=1) but < current
        // sequence → in-range branch: keyframe + delta frames.
        let mut in_range = connect_with_replay(&base, "inst-1", "sess-rep", 2).await;
        let _ = recv_json(&mut in_range).await; // hello
        let kf = recv_json(&mut in_range).await;
        assert_eq!(kf["op"], "keyframe");
        // Three delta frames after seq=2.
        for _ in 0..3 {
            let f = recv_json(&mut in_range).await;
            assert_eq!(f["op"], "output");
        }

        // Now exercise the out-of-range branch by manually evicting older
        // frames. We do this directly through the state's ring buffer.
        {
            let s = state.session_registry.get("inst-1", "sess-rep").unwrap();
            // Drop the first two entries to advance oldest_seq() to 3.
            {
                let mut buf = s.replay.write();
                buf.remove(0);
                buf.remove(0);
            }
            assert_eq!(s.oldest_seq(), 3);
        }

        let mut oor = connect_with_replay(&base, "inst-1", "sess-rep", 0).await;
        let _ = recv_json(&mut oor).await; // hello
        let err = recv_json(&mut oor).await;
        assert_eq!(err["op"], "error");
        assert_eq!(err["payload"]["code"], "replay.out_of_range");
        let kf = recv_json(&mut oor).await;
        assert_eq!(kf["op"], "keyframe");
    }

    // ---- Sec-WebSocket-Protocol negotiation (#240) ----

    #[tokio::test]
    async fn ws_upgrade_echoes_subprotocol_when_present() {
        let (base, _state) = spawn_server("inst-sp1").await;
        let (mut ws, resp) = connect_with_subprotocol(&base, "inst-sp1", "sess-sp", SUBPROTOCOL)
            .await
            .expect("upgrade with pty-ws.v1 must succeed");
        // The server MUST echo the negotiated subprotocol on the
        // 101 Switching Protocols response.
        let echoed = resp
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        assert_eq!(
            echoed.as_deref(),
            Some(SUBPROTOCOL),
            "server must echo Sec-WebSocket-Protocol: pty-ws.v1"
        );
        // Sanity: the binding_hello still arrives.
        let hello = recv_json(&mut ws).await;
        assert_eq!(hello["op"], "binding_hello");
    }

    #[tokio::test]
    async fn ws_upgrade_rejects_conflicting_subprotocol() {
        let (base, _state) = spawn_server("inst-sp2").await;
        let result = connect_with_subprotocol(&base, "inst-sp2", "sess-sp", "chat.v1").await;
        let err = result.expect_err("upgrade with chat.v1 must be rejected");
        // tokio-tungstenite surfaces the rejection as Http(response).
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => {
                assert_eq!(resp.status().as_u16(), 400);
                let body = resp.body().as_ref().expect("error body present");
                let body_str = std::str::from_utf8(body).expect("utf-8 body");
                let parsed: Value = serde_json::from_str(body_str).expect("body is JSON object");
                assert_eq!(parsed["error"], "unsupported_subprotocol");
                let supported = parsed["supported"].as_array().expect("supported array");
                assert!(supported.iter().any(|v| v.as_str() == Some(SUBPROTOCOL)));
            }
            other => panic!("expected Http(400) rejection, got {:?}", other),
        }
    }

    // ---- PtyBridge integration (#237) ----

    use crate::bindings::pty_bridge::test_support::{BridgeCall, MockPtyBridge};

    #[tokio::test]
    async fn bridge_start_session_called_on_first_controller_join() {
        let mock = MockPtyBridge::new();
        let (base, _state) = spawn_server_with_bridge("inst-br1", mock.clone()).await;

        let mut c1 = connect(&base, "inst-br1", "sess-br").await;
        let _ = recv_json(&mut c1).await; // hello
        send_op(&mut c1, "pty.join_session", json!({})).await;
        // drain role_assigned + membership_changed
        let _ = recv_json(&mut c1).await;
        let _ = recv_json(&mut c1).await;

        // Give the bridge reader task a moment to register the start.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let calls = mock.calls();
        let starts: Vec<_> = calls
            .iter()
            .filter(|c| matches!(c, BridgeCall::Start { .. }))
            .collect();
        assert_eq!(starts.len(), 1, "start_session called exactly once");
        if let BridgeCall::Start {
            instance_id,
            session_id,
            ..
        } = starts[0]
        {
            assert_eq!(instance_id, "inst-br1");
            assert_eq!(session_id, "sess-br");
        }

        // A second observer joining must NOT trigger another start.
        let mut c2 = connect(&base, "inst-br1", "sess-br").await;
        let _ = recv_json(&mut c2).await;
        send_op(&mut c2, "pty.join_session", json!({})).await;
        let _ = recv_json(&mut c2).await;
        let _ = recv_json(&mut c2).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let starts_after: Vec<_> = mock
            .calls()
            .into_iter()
            .filter(|c| matches!(c, BridgeCall::Start { .. }))
            .collect();
        assert_eq!(starts_after.len(), 1, "no duplicate start on observer join");
    }

    #[tokio::test]
    async fn bridge_write_input_called_on_session_input_when_real_bridge() {
        let mock = MockPtyBridge::new();
        let (base, _state) = spawn_server_with_bridge("inst-br2", mock.clone()).await;

        let mut ctrl = connect(&base, "inst-br2", "sess-in").await;
        let _ = recv_json(&mut ctrl).await; // hello
        send_op(&mut ctrl, "pty.join_session", json!({})).await;
        let _ = recv_json(&mut ctrl).await;
        let _ = recv_json(&mut ctrl).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        // base64 of "ls\n" = "bHMK"
        send_op(&mut ctrl, "pty.session_input", json!({ "data": "bHMK" })).await;

        // Allow async write_input + potential echo to happen.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let inputs: Vec<_> = mock
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                BridgeCall::Input { data, .. } => Some(data),
                _ => None,
            })
            .collect();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0], b"ls\n");

        // Regression: real-bridge mode must NOT echo input as Output.
        // We try to receive with a short timeout; nothing should arrive.
        let next = tokio::time::timeout(Duration::from_millis(150), ctrl.next()).await;
        assert!(
            next.is_err(),
            "real bridge must not echo input as Output frame; got {:?}",
            next
        );
    }

    #[tokio::test]
    async fn bridge_resize_called_on_session_resize() {
        let mock = MockPtyBridge::new();
        let (base, _state) = spawn_server_with_bridge("inst-br3", mock.clone()).await;

        let mut ctrl = connect(&base, "inst-br3", "sess-rz").await;
        let _ = recv_json(&mut ctrl).await; // hello
        send_op(&mut ctrl, "pty.join_session", json!({})).await;
        let _ = recv_json(&mut ctrl).await;
        let _ = recv_json(&mut ctrl).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        send_op(
            &mut ctrl,
            "pty.session_resize",
            json!({ "cols": 132, "rows": 50 }),
        )
        .await;

        // Resize is broadcast as a frame; drain it.
        let frame = recv_json(&mut ctrl).await;
        assert_eq!(frame["op"], "resize");
        assert_eq!(frame["payload"]["cols"], 132);
        assert_eq!(frame["payload"]["rows"], 50);

        let resizes: Vec<_> = mock
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                BridgeCall::Resize { cols, rows, .. } => Some((cols, rows)),
                _ => None,
            })
            .collect();
        assert_eq!(resizes, vec![(132u16, 50u16)]);
    }

    #[tokio::test]
    async fn bridge_close_session_on_last_member_leave() {
        let mock = MockPtyBridge::new();
        let (base, _state) = spawn_server_with_bridge("inst-br4", mock.clone()).await;

        {
            let mut ctrl = connect(&base, "inst-br4", "sess-cl").await;
            let _ = recv_json(&mut ctrl).await; // hello
            send_op(&mut ctrl, "pty.join_session", json!({})).await;
            let _ = recv_json(&mut ctrl).await;
            let _ = recv_json(&mut ctrl).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            // Drop ctrl → disconnect triggers cleanup.
        }
        // Give the cleanup task a moment.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let closes: Vec<_> = mock
            .calls()
            .into_iter()
            .filter(|c| matches!(c, BridgeCall::Close { .. }))
            .collect();
        assert_eq!(closes.len(), 1, "close_session called on last leave");
    }

    #[tokio::test]
    async fn bridge_output_chunks_feed_session_frames() {
        let mock = MockPtyBridge::new();
        let (base, _state) = spawn_server_with_bridge("inst-br5", mock.clone()).await;

        let mut ctrl = connect(&base, "inst-br5", "sess-out").await;
        let _ = recv_json(&mut ctrl).await; // hello
        send_op(&mut ctrl, "pty.join_session", json!({})).await;
        let _ = recv_json(&mut ctrl).await;
        let _ = recv_json(&mut ctrl).await;

        // Wait for the bridge reader to register and grab the sender.
        let mut sender = None;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            if let Some(s) = mock.sender_for("inst-br5", "sess-out") {
                sender = Some(s);
                break;
            }
        }
        let sender = sender.expect("bridge reader registered a sender");

        // Push bytes through the bridge channel; they must arrive as
        // base64-encoded `output` frames on the controller's socket.
        sender.send(b"hello".to_vec()).await.unwrap();
        let frame = recv_json(&mut ctrl).await;
        assert_eq!(frame["op"], "output");
        let data = frame["payload"]["data"].as_str().unwrap();
        let decoded = B64.decode(data).unwrap();
        assert_eq!(decoded, b"hello");
    }

    #[tokio::test]
    async fn noop_bridge_keeps_existing_echo_behavior() {
        // Sanity check: with the default NoOp bridge, the legacy
        // pty.session_input → output echo path still works so existing
        // suites (and v2.0 deployments without a real agent) keep their
        // observed fan-out semantics.
        let (base, _state) = spawn_server("inst-noop").await;

        let mut ctrl = connect(&base, "inst-noop", "sess-noop").await;
        let _ = recv_json(&mut ctrl).await; // hello
        send_op(&mut ctrl, "pty.join_session", json!({})).await;
        let _ = recv_json(&mut ctrl).await; // role_assigned
        let _ = recv_json(&mut ctrl).await; // membership_changed

        send_op(
            &mut ctrl,
            "pty.session_input",
            json!({ "data": "ZWNobw==" }),
        )
        .await;
        let echo = recv_json(&mut ctrl).await;
        assert_eq!(echo["op"], "output");
        assert_eq!(echo["payload"]["data"], "ZWNobw==");
    }

    #[tokio::test]
    async fn ws_upgrade_accepts_when_absent_lenient() {
        // Plain connect() does NOT set Sec-WebSocket-Protocol — exercises
        // the lenient v2.0 transition branch. We don't assert the warn
        // log; only that the upgrade succeeds and binding_hello flows.
        let (base, _state) = spawn_server("inst-sp3").await;
        let mut ws = connect(&base, "inst-sp3", "sess-sp").await;
        let hello = recv_json(&mut ws).await;
        assert_eq!(hello["op"], "binding_hello");
    }
}
