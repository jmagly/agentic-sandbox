//! Metadata-only Linux process, kernel, and resource activity normalization.
//!
//! Source adapters run beside the workload. They pass Linux Audit, eBPF, cgroup,
//! PSI, and allowlisted journal observations through this module before durable
//! spooling. Raw arguments, paths, cgroup names, and journal messages never enter
//! the normalized queue.

use crate::activity_spool::{ActivitySpool, SpoolError};
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLayer {
    Guest,
    Runtime,
    Host,
}

impl SourceLayer {
    fn as_str(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Runtime => "runtime",
            Self::Host => "host",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxRuntime {
    QemuKvm,
    CloudHypervisor,
    Docker,
    Host,
}

impl LinuxRuntime {
    fn as_str(self) -> &'static str {
        match self {
            Self::QemuKvm => "qemu-kvm",
            Self::CloudHypervisor => "cloud-hypervisor",
            Self::Docker => "docker",
            Self::Host => "host",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CollectorScope {
    pub tenant_id: String,
    pub host_id: String,
    pub instance_id: String,
    pub agent_id: String,
    pub collector_id: String,
    pub boot_id: String,
    pub layer: SourceLayer,
    pub runtime: LinuxRuntime,
}

#[derive(Debug, Clone)]
pub struct CollectorConfig {
    pub queue_capacity: usize,
    pub journal_units: BTreeSet<String>,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 4_096,
            journal_units: [
                "agent-client.service".to_string(),
                "agentic-sandbox.service".to_string(),
                "kernel".to_string(),
            ]
            .into_iter()
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessKind {
    Exec,
    Exit,
}

#[derive(Debug, Clone)]
pub struct KernelProcessEvent<'a> {
    pub kind: ProcessKind,
    pub pid: u32,
    pub ppid: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub start_time_ticks: u64,
    pub executable: Option<&'a str>,
    pub argv: Option<&'a [String]>,
    pub cgroup: Option<&'a str>,
    pub exit_code: Option<i32>,
    pub source: &'a str,
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
    #[error("invalid Linux Audit record: {0}")]
    InvalidAudit(String),
    #[error(transparent)]
    Spool(#[from] SpoolError),
}

/// Bounded normalizer. It is deliberately not a privileged source reader.
pub struct LinuxActivityCollector {
    scope: CollectorScope,
    config: CollectorConfig,
    queue: VecDeque<Value>,
    next_sequence: u64,
    pending_loss: u64,
    health: CollectorHealth,
}

impl LinuxActivityCollector {
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

    /// Normalize a Linux Audit SYSCALL record for execve/execveat.
    ///
    /// Audit arguments and `proctitle` are used only to derive a digest. They
    /// are never copied into the returned event.
    pub fn observe_audit(&mut self, line: &str) -> Result<bool, CollectorError> {
        let fields = parse_audit_fields(line);
        let record_type = fields.get("type").map(String::as_str).unwrap_or("");
        if record_type != "SYSCALL" {
            return Ok(false);
        }
        let syscall = fields.get("syscall").map(String::as_str).unwrap_or("");
        if !matches!(syscall, "59" | "322" | "execve" | "execveat") {
            return Ok(false);
        }
        let pid = parse_u32(&fields, "pid")?;
        let ppid = parse_optional_u32(&fields, "ppid")?;
        let uid = parse_optional_u32(&fields, "uid")?;
        let gid = parse_optional_u32(&fields, "gid")?;
        let start_time_ticks = fields
            .get("start_time_ticks")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let argv_material = fields
            .iter()
            .filter(|(key, _)| {
                key.as_str() == "proctitle"
                    || (key.starts_with('a') && key[1..].bytes().all(|b| b.is_ascii_digit()))
            })
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\0");
        let argv = (!argv_material.is_empty()).then(|| vec![argv_material]);
        self.observe_process(KernelProcessEvent {
            kind: ProcessKind::Exec,
            pid,
            ppid,
            uid,
            gid,
            start_time_ticks,
            executable: fields.get("exe").map(String::as_str),
            argv: argv.as_deref(),
            cgroup: fields.get("cgroup").map(String::as_str),
            exit_code: None,
            source: "linux-audit",
        });
        Ok(true)
    }

    /// Normalize process lifecycle observations from a host-side Audit/eBPF
    /// adapter. This covers exit events that the Audit baseline cannot emit
    /// consistently on every supported kernel.
    pub fn observe_process(&mut self, observation: KernelProcessEvent<'_>) {
        let process_id = format!(
            "{}:{}:{}",
            digest(&self.scope.boot_id),
            observation.pid,
            observation.start_time_ticks
        );
        let mut payload = Map::new();
        payload.insert("source_adapter".into(), json!(observation.source));
        payload.insert("boot_id_digest".into(), json!(digest(&self.scope.boot_id)));
        payload.insert(
            "start_time_ticks".into(),
            json!(observation.start_time_ticks),
        );
        if let Some(executable) = observation.executable {
            payload.insert("executable_digest".into(), json!(digest(executable)));
            if let Some(name) = executable.rsplit('/').next().filter(|v| safe_basename(v)) {
                payload.insert("executable_basename".into(), json!(name));
            }
        }
        if let Some(argv) = observation.argv {
            payload.insert("argument_count".into(), json!(argv.len()));
            payload.insert("arguments_digest".into(), json!(digest(&argv.join("\0"))));
        }
        if let Some(cgroup) = observation.cgroup {
            payload.insert("cgroup_digest".into(), json!(digest(cgroup)));
        }
        if let Some(ppid) = observation.ppid {
            payload.insert("parent_pid".into(), json!(ppid));
        }

        let (event_name, status, retention) = match observation.kind {
            ProcessKind::Exec => ("process.exec", "started", "standard"),
            ProcessKind::Exit => ("process.exit", "success", "standard"),
        };
        let mut event = self.base_event(event_name, "runtime", retention, payload);
        event["correlation"]["process_id"] = json!(process_id);
        event["actor"] = json!({
            "type": "process",
            "id": process_id,
            "uid": observation.uid,
            "gid": observation.gid,
            "pid": observation.pid
        });
        event["outcome"] = json!({"status": status, "exit_code": observation.exit_code});
        self.enqueue(event);
    }

    /// Emit deltas from cgroup v2 `memory.events` (for example `oom` and
    /// `oom_kill`). Callers retain the previous sample and pass only changes.
    pub fn observe_memory_events(&mut self, content: &str) -> usize {
        let counters = parse_counter_file(content);
        let mut emitted = 0;
        for key in ["oom", "oom_kill", "high", "max"] {
            let value = counters.get(key).copied().unwrap_or(0);
            if value == 0 {
                continue;
            }
            let name = if key.starts_with("oom") {
                "system.oom"
            } else {
                "runtime.memory_pressure"
            };
            let payload = Map::from_iter([
                ("counter".into(), json!(key)),
                ("delta".into(), json!(value)),
                ("source_file".into(), json!("memory.events")),
            ]);
            let retention = if key.starts_with("oom") {
                "security"
            } else {
                "standard"
            };
            let event = self.base_event(name, "system", retention, payload);
            self.enqueue(event);
            emitted += 1;
        }
        emitted
    }

    /// Normalize the aggregate fields of a PSI line. Per-process content is
    /// intentionally not accepted.
    pub fn observe_psi(&mut self, resource: &str, line: &str) -> bool {
        if !matches!(resource, "cpu" | "memory" | "io") {
            self.record_unsupported("psi.resource", "resource_not_allowlisted");
            return false;
        }
        let mut payload = Map::new();
        payload.insert("resource".into(), json!(resource));
        let mut parts = line.split_ascii_whitespace();
        let Some(stall_kind @ ("some" | "full")) = parts.next() else {
            return false;
        };
        payload.insert("stall_kind".into(), json!(stall_kind));
        for part in parts {
            let Some((key, raw)) = part.split_once('=') else {
                continue;
            };
            if matches!(key, "avg10" | "avg60" | "avg300") {
                if let Ok(value) = raw.parse::<f64>() {
                    if value.is_finite() && value >= 0.0 {
                        payload.insert(key.into(), json!(value));
                    }
                }
            } else if key == "total" {
                if let Ok(value) = raw.parse::<u64>() {
                    payload.insert("total_stall_micros".into(), json!(value));
                }
            }
        }
        let event = self.base_event("runtime.resource_pressure", "runtime", "standard", payload);
        self.enqueue(event);
        true
    }

    /// Accept an allowlisted journal unit and retain only a digest of the raw
    /// record/message. Unknown units are counted as unsupported, not stored.
    pub fn observe_journal(&mut self, unit: &str, priority: u8, raw_record: &str) -> bool {
        if !self.config.journal_units.contains(unit) {
            self.record_unsupported("journal.unit", "unit_not_allowlisted");
            return false;
        }
        let payload = Map::from_iter([
            ("unit".into(), json!(unit)),
            ("priority".into(), json!(priority.min(7))),
            ("record_digest".into(), json!(digest(raw_record))),
        ]);
        let event = self.base_event("system.journal", "system", "standard", payload);
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

    pub fn record_unsupported(&mut self, event_class: &str, reason: &str) {
        self.health.unsupported += 1;
        let payload = Map::from_iter([
            ("event_class".into(), json!(event_class)),
            ("reason".into(), json!(reason)),
        ]);
        let event = self.base_event("telemetry.unsupported", "integrity", "standard", payload);
        self.enqueue(event);
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

    pub fn queue_depth(&self) -> usize {
        self.queue.len()
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
        json!({
            "schema_version": "activity.event/v1",
            "event_id": Uuid::now_v7().to_string(),
            "event_name": event_name,
            "plane": plane,
            "occurred_at": now,
            "observed_at": now,
            "source": {
                "collector": self.scope.collector_id,
                "layer": self.scope.layer.as_str(),
                "runtime": self.scope.runtime.as_str(),
                "trust": "observed",
                "clock_id": format!("boot:sha256:{}", digest_hex(&self.scope.boot_id))
            },
            "correlation": {
                "tenant_id": self.scope.tenant_id,
                "host_id": self.scope.host_id,
                "instance_id": self.scope.instance_id,
                "agent_id": self.scope.agent_id
            },
            "sensitivity": "metadata",
            "retention_class": retention_class,
            "payload": payload,
            "integrity": {"collector_sequence": self.next_sequence}
        })
    }
}

fn digest(value: &str) -> String {
    format!("sha256:{}", digest_hex(value))
}

fn digest_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn safe_basename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn parse_counter_file(content: &str) -> BTreeMap<&str, u64> {
    content
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(char::is_whitespace)?;
            Some((key, value.trim().parse().ok()?))
        })
        .collect()
}

fn parse_u32(fields: &BTreeMap<String, String>, key: &str) -> Result<u32, CollectorError> {
    fields
        .get(key)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| CollectorError::InvalidAudit(format!("missing or invalid {key}")))
}

fn parse_optional_u32(
    fields: &BTreeMap<String, String>,
    key: &str,
) -> Result<Option<u32>, CollectorError> {
    fields
        .get(key)
        .map(|value| {
            value
                .parse()
                .map_err(|_| CollectorError::InvalidAudit(format!("invalid {key}")))
        })
        .transpose()
}

fn parse_audit_fields(line: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let key_start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() && bytes[index] != b'=' {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            continue;
        }
        let key = &line[key_start..index];
        index += 1;
        let value = if index < bytes.len() && bytes[index] == b'"' {
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index] != b'"' {
                index += 1;
            }
            let value = &line[start..index];
            index = (index + 1).min(bytes.len());
            value
        } else {
            let start = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            &line[start..index]
        };
        fields.insert(key.to_string(), value.to_string());
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn scope(layer: SourceLayer, runtime: LinuxRuntime) -> CollectorScope {
        CollectorScope {
            tenant_id: "tenant-a".into(),
            host_id: "host-a".into(),
            instance_id: "instance-a".into(),
            agent_id: "agent-a".into(),
            collector_id: format!("linux-{}", layer.as_str()),
            boot_id: "2ef2d56a-boot".into(),
            layer,
            runtime,
        }
    }

    fn collector(layer: SourceLayer, runtime: LinuxRuntime) -> LinuxActivityCollector {
        LinuxActivityCollector::new(scope(layer, runtime), CollectorConfig::default(), 1).unwrap()
    }

    #[test]
    fn audit_direct_exec_is_observed_without_raw_arguments_or_paths() {
        let mut collector = collector(SourceLayer::Guest, LinuxRuntime::QemuKvm);
        let raw = r#"type=SYSCALL msg=audit(1.2:3) syscall=59 pid=42 ppid=7 uid=1000 gid=1000 exe="/usr/bin/curl" a0="curl" a1="Authorization: Bearer top-secret" proctitle="curl --token top-secret" cgroup="/tenant/secret-name" start_time_ticks=99"#;
        assert!(collector.observe_audit(raw).unwrap());
        let event = collector.drain(1).pop().unwrap();
        let encoded = serde_json::to_string(&event).unwrap();
        assert_eq!(event["event_name"], "process.exec");
        assert_eq!(event["source"]["layer"], "guest");
        assert_eq!(event["payload"]["executable_basename"], "curl");
        assert!(event["payload"]["arguments_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert!(!encoded.contains("top-secret"));
        assert!(!encoded.contains("/usr/bin/curl"));
        assert!(!encoded.contains("/tenant/secret-name"));
        assert!(!encoded.to_ascii_lowercase().contains("authorization"));
    }

    #[test]
    fn e_bpf_exit_has_stable_process_identity_and_exit_outcome() {
        let mut collector = collector(SourceLayer::Host, LinuxRuntime::Docker);
        collector.observe_process(KernelProcessEvent {
            kind: ProcessKind::Exit,
            pid: 55,
            ppid: Some(1),
            uid: Some(1000),
            gid: Some(1000),
            start_time_ticks: 123,
            executable: Some("/usr/bin/worker"),
            argv: None,
            cgroup: Some("/docker/tenant-container"),
            exit_code: Some(137),
            source: "ebpf-tracepoint",
        });
        let event = collector.drain(1).pop().unwrap();
        assert_eq!(event["event_name"], "process.exit");
        assert_eq!(event["outcome"]["exit_code"], 137);
        assert_eq!(event["source"]["layer"], "host");
        assert!(event["correlation"]["process_id"]
            .as_str()
            .unwrap()
            .ends_with(":55:123"));
    }

    #[test]
    fn runtime_matrix_keeps_guest_host_and_runtime_sources_distinct() {
        for (layer, runtime, expected_runtime) in [
            (SourceLayer::Guest, LinuxRuntime::QemuKvm, "qemu-kvm"),
            (
                SourceLayer::Guest,
                LinuxRuntime::CloudHypervisor,
                "cloud-hypervisor",
            ),
            (SourceLayer::Host, LinuxRuntime::Docker, "docker"),
            (SourceLayer::Host, LinuxRuntime::Host, "host"),
            (SourceLayer::Runtime, LinuxRuntime::Docker, "docker"),
        ] {
            let mut collector = collector(layer, runtime);
            collector.record_restart("fixture");
            let event = collector.drain(1).pop().unwrap();
            assert_eq!(event["source"]["layer"], layer.as_str());
            assert_eq!(event["source"]["runtime"], expected_runtime);
        }
    }

    #[test]
    fn cgroup_memory_and_psi_samples_are_correlated_as_metadata() {
        let mut collector = collector(SourceLayer::Host, LinuxRuntime::Docker);
        assert_eq!(
            collector.observe_memory_events("low 0\nhigh 2\nmax 0\noom 1\noom_kill 1\n"),
            3
        );
        assert!(collector.observe_psi(
            "memory",
            "some avg10=0.15 avg60=0.02 avg300=0.00 total=8123"
        ));
        let events = collector.drain(10);
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event_name"] == "system.oom")
                .count(),
            2
        );
        assert!(events
            .iter()
            .any(|event| event["event_name"] == "runtime.memory_pressure"));
        let psi = events
            .iter()
            .find(|event| event["event_name"] == "runtime.resource_pressure")
            .unwrap();
        assert_eq!(psi["payload"]["total_stall_micros"], 8123);
    }

    #[test]
    fn journal_allowlist_and_restart_records_do_not_retain_messages() {
        let mut collector = collector(SourceLayer::Guest, LinuxRuntime::CloudHypervisor);
        assert!(collector.observe_journal("kernel", 3, "panic token=super-secret"));
        assert!(!collector.observe_journal("untrusted-workload.service", 4, "password=bad"));
        collector.record_restart("watchdog saw secret=another");
        let encoded = serde_json::to_string(&collector.drain(10)).unwrap();
        assert!(!encoded.contains("super-secret"));
        assert!(!encoded.contains("password=bad"));
        assert!(!encoded.contains("another"));
        assert!(encoded.contains("telemetry.unsupported"));
        assert!(encoded.contains("telemetry.collector_restart"));
    }

    #[test]
    fn bounded_queue_reports_loss_after_capacity_recovers() {
        let mut config = CollectorConfig::default();
        config.queue_capacity = 2;
        let mut collector =
            LinuxActivityCollector::new(scope(SourceLayer::Host, LinuxRuntime::Host), config, 1)
                .unwrap();
        collector.record_restart("one");
        collector.record_restart("two");
        collector.record_restart("three");
        assert_eq!(collector.health().dropped, 1);
        assert_eq!(collector.drain(1).len(), 1);
        let remaining = collector.drain(10);
        assert!(remaining
            .iter()
            .any(|event| event["event_name"] == "telemetry.loss"));
    }

    #[test]
    fn spool_sequence_survives_collector_restart() {
        let directory = tempdir().unwrap();
        let spool = ActivitySpool::open(directory.path().join("linux.jsonl"), 64 * 1024).unwrap();
        let mut first = LinuxActivityCollector::from_spool(
            scope(SourceLayer::Host, LinuxRuntime::Host),
            CollectorConfig::default(),
            &spool,
        )
        .unwrap();
        first.record_restart("initial");
        assert_eq!(first.flush_to_spool(&spool, 10).unwrap(), 1);
        let mut second = LinuxActivityCollector::from_spool(
            scope(SourceLayer::Host, LinuxRuntime::Host),
            CollectorConfig::default(),
            &spool,
        )
        .unwrap();
        second.record_restart("restart");
        assert_eq!(second.flush_to_spool(&spool, 10).unwrap(), 1);
        let pending = spool.pending(10).unwrap();
        assert_eq!(pending[0]["integrity"]["collector_sequence"], 1);
        assert_eq!(pending[1]["integrity"]["collector_sequence"], 2);
    }

    #[test]
    fn normalized_events_are_small_and_uuid_v7() {
        let mut collector = collector(SourceLayer::Host, LinuxRuntime::Docker);
        for pid in 1..=100 {
            collector.observe_process(KernelProcessEvent {
                kind: ProcessKind::Exec,
                pid,
                ppid: Some(1),
                uid: Some(1000),
                gid: Some(1000),
                start_time_ticks: pid as u64 * 10,
                executable: Some("/usr/bin/task-worker"),
                argv: Some(&["task-worker".into(), "--safe".into()]),
                cgroup: Some("/tenant/redacted"),
                exit_code: None,
                source: "linux-audit",
            });
        }
        for event in collector.drain(100) {
            let encoded_len = serde_json::to_vec(&event).unwrap().len();
            assert!(encoded_len <= 1_536, "encoded event is {encoded_len} bytes");
            let id = Uuid::parse_str(event["event_id"].as_str().unwrap()).unwrap();
            assert_eq!(id.get_version_num(), 7);
        }
    }
}
