//! Metadata-only normalization for macOS Endpoint Security and Unified Logging.
//!
//! The privileged native adapter supplies observations to this module. Raw
//! paths, arguments, environment variables, and log messages are accepted only
//! long enough to derive digests and counts; they never enter the event queue.

use crate::activity_spool::{ActivitySpool, SpoolError};
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, VecDeque};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacRuntime {
    Host,
    DockerDesktopLinuxVm,
}

impl MacRuntime {
    fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::DockerDesktopLinuxVm => "docker-desktop-linux-vm",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CollectorScope {
    pub tenant_id: String,
    pub host_id: String,
    pub instance_id: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub command_id: Option<String>,
    pub collector_id: String,
    pub boot_id: String,
    pub runtime: MacRuntime,
}

#[derive(Debug, Clone)]
pub struct CollectorConfig {
    pub queue_capacity: usize,
    pub unified_log_subsystems: BTreeSet<String>,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 4_096,
            unified_log_subsystems: [
                "io.aiwg.agentic-sandbox".to_string(),
                "com.apple.endpointsecurity".to_string(),
            ]
            .into_iter()
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointEventKind {
    Exec,
    Exit,
    FileCloseModified,
    FileUnlink,
    FileRename,
}

#[derive(Debug, Clone)]
pub struct EndpointObservation<'a> {
    pub kind: EndpointEventKind,
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub audit_token_digest_material: &'a str,
    pub executable_path: Option<&'a str>,
    pub target_path: Option<&'a str>,
    pub destination_path: Option<&'a str>,
    pub arguments: Option<&'a [String]>,
    pub environment: Option<&'a [String]>,
    pub exit_code: Option<i32>,
    pub modified: bool,
}

#[derive(Debug, Clone)]
pub struct UnifiedLogObservation<'a> {
    pub subsystem: &'a str,
    pub category: &'a str,
    pub level: &'a str,
    pub event_name: &'a str,
    pub message: &'a str,
    pub process_id: Option<&'a str>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectorHealth {
    pub accepted: u64,
    pub dropped: u64,
    pub unsupported: u64,
    pub restarts: u64,
    pub max_queue_depth: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    #[error("queue_capacity must be greater than zero")]
    InvalidCapacity,
    #[error(transparent)]
    Spool(#[from] SpoolError),
}

/// Bounded normalizer used by the separately entitled native source reader.
pub struct MacosActivityCollector {
    scope: CollectorScope,
    config: CollectorConfig,
    queue: VecDeque<Value>,
    next_sequence: u64,
    pending_loss: u64,
    health: CollectorHealth,
}

impl MacosActivityCollector {
    pub fn new(
        scope: CollectorScope,
        config: CollectorConfig,
        next_sequence: u64,
    ) -> Result<Self, CollectorError> {
        if config.queue_capacity == 0 {
            return Err(CollectorError::InvalidCapacity);
        }
        Ok(Self {
            scope,
            config,
            queue: VecDeque::new(),
            next_sequence: next_sequence.max(1),
            pending_loss: 0,
            health: CollectorHealth::default(),
        })
    }

    pub fn from_spool(
        scope: CollectorScope,
        config: CollectorConfig,
        spool: &ActivitySpool,
    ) -> Result<Self, CollectorError> {
        Self::new(scope, config, spool.next_sequence()?)
    }

    pub fn observe_endpoint(&mut self, observation: EndpointObservation<'_>) {
        let process_id = format!(
            "{}:{}:{}",
            digest(&self.scope.boot_id),
            observation.pid,
            digest(observation.audit_token_digest_material)
        );
        let mut payload = Map::from_iter([
            ("source_adapter".into(), json!("endpoint-security")),
            ("content_captured".into(), json!(false)),
            (
                "audit_token_digest".into(),
                json!(digest(observation.audit_token_digest_material)),
            ),
        ]);
        if let Some(path) = observation.executable_path {
            payload.insert("executable_path_digest".into(), json!(digest(path)));
            if let Some(name) = path.rsplit('/').next().filter(|name| safe_basename(name)) {
                payload.insert("executable_basename".into(), json!(name));
            }
        }
        if let Some(path) = observation.target_path {
            payload.insert("target_path_digest".into(), json!(digest(path)));
        }
        if let Some(path) = observation.destination_path {
            payload.insert("destination_path_digest".into(), json!(digest(path)));
        }
        if let Some(arguments) = observation.arguments {
            payload.insert("argument_count".into(), json!(arguments.len()));
            payload.insert(
                "arguments_digest".into(),
                json!(digest(&arguments.join("\0"))),
            );
        }
        if let Some(environment) = observation.environment {
            payload.insert("environment_count".into(), json!(environment.len()));
            payload.insert(
                "environment_digest".into(),
                json!(digest(&environment.join("\0"))),
            );
        }
        if matches!(observation.kind, EndpointEventKind::FileCloseModified) {
            payload.insert("modified".into(), json!(observation.modified));
        }

        let (event_name, plane, retention, status) = match observation.kind {
            EndpointEventKind::Exec => ("process.exec", "runtime", "standard", "started"),
            EndpointEventKind::Exit => ("process.exit", "runtime", "standard", "success"),
            EndpointEventKind::FileCloseModified => {
                ("file.close_modified", "filesystem", "standard", "observed")
            }
            EndpointEventKind::FileUnlink => ("file.unlink", "filesystem", "security", "observed"),
            EndpointEventKind::FileRename => ("file.rename", "filesystem", "security", "observed"),
        };
        let mut event = self.base_event(event_name, plane, retention, payload);
        event["correlation"]["process_id"] = json!(process_id);
        event["actor"] = json!({
            "type": "process",
            "id": process_id,
            "pid": observation.pid,
            "parent_pid": observation.parent_pid
        });
        event["outcome"] = json!({"status": status, "exit_code": observation.exit_code});
        self.enqueue(event);
    }

    /// Retain allowlisted structured metadata and only a digest of log text.
    pub fn observe_unified_log(&mut self, observation: UnifiedLogObservation<'_>) -> bool {
        if !self
            .config
            .unified_log_subsystems
            .contains(observation.subsystem)
            || !safe_label(observation.category)
            || !safe_label(observation.event_name)
            || !matches!(
                observation.level,
                "debug" | "info" | "notice" | "error" | "fault"
            )
        {
            self.record_unsupported("unified-log", "metadata_not_allowlisted");
            return false;
        }
        let payload = Map::from_iter([
            ("subsystem".into(), json!(observation.subsystem)),
            ("category".into(), json!(observation.category)),
            ("level".into(), json!(observation.level)),
            ("event_name".into(), json!(observation.event_name)),
            ("message_digest".into(), json!(digest(observation.message))),
            ("content_captured".into(), json!(false)),
        ]);
        let mut event = self.base_event("system.unified_log", "system", "standard", payload);
        if let Some(process_id) = observation.process_id {
            event["correlation"]["process_id"] = json!(process_id);
        }
        self.enqueue(event);
        true
    }

    pub fn record_restart(&mut self, reason: &str) {
        self.health.restarts += 1;
        let payload = Map::from_iter([
            ("restart_count".into(), json!(self.health.restarts)),
            ("reason_digest".into(), json!(digest(reason))),
        ]);
        let event = self.base_event(
            "telemetry.collector_restart",
            "integrity",
            "security",
            payload,
        );
        self.enqueue(event);
    }

    pub fn record_clock_uncertainty(&mut self, uncertainty_micros: u64, reason: &str) {
        let payload = Map::from_iter([
            ("uncertainty_micros".into(), json!(uncertainty_micros)),
            ("reason_digest".into(), json!(digest(reason))),
        ]);
        let event = self.base_event(
            "telemetry.clock_uncertainty",
            "integrity",
            "security",
            payload,
        );
        self.enqueue(event);
    }

    pub fn record_unsupported(&mut self, event_class: &str, reason: &str) {
        self.health.unsupported += 1;
        let payload = Map::from_iter([
            ("event_class".into(), json!(event_class)),
            ("reason".into(), json!(reason)),
        ]);
        let event = self.base_event("telemetry.unsupported", "integrity", "standard", payload);
        self.enqueue(event);
    }

    /// Network Extension is intentionally not inferred from Endpoint Security.
    pub fn record_network_limit(&mut self, reason: &str) {
        self.record_unsupported("network.flow", reason);
    }

    pub fn drain(&mut self, limit: usize) -> Vec<Value> {
        let mut events = Vec::with_capacity(limit.min(self.queue.len()));
        while events.len() < limit {
            let Some(event) = self.queue.pop_front() else {
                break;
            };
            events.push(event);
        }
        self.enqueue_pending_loss();
        events
    }

    pub fn flush_to_spool(
        &mut self,
        spool: &ActivitySpool,
        limit: usize,
    ) -> Result<usize, CollectorError> {
        let mut written = 0;
        while written < limit {
            let Some(event) = self.queue.front() else {
                break;
            };
            spool.append(event)?;
            self.queue.pop_front();
            written += 1;
        }
        self.enqueue_pending_loss();
        Ok(written)
    }

    pub fn health(&self) -> CollectorHealth {
        self.health.clone()
    }

    fn enqueue(&mut self, event: Value) {
        if self.queue.len() >= self.config.queue_capacity {
            self.health.dropped += 1;
            self.pending_loss += 1;
            return;
        }
        self.next_sequence += 1;
        self.health.accepted += 1;
        self.queue.push_back(event);
        self.health.max_queue_depth = self.health.max_queue_depth.max(self.queue.len());
    }

    fn enqueue_pending_loss(&mut self) {
        if self.pending_loss == 0 || self.queue.len() >= self.config.queue_capacity {
            return;
        }
        let lost = std::mem::take(&mut self.pending_loss);
        let payload = Map::from_iter([
            ("lost_observations".into(), json!(lost)),
            ("reason".into(), json!("normalization_queue_full")),
            ("queue_capacity".into(), json!(self.config.queue_capacity)),
        ]);
        let event = self.base_event("telemetry.loss", "integrity", "security", payload);
        self.enqueue(event);
    }

    fn base_event(
        &self,
        event_name: &str,
        plane: &str,
        retention_class: &str,
        payload: Map<String, Value>,
    ) -> Value {
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let mut event = json!({
            "schema_version": "activity.event/v1",
            "event_id": Uuid::now_v7().to_string(),
            "event_name": event_name,
            "plane": plane,
            "occurred_at": now,
            "observed_at": now,
            "source": {
                "collector": self.scope.collector_id,
                "layer": "host",
                "runtime": self.scope.runtime.as_str(),
                "trust": "observed",
                "clock_id": format!("boot:sha256:{}", digest_hex(&self.scope.boot_id))
            },
            "correlation": {
                "tenant_id": self.scope.tenant_id,
                "host_id": self.scope.host_id,
                "instance_id": self.scope.instance_id,
                "agent_id": self.scope.agent_id,
                "session_id": self.scope.session_id,
                "tool_call_id": self.scope.tool_call_id,
                "command_id": self.scope.command_id
            },
            "sensitivity": "metadata",
            "retention_class": retention_class,
            "payload": payload,
            "integrity": {"collector_sequence": self.next_sequence}
        });
        event["correlation"]
            .as_object_mut()
            .expect("correlation is an object")
            .retain(|_, value| !value.is_null());
        event
    }
}

fn digest(value: &str) -> String {
    format!("sha256:{}", digest_hex(value))
}

fn digest_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn safe_basename(value: &str) -> bool {
    safe_label(value) && value.len() <= 128
}

fn safe_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn scope(runtime: MacRuntime) -> CollectorScope {
        CollectorScope {
            tenant_id: "tenant-1".into(),
            host_id: "mutsu".into(),
            instance_id: "instance-1".into(),
            agent_id: "agent-1".into(),
            session_id: Some("session-1".into()),
            tool_call_id: Some("tool-1".into()),
            command_id: Some("command-1".into()),
            collector_id: "macos-endpoint-security".into(),
            boot_id: "secret-boot-id".into(),
            runtime,
        }
    }

    fn endpoint(kind: EndpointEventKind) -> EndpointObservation<'static> {
        EndpointObservation {
            kind,
            pid: 42,
            parent_pid: Some(1),
            audit_token_digest_material: "private-audit-token",
            executable_path: Some("/Users/private/bin/runner"),
            target_path: Some("/Users/private/secrets.txt"),
            destination_path: Some("/Users/private/archive.txt"),
            arguments: None,
            environment: None,
            exit_code: Some(0),
            modified: true,
        }
    }

    #[test]
    fn endpoint_events_never_retain_sensitive_inputs() {
        let mut collector =
            MacosActivityCollector::new(scope(MacRuntime::Host), CollectorConfig::default(), 1)
                .unwrap();
        let arguments = vec!["--token=secret".to_string()];
        let environment = vec!["API_KEY=secret".to_string()];
        for kind in [
            EndpointEventKind::Exec,
            EndpointEventKind::Exit,
            EndpointEventKind::FileCloseModified,
            EndpointEventKind::FileUnlink,
            EndpointEventKind::FileRename,
        ] {
            let mut observation = endpoint(kind);
            observation.arguments = Some(&arguments);
            observation.environment = Some(&environment);
            collector.observe_endpoint(observation);
        }
        let encoded = serde_json::to_string(&collector.drain(20)).unwrap();
        for forbidden in [
            "/Users/private",
            "secrets.txt",
            "--token=secret",
            "API_KEY=secret",
            "private-audit-token",
        ] {
            assert!(!encoded.contains(forbidden), "leaked {forbidden}");
        }
        assert!(encoded.contains("path_digest"));
        assert!(encoded.contains("argument_count"));
        assert!(encoded.contains("environment_count"));
        assert!(encoded.contains("session-1"));
        assert!(encoded.contains("tool-1"));
        assert!(encoded.contains("command-1"));
    }

    #[test]
    fn unified_log_is_allowlisted_and_message_is_digested() {
        let mut collector =
            MacosActivityCollector::new(scope(MacRuntime::Host), CollectorConfig::default(), 1)
                .unwrap();
        assert!(collector.observe_unified_log(UnifiedLogObservation {
            subsystem: "io.aiwg.agentic-sandbox",
            category: "collector",
            level: "notice",
            event_name: "connected",
            message: "operator secret appeared here",
            process_id: Some("process-1"),
        }));
        let encoded = serde_json::to_string(&collector.drain(10)).unwrap();
        assert!(!encoded.contains("operator secret"));
        assert!(encoded.contains("message_digest"));
        assert!(encoded.contains("process-1"));
    }

    #[test]
    fn loss_restart_clock_and_network_limits_are_explicit() {
        let mut config = CollectorConfig::default();
        config.queue_capacity = 1;
        let mut collector =
            MacosActivityCollector::new(scope(MacRuntime::Host), config, 1).unwrap();
        collector.record_restart("crash report with private path");
        collector.record_clock_uncertainty(250_000, "sleep resume");
        assert_eq!(collector.health().dropped, 1);
        let first = collector.drain(1);
        assert_eq!(first[0]["event_name"], "telemetry.collector_restart");
        let loss = collector.drain(1);
        assert_eq!(loss[0]["event_name"], "telemetry.loss");
        collector.record_network_limit("network_extension_not_installed");
        let unsupported = collector.drain(1);
        assert_eq!(unsupported[0]["event_name"], "telemetry.unsupported");

        let mut clock_collector =
            MacosActivityCollector::new(scope(MacRuntime::Host), CollectorConfig::default(), 1)
                .unwrap();
        clock_collector.record_clock_uncertainty(250_000, "sleep resume");
        let clock = clock_collector.drain(1);
        assert_eq!(clock[0]["event_name"], "telemetry.clock_uncertainty");
        assert_eq!(clock[0]["payload"]["uncertainty_micros"], 250_000);
    }

    #[test]
    fn docker_desktop_is_separately_labeled_and_spooled() {
        let directory = tempdir().unwrap();
        let spool = ActivitySpool::open(directory.path().join("macos.jsonl"), 64 * 1024).unwrap();
        let mut collector = MacosActivityCollector::from_spool(
            scope(MacRuntime::DockerDesktopLinuxVm),
            CollectorConfig::default(),
            &spool,
        )
        .unwrap();
        collector.observe_endpoint(endpoint(EndpointEventKind::Exec));
        assert_eq!(collector.flush_to_spool(&spool, 10).unwrap(), 1);
        let pending = spool.pending(10).unwrap();
        assert_eq!(pending[0]["source"]["runtime"], "docker-desktop-linux-vm");
        assert_eq!(pending[0]["integrity"]["collector_sequence"], 1);
    }
}
