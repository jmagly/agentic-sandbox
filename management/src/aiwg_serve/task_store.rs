//! SQLite-backed `TaskStore` for the v2 A2A executor (issue #205).
//!
//! # Design notes
//!
//! 1. **JSON-blob storage of A2A types** — A2A `Task`, `TaskStatus`, `Artifact`,
//!    and `PushNotificationConfig` payloads are persisted as `serde_json::Value`
//!    in `*_json` columns. Typed wrappers around `a2a-rs` are deferred to #208
//!    so this module remains self-contained and Wave-2 buildable without the
//!    A2A crate bootstrap.
//!
//! 2. **Single-mutex connection** — `Arc<Mutex<rusqlite::Connection>>`. SQLite
//!    serializes writers anyway under the default journal mode, so a single
//!    connection avoids locking contention from rusqlite's threading model.
//!    If contention emerges (e.g. heavy idempotency-cache reads coinciding with
//!    task upserts) we will graduate to an `r2d2` pool with one writer + many
//!    readers.
//!
//! 3. **WAL + NORMAL durability** — `journal_mode=WAL` permits concurrent
//!    readers during writes; `synchronous=NORMAL` trades a small fsync window
//!    on power-loss for substantially higher throughput. Per ADR-014 the
//!    outbox lives upstream of the store; a missed terminal write is
//!    recoverable via re-emit. `foreign_keys=ON` is enforced so artifact and
//!    push-config rows cannot orphan.
//!
//! 4. **Schema migrations via `user_version`** — Bootstrap sets
//!    `PRAGMA user_version = 1`. Future migrations branch on the current
//!    value; #207 (migration tool) consumes this primitive.
//!
//! 5. **Retention policy** — `purge_expired` deletes terminal tasks whose
//!    `terminal_at` is older than the supplied retention window. Idempotency
//!    cache entries have their own expiry column and are pruned by
//!    `idempotency_purge_expired`.
//!
//! # A2A `TaskState` wire mapping
//!
//! | Variant         | Wire string        |
//! |-----------------|--------------------|
//! | `Submitted`     | `submitted`        |
//! | `Working`       | `working`          |
//! | `Completed`     | `completed`        |
//! | `Failed`        | `failed`           |
//! | `Canceled`      | `canceled`         |
//! | `InputRequired` | `input-required`   |
//! | `Rejected`      | `rejected`         |
//! | `AuthRequired`  | `auth-required`    |
//!
//! Terminal states (per A2A): `completed`, `failed`, `canceled`, `rejected`.

use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

const SCHEMA_USER_VERSION: i32 = 1;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS tasks (
  task_id TEXT PRIMARY KEY,
  context_id TEXT,
  state TEXT NOT NULL,
  fail_kind TEXT,
  status_json TEXT NOT NULL,
  metadata_json TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  terminal_at TEXT
);
CREATE TABLE IF NOT EXISTS task_artifacts (
  artifact_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  artifact_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS push_notification_configs (
  config_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  url TEXT NOT NULL,
  auth_json TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS idempotency_cache (
  message_id TEXT PRIMARY KEY,
  request_hash TEXT NOT NULL,
  response_status INTEGER NOT NULL,
  response_body TEXT NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tasks_state_idx ON tasks(state);
CREATE INDEX IF NOT EXISTS tasks_terminal_at_idx ON tasks(terminal_at);
CREATE INDEX IF NOT EXISTS idempotency_expiry_idx ON idempotency_cache(expires_at);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Submitted,
    Working,
    Completed,
    Failed,
    Canceled,
    InputRequired,
    Rejected,
    AuthRequired,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskState::Submitted => "submitted",
            TaskState::Working => "working",
            TaskState::Completed => "completed",
            TaskState::Failed => "failed",
            TaskState::Canceled => "canceled",
            TaskState::InputRequired => "input-required",
            TaskState::Rejected => "rejected",
            TaskState::AuthRequired => "auth-required",
        }
    }

    /// Terminal in the A2A sense: no further state transitions are permitted.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled | TaskState::Rejected
        )
    }
}

impl FromStr for TaskState {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "submitted" => TaskState::Submitted,
            "working" => TaskState::Working,
            "completed" => TaskState::Completed,
            "failed" => TaskState::Failed,
            "canceled" => TaskState::Canceled,
            "input-required" => TaskState::InputRequired,
            "rejected" => TaskState::Rejected,
            "auth-required" => TaskState::AuthRequired,
            other => return Err(anyhow!("unknown TaskState: {other}")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailKind {
    Application,
    Infrastructure,
}

impl FailKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FailKind::Application => "application",
            FailKind::Infrastructure => "infrastructure",
        }
    }
}

impl FromStr for FailKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "application" => FailKind::Application,
            "infrastructure" => FailKind::Infrastructure,
            other => return Err(anyhow!("unknown FailKind: {other}")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub task_id: String,
    pub context_id: Option<String>,
    pub state: TaskState,
    pub fail_kind: Option<FailKind>,
    pub status_json: serde_json::Value,
    pub metadata_json: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub terminal_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub state: Option<TaskState>,
    pub limit: Option<u64>,
    /// When `false`, terminal tasks are excluded.
    pub include_terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRow {
    pub artifact_id: String,
    pub task_id: String,
    pub artifact_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushNotificationConfigRow {
    pub config_id: String,
    pub task_id: String,
    pub url: String,
    pub auth_json: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyEntry {
    pub message_id: String,
    pub request_hash: String,
    pub response_status: u16,
    pub response_body: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct TaskStore {
    conn: Arc<Mutex<Connection>>,
}

fn fmt_ts(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .with_context(|| format!("parsing timestamp {s}"))?
        .with_timezone(&Utc))
}

fn parse_opt_ts(s: Option<String>) -> Result<Option<DateTime<Utc>>> {
    match s {
        Some(v) => Ok(Some(parse_ts(&v)?)),
        None => Ok(None),
    }
}

fn json_to_string(v: &serde_json::Value) -> Result<String> {
    serde_json::to_string(v).context("serializing JSON column")
}

fn parse_json(s: &str) -> Result<serde_json::Value> {
    serde_json::from_str(s).context("parsing JSON column")
}

fn parse_opt_json(s: Option<String>) -> Result<Option<serde_json::Value>> {
    match s {
        Some(v) => Ok(Some(parse_json(&v)?)),
        None => Ok(None),
    }
}

impl TaskStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        debug!(path = %path.display(), "opening TaskStore");
        let conn = Connection::open(path)
            .with_context(|| format!("opening sqlite at {}", path.display()))?;
        let store = Self::from_conn(conn)?;
        info!(path = %path.display(), "TaskStore ready");
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory sqlite")?;
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        // Pragmas first, then schema.
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("setting journal_mode=WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .context("setting synchronous=NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .context("setting foreign_keys=ON")?;

        conn.execute_batch(SCHEMA_SQL).context("bootstrap schema")?;

        // Stamp user_version for future migrations (#207).
        let cur: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .context("reading user_version")?;
        if cur < SCHEMA_USER_VERSION {
            conn.pragma_update(None, "user_version", SCHEMA_USER_VERSION)
                .context("stamping user_version")?;
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // Poisoning is fatal to the store; surface via panic so a supervisor
        // can restart. Recovery without dropping the connection isn't safe.
        self.conn.lock().expect("TaskStore mutex poisoned")
    }

    // ---------- tasks ----------

    /// Insert or replace a task.
    ///
    /// Sets `terminal_at = updated_at` exactly once when the task transitions
    /// into a terminal state (and was not already terminal). Existing
    /// `terminal_at` values are preserved.
    pub fn upsert_task(&self, row: &TaskRow) -> Result<()> {
        let conn = self.lock();
        let tx = conn.unchecked_transaction().context("begin tx")?;

        let prior_terminal_at: Option<Option<String>> = tx
            .query_row(
                "SELECT terminal_at FROM tasks WHERE task_id = ?1",
                params![row.task_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .context("loading prior terminal_at")?;

        let terminal_at = match (prior_terminal_at.flatten(), row.terminal_at) {
            (Some(prior), _) => Some(prior),
            (None, Some(supplied)) => Some(fmt_ts(&supplied)),
            (None, None) if row.state.is_terminal() => Some(fmt_ts(&row.updated_at)),
            (None, None) => None,
        };

        tx.execute(
            "INSERT INTO tasks (task_id, context_id, state, fail_kind, status_json, metadata_json, \
             created_at, updated_at, terminal_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT(task_id) DO UPDATE SET \
             context_id = excluded.context_id, \
             state = excluded.state, \
             fail_kind = excluded.fail_kind, \
             status_json = excluded.status_json, \
             metadata_json = excluded.metadata_json, \
             updated_at = excluded.updated_at, \
             terminal_at = excluded.terminal_at",
            params![
                row.task_id,
                row.context_id,
                row.state.as_str(),
                row.fail_kind.map(|f| f.as_str()),
                json_to_string(&row.status_json)?,
                row.metadata_json
                    .as_ref()
                    .map(json_to_string)
                    .transpose()?,
                fmt_ts(&row.created_at),
                fmt_ts(&row.updated_at),
                terminal_at,
            ],
        )
        .context("upsert task")?;

        tx.commit().context("commit tx")?;
        Ok(())
    }

    pub fn get_task(&self, task_id: &str) -> Result<Option<TaskRow>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT task_id, context_id, state, fail_kind, status_json, metadata_json, \
                 created_at, updated_at, terminal_at FROM tasks WHERE task_id = ?1",
                params![task_id],
                row_to_task,
            )
            .optional()
            .context("get_task query")?;
        row.transpose()
    }

    pub fn list_tasks(&self, filter: ListFilter) -> Result<Vec<TaskRow>> {
        let conn = self.lock();
        let mut sql = String::from(
            "SELECT task_id, context_id, state, fail_kind, status_json, metadata_json, \
             created_at, updated_at, terminal_at FROM tasks",
        );
        let mut clauses: Vec<String> = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(state) = filter.state {
            clauses.push(format!("state = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(state.as_str().to_string()));
        }
        if !filter.include_terminal {
            clauses.push("terminal_at IS NULL".to_string());
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at ASC");
        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let mut stmt = conn.prepare(&sql).context("prepare list_tasks")?;
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), row_to_task)
            .context("query list_tasks")?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.context("row error")??);
        }
        Ok(out)
    }

    pub fn delete_task(&self, task_id: &str) -> Result<bool> {
        let conn = self.lock();
        let tx = conn.unchecked_transaction().context("begin tx")?;
        tx.execute(
            "DELETE FROM task_artifacts WHERE task_id = ?1",
            params![task_id],
        )
        .context("delete artifacts")?;
        tx.execute(
            "DELETE FROM push_notification_configs WHERE task_id = ?1",
            params![task_id],
        )
        .context("delete push configs")?;
        let n = tx
            .execute("DELETE FROM tasks WHERE task_id = ?1", params![task_id])
            .context("delete task")?;
        tx.commit().context("commit tx")?;
        Ok(n > 0)
    }

    pub fn count_tasks(&self) -> Result<u64> {
        let conn = self.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .context("count_tasks")?;
        Ok(n as u64)
    }

    // ---------- artifacts ----------

    pub fn append_artifact(
        &self,
        task_id: &str,
        artifact_id: &str,
        artifact_json: &serde_json::Value,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO task_artifacts (artifact_id, task_id, artifact_json, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                artifact_id,
                task_id,
                json_to_string(artifact_json)?,
                fmt_ts(&Utc::now()),
            ],
        )
        .context("append_artifact")?;
        Ok(())
    }

    pub fn list_artifacts(&self, task_id: &str) -> Result<Vec<ArtifactRow>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT artifact_id, task_id, artifact_json, created_at \
                 FROM task_artifacts WHERE task_id = ?1 ORDER BY created_at ASC",
            )
            .context("prepare list_artifacts")?;
        let rows = stmt
            .query_map(params![task_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .context("query list_artifacts")?;

        let mut out = Vec::new();
        for r in rows {
            let (artifact_id, task_id, artifact_json, created_at) = r.context("row error")?;
            out.push(ArtifactRow {
                artifact_id,
                task_id,
                artifact_json: parse_json(&artifact_json)?,
                created_at: parse_ts(&created_at)?,
            });
        }
        Ok(out)
    }

    // ---------- push notification configs ----------

    pub fn put_push_config(&self, cfg: &PushNotificationConfigRow) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO push_notification_configs (config_id, task_id, url, auth_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(config_id) DO UPDATE SET \
             task_id = excluded.task_id, \
             url = excluded.url, \
             auth_json = excluded.auth_json",
            params![
                cfg.config_id,
                cfg.task_id,
                cfg.url,
                cfg.auth_json.as_ref().map(json_to_string).transpose()?,
                fmt_ts(&cfg.created_at),
            ],
        )
        .context("put_push_config")?;
        Ok(())
    }

    pub fn get_push_config(&self, config_id: &str) -> Result<Option<PushNotificationConfigRow>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT config_id, task_id, url, auth_json, created_at \
                 FROM push_notification_configs WHERE config_id = ?1",
                params![config_id],
                row_to_push,
            )
            .optional()
            .context("get_push_config")?;
        row.transpose()
    }

    pub fn list_push_configs(&self, task_id: &str) -> Result<Vec<PushNotificationConfigRow>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT config_id, task_id, url, auth_json, created_at \
                 FROM push_notification_configs WHERE task_id = ?1 ORDER BY created_at ASC",
            )
            .context("prepare list_push_configs")?;
        let rows = stmt
            .query_map(params![task_id], row_to_push)
            .context("query list_push_configs")?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.context("row error")??);
        }
        Ok(out)
    }

    pub fn delete_push_config(&self, config_id: &str) -> Result<bool> {
        let conn = self.lock();
        let n = conn
            .execute(
                "DELETE FROM push_notification_configs WHERE config_id = ?1",
                params![config_id],
            )
            .context("delete_push_config")?;
        Ok(n > 0)
    }

    // ---------- retention ----------

    /// Delete terminal tasks whose `terminal_at` is older than
    /// `now - terminal_retention`. Returns the number of tasks removed.
    pub fn purge_expired(&self, terminal_retention: Duration) -> Result<u64> {
        let cutoff = Utc::now() - terminal_retention;
        let cutoff_str = fmt_ts(&cutoff);
        let conn = self.lock();
        let tx = conn.unchecked_transaction().context("begin tx")?;

        // Collect victims first so dependent rows can be removed.
        let victims: Vec<String> = {
            let mut stmt = tx
                .prepare(
                    "SELECT task_id FROM tasks \
                     WHERE terminal_at IS NOT NULL AND terminal_at < ?1",
                )
                .context("prepare purge select")?;
            let rows = stmt
                .query_map(params![cutoff_str], |r| r.get::<_, String>(0))
                .context("query purge select")?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .context("collect victims")?
        };

        for tid in &victims {
            tx.execute("DELETE FROM task_artifacts WHERE task_id = ?1", params![tid])
                .context("purge artifacts")?;
            tx.execute(
                "DELETE FROM push_notification_configs WHERE task_id = ?1",
                params![tid],
            )
            .context("purge push configs")?;
        }
        let n = tx
            .execute(
                "DELETE FROM tasks WHERE terminal_at IS NOT NULL AND terminal_at < ?1",
                params![cutoff_str],
            )
            .context("purge tasks")?;
        tx.commit().context("commit tx")?;
        Ok(n as u64)
    }

    // ---------- idempotency cache ----------

    pub fn idempotency_get(&self, message_id: &str) -> Result<Option<IdempotencyEntry>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT message_id, request_hash, response_status, response_body, created_at, expires_at \
                 FROM idempotency_cache WHERE message_id = ?1",
                params![message_id],
                row_to_idem,
            )
            .optional()
            .context("idempotency_get")?;
        row.transpose()
    }

    pub fn idempotency_put(&self, entry: &IdempotencyEntry) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO idempotency_cache \
             (message_id, request_hash, response_status, response_body, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(message_id) DO UPDATE SET \
             request_hash = excluded.request_hash, \
             response_status = excluded.response_status, \
             response_body = excluded.response_body, \
             expires_at = excluded.expires_at",
            params![
                entry.message_id,
                entry.request_hash,
                entry.response_status as i64,
                json_to_string(&entry.response_body)?,
                fmt_ts(&entry.created_at),
                fmt_ts(&entry.expires_at),
            ],
        )
        .context("idempotency_put")?;
        Ok(())
    }

    pub fn idempotency_purge_expired(&self) -> Result<u64> {
        let now = fmt_ts(&Utc::now());
        let conn = self.lock();
        let n = conn
            .execute(
                "DELETE FROM idempotency_cache WHERE expires_at < ?1",
                params![now],
            )
            .context("idempotency_purge_expired")?;
        Ok(n as u64)
    }

    pub fn idempotency_count(&self) -> Result<u64> {
        let conn = self.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM idempotency_cache", [], |r| r.get(0))
            .context("idempotency_count")?;
        Ok(n as u64)
    }

    /// Evict the `n` oldest idempotency-cache rows (by `created_at` ASC).
    /// Used by [`crate::aiwg_serve::idempotency::IdempotencyCache::evict_to_cap`]
    /// for soft LRU enforcement when `idempotency_count` exceeds the configured
    /// cap. Returns the number of rows actually deleted (may be less than `n`
    /// if the table has fewer rows). A no-op when `n == 0`.
    pub fn idempotency_evict_oldest(&self, n: u64) -> Result<u64> {
        if n == 0 {
            return Ok(0);
        }
        let conn = self.lock();
        let removed = conn
            .execute(
                "DELETE FROM idempotency_cache WHERE message_id IN (\
                 SELECT message_id FROM idempotency_cache \
                 ORDER BY created_at ASC LIMIT ?1)",
                params![n as i64],
            )
            .context("idempotency_evict_oldest")?;
        Ok(removed as u64)
    }
}

// ---------- row decoders ----------
//
// Returning `rusqlite::Result<Result<T>>` lets us surface JSON/timestamp parse
// errors without conflating them with sqlite-level row errors.

fn row_to_task(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<TaskRow>> {
    let task_id: String = r.get(0)?;
    let context_id: Option<String> = r.get(1)?;
    let state: String = r.get(2)?;
    let fail_kind: Option<String> = r.get(3)?;
    let status_json: String = r.get(4)?;
    let metadata_json: Option<String> = r.get(5)?;
    let created_at: String = r.get(6)?;
    let updated_at: String = r.get(7)?;
    let terminal_at: Option<String> = r.get(8)?;

    Ok((|| -> Result<TaskRow> {
        Ok(TaskRow {
            task_id,
            context_id,
            state: TaskState::from_str(&state)?,
            fail_kind: fail_kind.map(|s| FailKind::from_str(&s)).transpose()?,
            status_json: parse_json(&status_json)?,
            metadata_json: parse_opt_json(metadata_json)?,
            created_at: parse_ts(&created_at)?,
            updated_at: parse_ts(&updated_at)?,
            terminal_at: parse_opt_ts(terminal_at)?,
        })
    })())
}

fn row_to_push(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<PushNotificationConfigRow>> {
    let config_id: String = r.get(0)?;
    let task_id: String = r.get(1)?;
    let url: String = r.get(2)?;
    let auth_json: Option<String> = r.get(3)?;
    let created_at: String = r.get(4)?;
    Ok((|| -> Result<PushNotificationConfigRow> {
        Ok(PushNotificationConfigRow {
            config_id,
            task_id,
            url,
            auth_json: parse_opt_json(auth_json)?,
            created_at: parse_ts(&created_at)?,
        })
    })())
}

fn row_to_idem(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<IdempotencyEntry>> {
    let message_id: String = r.get(0)?;
    let request_hash: String = r.get(1)?;
    let response_status: i64 = r.get(2)?;
    let response_body: String = r.get(3)?;
    let created_at: String = r.get(4)?;
    let expires_at: String = r.get(5)?;
    Ok((|| -> Result<IdempotencyEntry> {
        Ok(IdempotencyEntry {
            message_id,
            request_hash,
            response_status: response_status as u16,
            response_body: parse_json(&response_body)?,
            created_at: parse_ts(&created_at)?,
            expires_at: parse_ts(&expires_at)?,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn ts(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap()
    }

    fn task(id: &str, state: TaskState, t: DateTime<Utc>) -> TaskRow {
        TaskRow {
            task_id: id.into(),
            context_id: Some(format!("ctx-{id}")),
            state,
            fail_kind: None,
            status_json: json!({"state": state.as_str()}),
            metadata_json: Some(json!({"src": "test"})),
            created_at: t,
            updated_at: t,
            terminal_at: None,
        }
    }

    #[test]
    fn open_creates_schema() {
        let s = TaskStore::open_in_memory().unwrap();
        let conn = s.lock();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for t in [
            "idempotency_cache",
            "push_notification_configs",
            "task_artifacts",
            "tasks",
        ] {
            assert!(tables.contains(&t.to_string()), "missing {t}");
        }
        let indices: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name NOT LIKE 'sqlite_%'")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for i in [
            "tasks_state_idx",
            "tasks_terminal_at_idx",
            "idempotency_expiry_idx",
        ] {
            assert!(indices.contains(&i.to_string()), "missing index {i}");
        }
        let v: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_USER_VERSION);
    }

    #[test]
    fn upsert_round_trip() {
        let s = TaskStore::open_in_memory().unwrap();
        let row = task("t1", TaskState::Working, ts(2026, 1, 1));
        s.upsert_task(&row).unwrap();
        let got = s.get_task("t1").unwrap().expect("present");
        assert_eq!(got, row);
        assert_eq!(s.count_tasks().unwrap(), 1);
    }

    #[test]
    fn terminal_at_set_on_completion() {
        let s = TaskStore::open_in_memory().unwrap();
        let mut row = task("t2", TaskState::Working, ts(2026, 1, 1));
        s.upsert_task(&row).unwrap();
        assert!(s.get_task("t2").unwrap().unwrap().terminal_at.is_none());

        row.state = TaskState::Completed;
        row.updated_at = ts(2026, 1, 2);
        s.upsert_task(&row).unwrap();
        let got = s.get_task("t2").unwrap().unwrap();
        assert_eq!(got.terminal_at, Some(ts(2026, 1, 2)));

        // A subsequent (illegal but tested) upsert must NOT clobber terminal_at.
        row.updated_at = ts(2026, 1, 3);
        s.upsert_task(&row).unwrap();
        let got2 = s.get_task("t2").unwrap().unwrap();
        assert_eq!(got2.terminal_at, Some(ts(2026, 1, 2)));
    }

    #[test]
    fn list_filter_by_state() {
        let s = TaskStore::open_in_memory().unwrap();
        s.upsert_task(&task("a", TaskState::Working, ts(2026, 1, 1)))
            .unwrap();
        s.upsert_task(&task("b", TaskState::Submitted, ts(2026, 1, 2)))
            .unwrap();
        s.upsert_task(&task("c", TaskState::Working, ts(2026, 1, 3)))
            .unwrap();
        let working = s
            .list_tasks(ListFilter {
                state: Some(TaskState::Working),
                limit: None,
                include_terminal: false,
            })
            .unwrap();
        assert_eq!(working.len(), 2);
        assert_eq!(working[0].task_id, "a");
        assert_eq!(working[1].task_id, "c");
    }

    #[test]
    fn artifacts_attached_to_task() {
        let s = TaskStore::open_in_memory().unwrap();
        s.upsert_task(&task("t", TaskState::Working, ts(2026, 1, 1)))
            .unwrap();
        s.append_artifact("t", "a1", &json!({"kind": "log"}))
            .unwrap();
        s.append_artifact("t", "a2", &json!({"kind": "result"}))
            .unwrap();
        let got = s.list_artifacts("t").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].artifact_id, "a1");
        assert_eq!(got[1].artifact_json, json!({"kind": "result"}));
    }

    #[test]
    fn push_config_crud() {
        let s = TaskStore::open_in_memory().unwrap();
        s.upsert_task(&task("t", TaskState::Working, ts(2026, 1, 1)))
            .unwrap();
        let cfg = PushNotificationConfigRow {
            config_id: "c1".into(),
            task_id: "t".into(),
            url: "https://example.com/wh".into(),
            auth_json: Some(json!({"scheme": "bearer"})),
            created_at: ts(2026, 1, 2),
        };
        s.put_push_config(&cfg).unwrap();
        let got = s.get_push_config("c1").unwrap().unwrap();
        assert_eq!(got, cfg);
        let listed = s.list_push_configs("t").unwrap();
        assert_eq!(listed.len(), 1);
        assert!(s.delete_push_config("c1").unwrap());
        assert!(s.get_push_config("c1").unwrap().is_none());
    }

    #[test]
    fn idempotency_round_trip() {
        let s = TaskStore::open_in_memory().unwrap();
        let now = Utc::now();
        let entry = IdempotencyEntry {
            message_id: "m1".into(),
            request_hash: "deadbeef".into(),
            response_status: 200,
            response_body: json!({"ok": true}),
            created_at: now,
            expires_at: now + Duration::seconds(60),
        };
        s.idempotency_put(&entry).unwrap();
        let got = s.idempotency_get("m1").unwrap().unwrap();
        assert_eq!(got.request_hash, "deadbeef");
        assert_eq!(got.response_status, 200);
        assert_eq!(s.idempotency_count().unwrap(), 1);

        // Force expiry by writing an already-expired entry.
        let stale = IdempotencyEntry {
            message_id: "m2".into(),
            request_hash: "feed".into(),
            response_status: 200,
            response_body: json!(null),
            created_at: now - Duration::hours(2),
            expires_at: now - Duration::hours(1),
        };
        s.idempotency_put(&stale).unwrap();
        let purged = s.idempotency_purge_expired().unwrap();
        assert_eq!(purged, 1);
        assert!(s.idempotency_get("m2").unwrap().is_none());
        assert!(s.idempotency_get("m1").unwrap().is_some());
    }

    #[test]
    fn idempotency_evict_oldest_removes_in_age_order() {
        let s = TaskStore::open_in_memory().unwrap();
        let base = Utc::now();
        // Insert 5 entries with strictly increasing created_at.
        for i in 0..5u32 {
            let entry = IdempotencyEntry {
                message_id: format!("m{i}"),
                request_hash: format!("h{i}"),
                response_status: 200,
                response_body: json!({"i": i}),
                created_at: base + Duration::seconds(i as i64),
                expires_at: base + Duration::hours(1),
            };
            s.idempotency_put(&entry).unwrap();
        }
        assert_eq!(s.idempotency_count().unwrap(), 5);

        // No-op for zero.
        assert_eq!(s.idempotency_evict_oldest(0).unwrap(), 0);
        assert_eq!(s.idempotency_count().unwrap(), 5);

        // Evict the two oldest (m0, m1).
        let removed = s.idempotency_evict_oldest(2).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(s.idempotency_count().unwrap(), 3);
        assert!(s.idempotency_get("m0").unwrap().is_none());
        assert!(s.idempotency_get("m1").unwrap().is_none());
        assert!(s.idempotency_get("m2").unwrap().is_some());
        assert!(s.idempotency_get("m4").unwrap().is_some());

        // Requesting more than present caps at table size.
        let removed = s.idempotency_evict_oldest(100).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(s.idempotency_count().unwrap(), 0);
    }

    #[test]
    fn restart_preserves_non_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.db");
        let row = task("persist", TaskState::Working, ts(2026, 2, 1));
        {
            let s = TaskStore::open(&path).unwrap();
            s.upsert_task(&row).unwrap();
        }
        let s2 = TaskStore::open(&path).unwrap();
        let got = s2.get_task("persist").unwrap().unwrap();
        assert_eq!(got, row);
    }

    #[test]
    fn purge_expired_removes_terminal_past_retention() {
        let s = TaskStore::open_in_memory().unwrap();
        let now = Utc::now();
        let stale = TaskRow {
            task_id: "old".into(),
            context_id: None,
            state: TaskState::Completed,
            fail_kind: None,
            status_json: json!({"state": "completed"}),
            metadata_json: None,
            created_at: now - Duration::days(40),
            updated_at: now - Duration::days(31),
            terminal_at: Some(now - Duration::days(31)),
        };
        let fresh = TaskRow {
            task_id: "new".into(),
            context_id: None,
            state: TaskState::Completed,
            fail_kind: None,
            status_json: json!({"state": "completed"}),
            metadata_json: None,
            created_at: now - Duration::days(2),
            updated_at: now - Duration::days(1),
            terminal_at: Some(now - Duration::days(1)),
        };
        s.upsert_task(&stale).unwrap();
        s.upsert_task(&fresh).unwrap();
        let removed = s.purge_expired(Duration::days(30)).unwrap();
        assert_eq!(removed, 1);
        assert!(s.get_task("old").unwrap().is_none());
        assert!(s.get_task("new").unwrap().is_some());
    }

    #[test]
    fn atomic_write_under_crash_injection() {
        // Approximate crash injection via transactional rollback: open a
        // transaction, insert a task, drop without commit. A subsequent read
        // must NOT see the row, proving upserts are atomic at the SQL layer.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.db");
        {
            let s = TaskStore::open(&path).unwrap();
            let conn = s.lock();
            let tx = conn.unchecked_transaction().unwrap();
            tx.execute(
                "INSERT INTO tasks (task_id, context_id, state, fail_kind, status_json, \
                 metadata_json, created_at, updated_at, terminal_at) \
                 VALUES (?1, NULL, ?2, NULL, ?3, NULL, ?4, ?4, NULL)",
                params![
                    "ghost",
                    "working",
                    "{}",
                    fmt_ts(&Utc::now()),
                ],
            )
            .unwrap();
            // Drop tx without commit; rusqlite rolls back on drop.
            drop(tx);
        }
        let s2 = TaskStore::open(&path).unwrap();
        assert!(s2.get_task("ghost").unwrap().is_none());
        assert_eq!(s2.count_tasks().unwrap(), 0);
    }
}
