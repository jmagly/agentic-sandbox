//! Event handling for VM lifecycle and agent activities
//!
//! Receives events from libvirt and agent connections and broadcasts them via WebSocket.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

use super::server::AppState;

/// Maximum events to retain per source
const MAX_EVENTS_PER_SOURCE: usize = 100;

/// Event types - both VM lifecycle and agent events
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VmEventType {
    // VM lifecycle events
    #[serde(rename = "vm.started")]
    Started,
    #[serde(rename = "vm.stopped")]
    Stopped,
    #[serde(rename = "vm.crashed")]
    Crashed,
    #[serde(rename = "vm.shutdown")]
    Shutdown,
    #[serde(rename = "vm.rebooted")]
    Rebooted,
    #[serde(rename = "vm.suspended")]
    Suspended,
    #[serde(rename = "vm.resumed")]
    Resumed,
    #[serde(rename = "vm.defined")]
    Defined,
    #[serde(rename = "vm.undefined")]
    Undefined,
    #[serde(rename = "vm.pmsuspended")]
    PmSuspended,
    // Agent events
    #[serde(rename = "agent.connected")]
    AgentConnected,
    #[serde(rename = "agent.disconnected")]
    AgentDisconnected,
    #[serde(rename = "agent.registered")]
    AgentRegistered,
    #[serde(rename = "agent.heartbeat")]
    AgentHeartbeat,
    #[serde(rename = "agent.command.started")]
    CommandStarted,
    #[serde(rename = "agent.command.completed")]
    CommandCompleted,
    #[serde(rename = "agent.pty.created")]
    PtyCreated,
    #[serde(rename = "agent.pty.closed")]
    PtyClosed,
    // Session reconciliation events
    #[serde(rename = "session.query_sent")]
    SessionQuerySent,
    #[serde(rename = "session.report_received")]
    SessionReportReceived,
    #[serde(rename = "session.reconcile_started")]
    SessionReconcileStarted,
    #[serde(rename = "session.reconcile_complete")]
    SessionReconcileComplete,
    #[serde(rename = "session.killed")]
    SessionKilled,
    #[serde(rename = "session.preserved")]
    SessionPreserved,
    #[serde(rename = "session.reconcile_failed")]
    SessionReconcileFailed,
    // Operator auth events
    #[serde(rename = "operator.tokens_reloaded")]
    OperatorTokensReloaded,
    // Container lifecycle events
    #[serde(rename = "container.started")]
    ContainerStarted,
    #[serde(rename = "container.stopped")]
    ContainerStopped,
    #[serde(rename = "container.created")]
    ContainerCreated,
    #[serde(rename = "container.removed")]
    ContainerRemoved,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for VmEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmEventType::Started => write!(f, "vm.started"),
            VmEventType::Stopped => write!(f, "vm.stopped"),
            VmEventType::Crashed => write!(f, "vm.crashed"),
            VmEventType::Shutdown => write!(f, "vm.shutdown"),
            VmEventType::Rebooted => write!(f, "vm.rebooted"),
            VmEventType::Suspended => write!(f, "vm.suspended"),
            VmEventType::Resumed => write!(f, "vm.resumed"),
            VmEventType::Defined => write!(f, "vm.defined"),
            VmEventType::Undefined => write!(f, "vm.undefined"),
            VmEventType::PmSuspended => write!(f, "vm.pmsuspended"),
            VmEventType::AgentConnected => write!(f, "agent.connected"),
            VmEventType::AgentDisconnected => write!(f, "agent.disconnected"),
            VmEventType::AgentRegistered => write!(f, "agent.registered"),
            VmEventType::AgentHeartbeat => write!(f, "agent.heartbeat"),
            VmEventType::CommandStarted => write!(f, "agent.command.started"),
            VmEventType::CommandCompleted => write!(f, "agent.command.completed"),
            VmEventType::PtyCreated => write!(f, "agent.pty.created"),
            VmEventType::PtyClosed => write!(f, "agent.pty.closed"),
            VmEventType::SessionQuerySent => write!(f, "session.query_sent"),
            VmEventType::SessionReportReceived => write!(f, "session.report_received"),
            VmEventType::SessionReconcileStarted => write!(f, "session.reconcile_started"),
            VmEventType::SessionReconcileComplete => write!(f, "session.reconcile_complete"),
            VmEventType::SessionKilled => write!(f, "session.killed"),
            VmEventType::SessionPreserved => write!(f, "session.preserved"),
            VmEventType::SessionReconcileFailed => write!(f, "session.reconcile_failed"),
            VmEventType::OperatorTokensReloaded => write!(f, "operator.tokens_reloaded"),
            VmEventType::ContainerStarted => write!(f, "container.started"),
            VmEventType::ContainerStopped => write!(f, "container.stopped"),
            VmEventType::ContainerCreated => write!(f, "container.created"),
            VmEventType::ContainerRemoved => write!(f, "container.removed"),
            VmEventType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Event details
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VmEventDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<i32>,
    // Agent-specific details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    // Session reconciliation details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kill_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ids: Option<Vec<String>>,
}

/// VM lifecycle event from the event bridge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmEvent {
    pub event_type: VmEventType,
    pub vm_name: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub details: VmEventDetails,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

/// Incoming event from vm-event-bridge (more flexible parsing)
#[derive(Debug, Deserialize)]
pub struct IncomingVmEvent {
    pub event_type: String,
    pub vm_name: String,
    pub timestamp: String,
    #[serde(default)]
    pub details: HashMap<String, serde_json::Value>,
    pub agent_id: Option<String>,
    pub trace_id: Option<String>,
}

/// Broadcast channel capacity for the SSE follow stream. Slow subscribers
/// that lag past this drop oldest frames (`broadcast::error::RecvError::Lagged`)
/// and the SSE handler emits a `lagged` comment so clients can resync.
const EVENT_BROADCAST_CAPACITY: usize = 1024;

/// Event store for recent events
pub struct EventStore {
    /// Events per source (VM name or agent ID), most recent first
    events: RwLock<HashMap<String, Vec<VmEvent>>>,
    /// Optional durable JSONL archive for events evicted from the hot window.
    archive: RwLock<Option<EventArchive>>,
    /// Total event count
    total_count: RwLock<u64>,
    /// Events evicted from the hot in-memory window.
    evicted_count: RwLock<u64>,
    /// Events successfully appended to the durable archive.
    archived_count: RwLock<u64>,
    /// Failed durable archive append attempts.
    archive_write_failures: RwLock<u64>,
    /// Last event ID for change detection
    last_event_id: RwLock<u64>,
    /// Live event fan-out for SSE subscribers. Senders never block; if a
    /// subscriber's channel fills up it receives a `Lagged` error and the
    /// SSE handler reports a gap.
    tx: broadcast::Sender<VmEvent>,
}

impl Default for EventStore {
    fn default() -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
        Self {
            events: RwLock::new(HashMap::new()),
            archive: RwLock::new(None),
            total_count: RwLock::new(0),
            evicted_count: RwLock::new(0),
            archived_count: RwLock::new(0),
            archive_write_failures: RwLock::new(0),
            last_event_id: RwLock::new(0),
            tx,
        }
    }
}

impl EventStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set_archive_path(&self, path: impl Into<PathBuf>) {
        let mut archive = self.archive.write().await;
        *archive = Some(EventArchive::new(path.into()));
    }

    /// Subscribe to live events. New subscribers only see events added
    /// after the call returns; pre-existing buffered events should be
    /// fetched separately via `get_all_events` / `get_events_since`.
    pub fn subscribe(&self) -> broadcast::Receiver<VmEvent> {
        self.tx.subscribe()
    }

    /// Add an event to the store
    pub async fn add_event(&self, event: VmEvent) {
        let source = event
            .agent_id
            .clone()
            .unwrap_or_else(|| event.vm_name.clone());
        let mut events = self.events.write().await;
        let source_events = events.entry(source).or_insert_with(Vec::new);

        // Insert at beginning (most recent first)
        source_events.insert(0, event.clone());

        // Trim to max size
        let evicted = source_events.len().saturating_sub(MAX_EVENTS_PER_SOURCE);
        let evicted_events = if evicted > 0 {
            source_events.split_off(MAX_EVENTS_PER_SOURCE)
        } else {
            Vec::new()
        };
        if evicted > 0 {
            let mut evicted_count = self.evicted_count.write().await;
            *evicted_count += evicted as u64;
        }

        // Increment counters
        let mut count = self.total_count.write().await;
        *count += 1;
        let mut last_id = self.last_event_id.write().await;
        *last_id += 1;
        // Drop locks before broadcasting so subscribers can't deadlock the
        // store. `send` returns Err only when there are no receivers — fine.
        drop(events);
        drop(count);
        drop(last_id);
        if !evicted_events.is_empty() {
            let archive = self.archive.read().await.clone();
            if let Some(archive) = archive {
                match archive.append_many(&evicted_events).await {
                    Ok(()) => {
                        let mut archived_count = self.archived_count.write().await;
                        *archived_count += evicted_events.len() as u64;
                    }
                    Err(e) => {
                        let mut failures = self.archive_write_failures.write().await;
                        *failures += 1;
                        warn!(error = %e, "failed to archive evicted events");
                    }
                }
            }
        }
        let _ = self.tx.send(event);
    }

    /// Events newer than `since` (inclusive of `since` excluded), most recent first.
    /// Used by SSE follow mode to pre-stream the buffered window before
    /// switching to live subscription.
    pub async fn get_events_since(&self, since: DateTime<Utc>) -> Vec<VmEvent> {
        let events = self.events.read().await;
        let mut all: Vec<VmEvent> = events
            .values()
            .flatten()
            .filter(|e| e.timestamp > since)
            .cloned()
            .collect();
        all.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        all
    }

    /// Get the last event ID (for change detection)
    pub async fn last_event_id(&self) -> u64 {
        *self.last_event_id.read().await
    }

    /// Get events for a specific VM
    pub async fn get_vm_events(&self, vm_name: &str, limit: usize) -> Vec<VmEvent> {
        let events = self.events.read().await;
        events
            .get(vm_name)
            .map(|e| e.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// Get all recent events across all VMs
    pub async fn get_all_events(&self, limit: usize) -> Vec<VmEvent> {
        let events = self.events.read().await;
        let mut all_events: Vec<VmEvent> = events.values().flatten().cloned().collect();

        // Sort by timestamp descending
        all_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all_events.truncate(limit);
        all_events
    }

    pub async fn query_events(
        &self,
        filter: EventFilter,
        limit: usize,
        include_archived: bool,
    ) -> Vec<VmEvent> {
        let events = self.events.read().await;
        let mut all_events: Vec<VmEvent> = events
            .values()
            .flatten()
            .filter(|event| filter.matches(event))
            .cloned()
            .collect();
        drop(events);

        if include_archived {
            let archive = self.archive.read().await.clone();
            if let Some(archive) = archive {
                match archive.query(&filter).await {
                    Ok(mut archived) => all_events.append(&mut archived),
                    Err(e) => warn!(error = %e, "failed to query archived events"),
                }
            }
        }

        all_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all_events.truncate(limit);
        all_events
    }

    /// Get total event count
    pub async fn total_count(&self) -> u64 {
        *self.total_count.read().await
    }

    /// Snapshot the hot in-memory event window for metrics.
    pub async fn metrics_snapshot(&self) -> EventStoreMetrics {
        let events = self.events.read().await;
        let in_memory_count = events.values().map(|source| source.len() as u64).sum();
        EventStoreMetrics {
            in_memory_count,
            source_count: events.len() as u64,
            max_events_per_source: MAX_EVENTS_PER_SOURCE as u64,
            total_count: *self.total_count.read().await,
            evicted_count: *self.evicted_count.read().await,
            archived_count: *self.archived_count.read().await,
            archive_write_failures: *self.archive_write_failures.read().await,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventStoreMetrics {
    pub in_memory_count: u64,
    pub source_count: u64,
    pub max_events_per_source: u64,
    pub total_count: u64,
    pub evicted_count: u64,
    pub archived_count: u64,
    pub archive_write_failures: u64,
}

#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub since: Option<DateTime<Utc>>,
    pub source: Option<String>,
    pub event_type: Option<String>,
}

impl EventFilter {
    fn matches(&self, event: &VmEvent) -> bool {
        if let Some(since) = self.since {
            if event.timestamp <= since {
                return false;
            }
        }
        if let Some(source) = &self.source {
            let event_source = event
                .agent_id
                .clone()
                .unwrap_or_else(|| event.vm_name.clone());
            if &event_source != source {
                return false;
            }
        }
        if let Some(event_type) = &self.event_type {
            if event.event_type.to_string() != *event_type {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
struct EventArchive {
    path: PathBuf,
}

impl EventArchive {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    async fn append_many(&self, events: &[VmEvent]) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        for event in events {
            let line = serde_json::to_string(event)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await
    }

    async fn query(&self, filter: &EventFilter) -> std::io::Result<Vec<VmEvent>> {
        let content = match tokio::fs::read_to_string(&self.path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let events = content
            .lines()
            .filter_map(|line| serde_json::from_str::<VmEvent>(line).ok())
            .filter(|event| filter.matches(event))
            .collect();
        Ok(events)
    }
}

/// Global event store (lazy initialized)
static EVENT_STORE: std::sync::OnceLock<Arc<EventStore>> = std::sync::OnceLock::new();

/// Get the global event store
pub fn get_event_store() -> Arc<EventStore> {
    EVENT_STORE
        .get_or_init(|| Arc::new(EventStore::new()))
        .clone()
}

pub async fn configure_event_archive(path: impl Into<PathBuf>) {
    get_event_store().set_archive_path(path).await;
}

/// Add an event from the Rust libvirt monitor
pub async fn add_libvirt_event(
    event_type_str: &str,
    vm_name: String,
    timestamp: chrono::DateTime<Utc>,
    reason: Option<String>,
    uptime_seconds: Option<i64>,
) {
    let event_type = parse_event_type(event_type_str);

    let event = VmEvent {
        event_type: event_type.clone(),
        vm_name: vm_name.clone(),
        timestamp,
        details: VmEventDetails {
            reason,
            uptime_seconds,
            exit_status: None,
            ..Default::default()
        },
        agent_id: Some(vm_name.clone()),
        trace_id: None,
    };

    // Log the event
    match &event_type {
        VmEventType::Crashed => {
            warn!(
                vm = %event.vm_name,
                event = %event_type,
                uptime = ?uptime_seconds,
                "VM crashed"
            );
        }
        _ => {
            info!(
                vm = %event.vm_name,
                event = %event_type,
                "VM lifecycle event"
            );
        }
    }

    // Store the event
    let store = get_event_store();
    store.add_event(event).await;
}

/// Add an agent connection event
pub async fn add_agent_event(event_type: VmEventType, agent_id: String, details: VmEventDetails) {
    let event = VmEvent {
        event_type: event_type.clone(),
        vm_name: agent_id.clone(),
        timestamp: Utc::now(),
        details,
        agent_id: Some(agent_id.clone()),
        trace_id: None,
    };

    info!(
        agent = %agent_id,
        event = %event_type,
        "Agent event"
    );

    let store = get_event_store();
    store.add_event(event).await;
}

/// Add a container lifecycle event
pub async fn add_container_event(event_type_str: &str, container_name: String) {
    let event_type = parse_event_type(event_type_str);
    let event = VmEvent {
        event_type: event_type.clone(),
        vm_name: container_name.clone(),
        timestamp: Utc::now(),
        details: VmEventDetails::default(),
        agent_id: Some(container_name.clone()),
        trace_id: None,
    };

    info!(
        container = %event.vm_name,
        event = %event_type,
        "Container lifecycle event"
    );

    let store = get_event_store();
    store.add_event(event).await;
}

/// Convenience: Agent connected
pub async fn emit_agent_connected(agent_id: &str, ip_address: &str) {
    add_agent_event(
        VmEventType::AgentConnected,
        agent_id.to_string(),
        VmEventDetails {
            ip_address: Some(ip_address.to_string()),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Agent registered
pub async fn emit_agent_registered(agent_id: &str, hostname: &str, ip_address: &str) {
    add_agent_event(
        VmEventType::AgentRegistered,
        agent_id.to_string(),
        VmEventDetails {
            hostname: Some(hostname.to_string()),
            ip_address: Some(ip_address.to_string()),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Agent disconnected
pub async fn emit_agent_disconnected(agent_id: &str, reason: Option<String>) {
    add_agent_event(
        VmEventType::AgentDisconnected,
        agent_id.to_string(),
        VmEventDetails {
            reason,
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Command started
pub async fn emit_command_started(agent_id: &str, session_id: &str, command: &str) {
    add_agent_event(
        VmEventType::CommandStarted,
        agent_id.to_string(),
        VmEventDetails {
            session_id: Some(session_id.to_string()),
            command: Some(command.to_string()),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: PTY created
pub async fn emit_pty_created(agent_id: &str, session_id: &str) {
    add_agent_event(
        VmEventType::PtyCreated,
        agent_id.to_string(),
        VmEventDetails {
            session_id: Some(session_id.to_string()),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Session query sent
pub async fn emit_session_query_sent(agent_id: &str, report_all: bool) {
    add_agent_event(
        VmEventType::SessionQuerySent,
        agent_id.to_string(),
        VmEventDetails {
            report_all: Some(report_all),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Session report received
pub async fn emit_session_report_received(agent_id: &str, session_count: usize) {
    add_agent_event(
        VmEventType::SessionReportReceived,
        agent_id.to_string(),
        VmEventDetails {
            session_count: Some(session_count),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Session reconcile started
pub async fn emit_session_reconcile_started(agent_id: &str, keep_count: usize, kill_count: usize) {
    add_agent_event(
        VmEventType::SessionReconcileStarted,
        agent_id.to_string(),
        VmEventDetails {
            keep_count: Some(keep_count),
            kill_count: Some(kill_count),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Session reconcile complete
pub async fn emit_session_reconcile_complete(
    agent_id: &str,
    kept_count: usize,
    killed_count: usize,
    failed_count: usize,
) {
    add_agent_event(
        VmEventType::SessionReconcileComplete,
        agent_id.to_string(),
        VmEventDetails {
            keep_count: Some(kept_count),
            kill_count: Some(killed_count),
            failed_count: Some(failed_count),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Individual session killed
/// SIGHUP-driven token reload result. `count` is the number of currently-
/// active tokens after the reload (0 if it failed). `success=false`
/// indicates a parse/IO error; the previous map is unchanged in that case.
pub async fn emit_operator_tokens_reloaded(count: usize, success: bool) {
    add_agent_event(
        VmEventType::OperatorTokensReloaded,
        "operator-auth".to_string(),
        VmEventDetails {
            session_count: Some(count),
            reason: Some(if success { "ok".into() } else { "error".into() }),
            ..Default::default()
        },
    )
    .await;
}

pub async fn emit_session_killed(agent_id: &str, session_id: &str) {
    add_agent_event(
        VmEventType::SessionKilled,
        agent_id.to_string(),
        VmEventDetails {
            session_id: Some(session_id.to_string()),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Individual session preserved
pub async fn emit_session_preserved(agent_id: &str, session_id: &str) {
    add_agent_event(
        VmEventType::SessionPreserved,
        agent_id.to_string(),
        VmEventDetails {
            session_id: Some(session_id.to_string()),
            ..Default::default()
        },
    )
    .await;
}

/// Convenience: Session failed to kill
pub async fn emit_session_reconcile_failed(agent_id: &str, session_id: &str, reason: &str) {
    add_agent_event(
        VmEventType::SessionReconcileFailed,
        agent_id.to_string(),
        VmEventDetails {
            session_id: Some(session_id.to_string()),
            reason: Some(reason.to_string()),
            ..Default::default()
        },
    )
    .await;
}

/// Parse event type from string
fn parse_event_type(s: &str) -> VmEventType {
    match s {
        "vm.started" => VmEventType::Started,
        "vm.stopped" => VmEventType::Stopped,
        "vm.crashed" => VmEventType::Crashed,
        "vm.shutdown" => VmEventType::Shutdown,
        "vm.rebooted" => VmEventType::Rebooted,
        "vm.suspended" => VmEventType::Suspended,
        "vm.resumed" => VmEventType::Resumed,
        "vm.defined" => VmEventType::Defined,
        "vm.undefined" => VmEventType::Undefined,
        "vm.pmsuspended" => VmEventType::PmSuspended,
        "agent.connected" => VmEventType::AgentConnected,
        "agent.disconnected" => VmEventType::AgentDisconnected,
        "agent.registered" => VmEventType::AgentRegistered,
        "agent.heartbeat" => VmEventType::AgentHeartbeat,
        "agent.command.started" => VmEventType::CommandStarted,
        "agent.command.completed" => VmEventType::CommandCompleted,
        "agent.pty.created" => VmEventType::PtyCreated,
        "agent.pty.closed" => VmEventType::PtyClosed,
        "session.query_sent" => VmEventType::SessionQuerySent,
        "session.report_received" => VmEventType::SessionReportReceived,
        "session.reconcile_started" => VmEventType::SessionReconcileStarted,
        "session.reconcile_complete" => VmEventType::SessionReconcileComplete,
        "session.killed" => VmEventType::SessionKilled,
        "session.preserved" => VmEventType::SessionPreserved,
        "session.reconcile_failed" => VmEventType::SessionReconcileFailed,
        "container.started" => VmEventType::ContainerStarted,
        "container.stopped" => VmEventType::ContainerStopped,
        "container.created" => VmEventType::ContainerCreated,
        "container.removed" => VmEventType::ContainerRemoved,
        _ => VmEventType::Unknown,
    }
}

/// POST /api/v1/events - Receive events from vm-event-bridge
pub async fn receive_event(
    State(_state): State<AppState>,
    Json(incoming): Json<IncomingVmEvent>,
) -> impl IntoResponse {
    let event_type = parse_event_type(&incoming.event_type);

    // Parse timestamp
    let timestamp = DateTime::parse_from_rfc3339(&incoming.timestamp)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    // Extract details
    let details = VmEventDetails {
        reason: incoming
            .details
            .get("reason")
            .and_then(|v| v.as_str())
            .map(String::from),
        uptime_seconds: incoming
            .details
            .get("uptime_seconds")
            .and_then(|v| v.as_i64()),
        exit_status: incoming
            .details
            .get("exit_status")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        ..Default::default()
    };

    let event = VmEvent {
        event_type: event_type.clone(),
        vm_name: incoming.vm_name.clone(),
        timestamp,
        details,
        agent_id: incoming.agent_id,
        trace_id: incoming.trace_id,
    };

    // Log the event
    match &event_type {
        VmEventType::Crashed => {
            warn!(
                vm = %event.vm_name,
                event = %event_type,
                uptime = ?event.details.uptime_seconds,
                reason = ?event.details.reason,
                "VM crashed"
            );
        }
        _ => {
            info!(
                vm = %event.vm_name,
                event = %event_type,
                "VM lifecycle event"
            );
        }
    }

    // Store the event
    let store = get_event_store();
    store.add_event(event.clone()).await;

    // TODO: Broadcast via WebSocket to connected clients
    // TODO: Trigger crash loop detection for vm.crashed events
    // TODO: Increment Prometheus metrics

    (StatusCode::OK, Json(EventResponse { received: true }))
}

#[derive(Serialize)]
struct EventResponse {
    received: bool,
}

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    /// `follow=true` switches to SSE; otherwise returns JSON snapshot.
    #[serde(default)]
    follow: Option<bool>,
    /// RFC3339 timestamp; only events after this are returned (and, in
    /// follow mode, replayed before live streaming starts).
    since: Option<String>,
    /// Filter by event source (agent_id or vm_name). Exact match.
    source: Option<String>,
    /// Filter by event type (e.g. `agent.connected`, `vm.started`).
    /// Exact match against the wire-format string.
    event_type: Option<String>,
    /// Include durable archived events that were evicted from the hot window.
    #[serde(default)]
    include_archived: bool,
    /// Maximum events returned for JSON snapshots.
    limit: Option<usize>,
}

/// GET /api/v1/events
///
/// Two modes:
/// - default: JSON snapshot of the most recent events (back-compat).
/// - `?follow=true`: text/event-stream that first replays buffered events
///   matching the filter (since, source, event_type) and then streams new
///   ones live as they are added.
pub async fn list_events(Query(q): Query<EventQuery>, State(_state): State<AppState>) -> Response {
    let store = get_event_store();

    let since = q
        .since
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let filter = EventFilter {
        since,
        source: q.source.clone(),
        event_type: q.event_type.clone(),
    };
    let live_filter = filter.clone();
    let matches = move |e: &VmEvent| -> bool { live_filter.matches(e) };

    if q.follow.unwrap_or(false) {
        use axum::response::sse::{Event, KeepAlive, Sse};

        // Subscribe BEFORE the buffered fetch so we don't miss events that
        // arrive between the snapshot and the live stream starting.
        let mut rx = store.subscribe();

        let initial: Vec<VmEvent> = if let Some(ts) = since {
            store
                .get_events_since(ts)
                .await
                .into_iter()
                .filter(&matches)
                .collect()
        } else {
            // No `since` ⇒ no initial replay. Operators who want the full
            // buffered window can request it via the JSON form first.
            Vec::new()
        };

        let stream = async_stream::stream! {
            for ev in initial {
                if let Ok(data) = serde_json::to_string(&ev) {
                    yield Ok::<_, std::convert::Infallible>(Event::default().data(data));
                }
            }
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if !matches(&ev) {
                            continue;
                        }
                        if let Ok(data) = serde_json::to_string(&ev) {
                            yield Ok(Event::default().data(data));
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Tell the client they missed `n` events; they can
                        // reconnect with a fresh `since` to backfill.
                        yield Ok(Event::default()
                            .event("lagged")
                            .data(format!("{{\"missed\":{}}}", n)));
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        return Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    // JSON snapshot mode. Archived events are opt-in because the hot path
    // should stay bounded and fast for dashboard polling.
    let limit = q.limit.unwrap_or(100).min(5000);
    let events = store.query_events(filter, limit, q.include_archived).await;
    let total = store.total_count().await;
    let last_id = store.last_event_id().await;

    Json(EventListResponse {
        events,
        total_count: total,
        last_event_id: last_id,
    })
    .into_response()
}

#[derive(Serialize)]
struct EventListResponse {
    events: Vec<VmEvent>,
    total_count: u64,
    last_event_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_event_type() {
        assert_eq!(parse_event_type("vm.started"), VmEventType::Started);
        assert_eq!(parse_event_type("vm.crashed"), VmEventType::Crashed);
        assert_eq!(
            parse_event_type("container.started"),
            VmEventType::ContainerStarted
        );
        assert_eq!(parse_event_type("vm.unknown_123"), VmEventType::Unknown);
    }

    #[tokio::test]
    async fn test_event_store() {
        let store = EventStore::new();

        let event = VmEvent {
            event_type: VmEventType::Started,
            vm_name: "test-vm".to_string(),
            timestamp: Utc::now(),
            details: VmEventDetails::default(),
            agent_id: Some("test-vm".to_string()),
            trace_id: None,
        };

        store.add_event(event.clone()).await;

        let events = store.get_vm_events("test-vm", 10).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].vm_name, "test-vm");

        assert_eq!(store.total_count().await, 1);
    }

    #[tokio::test]
    async fn test_event_store_limit() {
        let store = EventStore::new();

        // Add more than MAX_EVENTS_PER_SOURCE
        for _i in 0..150 {
            let event = VmEvent {
                event_type: VmEventType::Started,
                vm_name: "test-vm".to_string(),
                timestamp: Utc::now(),
                details: VmEventDetails::default(),
                agent_id: Some("test-vm".to_string()), // Same agent_id for all events
                trace_id: None,
            };
            store.add_event(event).await;
        }

        let events = store.get_vm_events("test-vm", 200).await;
        assert_eq!(events.len(), MAX_EVENTS_PER_SOURCE);

        let metrics = store.metrics_snapshot().await;
        assert_eq!(metrics.in_memory_count, MAX_EVENTS_PER_SOURCE as u64);
        assert_eq!(metrics.source_count, 1);
        assert_eq!(metrics.max_events_per_source, MAX_EVENTS_PER_SOURCE as u64);
        assert_eq!(metrics.total_count, 150);
        assert_eq!(metrics.evicted_count, 50);
    }

    #[tokio::test]
    async fn event_store_archives_evicted_events_for_durable_query() {
        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("events.jsonl");
        let store = EventStore::new();
        store.set_archive_path(&archive_path).await;
        let base = Utc::now();

        for i in 0..150 {
            store
                .add_event(VmEvent {
                    event_type: VmEventType::Started,
                    vm_name: "test-vm".to_string(),
                    timestamp: base + chrono::Duration::seconds(i),
                    details: VmEventDetails::default(),
                    agent_id: Some("test-vm".to_string()),
                    trace_id: None,
                })
                .await;
        }

        let hot_only = store
            .query_events(
                EventFilter {
                    source: Some("test-vm".to_string()),
                    ..Default::default()
                },
                200,
                false,
            )
            .await;
        assert_eq!(hot_only.len(), MAX_EVENTS_PER_SOURCE);

        let with_archive = store
            .query_events(
                EventFilter {
                    source: Some("test-vm".to_string()),
                    ..Default::default()
                },
                200,
                true,
            )
            .await;
        assert_eq!(with_archive.len(), 150);
        assert_eq!(
            with_archive.first().unwrap().timestamp,
            base + chrono::Duration::seconds(149)
        );
        assert_eq!(with_archive.last().unwrap().timestamp, base);

        let archived_file = tokio::fs::read_to_string(&archive_path).await.unwrap();
        assert_eq!(archived_file.lines().count(), 50);

        let metrics = store.metrics_snapshot().await;
        assert_eq!(metrics.in_memory_count, MAX_EVENTS_PER_SOURCE as u64);
        assert_eq!(metrics.evicted_count, 50);
        assert_eq!(metrics.archived_count, 50);
        assert_eq!(metrics.archive_write_failures, 0);
    }

    #[tokio::test]
    async fn event_store_metrics_sum_multiple_sources() {
        let store = EventStore::new();
        for source in ["vm-a", "vm-b"] {
            for _ in 0..3 {
                store
                    .add_event(VmEvent {
                        event_type: VmEventType::Started,
                        vm_name: source.to_string(),
                        timestamp: Utc::now(),
                        details: VmEventDetails::default(),
                        agent_id: Some(source.to_string()),
                        trace_id: None,
                    })
                    .await;
            }
        }

        let metrics = store.metrics_snapshot().await;
        assert_eq!(metrics.in_memory_count, 6);
        assert_eq!(metrics.source_count, 2);
        assert_eq!(metrics.total_count, 6);
        assert_eq!(metrics.evicted_count, 0);
    }
}
