use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex, time::Duration};

use super::{CellAction, CellCommand, EffectStatus};

pub struct EffectLedger {
    connection: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectClaim {
    Acquired,
    Replayed { status: EffectStatus },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EffectLedgerRecord {
    pub operation_id: String,
    pub instance_id: String,
    pub generation: u64,
    pub action: CellAction,
    pub status: EffectStatus,
    /// Durable count of management-to-provider dispatch boundary crossings.
    ///
    /// This is not a claim that the external provider completed an effect. It
    /// increments only when this ledger atomically grants the single dispatch
    /// owner for an operation.
    pub provider_dispatch_count: u64,
    pub management_operation_id: Option<String>,
    pub terminal_code: Option<String>,
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum EffectLedgerError {
    #[error("effect ledger database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("effect ledger filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("effect ledger must not be group/world accessible")]
    InsecurePermissions,
    #[error("operation id is already bound to another request")]
    Collision,
    #[error("generation {command} is stale; management generation is {current}")]
    StaleGeneration { command: u64, current: u64 },
    #[error("generation {command} is not yet present; management generation is {current}")]
    FutureGeneration { command: u64, current: u64 },
    #[error("unknown operation {0}")]
    UnknownOperation(String),
    #[error("instance {0} has no management generation binding")]
    UnknownInstance(String),
    #[error("generation may advance only by one after a terminal destroy")]
    GenerationAdvanceNotAllowed,
    #[error("operation is already bound to another management operation")]
    ManagementOperationConflict,
    #[error("effect ledger contains an invalid persisted value: {0}")]
    CorruptRecord(String),
    #[error(
        "operation {operation_id} already has terminal status {existing:?}; conflicting terminal status {requested:?} was requested"
    )]
    TerminalConflict {
        operation_id: String,
        existing: EffectStatus,
        requested: EffectStatus,
    },
}

impl EffectLedger {
    pub fn open(path: &Path) -> Result<Self, EffectLedgerError> {
        verify_existing_permissions(path)?;
        let connection = Connection::open(path)?;
        restrict_permissions(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS celld_effects (
              operation_id TEXT PRIMARY KEY, request_hash TEXT NOT NULL, instance_id TEXT NOT NULL,
              generation INTEGER NOT NULL, action TEXT NOT NULL, status TEXT NOT NULL,
              provider_dispatch_count INTEGER NOT NULL DEFAULT 0,
              management_operation_id TEXT, terminal_code TEXT, result_json TEXT, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS celld_instance_generations (
              instance_id TEXT PRIMARY KEY, generation INTEGER NOT NULL,
              state TEXT NOT NULL, updated_at TEXT NOT NULL
            );",
        )?;
        if !column_exists(&connection, "celld_effects", "management_operation_id")? {
            connection.execute(
                "ALTER TABLE celld_effects ADD COLUMN management_operation_id TEXT",
                [],
            )?;
        }
        if !column_exists(&connection, "celld_effects", "terminal_code")? {
            connection.execute(
                "ALTER TABLE celld_effects ADD COLUMN terminal_code TEXT",
                [],
            )?;
        }
        if !column_exists(&connection, "celld_effects", "provider_dispatch_count")? {
            connection.execute(
                "ALTER TABLE celld_effects ADD COLUMN provider_dispatch_count INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn claim(
        &self,
        command: &CellCommand,
        management_generation: u64,
    ) -> Result<EffectClaim, EffectLedgerError> {
        if command.generation < management_generation {
            return Err(EffectLedgerError::StaleGeneration {
                command: command.generation,
                current: management_generation,
            });
        }
        if command.generation > management_generation {
            return Err(EffectLedgerError::FutureGeneration {
                command: command.generation,
                current: management_generation,
            });
        }
        let connection = self.connection.lock().expect("effect ledger lock poisoned");
        claim_on_connection(&connection, command)
    }

    pub fn claim_managed(&self, command: &CellCommand) -> Result<EffectClaim, EffectLedgerError> {
        let mut connection = self.connection.lock().expect("effect ledger lock poisoned");
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: Option<(u64, String)> = transaction
            .query_row(
                "SELECT generation,state FROM celld_instance_generations WHERE instance_id=?1",
                params![command.instance_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match current {
            None if command.action == CellAction::Provision => {
                transaction.execute(
                    "INSERT INTO celld_instance_generations(instance_id,generation,state,updated_at) VALUES(?1,?2,'provisioning',?3)",
                    params![command.instance_id, command.generation, chrono::Utc::now().to_rfc3339()],
                )?;
            }
            None => {
                return Err(EffectLedgerError::UnknownInstance(
                    command.instance_id.clone(),
                ))
            }
            Some((generation, _)) if command.generation < generation => {
                return Err(EffectLedgerError::StaleGeneration {
                    command: command.generation,
                    current: generation,
                });
            }
            Some((generation, _)) if command.generation == generation => {}
            Some((generation, state))
                if command.action == CellAction::Provision
                    && state == "destroyed"
                    && command.generation == generation.saturating_add(1) =>
            {
                transaction.execute(
                    "UPDATE celld_instance_generations SET generation=?2,state='provisioning',updated_at=?3 WHERE instance_id=?1",
                    params![command.instance_id, command.generation, chrono::Utc::now().to_rfc3339()],
                )?;
            }
            Some((generation, _)) if command.generation > generation.saturating_add(1) => {
                return Err(EffectLedgerError::FutureGeneration {
                    command: command.generation,
                    current: generation,
                });
            }
            Some(_) => return Err(EffectLedgerError::GenerationAdvanceNotAllowed),
        }
        let claim = claim_on_connection(&transaction, command)?;
        transaction.commit()?;
        Ok(claim)
    }

    pub fn begin_dispatch(&self, operation_id: &str) -> Result<bool, EffectLedgerError> {
        let connection = self.connection.lock().expect("effect ledger lock poisoned");
        let changed = connection.execute(
            "UPDATE celld_effects SET status='dispatched',provider_dispatch_count=provider_dispatch_count+1,updated_at=?2 WHERE operation_id=?1 AND status='pending'",
            params![operation_id, chrono::Utc::now().to_rfc3339()],
        )?;
        if changed == 1 {
            return Ok(true);
        }
        let exists: Option<String> = connection
            .query_row(
                "SELECT status FROM celld_effects WHERE operation_id=?1",
                params![operation_id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(EffectLedgerError::UnknownOperation(operation_id.into()));
        }
        Ok(false)
    }

    pub fn set_management_operation(
        &self,
        operation_id: &str,
        management_operation_id: &str,
    ) -> Result<(), EffectLedgerError> {
        let connection = self.connection.lock().expect("effect ledger lock poisoned");
        if let Some(existing) = self
            .record_locked(&connection, operation_id)?
            .and_then(|record| record.management_operation_id)
        {
            if existing != management_operation_id {
                return Err(EffectLedgerError::ManagementOperationConflict);
            }
            return Ok(());
        }
        let changed = connection.execute(
            "UPDATE celld_effects SET management_operation_id=COALESCE(management_operation_id,?2),updated_at=?3 WHERE operation_id=?1 AND status IN ('dispatched','unknown')",
            params![operation_id, management_operation_id, chrono::Utc::now().to_rfc3339()],
        )?;
        if changed == 0 && self.record_locked(&connection, operation_id)?.is_none() {
            return Err(EffectLedgerError::UnknownOperation(operation_id.into()));
        }
        Ok(())
    }

    pub fn mark_unknown(&self, operation_id: &str) -> Result<(), EffectLedgerError> {
        let connection = self.connection.lock().expect("effect ledger lock poisoned");
        let changed = connection.execute(
            "UPDATE celld_effects SET status='unknown',updated_at=?2 WHERE operation_id=?1 AND status='dispatched'",
            params![operation_id, chrono::Utc::now().to_rfc3339()],
        )?;
        if changed == 0 && self.record_locked(&connection, operation_id)?.is_none() {
            return Err(EffectLedgerError::UnknownOperation(operation_id.into()));
        }
        Ok(())
    }

    pub fn record(&self, operation_id: &str) -> Result<EffectLedgerRecord, EffectLedgerError> {
        let connection = self.connection.lock().expect("effect ledger lock poisoned");
        self.record_locked(&connection, operation_id)?
            .ok_or_else(|| EffectLedgerError::UnknownOperation(operation_id.into()))
    }

    fn record_locked(
        &self,
        connection: &Connection,
        operation_id: &str,
    ) -> Result<Option<EffectLedgerRecord>, EffectLedgerError> {
        let row: Option<(
            String,
            String,
            u64,
            String,
            String,
            u64,
            Option<String>,
            Option<String>,
            Option<String>,
        )> = connection
            .query_row(
                "SELECT operation_id,instance_id,generation,action,status,provider_dispatch_count,management_operation_id,terminal_code,result_json FROM celld_effects WHERE operation_id=?1",
                params![operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?)),
            )
            .optional()
            .map_err(EffectLedgerError::from)?;
        let Some((
            operation_id,
            instance_id,
            generation,
            action,
            status,
            provider_dispatch_count,
            management_operation_id,
            terminal_code,
            result,
        )) = row
        else {
            return Ok(None);
        };
        let action = parse_action(&action)
            .ok_or_else(|| EffectLedgerError::CorruptRecord("action".into()))?;
        let status = parse_status(&status)
            .ok_or_else(|| EffectLedgerError::CorruptRecord("status".into()))?;
        let result = result
            .map(|value| {
                serde_json::from_str(&value)
                    .map_err(|_| EffectLedgerError::CorruptRecord("result_json".into()))
            })
            .transpose()?;
        Ok(Some(EffectLedgerRecord {
            operation_id,
            instance_id,
            generation,
            action,
            status,
            provider_dispatch_count,
            management_operation_id,
            terminal_code,
            result,
        }))
    }

    pub fn current_generation(&self, instance_id: &str) -> Result<Option<u64>, EffectLedgerError> {
        let connection = self.connection.lock().expect("effect ledger lock poisoned");
        connection
            .query_row(
                "SELECT generation FROM celld_instance_generations WHERE instance_id=?1",
                params![instance_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(EffectLedgerError::from)
    }

    fn update_instance_state(
        connection: &Connection,
        operation_id: &str,
        status: EffectStatus,
    ) -> Result<(), EffectLedgerError> {
        if status != EffectStatus::Succeeded {
            return Ok(());
        }
        let effect: Option<(String, u64, String)> = connection
            .query_row(
                "SELECT instance_id,generation,action FROM celld_effects WHERE operation_id=?1",
                params![operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((instance_id, generation, action)) = effect else {
            return Ok(());
        };
        let state = match action.as_str() {
            "destroy" => "destroyed",
            "stop" => "stopped",
            "provision" | "start" => "active",
            _ => return Ok(()),
        };
        connection.execute(
            "UPDATE celld_instance_generations SET state=?3,updated_at=?4 WHERE instance_id=?1 AND generation=?2",
            params![instance_id, generation, state, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn complete(
        &self,
        operation_id: &str,
        status: EffectStatus,
        result: Option<&serde_json::Value>,
    ) -> Result<(), EffectLedgerError> {
        self.complete_with_code(operation_id, status, None, result)
    }

    pub fn complete_with_code(
        &self,
        operation_id: &str,
        status: EffectStatus,
        terminal_code: Option<&str>,
        result: Option<&serde_json::Value>,
    ) -> Result<(), EffectLedgerError> {
        if !status.is_terminal() {
            return Ok(());
        }
        let result_json = result
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let mut connection = self.connection.lock().expect("effect ledger lock poisoned");
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed=transaction.execute("UPDATE celld_effects SET status=?2,terminal_code=?3,result_json=?4,updated_at=?5 WHERE operation_id=?1 AND status NOT IN ('succeeded','failed','rejected')",params![operation_id,status_name(status),terminal_code,result_json,chrono::Utc::now().to_rfc3339()])?;
        if changed == 0 {
            let existing: Option<(String, Option<String>, Option<String>)> = transaction
                .query_row(
                    "SELECT status,terminal_code,result_json FROM celld_effects WHERE operation_id=?1",
                    params![operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((existing_status, existing_code, existing_result)) = existing else {
                return Err(EffectLedgerError::UnknownOperation(operation_id.into()));
            };
            let existing_status = parse_status(&existing_status)
                .ok_or_else(|| EffectLedgerError::CorruptRecord("status".into()))?;
            if existing_status != status
                || existing_code.as_deref() != terminal_code
                || existing_result != result_json
            {
                return Err(EffectLedgerError::TerminalConflict {
                    operation_id: operation_id.into(),
                    existing: existing_status,
                    requested: status,
                });
            }
        }
        Self::update_instance_state(&transaction, operation_id, status)?;
        transaction.commit()?;
        Ok(())
    }
}

fn claim_on_connection(
    connection: &Connection,
    command: &CellCommand,
) -> Result<EffectClaim, EffectLedgerError> {
    let inserted = connection.execute(
            "INSERT INTO celld_effects(operation_id,request_hash,instance_id,generation,action,status,updated_at)
             VALUES(?1,?2,?3,?4,?5,'pending',?6)
             ON CONFLICT(operation_id) DO NOTHING",
            params![
                command.operation_id,
                command.request_hash,
                command.instance_id,
                command.generation,
                action_name(command.action),
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
    if inserted == 1 {
        return Ok(EffectClaim::Acquired);
    }

    let (request_hash, status): (String, String) = connection.query_row(
        "SELECT request_hash,status FROM celld_effects WHERE operation_id=?1",
        params![command.operation_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if request_hash != command.request_hash {
        return Err(EffectLedgerError::Collision);
    }
    let status =
        parse_status(&status).ok_or_else(|| EffectLedgerError::CorruptRecord("status".into()))?;
    Ok(EffectClaim::Replayed { status })
}

fn column_exists(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, rusqlite::Error> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(unix)]
fn verify_existing_permissions(path: &Path) -> Result<(), EffectLedgerError> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.permissions().mode() & 0o077 != 0 => {
            Err(EffectLedgerError::InsecurePermissions)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(EffectLedgerError::Io(error)),
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), EffectLedgerError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn verify_existing_permissions(_path: &Path) -> Result<(), EffectLedgerError> {
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), EffectLedgerError> {
    Ok(())
}

fn action_name(action: CellAction) -> &'static str {
    match action {
        CellAction::Provision => "provision",
        CellAction::Start => "start",
        CellAction::Stop => "stop",
        CellAction::Destroy => "destroy",
        CellAction::Observe => "observe",
        CellAction::Repair => "repair",
    }
}
fn parse_action(value: &str) -> Option<CellAction> {
    match value {
        "provision" => Some(CellAction::Provision),
        "start" => Some(CellAction::Start),
        "stop" => Some(CellAction::Stop),
        "destroy" => Some(CellAction::Destroy),
        "observe" => Some(CellAction::Observe),
        "repair" => Some(CellAction::Repair),
        _ => None,
    }
}
fn status_name(status: EffectStatus) -> &'static str {
    match status {
        EffectStatus::Pending => "pending",
        EffectStatus::Dispatched => "dispatched",
        EffectStatus::Unknown => "unknown",
        EffectStatus::Succeeded => "succeeded",
        EffectStatus::Failed => "failed",
        EffectStatus::Rejected => "rejected",
    }
}
fn parse_status(value: &str) -> Option<EffectStatus> {
    match value {
        "pending" => Some(EffectStatus::Pending),
        "dispatched" => Some(EffectStatus::Dispatched),
        "unknown" => Some(EffectStatus::Unknown),
        "succeeded" => Some(EffectStatus::Succeeded),
        "failed" => Some(EffectStatus::Failed),
        "rejected" => Some(EffectStatus::Rejected),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier};

    #[cfg(unix)]
    #[test]
    fn ledger_file_is_private_and_insecure_existing_files_fail_closed() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("effects.db");
        let ledger = EffectLedger::open(&path).unwrap();
        drop(ledger);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            EffectLedger::open(&path),
            Err(EffectLedgerError::InsecurePermissions)
        ));
    }

    #[test]
    fn claim_is_durable_idempotent_and_generation_fenced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("effects.db");
        let command = CellCommand::new("op-1", "instance", 7, CellAction::Stop, json!({})).unwrap();
        {
            let ledger = EffectLedger::open(&path).unwrap();
            assert_eq!(ledger.claim(&command, 7).unwrap(), EffectClaim::Acquired);
            ledger
                .complete("op-1", EffectStatus::Succeeded, None)
                .unwrap();
        }
        let ledger = EffectLedger::open(&path).unwrap();
        assert_eq!(
            ledger.claim(&command, 7).unwrap(),
            EffectClaim::Replayed {
                status: EffectStatus::Succeeded
            }
        );
        assert!(matches!(
            ledger.claim(&command, 8),
            Err(EffectLedgerError::StaleGeneration { .. })
        ));
        let collision =
            CellCommand::new("op-1", "instance", 7, CellAction::Destroy, json!({})).unwrap();
        assert!(matches!(
            ledger.claim(&collision, 7),
            Err(EffectLedgerError::Collision)
        ));
    }

    #[test]
    fn legacy_ledger_migrates_dispatch_count_and_preserves_it_across_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy-effects.db");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE celld_effects (
                       operation_id TEXT PRIMARY KEY, request_hash TEXT NOT NULL, instance_id TEXT NOT NULL,
                       generation INTEGER NOT NULL, action TEXT NOT NULL, status TEXT NOT NULL,
                       management_operation_id TEXT, terminal_code TEXT, result_json TEXT, updated_at TEXT NOT NULL
                     );
                     CREATE TABLE celld_instance_generations (
                       instance_id TEXT PRIMARY KEY, generation INTEGER NOT NULL,
                       state TEXT NOT NULL, updated_at TEXT NOT NULL
                     );
                     INSERT INTO celld_effects(
                       operation_id,request_hash,instance_id,generation,action,status,updated_at
                     ) VALUES(
                       'op-migrated','legacy-request-hash','instance',1,'provision','pending','2026-08-23T00:00:00Z'
                     );",
                )
                .unwrap();
        }
        restrict_permissions(&path).unwrap();

        {
            let ledger = EffectLedger::open(&path).unwrap();
            assert_eq!(
                ledger
                    .record("op-migrated")
                    .unwrap()
                    .provider_dispatch_count,
                0
            );
            assert!(ledger.begin_dispatch("op-migrated").unwrap());
            assert!(!ledger.begin_dispatch("op-migrated").unwrap());
            assert_eq!(
                ledger
                    .record("op-migrated")
                    .unwrap()
                    .provider_dispatch_count,
                1
            );
        }
        let reopened = EffectLedger::open(&path).unwrap();
        assert!(!reopened.begin_dispatch("op-migrated").unwrap());
        assert_eq!(
            reopened
                .record("op-migrated")
                .unwrap()
                .provider_dispatch_count,
            1
        );
    }

    #[test]
    fn separate_connections_resolve_concurrent_claim_as_acquire_and_replay() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("concurrent-effects.db");
        let first = EffectLedger::open(&path).unwrap();
        let second = EffectLedger::open(&path).unwrap();
        let command = Arc::new(
            CellCommand::new("op-concurrent", "instance", 3, CellAction::Stop, json!({})).unwrap(),
        );
        let barrier = Arc::new(Barrier::new(3));

        let spawn_claim = |ledger: EffectLedger| {
            let command = Arc::clone(&command);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                ledger.claim(&command, 3)
            })
        };
        let first_claim = spawn_claim(first);
        let second_claim = spawn_claim(second);
        barrier.wait();

        let claims = [first_claim.join().unwrap(), second_claim.join().unwrap()];
        assert_eq!(
            claims
                .iter()
                .filter(|claim| matches!(claim, Ok(EffectClaim::Acquired)))
                .count(),
            1
        );
        assert_eq!(
            claims
                .iter()
                .filter(|claim| {
                    matches!(
                        claim,
                        Ok(EffectClaim::Replayed {
                            status: EffectStatus::Pending
                        })
                    )
                })
                .count(),
            1
        );
    }

    #[test]
    fn separate_connections_grant_one_dispatch_and_increment_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("concurrent-dispatch.db");
        let setup = EffectLedger::open(&path).unwrap();
        let command = CellCommand::new(
            "op-concurrent-dispatch",
            "instance",
            3,
            CellAction::Stop,
            json!({}),
        )
        .unwrap();
        assert_eq!(setup.claim(&command, 3).unwrap(), EffectClaim::Acquired);
        drop(setup);

        let first = EffectLedger::open(&path).unwrap();
        let second = EffectLedger::open(&path).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let spawn_dispatch = |ledger: EffectLedger| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                ledger.begin_dispatch("op-concurrent-dispatch")
            })
        };
        let first_dispatch = spawn_dispatch(first);
        let second_dispatch = spawn_dispatch(second);
        barrier.wait();

        let grants = [
            first_dispatch.join().unwrap().unwrap(),
            second_dispatch.join().unwrap().unwrap(),
        ];
        assert_eq!(grants.into_iter().filter(|granted| *granted).count(), 1);
        assert_eq!(
            EffectLedger::open(&path)
                .unwrap()
                .record("op-concurrent-dispatch")
                .unwrap()
                .provider_dispatch_count,
            1
        );
    }

    #[test]
    fn ten_thousand_duplicate_deliveries_per_action_keep_one_dispatch_owner() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = EffectLedger::open(&directory.path().join("duplicate-effects.db")).unwrap();
        for (index, action) in [
            CellAction::Provision,
            CellAction::Start,
            CellAction::Stop,
            CellAction::Destroy,
        ]
        .into_iter()
        .enumerate()
        {
            let operation_id = format!("op-duplicate-{index}");
            let command = CellCommand::new(
                &operation_id,
                "instance",
                7,
                action,
                json!({"substrate":"instrumented"}),
            )
            .unwrap();
            assert_eq!(ledger.claim(&command, 7).unwrap(), EffectClaim::Acquired);
            for _ in 1..10_000 {
                assert_eq!(
                    ledger.claim(&command, 7).unwrap(),
                    EffectClaim::Replayed {
                        status: EffectStatus::Pending
                    }
                );
            }
            assert!(ledger.begin_dispatch(&operation_id).unwrap());
            assert!(!ledger.begin_dispatch(&operation_id).unwrap());
            assert_eq!(
                ledger
                    .record(&operation_id)
                    .unwrap()
                    .provider_dispatch_count,
                1
            );
        }
    }

    #[test]
    fn conflicting_terminal_completion_is_typed_and_original_is_immutable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("terminal-effects.db");
        let ledger = EffectLedger::open(&path).unwrap();
        let command =
            CellCommand::new("op-terminal", "instance", 4, CellAction::Stop, json!({})).unwrap();
        assert_eq!(ledger.claim(&command, 4).unwrap(), EffectClaim::Acquired);
        ledger
            .complete(
                "op-terminal",
                EffectStatus::Succeeded,
                Some(&json!({"runtime_id":"runtime-a"})),
            )
            .unwrap();
        ledger
            .complete(
                "op-terminal",
                EffectStatus::Succeeded,
                Some(&json!({"runtime_id":"runtime-a"})),
            )
            .unwrap();

        assert!(matches!(
            ledger.complete(
                "op-terminal",
                EffectStatus::Failed,
                Some(&json!({"reason":"late failure"}))
            ),
            Err(EffectLedgerError::TerminalConflict {
                existing: EffectStatus::Succeeded,
                requested: EffectStatus::Failed,
                ..
            })
        ));
        assert_eq!(
            ledger.claim(&command, 4).unwrap(),
            EffectClaim::Replayed {
                status: EffectStatus::Succeeded
            }
        );
    }

    #[test]
    fn managed_generation_advances_only_after_destroy_and_dispatch_is_single_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("managed-effects.db");
        let ledger = EffectLedger::open(&path).unwrap();
        let provision = CellCommand::new(
            "op-provision",
            "instance",
            1,
            CellAction::Provision,
            json!({}),
        )
        .unwrap();
        assert_eq!(
            ledger.claim_managed(&provision).unwrap(),
            EffectClaim::Acquired
        );
        assert!(ledger.begin_dispatch("op-provision").unwrap());
        assert!(!ledger.begin_dispatch("op-provision").unwrap());
        ledger
            .set_management_operation("op-provision", "management-op-1")
            .unwrap();
        assert!(matches!(
            ledger.set_management_operation("op-provision", "management-op-2"),
            Err(EffectLedgerError::ManagementOperationConflict)
        ));
        ledger
            .complete(
                "op-provision",
                EffectStatus::Succeeded,
                Some(&json!({"runtime_id":"runtime-1"})),
            )
            .unwrap();
        assert_eq!(ledger.current_generation("instance").unwrap(), Some(1));
        assert_eq!(
            ledger
                .record("op-provision")
                .unwrap()
                .management_operation_id
                .as_deref(),
            Some("management-op-1")
        );

        let premature = CellCommand::new(
            "op-premature",
            "instance",
            2,
            CellAction::Provision,
            json!({}),
        )
        .unwrap();
        assert!(matches!(
            ledger.claim_managed(&premature),
            Err(EffectLedgerError::GenerationAdvanceNotAllowed)
        ));

        let destroy =
            CellCommand::new("op-destroy", "instance", 1, CellAction::Destroy, json!({})).unwrap();
        assert_eq!(
            ledger.claim_managed(&destroy).unwrap(),
            EffectClaim::Acquired
        );
        ledger
            .complete("op-destroy", EffectStatus::Succeeded, None)
            .unwrap();
        let next =
            CellCommand::new("op-next", "instance", 2, CellAction::Provision, json!({})).unwrap();
        assert_eq!(ledger.claim_managed(&next).unwrap(), EffectClaim::Acquired);
        assert_eq!(ledger.current_generation("instance").unwrap(), Some(2));

        let stale =
            CellCommand::new("op-stale", "instance", 1, CellAction::Stop, json!({})).unwrap();
        assert!(matches!(
            ledger.claim_managed(&stale),
            Err(EffectLedgerError::StaleGeneration { .. })
        ));
    }
}
