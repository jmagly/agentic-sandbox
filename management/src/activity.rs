//! Durable, loss-aware metadata activity ingest (`activity.event/v1`).
//!
//! This store is intentionally metadata-only. Restricted content and fields
//! that commonly carry secrets are rejected before a transaction begins.

use chrono::{DateTime, Utc};
use flate2::write::ZlibEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use parking_lot::Mutex;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

use crate::activity_governance::{AccessContext, GovernanceLedger, GovernedExport, Role};

pub const ACTIVITY_SCHEMA_VERSION: &str = "activity.event/v1";
const MAX_BATCH_EVENTS: usize = 1_000;
const MAX_DECODED_EVENT_CACHE_ENTRIES: usize = 5_000;
const MAX_ACTIVITY_ENVELOPE_BYTES: usize = 1_048_576;
const ACTIVITY_ENVELOPE_MAGIC: &[u8; 4] = b"ASEV";
const ACTIVITY_ENVELOPE_VERSION: u8 = 1;
const ACTIVITY_ENVELOPE_HEADER_BYTES: usize = 9;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActivityEvent {
    pub schema_version: String,
    pub event_id: String,
    pub event_name: String,
    pub plane: EventPlane,
    pub occurred_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub source: ActivitySource,
    pub correlation: ActivityCorrelation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<ActivityEntity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<ActivityEntity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ActivityOutcome>,
    pub sensitivity: Sensitivity,
    pub retention_class: RetentionClass,
    pub payload: Map<String, Value>,
    pub integrity: ActivityIntegrity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventPlane {
    Session,
    Action,
    Network,
    Runtime,
    System,
    Integrity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActivitySource {
    pub collector: String,
    pub layer: SourceLayer,
    pub runtime: ActivityRuntime,
    pub trust: SourceTrust,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_error_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceLayer {
    Guest,
    Runtime,
    Host,
    ControlPlane,
    Provider,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityRuntime {
    QemuKvm,
    CloudHypervisor,
    Docker,
    Host,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceTrust {
    Observed,
    Attested,
    SelfReported,
    Derived,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivityCorrelation {
    pub tenant_id: String,
    pub host_id: String,
    pub instance_id: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivityEntity {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivityOutcome {
    pub status: OutcomeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Started,
    Success,
    Failure,
    Allowed,
    Denied,
    Degraded,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Sensitivity {
    Metadata,
    RestrictedContent,
    SecretProhibited,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionClass {
    Standard,
    Security,
    ForensicHold,
    Ephemeral,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivityIntegrity {
    pub collector_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_previous_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_previous_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngestScope {
    pub tenant_id: String,
    pub host_id: String,
    pub instance_id: String,
    pub agent_id: String,
    pub collector_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestBatch {
    pub events: Vec<ActivityEvent>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IngestAck {
    pub schema_version: &'static str,
    pub accepted: usize,
    pub duplicates: usize,
    pub durable_through_sequence: u64,
    pub sequence_gaps_recorded: Vec<SequenceGap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SequenceGap {
    pub collector_id: String,
    pub first_missing_sequence: u64,
    pub last_missing_sequence: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ActivityQuery {
    /// Resume after this stable activity event identifier within the filtered
    /// timeline. The cursor is intentionally the existing event ID rather than
    /// a second database-specific offset.
    pub cursor: Option<String>,
    pub event_name: Option<String>,
    pub collector: Option<String>,
    pub trust: Option<SourceTrust>,
    pub plane: Option<EventPlane>,
    pub outcome: Option<OutcomeStatus>,
    pub session_id: Option<String>,
    pub mission_id: Option<String>,
    pub task_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub command_id: Option<String>,
    pub process_id: Option<String>,
    pub trace_id: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

impl ActivityQuery {
    fn matches(&self, event: &ActivityEvent) -> bool {
        optional_eq(&self.event_name, Some(&event.event_name))
            && optional_eq(&self.collector, Some(&event.source.collector))
            && self.trust.map(|v| event.source.trust == v).unwrap_or(true)
            && self.plane.map(|v| event.plane == v).unwrap_or(true)
            && self
                .outcome
                .map(|v| event.outcome.as_ref().map(|o| o.status) == Some(v))
                .unwrap_or(true)
            && optional_eq(&self.session_id, event.correlation.session_id.as_ref())
            && optional_eq(&self.mission_id, event.correlation.mission_id.as_ref())
            && optional_eq(&self.task_id, event.correlation.task_id.as_ref())
            && optional_eq(&self.tool_call_id, event.correlation.tool_call_id.as_ref())
            && optional_eq(&self.command_id, event.correlation.command_id.as_ref())
            && optional_eq(&self.process_id, event.correlation.process_id.as_ref())
            && optional_eq(&self.trace_id, event.correlation.trace_id.as_ref())
            && self.since.map(|t| event.occurred_at >= t).unwrap_or(true)
            && self.until.map(|t| event.occurred_at <= t).unwrap_or(true)
    }
}

fn optional_eq(filter: &Option<String>, value: Option<&String>) -> bool {
    filter
        .as_ref()
        .map(|wanted| value == Some(wanted))
        .unwrap_or(true)
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityQueryResult {
    pub schema_version: &'static str,
    pub events: Vec<ActivityEvent>,
    pub coverage: Vec<CollectorCoverage>,
    pub completeness: CompletenessAssessment,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub cursor_found: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CollectorCoverage {
    pub collector_id: String,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub event_count: usize,
    pub sequence_gaps: Vec<SequenceGap>,
    pub durable_loss_records: Vec<SequenceGap>,
    pub maximum_clock_error_ms: f64,
    pub last_observed_at: Option<DateTime<Utc>>,
    pub restart_count: usize,
    pub dropped_event_count: u64,
    pub unsupported_event_classes: Vec<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CompletenessAssessment {
    /// True only when every known collector is current and reports no loss.
    pub complete: bool,
    pub label: &'static str,
    pub collector_count: usize,
    pub sequence_gap_count: usize,
    pub durable_loss_count: usize,
    pub restart_count: usize,
    pub dropped_event_count: u64,
    pub stale_collector_count: usize,
    pub unsupported_event_classes: Vec<String>,
    pub maximum_clock_error_ms: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum ActivityError {
    #[error("invalid activity event: {0}")]
    Invalid(String),
    #[error("activity scope mismatch: {0}")]
    Scope(String),
    #[error("activity storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("activity record is corrupt: {0}")]
    Corrupt(String),
    #[error("activity capability unavailable: {0}")]
    Unavailable(String),
}

pub struct ActivityStore {
    db: Mutex<Connection>,
    decoded_event_cache: Mutex<HashMap<String, CachedActivityEvent>>,
    export_signer: parking_lot::RwLock<Option<ActivityExportSigner>>,
}

#[derive(Debug, Clone)]
struct CachedActivityEvent {
    stored: SqlValue,
    event: Arc<ActivityEvent>,
}

#[derive(Debug, Clone)]
struct ActivityExportSigner {
    key_id: String,
    key: Vec<u8>,
}

impl ActivityStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<Self>, ActivityError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ActivityError::Invalid(format!("create data directory: {e}")))?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Arc<Self>, ActivityError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Move committed WAL pages into the main database and release the WAL.
    /// Campaigns call this before a final footprint measurement so transient
    /// duplicate pages are reported separately from durable steady state.
    pub fn checkpoint_wal(&self) -> Result<(), ActivityError> {
        self.db
            .lock()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    fn from_connection(db: Connection) -> Result<Arc<Self>, ActivityError> {
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS activity_events (
               event_id TEXT PRIMARY KEY,
               tenant_id TEXT NOT NULL,
               host_id TEXT NOT NULL,
               instance_id TEXT NOT NULL,
               agent_id TEXT NOT NULL,
               collector_id TEXT NOT NULL,
               collector_sequence INTEGER NOT NULL,
               occurred_at TEXT NOT NULL,
               observed_at TEXT NOT NULL,
               event_json BLOB NOT NULL,
               UNIQUE(tenant_id, host_id, instance_id, agent_id, collector_id, collector_sequence)
             );
             CREATE INDEX IF NOT EXISTS activity_scope_time
               ON activity_events(tenant_id, host_id, instance_id, agent_id, occurred_at);
             CREATE TABLE IF NOT EXISTS activity_collector_state (
               tenant_id TEXT NOT NULL,
               host_id TEXT NOT NULL,
               instance_id TEXT NOT NULL,
               agent_id TEXT NOT NULL,
               collector_id TEXT NOT NULL,
               durable_through_sequence INTEGER NOT NULL,
               PRIMARY KEY(tenant_id, host_id, instance_id, agent_id, collector_id)
             );
             CREATE TABLE IF NOT EXISTS activity_loss_records (
               tenant_id TEXT NOT NULL,
               host_id TEXT NOT NULL,
               instance_id TEXT NOT NULL,
               agent_id TEXT NOT NULL,
               collector_id TEXT NOT NULL,
               first_missing_sequence INTEGER NOT NULL,
               last_missing_sequence INTEGER NOT NULL,
               detected_at TEXT NOT NULL,
               UNIQUE(tenant_id, host_id, instance_id, agent_id, collector_id,
                      first_missing_sequence, last_missing_sequence)
             );",
        )?;
        Ok(Arc::new(Self {
            db: Mutex::new(db),
            decoded_event_cache: Mutex::new(HashMap::new()),
            export_signer: parking_lot::RwLock::new(None),
        }))
    }

    /// Configure signed activity exports from a root-owned key file.
    /// The key is retained in memory and is never accepted over HTTP.
    pub fn configure_export_signer(
        &self,
        key_id: impl Into<String>,
        key: Vec<u8>,
    ) -> Result<(), ActivityError> {
        let key_id = key_id.into();
        if key_id.trim().is_empty() || key.len() < 32 {
            return Err(ActivityError::Invalid(
                "activity export key id must be non-empty and key must be at least 32 bytes".into(),
            ));
        }
        *self.export_signer.write() = Some(ActivityExportSigner { key_id, key });
        Ok(())
    }

    pub fn ingest(
        &self,
        scope: &IngestScope,
        batch: IngestBatch,
    ) -> Result<IngestAck, ActivityError> {
        if batch.events.is_empty() || batch.events.len() > MAX_BATCH_EVENTS {
            return Err(ActivityError::Invalid(format!(
                "batch must contain 1..={MAX_BATCH_EVENTS} events"
            )));
        }
        for event in &batch.events {
            validate_event(scope, event)?;
        }

        let mut db = self.db.lock();
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut accepted = 0;
        let mut duplicates = 0;
        let mut gaps = Vec::new();
        let mut durable = tx
            .query_row(
                "SELECT durable_through_sequence FROM activity_collector_state
                 WHERE tenant_id = ?1 AND host_id = ?2 AND instance_id = ?3
                   AND agent_id = ?4 AND collector_id = ?5",
                params![
                    scope.tenant_id,
                    scope.host_id,
                    scope.instance_id,
                    scope.agent_id,
                    scope.collector_id
                ],
                |row| row.get::<_, u64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let mut highest_observed = tx.query_row(
            "SELECT COALESCE(MAX(collector_sequence), 0) FROM activity_events
             WHERE tenant_id = ?1 AND host_id = ?2 AND instance_id = ?3
               AND agent_id = ?4 AND collector_id = ?5",
            params![
                scope.tenant_id,
                scope.host_id,
                scope.instance_id,
                scope.agent_id,
                scope.collector_id
            ],
            |row| row.get::<_, u64>(0),
        )?;

        for event in batch.events {
            let existing = tx
                .query_row(
                    "SELECT event_json FROM activity_events WHERE event_id = ?1",
                    params![event.event_id],
                    |row| row.get::<_, SqlValue>(0),
                )
                .optional()?;
            if let Some(existing) = existing {
                let existing_event = decode_stored_event(existing, scope)?;
                if existing_event != event {
                    return Err(ActivityError::Invalid(format!(
                        "event_id {} is already bound to different event data",
                        event.event_id
                    )));
                }
                duplicates += 1;
                continue;
            }
            let sequence = event.integrity.collector_sequence;
            let sequence_owner = tx
                .query_row(
                    "SELECT event_id FROM activity_events
                     WHERE tenant_id = ?1 AND host_id = ?2 AND instance_id = ?3
                       AND agent_id = ?4 AND collector_id = ?5 AND collector_sequence = ?6",
                    params![
                        scope.tenant_id,
                        scope.host_id,
                        scope.instance_id,
                        scope.agent_id,
                        scope.collector_id,
                        sequence,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(owner) = sequence_owner {
                return Err(ActivityError::Invalid(format!(
                    "collector_sequence {sequence} is already bound to event_id {owner}"
                )));
            }
            if sequence > highest_observed.saturating_add(1) {
                let gap = SequenceGap {
                    collector_id: scope.collector_id.clone(),
                    first_missing_sequence: highest_observed + 1,
                    last_missing_sequence: sequence - 1,
                };
                tx.execute(
                    "INSERT OR IGNORE INTO activity_loss_records
                     (tenant_id, host_id, instance_id, agent_id, collector_id,
                      first_missing_sequence, last_missing_sequence, detected_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        scope.tenant_id,
                        scope.host_id,
                        scope.instance_id,
                        scope.agent_id,
                        scope.collector_id,
                        gap.first_missing_sequence,
                        gap.last_missing_sequence,
                        Utc::now().to_rfc3339(),
                    ],
                )?;
                gaps.push(gap);
            }
            let envelope = encode_stored_event(&event)?;
            tx.execute(
                "INSERT INTO activity_events
                 (event_id, tenant_id, host_id, instance_id, agent_id, collector_id,
                  collector_sequence, occurred_at, observed_at, event_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    event.event_id,
                    event.correlation.tenant_id,
                    event.correlation.host_id,
                    event.correlation.instance_id,
                    event.correlation.agent_id,
                    event.source.collector,
                    sequence,
                    event.occurred_at.to_rfc3339(),
                    event.observed_at.to_rfc3339(),
                    envelope,
                ],
            )?;
            highest_observed = highest_observed.max(sequence);
            accepted += 1;

            // A durable ACK is a contiguous prefix, not the largest sequence
            // observed. Advancing across a hole could make a collector prune
            // an event that the server has never persisted.
            loop {
                let next = durable.saturating_add(1);
                let next_is_present = tx
                    .query_row(
                        "SELECT 1 FROM activity_events
                         WHERE tenant_id = ?1 AND host_id = ?2 AND instance_id = ?3
                           AND agent_id = ?4 AND collector_id = ?5 AND collector_sequence = ?6",
                        params![
                            scope.tenant_id,
                            scope.host_id,
                            scope.instance_id,
                            scope.agent_id,
                            scope.collector_id,
                            next,
                        ],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !next_is_present {
                    break;
                }
                durable = next;
            }
        }
        tx.execute(
            "INSERT INTO activity_collector_state
             (tenant_id, host_id, instance_id, agent_id, collector_id, durable_through_sequence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(tenant_id, host_id, instance_id, agent_id, collector_id) DO UPDATE SET
               durable_through_sequence = MAX(durable_through_sequence, excluded.durable_through_sequence)",
            params![
                scope.tenant_id,
                scope.host_id,
                scope.instance_id,
                scope.agent_id,
                scope.collector_id,
                durable
            ],
        )?;
        tx.commit()?;

        Ok(IngestAck {
            schema_version: ACTIVITY_SCHEMA_VERSION,
            accepted,
            duplicates,
            durable_through_sequence: durable,
            sequence_gaps_recorded: gaps,
        })
    }

    pub fn query(
        &self,
        scope: &IngestScope,
        query: &ActivityQuery,
    ) -> Result<ActivityQueryResult, ActivityError> {
        let db = self.db.lock();
        let mut stmt = db.prepare(
            "SELECT event_id, event_json FROM activity_events
             WHERE tenant_id = ?1 AND host_id = ?2 AND instance_id = ?3 AND agent_id = ?4
             ORDER BY occurred_at ASC LIMIT 5000",
        )?;
        let rows = stmt.query_map(
            params![
                scope.tenant_id,
                scope.host_id,
                scope.instance_id,
                scope.agent_id
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, SqlValue>(1)?)),
        )?;
        let mut all = Vec::new();
        let mut cache = self.decoded_event_cache.lock();
        for row in rows {
            let (event_id, stored) = row?;
            let cached = cache
                .get(&event_id)
                .filter(|entry| entry.stored == stored)
                .map(|entry| entry.event.clone());
            let event = match cached {
                Some(event) => event,
                None => {
                    let event = Arc::new(decode_stored_event(stored.clone(), scope)?);
                    if cache.contains_key(&event_id)
                        || cache.len() < MAX_DECODED_EVENT_CACHE_ENTRIES
                    {
                        cache.insert(
                            event_id,
                            CachedActivityEvent {
                                stored,
                                event: event.clone(),
                            },
                        );
                    }
                    event
                }
            };
            all.push(event);
        }
        let coverage = coverage_for_events(&db, scope, &all)?;
        let limit = query.limit.unwrap_or(200).min(5_000);
        let filtered: Vec<Arc<ActivityEvent>> = all
            .into_iter()
            .filter(|event| query.matches(event))
            .collect();
        let cursor_position = query
            .cursor
            .as_ref()
            .and_then(|cursor| filtered.iter().position(|event| event.event_id == *cursor));
        let cursor_found = query.cursor.is_none() || cursor_position.is_some();
        let start = cursor_position.map(|position| position + 1).unwrap_or(0);
        let has_more = filtered.len() > start.saturating_add(limit);
        let events: Vec<ActivityEvent> = filtered
            .into_iter()
            .skip(start)
            .take(limit)
            .map(|event| (*event).clone())
            .collect();
        let next_cursor = events.last().map(|event| event.event_id.clone());
        Ok(ActivityQueryResult {
            schema_version: ACTIVITY_SCHEMA_VERSION,
            events,
            completeness: assess_completeness(&coverage),
            coverage,
            next_cursor,
            has_more,
            cursor_found,
        })
    }

    pub fn export(
        &self,
        scope: &IngestScope,
        query: &ActivityQuery,
        actor_id: &str,
    ) -> Result<GovernedExport, ActivityError> {
        let signer = self.export_signer.read().clone().ok_or_else(|| {
            ActivityError::Unavailable("signed activity export is not configured".into())
        })?;
        let result = self.query(scope, query)?;
        let events = result
            .events
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ActivityError::Corrupt(e.to_string()))?;
        let access = AccessContext {
            actor_id: actor_id.to_owned(),
            tenant_id: scope.tenant_id.clone(),
            role: Role::Administrator,
        };
        GovernanceLedger::default()
            .export_metadata(
                &access,
                &scope.tenant_id,
                &scope.collector_id,
                events,
                &signer.key_id,
                &signer.key,
                Utc::now(),
            )
            .map_err(|e| ActivityError::Invalid(e.to_string()))
    }
}

fn encode_stored_event(event: &ActivityEvent) -> Result<Vec<u8>, ActivityError> {
    let json =
        serde_json::to_vec(event).map_err(|error| ActivityError::Corrupt(error.to_string()))?;
    encode_activity_json(&json)
}

fn encode_activity_json(json: &[u8]) -> Result<Vec<u8>, ActivityError> {
    if json.len() > MAX_ACTIVITY_ENVELOPE_BYTES {
        return Err(ActivityError::Invalid(format!(
            "serialized activity event exceeds {MAX_ACTIVITY_ENVELOPE_BYTES} bytes"
        )));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(json)
        .map_err(|error| ActivityError::Corrupt(format!("compress activity envelope: {error}")))?;
    let compressed = encoder
        .finish()
        .map_err(|error| ActivityError::Corrupt(format!("finish activity envelope: {error}")))?;
    let mut envelope = Vec::with_capacity(ACTIVITY_ENVELOPE_HEADER_BYTES + compressed.len());
    envelope.extend_from_slice(ACTIVITY_ENVELOPE_MAGIC);
    envelope.push(ACTIVITY_ENVELOPE_VERSION);
    envelope.extend_from_slice(&(json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&compressed);
    Ok(envelope)
}

fn decode_stored_event(
    stored: SqlValue,
    scope: &IngestScope,
) -> Result<ActivityEvent, ActivityError> {
    let json = match stored {
        SqlValue::Text(json) => json.into_bytes(),
        SqlValue::Blob(envelope) => decode_activity_envelope(&envelope)?,
        other => {
            return Err(ActivityError::Corrupt(format!(
                "unsupported SQLite storage type: {other:?}"
            )))
        }
    };
    if json.len() > MAX_ACTIVITY_ENVELOPE_BYTES {
        return Err(ActivityError::Corrupt(format!(
            "stored activity event exceeds {MAX_ACTIVITY_ENVELOPE_BYTES} decoded bytes"
        )));
    }
    let event: ActivityEvent = serde_json::from_slice(&json)
        .map_err(|error| ActivityError::Corrupt(format!("decode activity JSON: {error}")))?;
    validate_event(scope, &event).map_err(|error| {
        ActivityError::Corrupt(format!("invalid stored activity event: {error}"))
    })?;
    Ok(event)
}

fn decode_activity_envelope(envelope: &[u8]) -> Result<Vec<u8>, ActivityError> {
    if envelope.len() < ACTIVITY_ENVELOPE_HEADER_BYTES {
        return Err(ActivityError::Corrupt(
            "truncated activity envelope header".into(),
        ));
    }
    if &envelope[..ACTIVITY_ENVELOPE_MAGIC.len()] != ACTIVITY_ENVELOPE_MAGIC {
        return Err(ActivityError::Corrupt(
            "malformed activity envelope magic".into(),
        ));
    }
    if envelope[4] != ACTIVITY_ENVELOPE_VERSION {
        return Err(ActivityError::Corrupt(format!(
            "unknown activity envelope version {}",
            envelope[4]
        )));
    }
    let declared = u32::from_be_bytes(envelope[5..9].try_into().expect("fixed header")) as usize;
    if declared > MAX_ACTIVITY_ENVELOPE_BYTES {
        return Err(ActivityError::Corrupt(format!(
            "activity envelope declares {declared} decoded bytes; limit is {MAX_ACTIVITY_ENVELOPE_BYTES}"
        )));
    }
    let compressed = &envelope[ACTIVITY_ENVELOPE_HEADER_BYTES..];
    if compressed.is_empty() {
        return Err(ActivityError::Corrupt(
            "truncated activity envelope stream".into(),
        ));
    }
    let mut decoder = Decompress::new(true);
    let mut json = Vec::with_capacity(declared.saturating_add(1));
    let status = decoder
        .decompress_vec(compressed, &mut json, FlushDecompress::Finish)
        .map_err(|error| ActivityError::Corrupt(format!("decode activity envelope: {error}")))?;
    if status != Status::StreamEnd {
        return Err(ActivityError::Corrupt(
            "truncated or over-limit activity envelope stream".into(),
        ));
    }
    if json.len() > MAX_ACTIVITY_ENVELOPE_BYTES {
        return Err(ActivityError::Corrupt(format!(
            "decoded activity envelope exceeds {MAX_ACTIVITY_ENVELOPE_BYTES} bytes"
        )));
    }
    if json.len() != declared {
        return Err(ActivityError::Corrupt(format!(
            "activity envelope length mismatch: declared {declared}, decoded {}",
            json.len()
        )));
    }
    if decoder.total_in() as usize != compressed.len() {
        return Err(ActivityError::Corrupt(
            "activity envelope contains trailing compressed data".into(),
        ));
    }
    Ok(json)
}

fn validate_event(scope: &IngestScope, event: &ActivityEvent) -> Result<(), ActivityError> {
    if event.schema_version != ACTIVITY_SCHEMA_VERSION {
        return Err(ActivityError::Invalid("unsupported schema_version".into()));
    }
    let event_id = Uuid::parse_str(&event.event_id)
        .map_err(|_| ActivityError::Invalid("event_id must be a UUID".into()))?;
    if event_id.get_version_num() != 7 {
        return Err(ActivityError::Invalid("event_id must be UUIDv7".into()));
    }
    if event.integrity.collector_sequence == 0 {
        return Err(ActivityError::Invalid(
            "collector_sequence must be greater than zero".into(),
        ));
    }
    if event.integrity.collector_sequence > i64::MAX as u64 {
        return Err(ActivityError::Invalid(
            "collector_sequence exceeds the durable store range".into(),
        ));
    }
    if !valid_event_name(&event.event_name) {
        return Err(ActivityError::Invalid("invalid event_name".into()));
    }
    if event
        .source
        .clock_error_ms
        .is_some_and(|v| !v.is_finite() || v < 0.0)
    {
        return Err(ActivityError::Invalid(
            "clock_error_ms must be finite and non-negative".into(),
        ));
    }
    if event.correlation.tenant_id != scope.tenant_id
        || event.correlation.host_id != scope.host_id
        || event.correlation.instance_id != scope.instance_id
        || event.correlation.agent_id != scope.agent_id
        || event.source.collector != scope.collector_id
    {
        return Err(ActivityError::Scope(
            "event tenant/source hierarchy does not match authenticated ingest scope".into(),
        ));
    }
    if event.sensitivity != Sensitivity::Metadata {
        return Err(ActivityError::Invalid(
            "the metadata ingest endpoint does not accept restricted or secret-prohibited content"
                .into(),
        ));
    }
    if let Some(key) = find_prohibited_key(&event.payload) {
        return Err(ActivityError::Invalid(format!(
            "payload key `{key}` is prohibited from metadata storage"
        )));
    }
    validate_hex(&event.correlation.trace_id, 32, "trace_id")?;
    validate_hex(&event.correlation.span_id, 16, "span_id")?;
    if let Some(parent) = &event.correlation.parent_event_id {
        Uuid::parse_str(parent)
            .map_err(|_| ActivityError::Invalid("parent_event_id must be a UUID".into()))?;
    }
    Ok(())
}

fn valid_event_name(name: &str) -> bool {
    let parts: Vec<_> = name.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_lowercase()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        })
}

fn validate_hex(value: &Option<String>, len: usize, name: &str) -> Result<(), ActivityError> {
    if value.as_ref().is_some_and(|v| {
        v.len() != len
            || !v
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }) {
        return Err(ActivityError::Invalid(format!("invalid {name}")));
    }
    Ok(())
}

fn find_prohibited_key(payload: &Map<String, Value>) -> Option<String> {
    const PROHIBITED: &[&str] = &[
        "authorization",
        "cookie",
        "env",
        "environment",
        "file_content",
        "keystrokes",
        "packet_payload",
        "private_key",
        "prompt",
        "secret",
        "token",
    ];
    fn walk(value: &Value, prohibited: &[&str]) -> Option<String> {
        match value {
            Value::Object(map) => map.iter().find_map(|(key, value)| {
                let normalized = key.to_ascii_lowercase();
                if prohibited
                    .iter()
                    .any(|blocked| normalized.contains(blocked))
                {
                    Some(key.clone())
                } else {
                    walk(value, prohibited)
                }
            }),
            Value::Array(values) => values.iter().find_map(|value| walk(value, prohibited)),
            _ => None,
        }
    }
    walk(&Value::Object(payload.clone()), PROHIBITED)
}

fn coverage_for_events(
    db: &Connection,
    scope: &IngestScope,
    events: &[Arc<ActivityEvent>],
) -> Result<Vec<CollectorCoverage>, ActivityError> {
    let mut by_collector: BTreeMap<String, Vec<&ActivityEvent>> = BTreeMap::new();
    for event in events {
        by_collector
            .entry(event.source.collector.clone())
            .or_default()
            .push(event.as_ref());
    }
    let mut result = Vec::new();
    for (collector, mut items) in by_collector {
        items.sort_by_key(|event| event.integrity.collector_sequence);
        let sequences: BTreeSet<u64> = items
            .iter()
            .map(|event| event.integrity.collector_sequence)
            .collect();
        let first = sequences.first().copied();
        let last = sequences.last().copied();
        let mut gaps = Vec::new();
        if first.is_some() {
            let mut previous = 0_u64;
            for sequence in sequences.iter().copied() {
                if sequence > previous.saturating_add(1) {
                    gaps.push(SequenceGap {
                        collector_id: collector.clone(),
                        first_missing_sequence: previous + 1,
                        last_missing_sequence: sequence - 1,
                    });
                }
                previous = sequence;
            }
        }
        let mut stmt = db.prepare(
            "SELECT first_missing_sequence, last_missing_sequence
             FROM activity_loss_records
             WHERE tenant_id = ?1 AND host_id = ?2 AND instance_id = ?3
               AND agent_id = ?4 AND collector_id = ?5
             ORDER BY first_missing_sequence",
        )?;
        let durable_loss_records = stmt
            .query_map(
                params![
                    scope.tenant_id,
                    scope.host_id,
                    scope.instance_id,
                    scope.agent_id,
                    collector
                ],
                |row| {
                    Ok(SequenceGap {
                        collector_id: collector.clone(),
                        first_missing_sequence: row.get(0)?,
                        last_missing_sequence: row.get(1)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let maximum_clock_error_ms = items.iter().fold(0.0_f64, |maximum, event| {
            let observed_delta = (event.observed_at - event.occurred_at)
                .num_milliseconds()
                .unsigned_abs() as f64;
            maximum.max(observed_delta.max(event.source.clock_error_ms.unwrap_or(0.0)))
        });
        result.push(CollectorCoverage {
            collector_id: collector,
            first_sequence: first,
            last_sequence: last,
            event_count: items.len(),
            sequence_gaps: gaps,
            durable_loss_records,
            maximum_clock_error_ms,
            last_observed_at: items.iter().map(|event| event.observed_at).max(),
            restart_count: items
                .iter()
                .filter(|event| event.event_name == "collector.restarted")
                .count(),
            dropped_event_count: items
                .iter()
                .filter_map(|event| event.payload.get("dropped_events").and_then(Value::as_u64))
                .sum(),
            unsupported_event_classes: items
                .iter()
                .filter_map(|event| {
                    event
                        .payload
                        .get("unsupported_event_class")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            stale: items
                .iter()
                .map(|event| event.observed_at)
                .max()
                .is_none_or(|last| Utc::now().signed_duration_since(last).num_seconds() > 300),
        });
    }
    Ok(result)
}

fn assess_completeness(coverage: &[CollectorCoverage]) -> CompletenessAssessment {
    let unsupported_event_classes = coverage
        .iter()
        .flat_map(|item| item.unsupported_event_classes.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let sequence_gap_count = coverage.iter().map(|item| item.sequence_gaps.len()).sum();
    let durable_loss_count = coverage
        .iter()
        .map(|item| item.durable_loss_records.len())
        .sum();
    let restart_count = coverage.iter().map(|item| item.restart_count).sum();
    let dropped_event_count = coverage.iter().map(|item| item.dropped_event_count).sum();
    let stale_collector_count = coverage.iter().filter(|item| item.stale).count();
    let maximum_clock_error_ms = coverage
        .iter()
        .fold(0.0_f64, |max, item| max.max(item.maximum_clock_error_ms));
    let complete = !coverage.is_empty()
        && sequence_gap_count == 0
        && durable_loss_count == 0
        && restart_count == 0
        && dropped_event_count == 0
        && stale_collector_count == 0
        && unsupported_event_classes.is_empty();
    CompletenessAssessment {
        complete,
        label: if complete {
            "complete"
        } else {
            "incomplete-or-unknown"
        },
        collector_count: coverage.len(),
        sequence_gap_count,
        durable_loss_count,
        restart_count,
        dropped_event_count,
        stale_collector_count,
        unsupported_event_classes,
        maximum_clock_error_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn event(id: &str, sequence: u64) -> ActivityEvent {
        serde_json::from_value(serde_json::json!({
            "schema_version": "activity.event/v1",
            "event_id": id,
            "event_name": "process.exec",
            "plane": "action",
            "occurred_at": "2026-08-01T12:00:00Z",
            "observed_at": "2026-08-01T12:00:00.004Z",
            "source": {"collector":"collector-a","layer":"guest","runtime":"qemu-kvm","trust":"observed"},
            "correlation": {"tenant_id":"tenant-a","host_id":"host-a","instance_id":"instance-a","agent_id":"agent-a"},
            "sensitivity": "metadata",
            "retention_class": "security",
            "payload": {"executable_digest":"sha256:abc"},
            "integrity": {"collector_sequence": sequence}
        })).unwrap()
    }

    fn scope() -> IngestScope {
        IngestScope {
            tenant_id: "tenant-a".into(),
            host_id: "host-a".into(),
            instance_id: "instance-a".into(),
            agent_id: "agent-a".into(),
            collector_id: "collector-a".into(),
        }
    }

    fn insert_legacy_text(store: &ActivityStore, item: &ActivityEvent) {
        let json = serde_json::to_string(item).unwrap();
        store
            .db
            .lock()
            .execute(
                "INSERT INTO activity_events
                 (event_id, tenant_id, host_id, instance_id, agent_id, collector_id,
                  collector_sequence, occurred_at, observed_at, event_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    item.event_id,
                    item.correlation.tenant_id,
                    item.correlation.host_id,
                    item.correlation.instance_id,
                    item.correlation.agent_id,
                    item.source.collector,
                    item.integrity.collector_sequence,
                    item.occurred_at.to_rfc3339(),
                    item.observed_at.to_rfc3339(),
                    json,
                ],
            )
            .unwrap();
    }

    #[test]
    fn durable_ack_is_idempotent_and_survives_restart() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("activity.db");
        let first = ActivityStore::open(&path).unwrap();
        let e = event("0198f5f0-0000-7000-8000-000000000001", 1);
        let ack = first
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![e.clone()],
                },
            )
            .unwrap();
        assert_eq!(
            (ack.accepted, ack.duplicates, ack.durable_through_sequence),
            (1, 0, 1)
        );
        drop(first);
        let reopened = ActivityStore::open(&path).unwrap();
        let ack = reopened
            .ingest(&scope(), IngestBatch { events: vec![e] })
            .unwrap();
        assert_eq!(
            (ack.accepted, ack.duplicates, ack.durable_through_sequence),
            (0, 1, 1)
        );
    }

    #[test]
    fn compressed_and_legacy_rows_share_query_duplicate_and_export_contracts() {
        let store = ActivityStore::in_memory().unwrap();
        let legacy = event("0198f5f0-0000-7000-8000-000000000001", 1);
        let modern = event("0198f5f0-0001-7000-8000-000000000002", 2);
        insert_legacy_text(&store, &legacy);

        let ack = store
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![modern.clone()],
                },
            )
            .unwrap();
        assert_eq!(ack.durable_through_sequence, 2);
        let db = store.db.lock();
        let (storage_type, envelope): (String, Vec<u8>) = db
            .query_row(
                "SELECT typeof(event_json), event_json FROM activity_events WHERE event_id = ?1",
                params![modern.event_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        drop(db);
        assert_eq!(storage_type, "blob");
        assert!(envelope.starts_with(ACTIVITY_ENVELOPE_MAGIC));
        assert_eq!(envelope[4], ACTIVITY_ENVELOPE_VERSION);

        let duplicate = store
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![legacy.clone()],
                },
            )
            .unwrap();
        assert_eq!((duplicate.accepted, duplicate.duplicates), (0, 1));

        let result = store.query(&scope(), &ActivityQuery::default()).unwrap();
        assert_eq!(result.events, vec![legacy, modern]);
        store
            .configure_export_signer("mixed-representation-test", vec![0x55; 32])
            .unwrap();
        let export = store
            .export(&scope(), &ActivityQuery::default(), "operator")
            .unwrap();
        assert_eq!(export.manifest.event_count, 2);
    }

    #[test]
    fn corrupt_unknown_truncated_oversized_and_invalid_envelopes_fail_typed() {
        let valid = encode_stored_event(&event("0198f5f0-0000-7000-8000-000000000001", 1)).unwrap();
        let mut unknown = valid.clone();
        unknown[4] = ACTIVITY_ENVELOPE_VERSION + 1;
        let mut oversized = Vec::from(ACTIVITY_ENVELOPE_MAGIC.as_slice());
        oversized.push(ACTIVITY_ENVELOPE_VERSION);
        oversized.extend_from_slice(&((MAX_ACTIVITY_ENVELOPE_BYTES as u32) + 1).to_be_bytes());
        oversized.push(0);
        let invalid_schema =
            encode_activity_json(br#"{"schema_version":"activity.event/v1"}"#).unwrap();
        let mut trailing = valid.clone();
        trailing.extend_from_slice(b"trailing");
        let cases = vec![
            vec![0_u8; 3],
            unknown,
            valid[..valid.len() - 2].to_vec(),
            oversized,
            invalid_schema,
            trailing,
        ];

        for (case, envelope) in cases.into_iter().enumerate() {
            let store = ActivityStore::in_memory().unwrap();
            let item = event("0198f5f0-0000-7000-8000-000000000001", 1);
            store
                .ingest(
                    &scope(),
                    IngestBatch {
                        events: vec![item.clone()],
                    },
                )
                .unwrap();
            assert_eq!(
                store
                    .query(&scope(), &ActivityQuery::default())
                    .unwrap()
                    .events
                    .len(),
                1
            );
            store
                .db
                .lock()
                .execute(
                    "UPDATE activity_events SET event_json = ?1 WHERE event_id = ?2",
                    params![envelope, item.event_id],
                )
                .unwrap();
            let result = store.query(&scope(), &ActivityQuery::default());
            assert!(
                matches!(result, Err(ActivityError::Corrupt(_))),
                "corrupt envelope case {case} unexpectedly returned {result:?}"
            );
        }
    }

    #[test]
    fn oversized_new_event_is_rejected_before_storage() {
        let store = ActivityStore::in_memory().unwrap();
        let mut item = event("0198f5f0-0000-7000-8000-000000000001", 1);
        item.payload.insert(
            "bounded_metadata".into(),
            Value::String("x".repeat(MAX_ACTIVITY_ENVELOPE_BYTES)),
        );
        assert!(matches!(
            store.ingest(&scope(), IngestBatch { events: vec![item] }),
            Err(ActivityError::Invalid(_))
        ));
        let count: u64 = store
            .db
            .lock()
            .query_row("SELECT COUNT(*) FROM activity_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        let mut legacy = event("0198f5f0-0001-7000-8000-000000000002", 1);
        legacy.payload.insert(
            "bounded_metadata".into(),
            Value::String("x".repeat(MAX_ACTIVITY_ENVELOPE_BYTES)),
        );
        insert_legacy_text(&store, &legacy);
        assert!(matches!(
            store.query(&scope(), &ActivityQuery::default()),
            Err(ActivityError::Corrupt(_))
        ));
    }

    #[test]
    fn records_and_returns_sequence_loss_and_clock_bounds() {
        let store = ActivityStore::in_memory().unwrap();
        let ack = store
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![
                        event("0198f5f0-0000-7000-8000-000000000001", 1),
                        event("0198f5f0-0002-7000-8000-000000000003", 3),
                    ],
                },
            )
            .unwrap();
        assert_eq!(ack.durable_through_sequence, 1);
        assert_eq!(ack.sequence_gaps_recorded[0].first_missing_sequence, 2);
        let result = store.query(&scope(), &ActivityQuery::default()).unwrap();
        assert_eq!(result.events.len(), 2);
        assert_eq!(
            result.coverage[0].sequence_gaps[0].first_missing_sequence,
            2
        );
        assert_eq!(
            result.coverage[0].durable_loss_records[0].last_missing_sequence,
            2
        );
        assert_eq!(result.coverage[0].maximum_clock_error_ms, 4.0);

        let ack = store
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![event("0198f5f0-0001-7000-8000-000000000002", 2)],
                },
            )
            .unwrap();
        assert_eq!(ack.durable_through_sequence, 3);
        assert!(ack.sequence_gaps_recorded.is_empty());
    }

    #[test]
    fn rejects_event_id_and_sequence_rebinding() {
        let store = ActivityStore::in_memory().unwrap();
        let original = event("0198f5f0-0000-7000-8000-000000000001", 1);
        store
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![original.clone()],
                },
            )
            .unwrap();

        let mut rebound_id = original.clone();
        rebound_id.event_name = "process.exit".into();
        assert!(matches!(
            store.ingest(
                &scope(),
                IngestBatch {
                    events: vec![rebound_id]
                }
            ),
            Err(ActivityError::Invalid(_))
        ));

        let rebound_sequence = event("0198f5f0-0001-7000-8000-000000000002", 1);
        assert!(matches!(
            store.ingest(
                &scope(),
                IngestBatch {
                    events: vec![rebound_sequence]
                }
            ),
            Err(ActivityError::Invalid(_))
        ));
    }

    #[test]
    fn sparse_sequence_coverage_does_not_scan_the_missing_range() {
        let store = ActivityStore::in_memory().unwrap();
        let sequence = i64::MAX as u64;
        let ack = store
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![event("0198f5f0-0000-7000-8000-000000000001", sequence)],
                },
            )
            .unwrap();
        assert_eq!(ack.durable_through_sequence, 0);
        assert_eq!(
            ack.sequence_gaps_recorded[0].last_missing_sequence,
            sequence - 1
        );
        let result = store.query(&scope(), &ActivityQuery::default()).unwrap();
        assert_eq!(result.coverage[0].last_sequence, Some(sequence));
        assert_eq!(
            result.coverage[0].sequence_gaps[0].last_missing_sequence,
            sequence - 1
        );

        let mut too_large = event("0198f5f0-0001-7000-8000-000000000002", 1);
        too_large.integrity.collector_sequence = sequence + 1;
        assert!(matches!(
            store.ingest(
                &scope(),
                IngestBatch {
                    events: vec![too_large]
                }
            ),
            Err(ActivityError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_scope_spoofing_and_secret_bearing_metadata() {
        let store = ActivityStore::in_memory().unwrap();
        let mut spoofed = event("0198f5f0-0000-7000-8000-000000000001", 1);
        spoofed.correlation.tenant_id = "tenant-b".into();
        assert!(matches!(
            store.ingest(
                &scope(),
                IngestBatch {
                    events: vec![spoofed]
                }
            ),
            Err(ActivityError::Scope(_))
        ));

        let mut secret = event("0198f5f0-0001-7000-8000-000000000002", 1);
        secret.payload.insert(
            "authorization_header".into(),
            Value::String("sentinel".into()),
        );
        assert!(matches!(
            store.ingest(
                &scope(),
                IngestBatch {
                    events: vec![secret]
                }
            ),
            Err(ActivityError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_unknown_sensitivity_and_retention_classes_at_decode() {
        let mut value =
            serde_json::to_value(event("0198f5f0-0000-7000-8000-000000000001", 1)).unwrap();
        value["sensitivity"] = Value::String("top-secret".into());
        assert!(serde_json::from_value::<ActivityEvent>(value).is_err());
    }

    #[test]
    fn stable_conformance_fixture_covers_every_plane_and_runtime() {
        let fixture = include_str!("../tests/fixtures/activity-event-v1/all-planes-runtimes.jsonl");
        let events: Vec<ActivityEvent> = fixture
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 6);
        let planes: BTreeSet<String> = events
            .iter()
            .map(|event| {
                serde_json::to_value(event.plane)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let runtimes: BTreeSet<String> = events
            .iter()
            .map(|event| {
                serde_json::to_value(event.source.runtime)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(planes.len(), 6);
        assert_eq!(runtimes.len(), 5);
    }

    #[test]
    fn timeline_filters_outcome_trust_and_collector() {
        let store = ActivityStore::in_memory().unwrap();
        let mut denied = event("0198f5f0-0000-7000-8000-000000000001", 1);
        denied.outcome = Some(ActivityOutcome {
            status: OutcomeStatus::Denied,
            exit_code: None,
            reason: Some("policy".into()),
        });
        store
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![denied],
                },
            )
            .unwrap();
        let result = store
            .query(
                &scope(),
                &ActivityQuery {
                    collector: Some("collector-a".into()),
                    trust: Some(SourceTrust::Observed),
                    outcome: Some(OutcomeStatus::Denied),
                    ..ActivityQuery::default()
                },
            )
            .unwrap();
        assert_eq!(result.events.len(), 1);
        assert!(!result.completeness.complete);
        assert!(result.completeness.maximum_clock_error_ms >= 4.0);
    }

    #[test]
    fn timeline_pages_by_stable_event_cursor() {
        let store = ActivityStore::in_memory().unwrap();
        let ids = [
            "0198f5f0-0000-7000-8000-000000000001",
            "0198f5f0-0000-7000-8000-000000000002",
            "0198f5f0-0000-7000-8000-000000000003",
        ];
        store
            .ingest(
                &scope(),
                IngestBatch {
                    events: ids
                        .iter()
                        .enumerate()
                        .map(|(index, id)| event(id, index as u64 + 1))
                        .collect(),
                },
            )
            .unwrap();

        let first = store
            .query(
                &scope(),
                &ActivityQuery {
                    limit: Some(2),
                    ..ActivityQuery::default()
                },
            )
            .unwrap();
        assert_eq!(first.events.len(), 2);
        assert!(first.has_more);
        assert_eq!(first.next_cursor.as_deref(), Some(ids[1]));

        let second = store
            .query(
                &scope(),
                &ActivityQuery {
                    cursor: first.next_cursor,
                    limit: Some(2),
                    ..ActivityQuery::default()
                },
            )
            .unwrap();
        assert!(second.cursor_found);
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].event_id, ids[2]);
        assert!(!second.has_more);

        let missing = store
            .query(
                &scope(),
                &ActivityQuery {
                    cursor: Some("0198f5f0-0000-7000-8000-ffffffffffff".into()),
                    ..ActivityQuery::default()
                },
            )
            .unwrap();
        assert!(!missing.cursor_found);
    }

    #[test]
    fn signed_export_fails_closed_then_verifies() {
        let store = ActivityStore::in_memory().unwrap();
        store
            .ingest(
                &scope(),
                IngestBatch {
                    events: vec![event("0198f5f0-0000-7000-8000-000000000001", 1)],
                },
            )
            .unwrap();
        assert!(matches!(
            store.export(&scope(), &ActivityQuery::default(), "operator"),
            Err(ActivityError::Unavailable(_))
        ));

        let key = vec![7_u8; 32];
        store
            .configure_export_signer("activity-export-test", key.clone())
            .unwrap();
        let export = store
            .export(&scope(), &ActivityQuery::default(), "operator")
            .unwrap();
        let anchor = export.manifest.checkpoint("test-anchor", Utc::now());
        export
            .manifest
            .verify(&export.events, &key, &anchor)
            .unwrap();
        assert_eq!(export.manifest.key_id, "activity-export-test");
    }
}
