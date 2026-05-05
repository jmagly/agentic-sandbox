//! GET /api/v1/logs — recent management-server tracing events.
//!
//! Backed by the in-memory ring buffer in `telemetry::log_buffer`. The
//! dashboard's "System" tab polls this for raw server logs.

use axum::extract::Query;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::telemetry::log_buffer::{snapshot, snapshot_since, LogEntry};

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    /// Maximum number of entries to return (default 200, hard cap 2000).
    pub limit: Option<usize>,
    /// RFC3339 timestamp; only return entries newer than this.
    pub since: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub logs: Vec<LogEntry>,
}

pub async fn list_logs(Query(q): Query<LogsQuery>) -> Json<LogsResponse> {
    let limit = q.limit.unwrap_or(200).min(2000);
    let logs = match q
        .since
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
    {
        Some(ts) => snapshot_since(ts.with_timezone(&Utc), limit),
        None => snapshot(limit),
    };
    Json(LogsResponse { logs })
}
