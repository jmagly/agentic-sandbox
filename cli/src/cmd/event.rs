//! `event list` — buffered server-side events snapshot.
//!
//! Backing route: `GET /api/v1/events` (poll snapshot). Tail/follow mode
//! is on the SSE form, lands in #162 alongside `event tail`.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::{jstr, table};

pub async fn list(
    c: &HttpClient,
    source: Option<&str>,
    since: Option<&str>,
    event_type: Option<&str>,
    as_json: bool,
) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(s) = source {
        q.push(("source".into(), s.into()));
    }
    if let Some(s) = since {
        // Accept either an RFC3339 timestamp or a duration like "1h".
        let ts = if let Ok(d) = super::parse_duration(s) {
            let now = Utc::now();
            let then = now - chrono::Duration::from_std(d).unwrap_or_default();
            then.to_rfc3339()
        } else if DateTime::parse_from_rfc3339(s).is_ok() {
            s.to_string()
        } else {
            anyhow::bail!("--since must be a duration (e.g. 1h) or RFC3339 timestamp");
        };
        q.push(("since".into(), ts));
    }
    if let Some(t) = event_type {
        q.push(("event_type".into(), t.into()));
    }
    let path = super::with_query("/api/v1/events", &q);
    let v: Value = c.get_value(&path).await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("events")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|e| {
                vec![
                    jstr(e, "timestamp", "-").to_string(),
                    jstr(e, "event_type", "-").to_string(),
                    jstr(e, "vm_name", "-").to_string(),
                    jstr(e, "agent_id", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["TIMESTAMP", "TYPE", "SOURCE", "AGENT"], &rows)
    })
}
