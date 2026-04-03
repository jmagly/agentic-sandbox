//! Event handling for VM lifecycle and agent activities
//!
//! Receives events from libvirt and agent connections and broadcasts them via WebSocket.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
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

/// Event store for recent events
#[derive(Default)]
pub struct EventStore {
    /// Events per source (VM name or agent ID), most recent first
    events: RwLock<HashMap<String, Vec<VmEvent>>>,
    /// Total event count
    total_count: RwLock<u64>,
    /// Last event ID for change detection
    last_event_id: RwLock<u64>,
}

impl EventStore {
    pub fn new() -> Self {
        Self::default()
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
        source_events.insert(0, event);

        // Trim to max size
        if source_events.len() > MAX_EVENTS_PER_SOURCE {
            source_events.truncate(MAX_EVENTS_PER_SOURCE);
        }

        // Increment counters
        let mut count = self.total_count.write().await;
        *count += 1;
        let mut last_id = self.last_event_id.write().await;
        *last_id += 1;
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

    /// Get total event count
    pub async fn total_count(&self) -> u64 {
        *self.total_count.read().await
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

/// GET /api/v1/events - List recent events
pub async fn list_events(State(_state): State<AppState>) -> impl IntoResponse {
    let store = get_event_store();
    let events = store.get_all_events(100).await;
    let total = store.total_count().await;
    let last_id = store.last_event_id().await;

    Json(EventListResponse {
        events,
        total_count: total,
        last_event_id: last_id,
    })
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
    }
}
