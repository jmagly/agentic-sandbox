//! v1 `missions.json` → v2 `missions.db` migration (#207).
//!
//! Reads a v1 [`MissionStore`](super::MissionStore) persistence file (the
//! `HashMap<String, MissionRecord>` JSON document written atomically by the
//! v1 `MissionStore::persist`), maps each record onto a v2 A2A
//! [`TaskRow`](super::task_store::TaskRow) following the state table below,
//! and writes the result through [`TaskStore::upsert_task`].
//!
//! # State mapping
//!
//! | v1 [`MissionState`](super::MissionState) | v2 [`TaskState`](super::task_store::TaskState) wire form | Notes |
//! |---|---|---|
//! | `Assigned`     | `submitted`      | |
//! | `Running`      | `working`        | |
//! | `HitlRequired` | `input-required` | |
//! | `Suspended`    | `working`        | `metadata.note = "v1: was Suspended"` |
//! | `Completed`    | `completed`      | terminal |
//! | `Failed`       | `failed`         | `fail_kind = infrastructure` per ADR-007 default |
//! | `Aborted`      | `canceled`       | terminal |
//!
//! Unmapped v1 fields land under `metadata_json.v1_extras` so no information
//! is lost in the cutover. A v1 origin marker — `metadata_json.v1_origin = true`
//! plus the v1 manifest under `v1_record` — is always written.
//!
//! # Safety
//!
//! - Refuses to overwrite an existing populated v2 DB unless `force` is set.
//! - `dry_run` performs validation + mapping but writes nothing and does not
//!   archive the v1 file.
//! - On success the v1 file is renamed to
//!   `missions.json.v1-archived-<RFC3339>` via `std::fs::rename`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

// task_store moved to the executor crate in #243; pull it from there.
use agentic_sandbox_executor::store::task_store::{FailKind, TaskRow, TaskState, TaskStore};
use super::{MissionRecord, MissionState};

/// Per-state mapping summary plus archive path. Returned by [`migrate`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MigrationReport {
    pub total: u64,
    pub submitted: u64,
    pub working: u64,
    pub input_required: u64,
    pub completed: u64,
    pub failed: u64,
    pub canceled: u64,
    /// Path the v1 file was archived to. `None` when `dry_run` is set.
    pub archived_to: Option<PathBuf>,
    /// True when the v2 DB was already populated and `force` was supplied.
    pub merged_into_existing: bool,
}

impl MigrationReport {
    fn record(&mut self, state: TaskState) {
        self.total += 1;
        match state {
            TaskState::Submitted => self.submitted += 1,
            TaskState::Working => self.working += 1,
            TaskState::InputRequired => self.input_required += 1,
            TaskState::Completed => self.completed += 1,
            TaskState::Failed => self.failed += 1,
            TaskState::Canceled => self.canceled += 1,
            // Rejected / AuthRequired aren't produced by v1 mapping but
            // counting them under a known bucket avoids a silent drop.
            TaskState::Rejected | TaskState::AuthRequired => self.failed += 1,
        }
    }

    /// Human-readable summary suitable for stdout.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Migration summary:\n  total:          {}\n  submitted:      {}\n  working:        {}\n  input-required: {}\n  completed:      {}\n  failed:         {}\n  canceled:       {}\n",
            self.total,
            self.submitted,
            self.working,
            self.input_required,
            self.completed,
            self.failed,
            self.canceled,
        );
        if let Some(p) = &self.archived_to {
            s.push_str(&format!("  archived v1:    {}\n", p.display()));
        }
        if self.merged_into_existing {
            s.push_str("  (merged into existing populated v2 DB via --force)\n");
        }
        s
    }
}

/// Migrate `input` (v1 missions.json) into `output` (v2 missions.db).
///
/// See module docs for state mapping and safety guarantees.
pub fn migrate(
    input: &Path,
    output: &Path,
    force: bool,
    dry_run: bool,
) -> Result<MigrationReport> {
    // ── 1. Read + parse v1 ──────────────────────────────────────────────────
    let raw = std::fs::read_to_string(input).with_context(|| {
        format!("reading v1 missions file at {}", input.display())
    })?;
    let v1: HashMap<String, MissionRecord> = serde_json::from_str(&raw).with_context(|| {
        format!(
            "parsing v1 missions JSON at {} (expected HashMap<String, MissionRecord>)",
            input.display()
        )
    })?;
    info!(count = v1.len(), "loaded v1 missions");

    // ── 2. Open / detect v2 DB ──────────────────────────────────────────────
    let mut merged_into_existing = false;
    let store = TaskStore::open(output)
        .with_context(|| format!("opening v2 TaskStore at {}", output.display()))?;
    let existing = store.count_tasks().context("counting existing tasks")?;
    if existing > 0 {
        if !force {
            return Err(anyhow!(
                "refusing to overwrite v2 DB at {}: contains {} task(s). Re-run with --force to merge.",
                output.display(),
                existing
            ));
        }
        warn!(
            existing,
            "v2 DB at {} is populated; --force supplied, merging without clearing",
            output.display()
        );
        merged_into_existing = true;
    }

    // ── 3. Map + upsert ─────────────────────────────────────────────────────
    let mut report = MigrationReport {
        merged_into_existing,
        ..Default::default()
    };
    for (mission_id, rec) in &v1 {
        let row = map_mission(mission_id, rec)?;
        report.record(row.state);
        if !dry_run {
            store
                .upsert_task(&row)
                .with_context(|| format!("upserting task {}", row.task_id))?;
        }
    }

    // ── 4. Archive v1 file ──────────────────────────────────────────────────
    if !dry_run {
        let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        // RFC3339 contains ':' which is awkward on some filesystems; keep as-is
        // — Linux ext4/xfs accept it and the spec says use RFC3339.
        let archive_name = format!(
            "{}.v1-archived-{}",
            input
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "missions.json".into()),
            ts
        );
        let archive_path = input
            .parent()
            .map(|p| p.join(&archive_name))
            .unwrap_or_else(|| PathBuf::from(&archive_name));
        std::fs::rename(input, &archive_path).with_context(|| {
            format!(
                "archiving v1 file {} → {}",
                input.display(),
                archive_path.display()
            )
        })?;
        report.archived_to = Some(archive_path);
    }

    Ok(report)
}

/// Map a single v1 [`MissionRecord`] onto a v2 [`TaskRow`].
///
/// `mission_id` is the HashMap key; v1 records also carry their `mission_id`
/// internally and the two are expected to agree. The key wins on mismatch
/// (it's the canonical lookup identity in v1).
fn map_mission(mission_id: &str, rec: &MissionRecord) -> Result<TaskRow> {
    let (state, fail_kind, note): (TaskState, Option<FailKind>, Option<&'static str>) =
        match rec.state {
            MissionState::Assigned => (TaskState::Submitted, None, None),
            MissionState::Running => (TaskState::Working, None, None),
            MissionState::HitlRequired => (TaskState::InputRequired, None, None),
            MissionState::Suspended => (TaskState::Working, None, Some("v1: was Suspended")),
            MissionState::Completed => (TaskState::Completed, None, None),
            // ADR-007: collapsed v1 `Failed` defaults to infrastructure — the
            // safer of the two retry classifications when origin is unknown.
            MissionState::Failed => (TaskState::Failed, Some(FailKind::Infrastructure), None),
            MissionState::Aborted => (TaskState::Canceled, None, None),
        };

    let created_at = parse_v1_ts(&rec.created_at)
        .with_context(|| format!("mission {mission_id}: bad created_at"))?;
    let updated_at = parse_v1_ts(&rec.updated_at)
        .with_context(|| format!("mission {mission_id}: bad updated_at"))?;

    let status_json = json!({
        "state": state.as_str(),
        "timestamp": updated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });

    // Preserve the entire v1 record verbatim + capture any future fields
    // that may exist on disk but aren't in this build's struct. We serialize
    // the typed record (lossless for known fields) and also store the raw
    // mission_id key for traceability.
    let v1_record = serde_json::to_value(rec).context("serializing v1 record")?;
    let mut metadata = serde_json::Map::new();
    metadata.insert("v1_origin".into(), json!(true));
    metadata.insert("v1_mission_id".into(), json!(mission_id));
    metadata.insert("v1_record".into(), v1_record);
    if let Some(n) = note {
        metadata.insert("note".into(), json!(n));
    }
    // `v1_extras` placeholder for unknown fields. Today v1 has no fields
    // beyond MissionRecord, so this is empty — but consumers can rely on
    // the key existing.
    metadata.insert("v1_extras".into(), json!({}));
    let metadata_json = Some(serde_json::Value::Object(metadata));

    Ok(TaskRow {
        task_id: mission_id.to_string(),
        context_id: None,
        state,
        fail_kind,
        status_json,
        metadata_json,
        created_at,
        updated_at,
        terminal_at: if state.is_terminal() {
            Some(updated_at)
        } else {
            None
        },
    })
}

fn parse_v1_ts(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .with_context(|| format!("parsing RFC3339 timestamp {s}"))?
        .with_timezone(&Utc))
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aiwg_serve::task_store::ListFilter;
    use std::collections::HashMap;

    fn v1_record(id: &str, state: MissionState) -> MissionRecord {
        MissionRecord {
            mission_id: id.into(),
            objective: format!("obj-{id}"),
            completion: format!("done-{id}"),
            state,
            pty_session_id: Some(format!("pty-{id}")),
            checkpoint_id: None,
            created_at: "2026-05-01T12:00:00Z".into(),
            updated_at: "2026-05-02T12:00:00Z".into(),
        }
    }

    fn write_v1_file(dir: &Path, map: &HashMap<String, MissionRecord>) -> PathBuf {
        let path = dir.join("missions.json");
        let raw = serde_json::to_string_pretty(map).unwrap();
        std::fs::write(&path, raw).unwrap();
        path
    }

    #[test]
    fn migrate_known_v1_file() {
        let tmp = tempfile::tempdir().unwrap();
        let mut map = HashMap::new();
        map.insert("m1".into(), v1_record("m1", MissionState::Running));
        map.insert("m2".into(), v1_record("m2", MissionState::Completed));
        map.insert("m3".into(), v1_record("m3", MissionState::HitlRequired));
        let v1_path = write_v1_file(tmp.path(), &map);
        let db_path = tmp.path().join("missions.db");

        let report = migrate(&v1_path, &db_path, false, false).unwrap();
        assert_eq!(report.total, 3);
        assert!(report.archived_to.is_some());
        assert!(!v1_path.exists(), "v1 file should be archived (renamed)");

        let store = TaskStore::open(&db_path).unwrap();
        let listed = store
            .list_tasks(ListFilter {
                state: None,
                limit: None,
                include_terminal: true,
            })
            .unwrap();
        assert_eq!(listed.len(), 3);
        let ids: Vec<&str> = listed.iter().map(|r| r.task_id.as_str()).collect();
        for want in ["m1", "m2", "m3"] {
            assert!(ids.contains(&want), "missing {want}");
        }
    }

    #[test]
    fn state_mapping_table() {
        let cases = [
            (MissionState::Assigned, TaskState::Submitted, None),
            (MissionState::Running, TaskState::Working, None),
            (MissionState::HitlRequired, TaskState::InputRequired, None),
            (MissionState::Suspended, TaskState::Working, None),
            (MissionState::Completed, TaskState::Completed, None),
            (
                MissionState::Failed,
                TaskState::Failed,
                Some(FailKind::Infrastructure),
            ),
            (MissionState::Aborted, TaskState::Canceled, None),
        ];
        for (v1_state, want_state, want_fail_kind) in cases {
            let rec = v1_record("x", v1_state.clone());
            let row = map_mission("x", &rec).unwrap();
            assert_eq!(row.state, want_state, "state mapping for {v1_state:?}");
            assert_eq!(
                row.fail_kind, want_fail_kind,
                "fail_kind mapping for {v1_state:?}",
            );
            if matches!(v1_state, MissionState::Suspended) {
                let meta = row.metadata_json.as_ref().unwrap();
                assert_eq!(
                    meta.get("note").and_then(|v| v.as_str()),
                    Some("v1: was Suspended")
                );
            }
        }
    }

    #[test]
    fn dry_run_does_not_write() {
        let tmp = tempfile::tempdir().unwrap();
        let mut map = HashMap::new();
        map.insert("m1".into(), v1_record("m1", MissionState::Running));
        let v1_path = write_v1_file(tmp.path(), &map);
        let db_path = tmp.path().join("missions.db");

        let report = migrate(&v1_path, &db_path, false, true).unwrap();
        assert_eq!(report.total, 1);
        assert!(report.archived_to.is_none());
        assert!(v1_path.exists(), "v1 file MUST remain on dry-run");

        let store = TaskStore::open(&db_path).unwrap();
        assert_eq!(store.count_tasks().unwrap(), 0);
    }

    #[test]
    fn force_required_when_db_populated() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("missions.db");
        // Pre-populate v2 DB with a single unrelated task.
        {
            let store = TaskStore::open(&db_path).unwrap();
            store
                .upsert_task(&TaskRow {
                    task_id: "preexisting".into(),
                    context_id: None,
                    state: TaskState::Working,
                    fail_kind: None,
                    status_json: json!({"state": "working"}),
                    metadata_json: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    terminal_at: None,
                })
                .unwrap();
        }

        // Build a v1 file (separate file path per attempt because the
        // first failing attempt must not archive it).
        let mut map = HashMap::new();
        map.insert("m1".into(), v1_record("m1", MissionState::Running));
        let v1_path = write_v1_file(tmp.path(), &map);

        // Without --force → error.
        let err = migrate(&v1_path, &db_path, false, false).unwrap_err();
        assert!(
            err.to_string().contains("refusing to overwrite"),
            "unexpected error: {err}"
        );
        assert!(v1_path.exists(), "v1 file must not be archived on error");

        // With --force → succeeds and merges.
        let report = migrate(&v1_path, &db_path, true, false).unwrap();
        assert!(report.merged_into_existing);
        assert_eq!(report.total, 1);
        let store = TaskStore::open(&db_path).unwrap();
        assert_eq!(store.count_tasks().unwrap(), 2);
        assert!(store.get_task("preexisting").unwrap().is_some());
        assert!(store.get_task("m1").unwrap().is_some());
    }

    #[test]
    fn terminal_at_populated_for_terminal_states() {
        let tmp = tempfile::tempdir().unwrap();
        let mut map = HashMap::new();
        map.insert("c".into(), v1_record("c", MissionState::Completed));
        map.insert("f".into(), v1_record("f", MissionState::Failed));
        map.insert("a".into(), v1_record("a", MissionState::Aborted));
        map.insert("r".into(), v1_record("r", MissionState::Running)); // control
        let v1_path = write_v1_file(tmp.path(), &map);
        let db_path = tmp.path().join("missions.db");

        migrate(&v1_path, &db_path, false, false).unwrap();
        let store = TaskStore::open(&db_path).unwrap();
        for id in ["c", "f", "a"] {
            let row = store.get_task(id).unwrap().unwrap();
            assert!(
                row.terminal_at.is_some(),
                "terminal_at missing for terminal task {id}"
            );
        }
        let running = store.get_task("r").unwrap().unwrap();
        assert!(running.terminal_at.is_none());
    }

    #[test]
    fn metadata_preserves_v1_manifest() {
        let rec = v1_record("m1", MissionState::Running);
        let row = map_mission("m1", &rec).unwrap();
        let meta = row.metadata_json.as_ref().unwrap();
        assert_eq!(meta.get("v1_origin").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            meta.get("v1_mission_id").and_then(|v| v.as_str()),
            Some("m1")
        );
        let v1_record_meta = meta.get("v1_record").unwrap();
        assert_eq!(
            v1_record_meta.get("objective").and_then(|v| v.as_str()),
            Some("obj-m1")
        );
        assert_eq!(
            v1_record_meta
                .get("pty_session_id")
                .and_then(|v| v.as_str()),
            Some("pty-m1")
        );
        assert!(meta.get("v1_extras").is_some());
    }

    #[test]
    fn failed_state_default_fail_kind_is_infrastructure() {
        let rec = v1_record("f", MissionState::Failed);
        let row = map_mission("f", &rec).unwrap();
        assert_eq!(row.state, TaskState::Failed);
        assert_eq!(row.fail_kind, Some(FailKind::Infrastructure));
    }
}
