use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};

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

#[derive(Debug, thiserror::Error)]
pub enum EffectLedgerError {
    #[error("effect ledger database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("operation id is already bound to another request")]
    Collision,
    #[error("generation {command} is stale; management generation is {current}")]
    StaleGeneration { command: u64, current: u64 },
    #[error("generation {command} is not yet present; management generation is {current}")]
    FutureGeneration { command: u64, current: u64 },
    #[error("unknown operation {0}")]
    UnknownOperation(String),
}

impl EffectLedger {
    pub fn open(path: &Path) -> Result<Self, EffectLedgerError> {
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS celld_effects (
              operation_id TEXT PRIMARY KEY, request_hash TEXT NOT NULL, instance_id TEXT NOT NULL,
              generation INTEGER NOT NULL, action TEXT NOT NULL, status TEXT NOT NULL,
              result_json TEXT, updated_at TEXT NOT NULL
            );",
        )?;
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
        let existing: Option<(String, String)> = connection
            .query_row(
                "SELECT request_hash,status FROM celld_effects WHERE operation_id=?1",
                params![command.operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((request_hash, status)) = existing {
            if request_hash != command.request_hash {
                return Err(EffectLedgerError::Collision);
            }
            return Ok(EffectClaim::Replayed {
                status: parse_status(&status),
            });
        }
        connection.execute("INSERT INTO celld_effects(operation_id,request_hash,instance_id,generation,action,status,updated_at) VALUES(?1,?2,?3,?4,?5,'pending',?6)",params![command.operation_id,command.request_hash,command.instance_id,command.generation,action_name(command.action),chrono::Utc::now().to_rfc3339()])?;
        Ok(EffectClaim::Acquired)
    }

    pub fn complete(
        &self,
        operation_id: &str,
        status: EffectStatus,
        result: Option<&serde_json::Value>,
    ) -> Result<(), EffectLedgerError> {
        if !status.is_terminal() {
            return Ok(());
        }
        let connection = self.connection.lock().expect("effect ledger lock poisoned");
        let changed=connection.execute("UPDATE celld_effects SET status=?2,result_json=?3,updated_at=?4 WHERE operation_id=?1 AND status NOT IN ('succeeded','failed','rejected')",params![operation_id,status_name(status),result.map(serde_json::to_string).transpose().map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,chrono::Utc::now().to_rfc3339()])?;
        if changed == 0 {
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
        }
        Ok(())
    }
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
fn parse_status(value: &str) -> EffectStatus {
    match value {
        "dispatched" => EffectStatus::Dispatched,
        "unknown" => EffectStatus::Unknown,
        "succeeded" => EffectStatus::Succeeded,
        "failed" => EffectStatus::Failed,
        "rejected" => EffectStatus::Rejected,
        _ => EffectStatus::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
}
