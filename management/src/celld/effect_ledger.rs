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
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
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
        Ok(EffectClaim::Replayed {
            status: parse_status(&status),
        })
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
        let result_json = result
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let connection = self.connection.lock().expect("effect ledger lock poisoned");
        let changed=connection.execute("UPDATE celld_effects SET status=?2,result_json=?3,updated_at=?4 WHERE operation_id=?1 AND status NOT IN ('succeeded','failed','rejected')",params![operation_id,status_name(status),result_json,chrono::Utc::now().to_rfc3339()])?;
        if changed == 0 {
            let existing: Option<(String, Option<String>)> = connection
                .query_row(
                    "SELECT status,result_json FROM celld_effects WHERE operation_id=?1",
                    params![operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((existing_status, existing_result)) = existing else {
                return Err(EffectLedgerError::UnknownOperation(operation_id.into()));
            };
            let existing_status = parse_status(&existing_status);
            if existing_status != status || existing_result != result_json {
                return Err(EffectLedgerError::TerminalConflict {
                    operation_id: operation_id.into(),
                    existing: existing_status,
                    requested: status,
                });
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
    use std::sync::{Arc, Barrier};

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
}
