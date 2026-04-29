//! `session` read-only verbs against the formal session registry.
//!
//! Backing routes:
//! - `GET /api/v1/sessions`              ← `session list` (`--agent`)
//! - `GET /api/v1/sessions` + filter     ← `session get <id>` (no GET /api/v1/sessions/{id} yet)

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::{jstr, kv, table};

pub async fn list(c: &HttpClient, agent: Option<&str>, as_json: bool) -> Result<()> {
    let v: Value = c.get_value("/api/v1/sessions").await?;
    let arr = v.as_array().cloned().unwrap_or_default();
    let filtered: Vec<Value> = arr
        .into_iter()
        .filter(|s| match agent {
            Some(a) => jstr(s, "agent_id", "") == a,
            None => true,
        })
        .collect();
    let payload = Value::Array(filtered.clone());
    super::emit(&payload, as_json, || {
        let rows: Vec<Vec<String>> = filtered
            .iter()
            .map(|s| {
                vec![
                    jstr(s, "session_id", "").to_string(),
                    jstr(s, "agent_id", "-").to_string(),
                    jstr(s, "name", "-").to_string(),
                    crate::output::jnum(s, "attachment_count"),
                    crate::output::jnum(s, "max_client_lag"),
                ]
            })
            .collect();
        table::render(&["SESSION_ID", "AGENT", "NAME", "ATT", "LAG"], &rows)
    })
}

pub async fn get(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    // No dedicated GET /api/v1/sessions/{id} exists yet; filter the list.
    let v: Value = c.get_value("/api/v1/sessions").await?;
    let arr = v.as_array().cloned().unwrap_or_default();
    let s = arr
        .iter()
        .find(|s| jstr(s, "session_id", "") == id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("session not found: {}", id))?;
    super::emit(&s, as_json, || {
        let controllers = s
            .get("controllers")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let observers = s
            .get("observers")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let pairs: Vec<(&str, String)> = vec![
            ("session_id", jstr(&s, "session_id", "").to_string()),
            ("agent_id", jstr(&s, "agent_id", "-").to_string()),
            ("command_id", jstr(&s, "command_id", "-").to_string()),
            ("name", jstr(&s, "name", "-").to_string()),
            ("attachment_count", crate::output::jnum(&s, "attachment_count")),
            ("controllers", controllers),
            ("observers", observers),
            ("replay_oldest_seq", crate::output::jnum(&s, "replay_oldest_seq")),
            ("replay_newest_seq", crate::output::jnum(&s, "replay_newest_seq")),
            ("replay_len", crate::output::jnum(&s, "replay_len")),
            ("replay_total_bytes", crate::output::jnum(&s, "replay_total_bytes")),
            ("max_client_lag", crate::output::jnum(&s, "max_client_lag")),
        ];
        kv::render(&pairs)
    })
}
