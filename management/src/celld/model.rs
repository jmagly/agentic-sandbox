use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const MAX_EFFECTS: usize = 1_000;
const MAX_HISTORY: usize = 10_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Requested,
    Provisioning,
    Enrolling,
    Ready,
    Stopping,
    Stopped,
    Failed,
    Unknown,
    Destroyed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CellAction {
    Provision,
    Start,
    Stop,
    Destroy,
    Observe,
    Repair,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    Pending,
    Dispatched,
    Unknown,
    Succeeded,
    Failed,
    Rejected,
}

impl EffectStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Rejected)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CellCommand {
    pub document_type: String,
    pub schema_version: String,
    pub operation_id: String,
    pub instance_id: String,
    pub generation: u64,
    pub action: CellAction,
    pub request_hash: String,
    pub issued_at: DateTime<Utc>,
    #[serde(default)]
    pub payload: Map<String, Value>,
}

impl CellCommand {
    pub fn new(
        operation_id: impl Into<String>,
        instance_id: impl Into<String>,
        generation: u64,
        action: CellAction,
        payload: Value,
    ) -> Result<Self, CellError> {
        let operation_id = operation_id.into();
        let instance_id = instance_id.into();
        let payload = payload
            .as_object()
            .cloned()
            .ok_or(CellError::PayloadNotObject)?;
        let request_hash =
            canonical_request_hash(&operation_id, &instance_id, generation, action, &payload)?;
        Ok(Self {
            document_type: "instance-cell-command".into(),
            schema_version: "1".into(),
            operation_id,
            instance_id,
            generation,
            action,
            request_hash,
            issued_at: Utc::now(),
            payload,
        })
    }

    pub fn verify_hash(&self) -> Result<(), CellError> {
        let actual = canonical_request_hash(
            &self.operation_id,
            &self.instance_id,
            self.generation,
            self.action,
            &self.payload,
        )?;
        if actual != self.request_hash {
            return Err(CellError::RequestHashMismatch);
        }
        Ok(())
    }
}

fn canonical_request_hash(
    operation_id: &str,
    instance_id: &str,
    generation: u64,
    action: CellAction,
    payload: &Map<String, Value>,
) -> Result<String, CellError> {
    let canonical = serde_jcs::to_vec(&json!({
        "operation_id": operation_id,
        "instance_id": instance_id,
        "generation": generation,
        "action": action,
        "payload": payload,
    }))
    .map_err(|error| CellError::Canonicalization(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagementObservation {
    pub generation: u64,
    pub runtime_state: String,
    pub agent_state: String,
    #[serde(default)]
    pub runtime_id: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectRecord {
    pub operation_id: String,
    pub request_hash: String,
    pub action: CellAction,
    pub generation: u64,
    pub status: EffectStatus,
    pub attempts: u32,
    #[serde(default)]
    pub retry_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub terminal_code: Option<String>,
    #[serde(default)]
    pub management_operation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CellEventKind {
    CommandAccepted,
    EffectDispatched,
    EffectUnknown,
    EffectTerminal,
    ObservationRecorded,
    DivergenceDetected,
    RepairApplied,
    Tombstoned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CellEvent {
    pub document_type: String,
    pub schema_version: String,
    pub event_id: String,
    pub instance_id: String,
    #[serde(default)]
    pub operation_id: Option<String>,
    pub generation: u64,
    pub sequence: u64,
    pub kind: CellEventKind,
    #[serde(default)]
    pub from_state: Option<LifecycleState>,
    #[serde(default)]
    pub to_state: Option<LifecycleState>,
    #[serde(default)]
    pub code: Option<String>,
    pub recorded_at: DateTime<Utc>,
    #[serde(default)]
    pub evidence: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstanceCell {
    pub document_type: String,
    pub schema_version: String,
    pub instance_id: String,
    pub generation: u64,
    pub desired_state: LifecycleState,
    #[serde(default)]
    pub observation: Option<ManagementObservation>,
    #[serde(default)]
    pub effects: Vec<EffectRecord>,
    pub history_sequence: u64,
    #[serde(default)]
    pub tombstone_until: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub history: Vec<CellEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandDisposition {
    Accepted,
    Replayed { status: EffectStatus },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileClassification {
    Converged,
    EffectPending,
    OutcomeUnknown,
    DesiredObservedDivergence,
    CellGenerationStale,
    ObservationStale,
    Tombstoned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReconcileResult {
    pub classification: ReconcileClassification,
    pub instance_id: String,
    pub generation: u64,
    pub desired_state: LifecycleState,
    pub observed_runtime_state: Option<String>,
    pub outstanding_operations: Vec<String>,
    pub repair: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CellError {
    #[error("instance id is required")]
    MissingInstanceId,
    #[error("operation id is required")]
    MissingOperationId,
    #[error("generation must be positive")]
    InvalidGeneration,
    #[error("command instance id does not match cell")]
    InstanceMismatch,
    #[error("stale generation: command {command}, current {current}")]
    StaleGeneration { command: u64, current: u64 },
    #[error("future generation: command {command}, current {current}")]
    FutureGeneration { command: u64, current: u64 },
    #[error("operation id is already bound to another request")]
    OperationCollision,
    #[error("request hash does not match canonical command")]
    RequestHashMismatch,
    #[error("command payload must be a JSON object")]
    PayloadNotObject,
    #[error("canonicalization failed: {0}")]
    Canonicalization(String),
    #[error("invalid transition: {state:?} cannot accept {action:?}")]
    InvalidTransition {
        state: LifecycleState,
        action: CellAction,
    },
    #[error("operation not found: {0}")]
    OperationNotFound(String),
    #[error("operation is already terminal: {0}")]
    OperationTerminal(String),
    #[error("operation {operation_id} belongs to fenced generation {generation}; current generation is {current}")]
    EffectGenerationFenced {
        operation_id: String,
        generation: u64,
        current: u64,
    },
    #[error("effect ledger limit reached")]
    EffectLimit,
    #[error("management observation generation is stale")]
    StaleObservation,
}

impl InstanceCell {
    pub fn new(instance_id: impl Into<String>, generation: u64) -> Result<Self, CellError> {
        let instance_id = instance_id.into();
        if instance_id.trim().is_empty() {
            return Err(CellError::MissingInstanceId);
        }
        if generation == 0 {
            return Err(CellError::InvalidGeneration);
        }
        Ok(Self {
            document_type: "instance-cell-state".into(),
            schema_version: "1".into(),
            instance_id,
            generation,
            desired_state: LifecycleState::Requested,
            observation: None,
            effects: Vec::new(),
            history_sequence: 0,
            tombstone_until: None,
            updated_at: Utc::now(),
            history: Vec::new(),
        })
    }

    pub fn accept(&mut self, command: &CellCommand) -> Result<CommandDisposition, CellError> {
        validate_identifier(&command.operation_id).map_err(|_| CellError::MissingOperationId)?;
        command.verify_hash()?;
        if command.instance_id != self.instance_id {
            return Err(CellError::InstanceMismatch);
        }
        if command.generation < self.generation {
            return Err(CellError::StaleGeneration {
                command: command.generation,
                current: self.generation,
            });
        }
        if command.generation > self.generation {
            return Err(CellError::FutureGeneration {
                command: command.generation,
                current: self.generation,
            });
        }
        if let Some(effect) = self
            .effects
            .iter()
            .find(|effect| effect.operation_id == command.operation_id)
        {
            if effect.request_hash != command.request_hash {
                return Err(CellError::OperationCollision);
            }
            return Ok(CommandDisposition::Replayed {
                status: effect.status,
            });
        }
        if self.effects.len() >= MAX_EFFECTS {
            return Err(CellError::EffectLimit);
        }
        let next_state = next_desired_state(self.desired_state, command.action)?;
        let previous = self.desired_state;
        self.desired_state = next_state;
        self.effects.push(EffectRecord {
            operation_id: command.operation_id.clone(),
            request_hash: command.request_hash.clone(),
            action: command.action,
            generation: command.generation,
            status: EffectStatus::Pending,
            attempts: 0,
            retry_at: None,
            terminal_code: None,
            management_operation_id: None,
        });
        self.record_event(
            Some(&command.operation_id),
            CellEventKind::CommandAccepted,
            Some(previous),
            Some(next_state),
            None,
            json!({"request_hash": command.request_hash, "action": command.action}),
        );
        Ok(CommandDisposition::Accepted)
    }

    pub fn record_dispatched(
        &mut self,
        operation_id: &str,
        management_operation_id: Option<String>,
    ) -> Result<(), CellError> {
        let current_generation = self.generation;
        let effect = self.effect_mut(operation_id)?;
        if effect.generation < current_generation {
            return Err(CellError::EffectGenerationFenced {
                operation_id: operation_id.into(),
                generation: effect.generation,
                current: current_generation,
            });
        }
        if effect.status.is_terminal() {
            return Err(CellError::OperationTerminal(operation_id.into()));
        }
        effect.status = EffectStatus::Dispatched;
        effect.attempts = effect.attempts.saturating_add(1);
        effect.management_operation_id = management_operation_id;
        effect.retry_at = None;
        self.record_event(
            Some(operation_id),
            CellEventKind::EffectDispatched,
            None,
            None,
            None,
            Value::Null,
        );
        Ok(())
    }

    pub fn record_unknown(
        &mut self,
        operation_id: &str,
        retry_after: Duration,
        code: impl Into<String>,
    ) -> Result<(), CellError> {
        let code = code.into();
        let retry_at = Utc::now() + retry_after;
        let current_generation = self.generation;
        let effect = self.effect_mut(operation_id)?;
        if effect.generation < current_generation {
            return Err(CellError::EffectGenerationFenced {
                operation_id: operation_id.into(),
                generation: effect.generation,
                current: current_generation,
            });
        }
        if effect.status.is_terminal() {
            return Err(CellError::OperationTerminal(operation_id.into()));
        }
        effect.status = EffectStatus::Unknown;
        effect.retry_at = Some(retry_at);
        self.desired_state = LifecycleState::Unknown;
        self.record_event(
            Some(operation_id),
            CellEventKind::EffectUnknown,
            None,
            Some(LifecycleState::Unknown),
            Some(code),
            json!({"retry_at": retry_at}),
        );
        Ok(())
    }

    pub fn record_terminal(
        &mut self,
        operation_id: &str,
        status: EffectStatus,
        code: Option<String>,
    ) -> Result<(), CellError> {
        if !status.is_terminal() {
            return Err(CellError::InvalidTransition {
                state: self.desired_state,
                action: CellAction::Repair,
            });
        }
        let previous = self.desired_state;
        let action = {
            let effect = self.effect_mut(operation_id)?;
            if effect.status.is_terminal() {
                if effect.status == status && effect.terminal_code == code {
                    return Ok(());
                }
                return Err(CellError::OperationTerminal(operation_id.into()));
            }
            effect.status = status;
            effect.retry_at = None;
            effect.terminal_code = code.clone();
            effect.action
        };
        self.desired_state = terminal_desired_state(action, status, self.desired_state);
        if self.desired_state == LifecycleState::Destroyed {
            self.tombstone_until = Some(Utc::now() + Duration::days(30));
        }
        self.record_event(
            Some(operation_id),
            if self.desired_state == LifecycleState::Destroyed {
                CellEventKind::Tombstoned
            } else {
                CellEventKind::EffectTerminal
            },
            Some(previous),
            Some(self.desired_state),
            code,
            json!({"status": status}),
        );
        Ok(())
    }

    pub fn observe(&mut self, observation: ManagementObservation) -> Result<(), CellError> {
        if observation.generation < self.generation {
            return Err(CellError::StaleObservation);
        }
        let previous = self.desired_state;
        if observation.generation > self.generation {
            let new_generation = observation.generation;
            let fenced = self
                .effects
                .iter_mut()
                .filter(|effect| effect.generation < new_generation && !effect.status.is_terminal())
                .map(|effect| {
                    effect.status = EffectStatus::Rejected;
                    effect.retry_at = None;
                    effect.terminal_code = Some("celld.stale_generation_fenced".into());
                    (effect.operation_id.clone(), effect.generation)
                })
                .collect::<Vec<_>>();
            self.generation = observation.generation;
            self.desired_state = LifecycleState::Unknown;
            for (operation_id, effect_generation) in fenced {
                self.record_event(
                    Some(&operation_id),
                    CellEventKind::EffectTerminal,
                    None,
                    Some(LifecycleState::Unknown),
                    Some("celld.stale_generation_fenced".into()),
                    json!({
                        "effect_generation": effect_generation,
                        "observed_generation": new_generation,
                        "status": EffectStatus::Rejected,
                    }),
                );
            }
        } else if observation.runtime_state == "ready" && observation.agent_state == "ready" {
            self.desired_state = LifecycleState::Ready;
        } else if observation.runtime_state == "stopped"
            && matches!(
                self.desired_state,
                LifecycleState::Stopping | LifecycleState::Unknown
            )
        {
            self.desired_state = LifecycleState::Stopped;
        } else if observation.runtime_state == "destroyed" {
            self.desired_state = LifecycleState::Destroyed;
            self.tombstone_until
                .get_or_insert(Utc::now() + Duration::days(30));
        }
        let observed_generation = observation.generation;
        let runtime_state = observation.runtime_state.clone();
        let agent_state = observation.agent_state.clone();
        self.observation = Some(observation);
        self.record_event(
            None,
            CellEventKind::ObservationRecorded,
            Some(previous),
            Some(self.desired_state),
            None,
            json!({
                "observed_generation": observed_generation,
                "runtime_state": runtime_state,
                "agent_state": agent_state,
            }),
        );
        Ok(())
    }

    pub fn reconcile(&mut self, management_generation: u64) -> ReconcileResult {
        let outstanding_operations = self
            .effects
            .iter()
            .filter(|effect| effect.generation == self.generation && !effect.status.is_terminal())
            .map(|effect| effect.operation_id.clone())
            .collect::<Vec<_>>();
        let observed_runtime_state = self
            .observation
            .as_ref()
            .map(|observation| observation.runtime_state.clone());
        let (classification, repair) = if self.desired_state == LifecycleState::Destroyed {
            (ReconcileClassification::Tombstoned, None)
        } else if self.generation < management_generation {
            (
                ReconcileClassification::CellGenerationStale,
                Some("refresh_cell_binding_from_management".into()),
            )
        } else if self.generation > management_generation {
            (
                ReconcileClassification::ObservationStale,
                Some("refresh_management_inventory_without_effect".into()),
            )
        } else if self.effects.iter().any(|effect| {
            effect.generation == self.generation && effect.status == EffectStatus::Unknown
        }) {
            (
                ReconcileClassification::OutcomeUnknown,
                Some("lookup_original_operations_and_inventory".into()),
            )
        } else if !outstanding_operations.is_empty() {
            (
                ReconcileClassification::EffectPending,
                Some("dispatch_pending_effects_by_original_operation_id".into()),
            )
        } else if observation_matches_desired(self.desired_state, self.observation.as_ref()) {
            (ReconcileClassification::Converged, None)
        } else {
            (
                ReconcileClassification::DesiredObservedDivergence,
                Some("apply_generation_safe_repair_operation".into()),
            )
        };
        if classification != ReconcileClassification::Converged {
            self.record_event(
                None,
                CellEventKind::DivergenceDetected,
                None,
                None,
                Some(format!("{classification:?}").to_ascii_lowercase()),
                json!({"management_generation": management_generation}),
            );
        }
        ReconcileResult {
            classification,
            instance_id: self.instance_id.clone(),
            generation: self.generation,
            desired_state: self.desired_state,
            observed_runtime_state,
            outstanding_operations,
            repair,
        }
    }

    pub fn validate_invariants(&self) -> Result<(), CellError> {
        if self.instance_id.trim().is_empty() {
            return Err(CellError::MissingInstanceId);
        }
        if self.generation == 0 {
            return Err(CellError::InvalidGeneration);
        }
        let mut ids = BTreeSet::new();
        for effect in &self.effects {
            if !ids.insert(&effect.operation_id) {
                return Err(CellError::OperationCollision);
            }
            if effect.generation > self.generation {
                return Err(CellError::FutureGeneration {
                    command: effect.generation,
                    current: self.generation,
                });
            }
        }
        if self.desired_state == LifecycleState::Destroyed && self.tombstone_until.is_none() {
            return Err(CellError::InvalidTransition {
                state: self.desired_state,
                action: CellAction::Destroy,
            });
        }
        Ok(())
    }

    fn effect_mut(&mut self, operation_id: &str) -> Result<&mut EffectRecord, CellError> {
        self.effects
            .iter_mut()
            .find(|effect| effect.operation_id == operation_id)
            .ok_or_else(|| CellError::OperationNotFound(operation_id.into()))
    }

    fn record_event(
        &mut self,
        operation_id: Option<&str>,
        kind: CellEventKind,
        from_state: Option<LifecycleState>,
        to_state: Option<LifecycleState>,
        code: Option<String>,
        evidence: Value,
    ) {
        self.history_sequence = self.history_sequence.saturating_add(1);
        self.updated_at = Utc::now();
        self.history.push(CellEvent {
            document_type: "instance-cell-event".into(),
            schema_version: "1".into(),
            event_id: format!("evt_{}", uuid::Uuid::now_v7().simple()),
            instance_id: self.instance_id.clone(),
            operation_id: operation_id.map(str::to_string),
            generation: self.generation,
            sequence: self.history_sequence,
            kind,
            from_state,
            to_state,
            code,
            recorded_at: self.updated_at,
            evidence: match evidence {
                Value::Object(evidence) => evidence,
                _ => Map::new(),
            },
        });
        if self.history.len() > MAX_HISTORY {
            let remove = self.history.len() - MAX_HISTORY;
            self.history.drain(0..remove);
        }
    }
}

fn validate_identifier(value: &str) -> Result<(), ()> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
    valid.then_some(()).ok_or(())
}

fn next_desired_state(
    current: LifecycleState,
    action: CellAction,
) -> Result<LifecycleState, CellError> {
    let next = match action {
        CellAction::Provision
            if matches!(
                current,
                LifecycleState::Requested | LifecycleState::Stopped | LifecycleState::Failed
            ) =>
        {
            LifecycleState::Provisioning
        }
        CellAction::Start
            if matches!(current, LifecycleState::Stopped | LifecycleState::Failed) =>
        {
            LifecycleState::Provisioning
        }
        CellAction::Stop
            if matches!(
                current,
                LifecycleState::Provisioning
                    | LifecycleState::Enrolling
                    | LifecycleState::Ready
                    | LifecycleState::Unknown
            ) =>
        {
            LifecycleState::Stopping
        }
        CellAction::Destroy if current != LifecycleState::Destroyed => LifecycleState::Stopping,
        CellAction::Observe => current,
        CellAction::Repair if current != LifecycleState::Destroyed => LifecycleState::Unknown,
        _ => {
            return Err(CellError::InvalidTransition {
                state: current,
                action,
            })
        }
    };
    Ok(next)
}

fn terminal_desired_state(
    action: CellAction,
    status: EffectStatus,
    current: LifecycleState,
) -> LifecycleState {
    match (action, status) {
        (CellAction::Provision | CellAction::Start, EffectStatus::Succeeded) => {
            LifecycleState::Enrolling
        }
        (CellAction::Stop, EffectStatus::Succeeded) => LifecycleState::Stopped,
        (CellAction::Destroy, EffectStatus::Succeeded) => LifecycleState::Destroyed,
        (_, EffectStatus::Failed | EffectStatus::Rejected) => LifecycleState::Failed,
        _ => current,
    }
}

fn observation_matches_desired(
    desired: LifecycleState,
    observation: Option<&ManagementObservation>,
) -> bool {
    let Some(observation) = observation else {
        return desired == LifecycleState::Requested;
    };
    match desired {
        LifecycleState::Requested => observation.runtime_state == "absent",
        LifecycleState::Provisioning => matches!(
            observation.runtime_state.as_str(),
            "pending" | "provisioning"
        ),
        LifecycleState::Enrolling => {
            observation.runtime_state == "ready" && observation.agent_state != "ready"
        }
        LifecycleState::Ready => {
            observation.runtime_state == "ready" && observation.agent_state == "ready"
        }
        LifecycleState::Stopping => observation.runtime_state == "stopping",
        LifecycleState::Stopped => observation.runtime_state == "stopped",
        LifecycleState::Failed => observation.runtime_state == "failed",
        LifecycleState::Unknown => false,
        LifecycleState::Destroyed => observation.runtime_state == "destroyed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(op: &str, generation: u64, action: CellAction) -> CellCommand {
        CellCommand::new(
            op,
            "instance-a",
            generation,
            action,
            json!({"runtime":"qemu"}),
        )
        .unwrap()
    }

    #[test]
    fn duplicate_operation_replays_without_a_second_effect() {
        let mut cell = InstanceCell::new("instance-a", 1).unwrap();
        let command = command("op-1", 1, CellAction::Provision);
        assert_eq!(cell.accept(&command).unwrap(), CommandDisposition::Accepted);
        assert_eq!(
            cell.accept(&command).unwrap(),
            CommandDisposition::Replayed {
                status: EffectStatus::Pending
            }
        );
        assert_eq!(cell.effects.len(), 1);
    }

    #[test]
    fn command_payload_is_always_a_json_object() {
        assert_eq!(
            CellCommand::new("op-array", "instance-a", 1, CellAction::Observe, json!([])),
            Err(CellError::PayloadNotObject)
        );
        assert!(serde_json::from_value::<CellCommand>(json!({
            "document_type":"instance-cell-command",
            "schema_version":"1",
            "operation_id":"op-array",
            "instance_id":"instance-a",
            "generation":1,
            "action":"observe",
            "request_hash":"0".repeat(64),
            "issued_at":Utc::now(),
            "payload":[]
        }))
        .is_err());

        let without_payload: CellCommand = serde_json::from_value(json!({
            "document_type":"instance-cell-command",
            "schema_version":"1",
            "operation_id":"op-empty",
            "instance_id":"instance-a",
            "generation":1,
            "action":"observe",
            "request_hash":"0".repeat(64),
            "issued_at":Utc::now()
        }))
        .unwrap();
        assert!(without_payload.payload.is_empty());
        assert!(serde_json::to_value(without_payload).unwrap()["payload"].is_object());
    }

    #[test]
    fn every_event_serializes_object_evidence() {
        let mut cell = InstanceCell::new("instance-a", 1).unwrap();
        cell.accept(&command("op-1", 1, CellAction::Provision))
            .unwrap();
        cell.record_dispatched("op-1", Some("management-op".into()))
            .unwrap();
        cell.record_unknown("op-1", Duration::seconds(1), "timeout")
            .unwrap();
        let serialized = serde_json::to_value(&cell).unwrap();
        for event in serialized["history"].as_array().unwrap() {
            assert!(event["evidence"].is_object(), "event evidence: {event}");
        }
    }

    #[test]
    fn operation_id_cannot_be_reused_for_another_payload() {
        let mut cell = InstanceCell::new("instance-a", 1).unwrap();
        cell.accept(&command("op-1", 1, CellAction::Provision))
            .unwrap();
        let other = CellCommand::new(
            "op-1",
            "instance-a",
            1,
            CellAction::Provision,
            json!({"runtime":"docker"}),
        )
        .unwrap();
        assert_eq!(cell.accept(&other), Err(CellError::OperationCollision));
    }

    #[test]
    fn stale_generation_is_rejected_before_ledger_mutation() {
        let mut cell = InstanceCell::new("instance-a", 2).unwrap();
        let result = cell.accept(&command("op-stale", 1, CellAction::Stop));
        assert_eq!(
            result,
            Err(CellError::StaleGeneration {
                command: 1,
                current: 2
            })
        );
        assert!(cell.effects.is_empty());
    }

    #[test]
    fn lost_response_stays_on_original_operation_identity() {
        let mut cell = InstanceCell::new("instance-a", 1).unwrap();
        let command = command("op-unknown", 1, CellAction::Provision);
        cell.accept(&command).unwrap();
        cell.record_dispatched("op-unknown", Some("mgmt-op".into()))
            .unwrap();
        cell.record_unknown("op-unknown", Duration::seconds(5), "timeout")
            .unwrap();
        let result = cell.reconcile(1);
        assert_eq!(
            result.classification,
            ReconcileClassification::OutcomeUnknown
        );
        assert_eq!(result.outstanding_operations, vec!["op-unknown"]);
        assert_eq!(cell.effects.len(), 1);
    }

    #[test]
    fn newer_management_generation_fences_old_effects() {
        let mut cell = InstanceCell::new("instance-a", 1).unwrap();
        cell.accept(&command("op-old", 1, CellAction::Provision))
            .unwrap();
        cell.observe(ManagementObservation {
            generation: 2,
            runtime_state: "ready".into(),
            agent_state: "ready".into(),
            runtime_id: Some("runtime-new".into()),
            provider: Some("libvirt".into()),
            observed_at: Utc::now(),
        })
        .unwrap();
        assert_eq!(cell.generation, 2);
        assert_eq!(cell.effects[0].status, EffectStatus::Rejected);
        assert_eq!(
            cell.effects[0].terminal_code.as_deref(),
            Some("celld.stale_generation_fenced")
        );
        assert!(matches!(
            cell.record_dispatched("op-old", None),
            Err(CellError::EffectGenerationFenced {
                generation: 1,
                current: 2,
                ..
            })
        ));
        let reconciliation = cell.reconcile(2);
        assert!(reconciliation.outstanding_operations.is_empty());
        assert_ne!(
            reconciliation.repair.as_deref(),
            Some("dispatch_pending_effects_by_original_operation_id")
        );
        assert_eq!(
            cell.accept(&command("op-stop-old", 1, CellAction::Stop)),
            Err(CellError::StaleGeneration {
                command: 1,
                current: 2
            })
        );
    }

    #[test]
    fn destroy_retains_a_tombstone() {
        let mut cell = InstanceCell::new("instance-a", 1).unwrap();
        let provision = command("op-p", 1, CellAction::Provision);
        cell.accept(&provision).unwrap();
        cell.record_terminal("op-p", EffectStatus::Succeeded, None)
            .unwrap();
        cell.observe(ManagementObservation {
            generation: 1,
            runtime_state: "ready".into(),
            agent_state: "ready".into(),
            runtime_id: Some("runtime-a".into()),
            provider: Some("docker".into()),
            observed_at: Utc::now(),
        })
        .unwrap();
        let destroy = command("op-d", 1, CellAction::Destroy);
        cell.accept(&destroy).unwrap();
        cell.record_terminal("op-d", EffectStatus::Succeeded, None)
            .unwrap();
        assert_eq!(cell.desired_state, LifecycleState::Destroyed);
        assert!(cell.tombstone_until.is_some());
        cell.validate_invariants().unwrap();
    }
}
