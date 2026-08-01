//! Bounded, durable collector spool for `activity.event/v1` batches.
//!
//! Collectors append normalized metadata before export. A durable management
//! acknowledgement removes only records at or below the acknowledged sequence.

use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SpoolError {
    #[error("activity spool quota exceeded ({used} + {incoming} > {limit} bytes)")]
    Full {
        used: u64,
        incoming: u64,
        limit: u64,
    },
    #[error("invalid activity spool record: {0}")]
    Invalid(String),
    #[error("activity spool I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("activity spool JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct ActivitySpool {
    path: PathBuf,
    max_bytes: u64,
}

impl ActivitySpool {
    pub fn open(path: impl Into<PathBuf>, max_bytes: u64) -> Result<Self, SpoolError> {
        if max_bytes == 0 {
            return Err(SpoolError::Invalid(
                "max_bytes must be greater than zero".into(),
            ));
        }
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new().create(true).append(true).open(&path)?;
        let spool = Self { path, max_bytes };
        let record_high_watermark = spool
            .records()?
            .last()
            .map(record_sequence)
            .transpose()?
            .unwrap_or(0);
        if record_high_watermark > spool.read_high_watermark()? {
            spool.persist_high_watermark(record_high_watermark)?;
        }
        Ok(spool)
    }

    pub fn next_sequence(&self) -> Result<u64, SpoolError> {
        let record_high_watermark = self
            .records()?
            .last()
            .map(record_sequence)
            .transpose()?
            .unwrap_or(0);
        Ok(record_high_watermark
            .max(self.read_high_watermark()?)
            .saturating_add(1))
    }

    pub fn append(&self, event: &Value) -> Result<(), SpoolError> {
        validate_record(event)?;
        let sequence = record_sequence(event)?;
        let event_id = event
            .get("event_id")
            .and_then(Value::as_str)
            .expect("validated event_id");
        for existing in self.records()? {
            let existing_sequence = record_sequence(&existing)?;
            let existing_id = existing
                .get("event_id")
                .and_then(Value::as_str)
                .expect("validated event_id");
            if existing_sequence == sequence || existing_id == event_id {
                if existing == *event {
                    return Ok(());
                }
                return Err(SpoolError::Invalid(format!(
                    "event_id or collector_sequence is already bound in the spool ({event_id}, {sequence})"
                )));
            }
        }
        let high_watermark = self.read_high_watermark()?;
        if sequence != high_watermark.saturating_add(1) {
            return Err(SpoolError::Invalid(format!(
                "collector_sequence must be the next monotonic value (expected {}, got {sequence})",
                high_watermark.saturating_add(1)
            )));
        }
        let mut encoded = serde_json::to_vec(event)?;
        encoded.push(b'\n');
        let used = fs::metadata(&self.path)?.len();
        if used.saturating_add(encoded.len() as u64) > self.max_bytes {
            return Err(SpoolError::Full {
                used,
                incoming: encoded.len() as u64,
                limit: self.max_bytes,
            });
        }
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        file.write_all(&encoded)?;
        file.sync_data()?;
        self.persist_high_watermark(sequence)?;
        Ok(())
    }

    pub fn pending(&self, limit: usize) -> Result<Vec<Value>, SpoolError> {
        Ok(self.records()?.into_iter().take(limit.min(1_000)).collect())
    }

    pub fn acknowledge_through(&self, sequence: u64) -> Result<(), SpoolError> {
        let remaining: Vec<Value> = self
            .records()?
            .into_iter()
            .filter(|event| {
                record_sequence(event)
                    .map(|value| value > sequence)
                    .unwrap_or(true)
            })
            .collect();
        let temporary = self.path.with_extension("jsonl.next");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&temporary)?;
            for event in remaining {
                serde_json::to_writer(&mut file, &event)?;
                file.write_all(b"\n")?;
            }
            file.sync_all()?;
        }
        fs::rename(&temporary, &self.path)?;
        sync_parent(&self.path)?;
        Ok(())
    }

    fn records(&self) -> Result<Vec<Value>, SpoolError> {
        let file = File::open(&self.path)?;
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line)?;
            validate_record(&value)?;
            records.push(value);
        }
        records.sort_by_key(|event| record_sequence(event).unwrap_or(u64::MAX));
        Ok(records)
    }

    fn sequence_path(&self) -> PathBuf {
        self.path.with_extension(format!(
            "{}.sequence",
            self.path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("spool")
        ))
    }

    fn read_high_watermark(&self) -> Result<u64, SpoolError> {
        let path = self.sequence_path();
        match fs::read_to_string(path) {
            Ok(value) => value
                .trim()
                .parse::<u64>()
                .map_err(|_| SpoolError::Invalid("invalid sequence high-watermark".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error.into()),
        }
    }

    fn persist_high_watermark(&self, sequence: u64) -> Result<(), SpoolError> {
        let path = self.sequence_path();
        let temporary = path.with_extension(format!(
            "{}.next",
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("sequence")
        ));
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&temporary)?;
            writeln!(file, "{sequence}")?;
            file.sync_all()?;
        }
        fs::rename(&temporary, &path)?;
        sync_parent(&path)?;
        Ok(())
    }
}

fn validate_record(event: &Value) -> Result<(), SpoolError> {
    if event.get("schema_version").and_then(Value::as_str) != Some("activity.event/v1") {
        return Err(SpoolError::Invalid("unsupported schema_version".into()));
    }
    let sequence = record_sequence(event)?;
    if sequence == 0 {
        return Err(SpoolError::Invalid(
            "collector_sequence must be positive".into(),
        ));
    }
    if event.get("event_id").and_then(Value::as_str).is_none() {
        return Err(SpoolError::Invalid("event_id is required".into()));
    }
    Ok(())
}

fn record_sequence(event: &Value) -> Result<u64, SpoolError> {
    event
        .get("integrity")
        .and_then(|integrity| integrity.get("collector_sequence"))
        .and_then(Value::as_u64)
        .ok_or_else(|| SpoolError::Invalid("integrity.collector_sequence is required".into()))
}

fn sync_parent(path: &Path) -> Result<(), SpoolError> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn event(sequence: u64) -> Value {
        serde_json::json!({
            "schema_version": "activity.event/v1",
            "event_id": format!("event-{sequence}"),
            "integrity": {"collector_sequence": sequence}
        })
    }

    #[test]
    fn restart_replay_and_durable_ack_preserve_only_unacked_records() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("collector.jsonl");
        let spool = ActivitySpool::open(&path, 4096).unwrap();
        spool.append(&event(1)).unwrap();
        spool.append(&event(2)).unwrap();
        drop(spool);

        let reopened = ActivitySpool::open(&path, 4096).unwrap();
        assert_eq!(reopened.next_sequence().unwrap(), 3);
        reopened.acknowledge_through(1).unwrap();
        let pending = reopened.pending(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(record_sequence(&pending[0]).unwrap(), 2);
        reopened.acknowledge_through(2).unwrap();
        assert!(reopened.pending(10).unwrap().is_empty());
        assert_eq!(reopened.next_sequence().unwrap(), 3);

        drop(reopened);
        let restarted_empty = ActivitySpool::open(&path, 4096).unwrap();
        assert_eq!(restarted_empty.next_sequence().unwrap(), 3);
    }

    #[test]
    fn quota_is_hard_and_does_not_corrupt_existing_records() {
        let dir = tempdir().unwrap();
        let spool = ActivitySpool::open(dir.path().join("collector.jsonl"), 100).unwrap();
        spool.append(&event(1)).unwrap();
        assert!(matches!(
            spool.append(&event(2)),
            Err(SpoolError::Full { .. })
        ));
        assert_eq!(spool.pending(10).unwrap().len(), 1);
    }

    #[test]
    fn append_is_idempotent_but_rejects_sequence_reuse_and_skips() {
        let dir = tempdir().unwrap();
        let spool = ActivitySpool::open(dir.path().join("collector.jsonl"), 4096).unwrap();
        spool.append(&event(1)).unwrap();
        spool.append(&event(1)).unwrap();
        assert_eq!(spool.pending(10).unwrap().len(), 1);

        let mut rebound = event(1);
        rebound["event_id"] = Value::String("different-event".into());
        assert!(matches!(
            spool.append(&rebound),
            Err(SpoolError::Invalid(_))
        ));
        assert!(matches!(
            spool.append(&event(3)),
            Err(SpoolError::Invalid(_))
        ));
        assert_eq!(spool.next_sequence().unwrap(), 2);
    }
}
