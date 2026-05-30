//! Outbound registration and event push to an `aiwg serve` instance.
//!
//! When `AIWG_SERVE_ENDPOINT` is set the management server:
//! 1. POSTs to `/api/sandboxes/register` on startup and retries until it lands.
//! 2. POSTs to `/api/v1/executors/register` per executor.v1.md (#193).
//! 3. Opens a persistent WebSocket to `ws://{endpoint}/ws/sandbox/{sandbox_id}`
//!    and pushes [`SandboxEvent`] messages as they occur.
//! 4. Opens a second WS to `ws://{endpoint}/ws/executors/{executor_id}` for
//!    executor-contract events (mission.* vocabulary) and receives inbound
//!    events such as `mission.hitl_responded`.
//! 5. Reconnects with exponential backoff (1 s → 30 s) if the WS drops.
//! 6. DELETEs the registration on clean shutdown (best-effort).
//!
//! All network I/O is non-blocking and does **not** block management server
//! startup — if `aiwg serve` is unreachable, the manager starts normally and
//! keeps retrying in the background.

// v2 TaskStore + IdempotencyCache moved to the executor crate in #243 to
// break the management↔executor workspace cycle. Re-export them here under
// the historical paths so the rest of management (main.rs, http handlers,
// tests) keeps compiling without churn. The v1→v2 migration tool stays in
// management because it depends on both MissionRecord (here) and TaskStore
// (in the executor).
pub use agentic_sandbox_executor::store::{idempotency, task_store};
pub mod migration;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Notify};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

// ────────────────────────────────────────────────────────────────────────────
// Sandbox event types (existing)
// ────────────────────────────────────────────────────────────────────────────

/// One session entry in `AgentSessions`. Mirrors the REST shape returned
/// by `GET /api/v1/agents/{id}/sessions` so consumers can use the same
/// type for both push and pull paths.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub session_name: String,
    /// "interactive" | "headless" | "background"
    pub session_type: String,
    pub command: String,
    pub created_at_secs: u64,
    pub has_screen: bool,
}

/// Events pushed from management server → aiwg serve dashboard.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SandboxEvent {
    /// An agent's gRPC stream connected and it sent its registration.
    AgentConnected {
        agent_id: String,
        hostname: String,
        ip_address: String,
        loadout: String,
        /// Stable per-agent UUIDv7 for persistent identity tracking (#917).
        /// Always present — absent only when receiving events from an old sandbox build.
        agent_instance_id: Option<String>,
    },
    /// An agent's gRPC stream disconnected or timed out.
    AgentDisconnected {
        agent_id: String,
        reason: Option<String>,
    },
    /// An agent transitioned to the `Ready` status (after cloud-init finished).
    AgentReady { agent_id: String },
    /// Cloud-init / loadout provisioning progress update.
    AgentProvisioning {
        agent_id: String,
        step: String,
        /// Raw JSON from `setup_progress_json`.
        progress_json: String,
    },
    /// A PTY or exec session was started on an agent.
    SessionStart {
        agent_id: String,
        session_id: String,
        command: String,
    },
    /// A session ended.
    SessionEnd {
        agent_id: String,
        session_id: String,
        exit_code: Option<i32>,
    },
    /// Authoritative snapshot of an agent's current session inventory (#192).
    /// Emitted after AgentConnected (initial sync, may be empty), and after
    /// every SessionStart / SessionEnd on the affected agent. AIWG should
    /// replace its per-agent cache with this list — it's authoritative,
    /// not a delta.
    AgentSessions {
        agent_id: String,
        sessions: Vec<SessionSummary>,
    },
    /// An agent is waiting for human input (HITL pause detected).
    HitlInputRequired {
        agent_id: String,
        session_id: String,
        hitl_id: String,
        prompt: String,
        context: String,
    },
}

// ────────────────────────────────────────────────────────────────────────────
// Executor event types (executor.v1.md §Event vocabulary, #193)
// ────────────────────────────────────────────────────────────────────────────

/// Executor-contract event envelope.  All mission.* events emitted by the
/// sandbox use this shape over the executor WS stream.
/// Schema ref: executor.aiwg.io/v1#/$defs/event_envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorEvent {
    pub event: String,
    pub executor_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    pub ts: String,
    pub data: serde_json::Value,
}

impl ExecutorEvent {
    fn now_ts() -> String {
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    /// Emit `mission.assigned` immediately on dispatch acceptance.
    pub fn mission_assigned(executor_id: &str, mission_id: &str, estimated_start: &str) -> Self {
        Self {
            event: "mission.assigned".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "state": "assigned",
                "estimated_start": estimated_start,
            }),
        }
    }

    /// Emit `mission.started` when the agent session begins inside a VM.
    pub fn mission_started(
        executor_id: &str,
        mission_id: &str,
        pty_session_id: Option<&str>,
    ) -> Self {
        let mut data = serde_json::json!({
            "state": "running",
            "agent_runtime": "claude-code",
        });
        if let Some(sid) = pty_session_id {
            data["pty_session_id"] = serde_json::Value::String(sid.into());
        }
        Self {
            event: "mission.started".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data,
        }
    }

    /// Emit `mission.progress` from an `AgentSessions` update tied to a
    /// running mission.
    pub fn mission_progress(
        executor_id: &str,
        mission_id: &str,
        summary: &str,
        session_count: usize,
    ) -> Self {
        Self {
            event: "mission.progress".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "phase": "execution",
                "summary": summary,
                "iteration": session_count,
            }),
        }
    }

    /// Emit `mission.hitl_required`.
    pub fn mission_hitl_required(
        executor_id: &str,
        mission_id: &str,
        hitl_id: &str,
        prompt: &str,
        context: &str,
    ) -> Self {
        Self {
            event: "mission.hitl_required".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "hitl_id": hitl_id,
                "prompt": prompt,
                "context": context,
            }),
        }
    }

    /// Emit `mission.suspended` before graceful shutdown.
    pub fn mission_suspended(
        executor_id: &str,
        mission_id: &str,
        checkpoint_id: &str,
        reason: &str,
    ) -> Self {
        Self {
            event: "mission.suspended".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "state": "suspended",
                "checkpoint_id": checkpoint_id,
                "reason": reason,
            }),
        }
    }

    /// Emit `mission.reconnected` after a restart when a mission is found
    /// to still be active.
    pub fn mission_reconnected(executor_id: &str, mission_id: &str, checkpoint_id: &str) -> Self {
        Self {
            event: "mission.reconnected".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "checkpoint_id": checkpoint_id,
            }),
        }
    }

    /// Emit `mission.resumed` after reconnect.
    pub fn mission_resumed(executor_id: &str, mission_id: &str) -> Self {
        Self {
            event: "mission.resumed".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "state": "running",
                "resumed_from": "suspended",
            }),
        }
    }

    /// Emit `mission.completed` on clean exit (exit_code 0).
    pub fn mission_completed(
        executor_id: &str,
        mission_id: &str,
        exit_code: i32,
        summary: &str,
    ) -> Self {
        Self {
            event: "mission.completed".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "state": "done",
                "exit_code": exit_code,
                "summary": summary,
            }),
        }
    }

    /// Emit `mission.failed` on non-zero exit or internal error.
    pub fn mission_failed(
        executor_id: &str,
        mission_id: &str,
        reason: &str,
        error: &str,
        exit_code: Option<i32>,
    ) -> Self {
        let mut data = serde_json::json!({
            "state": "failed",
            "reason": reason,
            "error": error,
        });
        if let Some(code) = exit_code {
            data["exit_code"] = serde_json::Value::Number(code.into());
        }
        Self {
            event: "mission.failed".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data,
        }
    }

    /// Emit `mission.failed` when a mission is quarantined by poison-pill detection.
    pub fn mission_quarantined(
        executor_id: &str,
        mission_id: &str,
        failure_count: u32,
        threshold: u32,
        reason: &str,
    ) -> Self {
        Self {
            event: "mission.failed".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "state": "failed_preserved",
                "reason": "mission_quarantined",
                "error": reason,
                "failure_count": failure_count,
                "threshold": threshold,
            }),
        }
    }

    /// Emit `mission.aborted` on operator-initiated abort.
    pub fn mission_aborted(executor_id: &str, mission_id: &str, reason: &str) -> Self {
        Self {
            event: "mission.aborted".into(),
            executor_id: executor_id.into(),
            mission_id: Some(mission_id.into()),
            ts: Self::now_ts(),
            data: serde_json::json!({
                "state": "aborted",
                "aborted_by": "operator",
                "reason": reason,
            }),
        }
    }

    /// Emit `executor.resync` on every WS reconnect (Resumable conformance).
    /// Lists all mission IDs the executor currently owns.
    pub fn executor_resync(executor_id: &str, owned_mission_ids: Vec<String>) -> Self {
        Self {
            event: "executor.resync".into(),
            executor_id: executor_id.into(),
            mission_id: None,
            ts: Self::now_ts(),
            data: serde_json::json!({
                "owned_mission_ids": owned_mission_ids,
                "protocol_version": "1.0.0",
            }),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Inbound events from aiwg serve → executor (received on executor WS)
// ────────────────────────────────────────────────────────────────────────────

/// Inbound events the executor receives from aiwg serve over the executor WS.
/// The primary inbound event is `mission.hitl_responded`.
#[derive(Debug, Clone, Deserialize)]
pub struct InboundExecutorEvent {
    pub event: String,
    pub executor_id: Option<String>,
    pub mission_id: Option<String>,
    pub ts: Option<String>,
    pub data: Option<serde_json::Value>,
}

// ────────────────────────────────────────────────────────────────────────────
// Mission store — tracks active missions for resync + event routing
// ────────────────────────────────────────────────────────────────────────────

/// Lifecycle state of a mission owned by this executor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionState {
    Assigned,
    Running,
    HitlRequired,
    Suspended,
    Quarantined,
    Completed,
    Failed,
    Aborted,
}

impl MissionState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            MissionState::Quarantined
                | MissionState::Completed
                | MissionState::Failed
                | MissionState::Aborted
        )
    }
}

/// Bounds repeated mission reconnect/resume loops before they can run forever.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionCrashLoopConfig {
    pub max_consecutive_failures: u32,
    pub window_minutes: i64,
}

impl Default for MissionCrashLoopConfig {
    fn default() -> Self {
        Self {
            max_consecutive_failures: 3,
            window_minutes: 10,
        }
    }
}

/// Operator-visible state for per-mission poison-pill detection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MissionCrashLoopStatus {
    pub consecutive_failures: u32,
    pub window_started_at: Option<String>,
    pub last_failure_reason: Option<String>,
    pub quarantined_at: Option<String>,
}

/// All information the executor tracks for a single mission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionRecord {
    pub mission_id: String,
    pub objective: String,
    pub completion: String,
    pub state: MissionState,
    /// PTY session ID associated with this mission (populated once started).
    pub pty_session_id: Option<String>,
    /// Checkpoint ID for suspended missions (Resumable conformance).
    pub checkpoint_id: Option<String>,
    /// Per-mission reconnect/failure loop accounting. Missing in old
    /// persisted stores, so it defaults during deserialization.
    #[serde(default)]
    pub crash_loop: MissionCrashLoopStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissionResumeDecision {
    Resume,
    Quarantine { failure_count: u32, reason: String },
}

/// Thread-safe in-memory store for active missions.
/// Shared between the HTTP dispatch handler and the background aiwg task.
///
/// Persistence (#193 closed gap 2): when a `persist_path` is set, every
/// mutation is followed by an atomic `tmp + rename` write. Reads stay
/// purely in-memory. After a mgmt-server restart, `load_or_default()`
/// reloads the file so AIWG sees its missions reconciled rather than
/// lost — this is what enables the `executor.resync` payload to be
/// non-empty on reconnect.
#[derive(Clone, Default)]
pub struct MissionStore {
    inner: Arc<RwLock<HashMap<String, MissionRecord>>>,
    persist_path: Arc<RwLock<Option<std::path::PathBuf>>>,
}

impl MissionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a MissionStore that loads from `path` if it exists, then
    /// persists every mutation back to it. A read or parse failure logs
    /// a warning and starts with an empty store — the persistence file
    /// is operational state, not authoritative truth.
    pub fn load_or_default(path: std::path::PathBuf) -> Self {
        let map: HashMap<String, MissionRecord> = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(raw) => match serde_json::from_str(&raw) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!(
                            "mission store at {} failed to parse ({}); starting empty",
                            path.display(),
                            e
                        );
                        HashMap::new()
                    }
                },
                Err(e) => {
                    warn!(
                        "mission store at {} unreadable ({}); starting empty",
                        path.display(),
                        e
                    );
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self {
            inner: Arc::new(RwLock::new(map)),
            persist_path: Arc::new(RwLock::new(Some(path))),
        }
    }

    /// Atomic write of the current map to disk. No-op when persistence
    /// is disabled. Errors are logged, never propagated — losing one
    /// persistence write does not invalidate the in-memory state.
    fn persist(&self) {
        let path = self.persist_path.read().unwrap().clone();
        let Some(path) = path else { return };
        let snapshot = self.inner.read().unwrap().clone();
        let json = match serde_json::to_string_pretty(&snapshot) {
            Ok(s) => s,
            Err(e) => {
                warn!("mission store serialize failed: {}", e);
                return;
            }
        };
        let tmp = path.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, json) {
            warn!("mission store tmp write failed ({}): {}", tmp.display(), e);
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            warn!(
                "mission store rename failed ({} → {}): {}",
                tmp.display(),
                path.display(),
                e
            );
        }
    }

    pub fn insert(&self, record: MissionRecord) {
        self.inner
            .write()
            .unwrap()
            .insert(record.mission_id.clone(), record);
        self.persist();
    }

    pub fn get(&self, mission_id: &str) -> Option<MissionRecord> {
        self.inner.read().unwrap().get(mission_id).cloned()
    }

    pub fn update_state(&self, mission_id: &str, state: MissionState) {
        let changed = {
            let mut guard = self.inner.write().unwrap();
            if let Some(rec) = guard.get_mut(mission_id) {
                rec.state = state;
                rec.updated_at = ExecutorEvent::now_ts();
                true
            } else {
                false
            }
        };
        if changed {
            self.persist();
        }
    }

    /// Record a reconnect/resume attempt for poison-pill detection.
    ///
    /// The counter is scoped to a rolling window. When the threshold is reached,
    /// the mission is moved to Quarantined so future resyncs exclude it until an
    /// operator makes an explicit unblock/retry decision.
    pub fn record_resume_attempt(
        &self,
        mission_id: &str,
        config: &MissionCrashLoopConfig,
    ) -> Option<MissionResumeDecision> {
        let reason = "repeated reconnect/resume failure; mission preserved for operator review";
        let now = Utc::now();
        let now_ts = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let changed = {
            let mut guard = self.inner.write().unwrap();
            let Some(rec) = guard.get_mut(mission_id) else {
                return None;
            };
            if rec.state.is_terminal() {
                return Some(MissionResumeDecision::Quarantine {
                    failure_count: rec.crash_loop.consecutive_failures,
                    reason: rec
                        .crash_loop
                        .last_failure_reason
                        .clone()
                        .unwrap_or_else(|| reason.to_string()),
                });
            }
            if rec.state == MissionState::Running {
                return Some(MissionResumeDecision::Resume);
            }

            let window_expired = rec
                .crash_loop
                .window_started_at
                .as_deref()
                .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
                .map(|started| {
                    now.signed_duration_since(started.with_timezone(&Utc))
                        > chrono::Duration::minutes(config.window_minutes)
                })
                .unwrap_or(true);

            if window_expired {
                rec.crash_loop.consecutive_failures = 0;
                rec.crash_loop.window_started_at = Some(now_ts.clone());
                rec.crash_loop.quarantined_at = None;
            }

            rec.crash_loop.consecutive_failures =
                rec.crash_loop.consecutive_failures.saturating_add(1);
            rec.crash_loop.last_failure_reason = Some(reason.to_string());
            rec.updated_at = now_ts.clone();

            if rec.crash_loop.consecutive_failures >= config.max_consecutive_failures {
                rec.state = MissionState::Quarantined;
                rec.crash_loop.quarantined_at = Some(now_ts);
                MissionResumeDecision::Quarantine {
                    failure_count: rec.crash_loop.consecutive_failures,
                    reason: reason.to_string(),
                }
            } else {
                MissionResumeDecision::Resume
            }
        };
        self.persist();
        Some(changed)
    }

    pub fn set_pty_session(&self, mission_id: &str, session_id: &str) {
        let changed = {
            let mut guard = self.inner.write().unwrap();
            if let Some(rec) = guard.get_mut(mission_id) {
                rec.pty_session_id = Some(session_id.into());
                rec.updated_at = ExecutorEvent::now_ts();
                true
            } else {
                false
            }
        };
        if changed {
            self.persist();
        }
    }

    pub fn set_checkpoint(&self, mission_id: &str, checkpoint_id: &str) {
        let changed = {
            let mut guard = self.inner.write().unwrap();
            if let Some(rec) = guard.get_mut(mission_id) {
                rec.checkpoint_id = Some(checkpoint_id.into());
                rec.updated_at = ExecutorEvent::now_ts();
                true
            } else {
                false
            }
        };
        if changed {
            self.persist();
        }
    }

    /// List IDs of all non-terminal missions (used for executor.resync).
    pub fn active_mission_ids(&self) -> Vec<String> {
        self.inner
            .read()
            .unwrap()
            .values()
            .filter(|r| !r.state.is_terminal())
            .map(|r| r.mission_id.clone())
            .collect()
    }

    pub fn all(&self) -> Vec<MissionRecord> {
        self.inner.read().unwrap().values().cloned().collect()
    }

    /// Reverse lookup: given a PTY session_id, find the owning mission_id.
    /// Used by the dispatcher and HITL hook to translate session-scoped
    /// events into mission-scoped events without callers needing to track
    /// the mapping themselves.
    pub fn find_by_session(&self, session_id: &str) -> Option<String> {
        self.inner
            .read()
            .unwrap()
            .values()
            .find(|r| r.pty_session_id.as_deref() == Some(session_id))
            .map(|r| r.mission_id.clone())
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Config
// ────────────────────────────────────────────────────────────────────────────

/// Configuration for the aiwg serve integration, read from env vars.
#[derive(Debug, Clone)]
pub struct AiwgServeConfig {
    /// HTTP base URL for `aiwg serve`, e.g. `http://localhost:7337`.
    pub endpoint: String,
    /// Display name for this sandbox in the dashboard.
    pub sandbox_name: String,
    /// Stable instance identity (UUID persisted across restarts).
    pub instance_id: String,
    /// This sandbox's gRPC endpoint (advertised to aiwg serve).
    pub grpc_endpoint: String,
    /// This sandbox's WebSocket endpoint.
    pub ws_endpoint: String,
    /// This sandbox's HTTP dashboard endpoint.
    pub http_endpoint: String,
}

impl AiwgServeConfig {
    /// Load from environment.  Returns `None` if `AIWG_SERVE_ENDPOINT` is not
    /// set (integration disabled).
    pub fn from_env(listen_addr: &str, instance_id: String) -> Option<Self> {
        let endpoint = std::env::var("AIWG_SERVE_ENDPOINT").ok()?;
        let host = listen_addr.split(':').next().unwrap_or("localhost");
        let base_port: u16 = listen_addr
            .split(':')
            .nth(1)
            .and_then(|p| p.parse().ok())
            .unwrap_or(8120);
        Some(Self {
            endpoint,
            sandbox_name: std::env::var("AIWG_SERVE_NAME")
                .unwrap_or_else(|_| "agentic-sandbox".to_string()),
            instance_id,
            grpc_endpoint: format!("{}:{}", host, base_port),
            ws_endpoint: format!("ws://{}:{}", host, base_port + 1),
            http_endpoint: format!("http://{}:{}", host, base_port + 2),
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Public handle
// ────────────────────────────────────────────────────────────────────────────

/// Observable connection state — updated by the background task.
#[derive(Debug, Clone, Serialize)]
pub struct AiwgConnState {
    pub configured: bool,
    pub connected: bool,
    pub endpoint: String,
    pub sandbox_id: Option<String>,
    /// Executor registration result (#193). `None` until the first
    /// registration attempt completes; `Some(Ok(executor_id))` if the
    /// executor-contract route is available on the AIWG side, or
    /// `Some(Err(reason))` if the route returned 404 / unavailable.
    /// Sandbox registration is independent and continues regardless.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor_register_error: Option<String>,
    /// Bearer token issued by AIWG at executor registration (#193 pass 3).
    /// Used to authenticate inbound `POST /api/v1/sessions/:id/dispatch`
    /// requests. Skipped from JSON output — never expose tokens via /aiwg/status.
    #[serde(skip)]
    pub executor_token: Option<String>,
}

/// Cheap handle that any component can use to emit [`SandboxEvent`]s or
/// [`ExecutorEvent`]s.
///
/// Cloning the handle is O(1) — it's just an `Arc` under the hood.
/// `emit()` and `emit_executor()` are fire-and-forget; they will not block
/// even if the aiwg serve connection is temporarily down (events are buffered
/// in the channel up to 256 messages, then dropped).
#[derive(Clone)]
pub struct AiwgServeHandle {
    tx: mpsc::Sender<SandboxEvent>,
    /// Executor-contract events use a separate channel so sandbox and executor
    /// streams remain independent.
    executor_tx: mpsc::Sender<ExecutorEvent>,
    state: Arc<RwLock<AiwgConnState>>,
    reconnect: Arc<Notify>,
    /// Sender end of the channel for inbound events received from aiwg serve
    /// on the executor WS (e.g. mission.hitl_responded). HTTP handlers and
    /// other components subscribe via `subscribe_inbound`.
    inbound_tx: tokio::sync::broadcast::Sender<InboundExecutorEvent>,
}

impl AiwgServeHandle {
    /// Emit a [`SandboxEvent`] (non-blocking, best-effort).
    pub fn emit(&self, event: SandboxEvent) {
        if let Err(e) = self.tx.try_send(event) {
            debug!("aiwg serve event dropped ({})", e);
        }
    }

    /// Emit an executor-contract [`ExecutorEvent`] (non-blocking, best-effort).
    pub fn emit_executor(&self, event: ExecutorEvent) {
        if let Err(e) = self.executor_tx.try_send(event) {
            debug!("executor event dropped ({})", e);
        }
    }

    /// Current connection state snapshot.
    pub fn conn_state(&self) -> AiwgConnState {
        self.state.read().unwrap().clone()
    }

    /// Signal the background task to reconnect immediately (skips backoff sleep).
    pub fn trigger_reconnect(&self) {
        self.reconnect.notify_one();
    }

    /// Subscribe to inbound events from aiwg serve (e.g. `mission.hitl_responded`).
    pub fn subscribe_inbound(&self) -> tokio::sync::broadcast::Receiver<InboundExecutorEvent> {
        self.inbound_tx.subscribe()
    }

    /// Convenience: return the executor_id if registered.
    pub fn executor_id(&self) -> Option<String> {
        self.state.read().unwrap().executor_id.clone()
    }

    /// Constant-time-style bearer-token check for the dispatch route (#193 pass 3).
    /// Returns `true` if the executor is registered AND the supplied token matches
    /// the bearer issued by AIWG at registration. Returns `false` if the executor
    /// is unregistered (no token to compare) or the token differs.
    pub fn verify_bearer(&self, token: &str) -> bool {
        let stored = self.state.read().unwrap().executor_token.clone();
        match stored {
            Some(s) if !s.is_empty() && s.as_bytes().len() == token.as_bytes().len() => {
                // ct_eq via xor accumulator — avoids leaking token length differences
                // beyond the length-prefix check above (stored length is fixed by AIWG).
                let mut diff: u8 = 0;
                for (a, b) in s.as_bytes().iter().zip(token.as_bytes()) {
                    diff |= a ^ b;
                }
                diff == 0
            }
            _ => false,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Spawn
// ────────────────────────────────────────────────────────────────────────────

/// Spawn the aiwg serve background task and return an [`AiwgServeHandle`].
///
/// The task registers, then enters a push/reconnect loop.  It runs
/// independently of management server operation.
pub fn spawn(
    config: AiwgServeConfig,
    version: &'static str,
    missions: MissionStore,
) -> AiwgServeHandle {
    let (tx, rx) = mpsc::channel::<SandboxEvent>(256);
    let (executor_tx, executor_rx) = mpsc::channel::<ExecutorEvent>(256);
    let (inbound_tx, _) = tokio::sync::broadcast::channel::<InboundExecutorEvent>(64);
    let state = Arc::new(RwLock::new(AiwgConnState {
        configured: true,
        connected: false,
        endpoint: config.endpoint.clone(),
        sandbox_id: None,
        executor_id: None,
        executor_register_error: None,
        executor_token: None,
    }));
    let reconnect = Arc::new(Notify::new());
    // Wrap executor_rx in Arc<Mutex<>> (std) so the forwarder can share it
    // across reconnect cycles. Each cycle's forwarder holds the lock only
    // while calling try_recv(), then releases — a non-async std lock is
    // appropriate here since we never await while holding it.
    let executor_rx_shared = Arc::new(Mutex::new(executor_rx));
    tokio::spawn(background_task(
        config,
        version,
        rx,
        executor_rx_shared,
        inbound_tx.clone(),
        missions,
        state.clone(),
        reconnect.clone(),
    ));
    AiwgServeHandle {
        tx,
        executor_tx,
        state,
        reconnect,
        inbound_tx,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Background task
// ────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn background_task(
    config: AiwgServeConfig,
    version: &'static str,
    mut rx: mpsc::Receiver<SandboxEvent>,
    executor_rx_shared: Arc<Mutex<mpsc::Receiver<ExecutorEvent>>>,
    inbound_tx: tokio::sync::broadcast::Sender<InboundExecutorEvent>,
    missions: MissionStore,
    state: Arc<RwLock<AiwgConnState>>,
    reconnect: Arc<Notify>,
) {
    // Single shared client — creating a new reqwest::Client per request spawns
    // Hyper background tasks (eventfd wakers) and causes FD leaks under retry loops.
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client build failed");

    let mut backoff = Duration::from_secs(1);

    loop {
        // ── Register ─────────────────────────────────────────────────────────
        let (sandbox_id, token) = register_loop(&config, version, &http_client).await;
        backoff = Duration::from_secs(1);
        {
            let mut s = state.write().unwrap();
            s.sandbox_id = Some(sandbox_id.clone());
        }

        // ── Register as executor (#193, AIWG executor.v1.md) ─────────────────
        // Best-effort: this route is added by AIWG #1179. Until that lands
        // we'll get 404 / connection-refused — log a warning and proceed.
        // Reuses the sandbox instance_id as the executor_id so dashboard
        // correlation works (one identity, two registrations).
        let executor_token = match register_executor(&config, version, &http_client).await {
            Ok((executor_id, exec_token)) => {
                info!(
                    executor_id = %executor_id,
                    "Registered as executor with aiwg serve"
                );
                let mut s = state.write().unwrap();
                s.executor_id = Some(executor_id);
                s.executor_register_error = None;
                s.executor_token = Some(exec_token.clone());
                Some(exec_token)
            }
            Err(e) => {
                let msg = e.to_string();
                warn!("Executor registration unavailable ({msg}); sandbox registration will continue. This is expected until AIWG #1179 lands.");
                let mut s = state.write().unwrap();
                s.executor_id = None;
                s.executor_register_error = Some(msg);
                s.executor_token = None;
                None
            }
        };

        // ── Push sandbox events ──────────────────────────────────────────────
        let ws_url = build_sandbox_ws_url(&config.endpoint, &sandbox_id, &token);

        // Spawn the executor WS loop as a sibling task if we have an executor token.
        // It uses a separate channel so sandbox WS failures don't stall executor events.
        let executor_ws_handle = if let (Some(exec_tok), Some(executor_id)) = (
            executor_token.clone(),
            state.read().unwrap().executor_id.clone(),
        ) {
            let exec_ws_url = build_executor_ws_url(&config.endpoint, &executor_id, &exec_tok);
            let (fwd_tx, fwd_rx) = mpsc::channel::<ExecutorEvent>(256);
            // Drain from the shared executor_rx into our local forwarding channel.
            // We can't share mpsc::Receiver across tasks, so we park the actual
            // executor_rx into a separate forwarder below.
            let inbound = inbound_tx.clone();
            let missions_clone = missions.clone();
            let executor_id_clone = executor_id.clone();
            let handle = tokio::spawn(async move {
                executor_ws_loop(
                    &exec_ws_url,
                    fwd_rx,
                    inbound,
                    missions_clone,
                    executor_id_clone,
                )
                .await
            });
            Some((fwd_tx, handle))
        } else {
            None
        };

        // Forward executor events from the shared receiver into the executor WS
        // forwarding channel for this connection cycle.  The forwarder task holds
        // the Mutex guard only while dequeueing a single event, so the lock
        // contention is minimal.  When fwd_tx closes (executor WS task exits or
        // the cycle ends) the forwarder stops naturally.
        if let Some((ref fwd_tx, _)) = executor_ws_handle {
            let fwd = fwd_tx.clone();
            let shared = executor_rx_shared.clone();
            tokio::spawn(async move {
                loop {
                    // Acquire lock, try to receive one event, then release immediately.
                    let event = {
                        let mut guard = match shared.lock() {
                            Ok(g) => g,
                            Err(_) => break, // mutex poisoned
                        };
                        // Use try_recv to avoid holding the lock while blocked.
                        guard.try_recv().ok()
                    };
                    match event {
                        Some(ev) => {
                            if fwd.send(ev).await.is_err() {
                                break; // executor WS fwd channel closed
                            }
                        }
                        None => {
                            // No event ready — yield briefly to avoid spin-loop.
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    }
                }
            });
        }

        match push_loop(&ws_url, &mut rx, &state, &reconnect).await {
            Ok(()) => {
                info!("aiwg serve event channel closed");
                let _ = deregister_sandbox(&config, &sandbox_id, &http_client).await;
                let executor_id_snapshot = state.read().unwrap().executor_id.clone();
                if let (Some(executor_id), Some(exec_tok)) = (executor_id_snapshot, executor_token)
                {
                    let _ =
                        deregister_executor(&config, &executor_id, &exec_tok, &http_client).await;
                }
                state.write().unwrap().connected = false;
                return;
            }
            Err(e) => {
                state.write().unwrap().connected = false;
                warn!(
                    "aiwg serve WS lost ({}); re-registering in {:?}",
                    e, backoff
                );
                let _ = deregister_sandbox(&config, &sandbox_id, &http_client).await;
                // Sleep with backoff, but wake immediately if reconnect is triggered.
                tokio::select! {
                    _ = sleep(backoff) => {}
                    _ = reconnect.notified() => {
                        info!("aiwg serve reconnect triggered manually");
                    }
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

/// Retry registration indefinitely (with 5 s pause between attempts).
/// Returns `(sandbox_id, auth_token)`.
async fn register_loop(
    config: &AiwgServeConfig,
    version: &str,
    client: &reqwest::Client,
) -> (String, String) {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match register_sandbox(config, version, client).await {
            Ok((id, token)) => {
                info!(
                    attempt,
                    sandbox_id = %id,
                    "Registered with aiwg serve at {}",
                    config.endpoint
                );
                return (id, token);
            }
            Err(e) => {
                if attempt == 1 {
                    // On first failure, log at INFO so the operator knows the
                    // integration is configured but aiwg serve isn't up yet.
                    info!(
                        "aiwg serve not reachable at {} ({}); will retry every 5 s",
                        config.endpoint, e
                    );
                } else {
                    debug!("aiwg serve registration attempt {} failed: {}", attempt, e);
                }
                sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Executor WebSocket loop (#193 pass 2/4)
// ────────────────────────────────────────────────────────────────────────────

/// Open the executor WS stream and:
/// - send executor.resync immediately on connect
/// - drain outbound [`ExecutorEvent`]s from `rx`
/// - receive inbound events (mission.hitl_responded etc.) and broadcast
///   through `inbound_tx`
///
/// Returns when the WS drops or `rx` closes.
async fn executor_ws_loop(
    ws_url: &str,
    mut rx: mpsc::Receiver<ExecutorEvent>,
    inbound_tx: tokio::sync::broadcast::Sender<InboundExecutorEvent>,
    missions: MissionStore,
    executor_id: String,
) {
    let (ws, _) = match connect_async(ws_url).await {
        Ok(pair) => pair,
        Err(e) => {
            warn!("Executor WS connect failed: {}", e);
            return;
        }
    };
    info!("Executor WS connected: {}", redact_ws_url(ws_url));

    let (mut sink, mut stream) = ws.split();

    // Emit executor.resync immediately on connect (Resumable conformance).
    let owned = missions.active_mission_ids();
    let resync = ExecutorEvent::executor_resync(&executor_id, owned.clone());
    if let Ok(json) = serde_json::to_string(&resync) {
        let _ = sink.send(Message::Text(json)).await;
        debug!(count = owned.len(), "executor.resync sent");
    }
    // Per-mission reconnected → resumed pair (#193 closed gap 3).
    // Bound repeated reconnect/resume attempts per mission so a poison-pill
    // mission is preserved once instead of being replayed forever.
    let crash_loop_config = MissionCrashLoopConfig::default();
    for mission_id in &owned {
        match missions.record_resume_attempt(mission_id, &crash_loop_config) {
            Some(MissionResumeDecision::Resume) => {}
            Some(MissionResumeDecision::Quarantine {
                failure_count,
                reason,
            }) => {
                warn!(
                    mission_id = %mission_id,
                    failure_count,
                    threshold = crash_loop_config.max_consecutive_failures,
                    "Mission quarantined during executor resync"
                );
                let quarantined = ExecutorEvent::mission_quarantined(
                    &executor_id,
                    mission_id,
                    failure_count,
                    crash_loop_config.max_consecutive_failures,
                    &reason,
                );
                if let Ok(json) = serde_json::to_string(&quarantined) {
                    let _ = sink.send(Message::Text(json)).await;
                }
                continue;
            }
            None => continue,
        }

        let checkpoint_id = missions
            .get(mission_id)
            .and_then(|r| r.checkpoint_id)
            .unwrap_or_default();
        let reconnected =
            ExecutorEvent::mission_reconnected(&executor_id, mission_id, &checkpoint_id);
        if let Ok(json) = serde_json::to_string(&reconnected) {
            let _ = sink.send(Message::Text(json)).await;
        }
        let resumed = ExecutorEvent::mission_resumed(&executor_id, mission_id);
        if let Ok(json) = serde_json::to_string(&resumed) {
            let _ = sink.send(Message::Text(json)).await;
        }
        // Bump in-memory state back to Running — the loaded record may
        // be in any non-terminal state (Suspended after a planned
        // shutdown, HitlRequired after a crash mid-prompt). On reconnect
        // we declare the mission running again; the next real event
        // (hitl_required, completed, etc.) will refine.
        missions.update_state(mission_id, MissionState::Running);
    }

    let mut ping_ticker = tokio::time::interval(PING_INTERVAL);
    ping_ticker.tick().await;
    let mut waiting_for_pong = false;

    loop {
        tokio::select! {
            // ── Outbound executor events ──────────────────────────────────
            event = rx.recv() => {
                match event {
                    Some(ev) => {
                        match serde_json::to_string(&ev) {
                            Ok(json) => { let _ = sink.send(Message::Text(json)).await; }
                            Err(e) => { warn!("executor event serialize error: {}", e); }
                        }
                    }
                    None => {
                        // channel closed — clean shutdown
                        let _ = sink.close().await;
                        return;
                    }
                }
            }

            // ── Inbound frames from aiwg serve ────────────────────────────
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<InboundExecutorEvent>(&text) {
                            Ok(ev) => {
                                debug!(event = %ev.event, "inbound executor event");
                                let _ = inbound_tx.send(ev);
                            }
                            Err(e) => {
                                debug!("executor WS: unparseable inbound frame: {}: {}", e, text);
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        waiting_for_pong = false;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("executor WS closed");
                        return;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        warn!("executor WS error: {}", e);
                        return;
                    }
                }
            }

            // ── Keepalive ─────────────────────────────────────────────────
            _ = ping_ticker.tick() => {
                if waiting_for_pong {
                    warn!("executor WS pong timeout");
                    return;
                }
                let _ = sink.send(Message::Ping(vec![])).await;
                waiting_for_pong = true;
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Network helpers
// ────────────────────────────────────────────────────────────────────────────

/// Build the authenticated sandbox WebSocket URL.
fn build_sandbox_ws_url(endpoint: &str, sandbox_id: &str, token: &str) -> String {
    let ws_base = endpoint
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    format!("{}/ws/sandbox/{}?token={}", ws_base, sandbox_id, token)
}

/// Build the authenticated executor WebSocket URL.
/// Per executor.v1.md: `ws://<aiwg-serve>/ws/executors/:executor_id?token=<token>`
fn build_executor_ws_url(endpoint: &str, executor_id: &str, token: &str) -> String {
    let ws_base = endpoint
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    format!("{}/ws/executors/{}?token={}", ws_base, executor_id, token)
}

/// Redact the `token=` query parameter from a WebSocket URL before logging
/// (#267). Bearer tokens in URLs would otherwise land in journalctl, mgmt.log,
/// and `systemctl status` output. Matches `token=` anywhere in the query and
/// truncates the value to `<redacted>`; leaves the rest of the URL intact so
/// operator can still see endpoint, sandbox_id, executor_id.
fn redact_ws_url(url: &str) -> String {
    if let Some(idx) = url.find("token=") {
        let prefix = &url[..idx + "token=".len()];
        // Token ends at the next `&` or end-of-string.
        let after = &url[idx + "token=".len()..];
        let tail = match after.find('&') {
            Some(amp) => &after[amp..],
            None => "",
        };
        format!("{}<redacted>{}", prefix, tail)
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod redact_tests {
    use super::redact_ws_url;

    #[test]
    fn redacts_token_at_end() {
        let u = "ws://127.0.0.1:7337/ws/sandbox/sbx-1?token=abcdef123456";
        assert_eq!(
            redact_ws_url(u),
            "ws://127.0.0.1:7337/ws/sandbox/sbx-1?token=<redacted>"
        );
    }

    #[test]
    fn redacts_token_with_trailing_params() {
        let u = "ws://h/ws/executors/e-1?token=secret&foo=bar";
        assert_eq!(
            redact_ws_url(u),
            "ws://h/ws/executors/e-1?token=<redacted>&foo=bar"
        );
    }

    #[test]
    fn passes_through_when_no_token() {
        let u = "ws://h/ws/sandbox/sbx-1";
        assert_eq!(redact_ws_url(u), u);
    }
}

/// POST /api/sandboxes/register → `(sandbox_id, token)`.
async fn register_sandbox(
    config: &AiwgServeConfig,
    version: &str,
    client: &reqwest::Client,
) -> Result<(String, String)> {
    let resp = client
        .post(format!("{}/api/sandboxes/register", config.endpoint))
        .json(&serde_json::json!({
            "name":           config.sandbox_name,
            "instance_id":    config.instance_id,
            "grpc_endpoint":  config.grpc_endpoint,
            "ws_endpoint":    config.ws_endpoint,
            "http_endpoint":  config.http_endpoint,
            "capabilities":   ["vm", "pty"],
            "version":        version,
        }))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }

    let json: serde_json::Value = resp.json().await?;
    let id = json["sandbox_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing sandbox_id in registration response"))?
        .to_string();
    let token = json["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing token in registration response"))?
        .to_string();
    Ok((id, token))
}

/// POST /api/v1/executors/register — register this sandbox as a mission
/// executor per AIWG `executor.v1.md` (#193). One-shot: returns the
/// (executor_id, token) or an error if the route is unavailable.
///
/// Capabilities are static for now and reflect the agentic-sandbox
/// runtime: KVM VMs and Docker containers, claude-code agent runtime,
/// linux/x64 host, resumable across mgmt-server restarts (mission state
/// persists in dispatcher.rs), HITL pause/resume.
async fn register_executor(
    config: &AiwgServeConfig,
    version: &str,
    client: &reqwest::Client,
) -> Result<(String, String)> {
    let payload = serde_json::json!({
        "executor_id":   config.instance_id,
        "name":          format!("agentic-sandbox-{}", config.sandbox_name),
        "version":       version,
        "spec_version":  "1.0.0",
        "transport_endpoints": {
            "rest": config.http_endpoint,
            "ws":   config.ws_endpoint,
        },
        "capabilities": [
            "isolation:vm",
            "isolation:container",
            "runtime:claude-code",
            "platform:linux/x64",
            "resumable",
            "hitl",
        ],
    });

    let resp = client
        .post(format!("{}/api/v1/executors/register", config.endpoint))
        .json(&payload)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("HTTP {}", status);
    }

    let json: serde_json::Value = resp.json().await?;
    let id = json["executor_id"]
        .as_str()
        .unwrap_or(&config.instance_id)
        .to_string();
    let token = json["token"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();
    Ok((id, token))
}

/// DELETE /api/sandboxes/:id — deregister on clean shutdown.
async fn deregister_sandbox(
    config: &AiwgServeConfig,
    sandbox_id: &str,
    client: &reqwest::Client,
) -> Result<()> {
    client
        .delete(format!("{}/api/sandboxes/{}", config.endpoint, sandbox_id))
        .send()
        .await?;
    info!("Deregistered sandbox {} from aiwg serve", sandbox_id);
    Ok(())
}

/// DELETE /api/v1/executors/:id — deregister executor on clean shutdown.
async fn deregister_executor(
    config: &AiwgServeConfig,
    executor_id: &str,
    token: &str,
    client: &reqwest::Client,
) -> Result<()> {
    client
        .delete(format!(
            "{}/api/v1/executors/{}",
            config.endpoint, executor_id
        ))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?;
    info!("Deregistered executor {} from aiwg serve", executor_id);
    Ok(())
}

const PING_INTERVAL: Duration = Duration::from_secs(20);
const PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// Open WebSocket and drain events until connection drops, channel closes, or
/// a manual reconnect is requested.
///
/// Returns `Ok(())` when the event channel closes (clean shutdown).
/// Returns `Err(_)` when the WS connection fails, the server closes the
/// connection, a ping times out, or `reconnect` is signalled.
async fn push_loop(
    ws_url: &str,
    rx: &mut mpsc::Receiver<SandboxEvent>,
    state: &Arc<RwLock<AiwgConnState>>,
    reconnect: &Arc<Notify>,
) -> Result<()> {
    let (ws, _) = connect_async(ws_url).await?;
    state.write().unwrap().connected = true;
    info!("aiwg serve WS connected: {}", redact_ws_url(ws_url));

    let (mut sink, mut stream) = ws.split();

    let mut ping_ticker = tokio::time::interval(PING_INTERVAL);
    ping_ticker.tick().await; // consume immediate first tick
    let mut waiting_for_pong = false;

    loop {
        tokio::select! {
            // ── Outbound events ───────────────────────────────────────────
            event = rx.recv() => {
                match event {
                    Some(ev) => {
                        let json = serde_json::to_string(&ev)?;
                        sink.send(Message::Text(json)).await?;
                    }
                    None => {
                        // Sender dropped — clean shutdown.
                        let _ = sink.close().await;
                        return Ok(());
                    }
                }
            }

            // ── Inbound frames ────────────────────────────────────────────
            // Reading continuously means we detect server-side Close frames
            // immediately rather than waiting up to PING_INTERVAL for a
            // write to fail.
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Pong(_))) => {
                        debug!("aiwg serve pong received");
                        waiting_for_pong = false;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        info!("aiwg serve closed WS: {:?}", frame);
                        anyhow::bail!("server closed connection");
                    }
                    Some(Ok(_)) => {} // ping / text echo — ignore
                    Some(Err(e)) => {
                        warn!("aiwg serve WS read error: {}", e);
                        return Err(anyhow::anyhow!(e));
                    }
                    None => {
                        anyhow::bail!("aiwg serve WS stream ended");
                    }
                }
            }

            // ── Periodic keepalive ────────────────────────────────────────
            _ = ping_ticker.tick() => {
                if waiting_for_pong {
                    anyhow::bail!("pong timeout — aiwg serve connection silently dead");
                }
                sink.send(Message::Ping(vec![])).await?;
                waiting_for_pong = true;
                debug!("aiwg serve ping sent");
            }

            // ── Manual reconnect ──────────────────────────────────────────
            // Consuming the notification here means the reconnect button is
            // honoured even while the WS is actively running.
            _ = reconnect.notified() => {
                info!("aiwg serve reconnect requested — dropping current connection");
                let _ = sink.close().await;
                anyhow::bail!("manual reconnect");
            }
        }
    }
}

// Suppress unused warning on PONG_TIMEOUT constant — it documents the intended
// timeout but the check is done inline with the waiting_for_pong flag.
const _: Duration = PONG_TIMEOUT;

#[cfg(test)]
mod tests {
    use super::*;

    fn mission(id: &str, state: MissionState) -> MissionRecord {
        let now = "2026-05-30T12:00:00.000Z".to_string();
        MissionRecord {
            mission_id: id.to_string(),
            objective: "test objective".to_string(),
            completion: "test completion".to_string(),
            state,
            pty_session_id: None,
            checkpoint_id: None,
            crash_loop: MissionCrashLoopStatus::default(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[test]
    fn resume_attempt_quarantines_after_default_threshold() {
        let store = MissionStore::new();
        store.insert(mission("m1", MissionState::Suspended));
        let cfg = MissionCrashLoopConfig::default();

        assert_eq!(
            store.record_resume_attempt("m1", &cfg),
            Some(MissionResumeDecision::Resume)
        );
        store.update_state("m1", MissionState::Suspended);
        assert_eq!(
            store.record_resume_attempt("m1", &cfg),
            Some(MissionResumeDecision::Resume)
        );
        store.update_state("m1", MissionState::Suspended);
        let decision = store.record_resume_attempt("m1", &cfg);

        assert!(matches!(
            decision,
            Some(MissionResumeDecision::Quarantine {
                failure_count: 3,
                ..
            })
        ));
        let rec = store.get("m1").unwrap();
        assert_eq!(rec.state, MissionState::Quarantined);
        assert_eq!(rec.crash_loop.consecutive_failures, 3);
        assert!(rec.crash_loop.last_failure_reason.is_some());
        assert!(rec.crash_loop.quarantined_at.is_some());
        assert!(store.active_mission_ids().is_empty());
    }

    #[test]
    fn running_mission_reconnect_does_not_increment_crash_loop() {
        let store = MissionStore::new();
        store.insert(mission("m1", MissionState::Running));

        assert_eq!(
            store.record_resume_attempt("m1", &MissionCrashLoopConfig::default()),
            Some(MissionResumeDecision::Resume)
        );

        let rec = store.get("m1").unwrap();
        assert_eq!(rec.state, MissionState::Running);
        assert_eq!(rec.crash_loop.consecutive_failures, 0);
        assert_eq!(store.active_mission_ids(), vec!["m1".to_string()]);
    }

    #[test]
    fn old_mission_records_default_crash_loop_status() {
        let raw = r#"{
          "m1": {
            "mission_id": "m1",
            "objective": "obj",
            "completion": "done",
            "state": "suspended",
            "pty_session_id": null,
            "checkpoint_id": null,
            "created_at": "2026-05-30T12:00:00.000Z",
            "updated_at": "2026-05-30T12:00:00.000Z"
          }
        }"#;

        let parsed: HashMap<String, MissionRecord> = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed["m1"].crash_loop.consecutive_failures, 0);
        assert!(parsed["m1"].crash_loop.last_failure_reason.is_none());
    }
}
