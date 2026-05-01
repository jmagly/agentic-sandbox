//! `event` verbs — buffered server-side events.
//!
//! Backing routes:
//! - `GET /api/v1/events`                 ← `event list` (snapshot)
//! - `GET /api/v1/events?follow=true&...` ← `event tail` (SSE)

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

/// `event tail` — SSE follow on `/api/v1/events?follow=true`. Same
/// query filters as `event list`, plus a client-side regex filter
/// applied to each event's wire-format JSON line.
pub async fn tail(
    c: &HttpClient,
    source: Option<&str>,
    since: Option<&str>,
    event_type: Option<&str>,
    filter: Option<&str>,
) -> Result<()> {
    use crate::client::sse::SseStream;
    use futures_util::StreamExt;

    let mut q: Vec<(String, String)> = vec![("follow".into(), "true".into())];
    if let Some(s) = source {
        q.push(("source".into(), s.into()));
    }
    if let Some(s) = since {
        let ts = if let Ok(d) = super::parse_duration(s) {
            (Utc::now() - chrono::Duration::from_std(d).unwrap_or_default()).to_rfc3339()
        } else if DateTime::parse_from_rfc3339(s).is_ok() {
            s.into()
        } else {
            anyhow::bail!("--since must be a duration (e.g. 1h) or RFC3339 timestamp");
        };
        q.push(("since".into(), ts));
    }
    if let Some(t) = event_type {
        q.push(("event_type".into(), t.into()));
    }
    let path = super::with_query("/api/v1/events", &q);

    let re = match filter {
        Some(p) => Some(
            regex::Regex::new(p)
                .map_err(|e| anyhow::anyhow!("--filter is not a valid regex: {e}"))?,
        ),
        None => None,
    };

    let mut s = SseStream::open(c, &path).await?;
    while let Some(ev) = s.next().await {
        let ev = ev?;
        // Server emits a special `lagged` event when subscribers fall
        // behind; surface it on stderr so `event tail | grep ...` still
        // works, but the operator sees they missed events.
        if ev.event.as_deref() == Some("lagged") {
            eprintln!("[event tail: {}]", ev.data);
            continue;
        }
        if ev.data.is_empty() {
            continue;
        }
        if let Some(ref re) = re {
            if !re.is_match(&ev.data) {
                continue;
            }
        }
        // Line-buffered passthrough — composes with downstream tools.
        println!("{}", ev.data);
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
    }
    Ok(())
}
