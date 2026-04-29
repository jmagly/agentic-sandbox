//! `ops` — long-running operations tracker.
//!
//! Backing routes:
//! - `GET /api/v1/operations/{id}`   ← `ops get <id>`, `ops wait <id> --timeout`

use anyhow::Result;
use serde_json::Value;
use std::time::{Duration, Instant};

use crate::client::http::HttpClient;
use crate::output::{jstr, kv};

pub async fn get(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(&format!("/api/v1/operations/{}", id)).await?;
    render(&v, as_json)
}

pub async fn wait(c: &HttpClient, id: &str, timeout: Duration, as_json: bool) -> Result<()> {
    let started = Instant::now();
    let mut backoff = Duration::from_millis(250);
    loop {
        let v: Value = c.get_value(&format!("/api/v1/operations/{}", id)).await?;
        let state = jstr(&v, "state", "");
        if matches!(state, "completed" | "failed" | "cancelled") {
            return render(&v, as_json);
        }
        if started.elapsed() >= timeout {
            // Mirror the documented timeout exit code via ClientError::Timeout.
            return Err(anyhow::Error::new(
                crate::client::http::ClientError::Timeout(timeout),
            ));
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

fn render(v: &Value, as_json: bool) -> Result<()> {
    super::emit(v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("operation_id", jstr(v, "operation_id", "").to_string()),
            ("op_type", jstr(v, "op_type", "-").to_string()),
            ("target", jstr(v, "target", "-").to_string()),
            ("state", jstr(v, "state", "-").to_string()),
            ("progress", crate::output::jnum(v, "progress")),
            ("created_at", jstr(v, "created_at", "-").to_string()),
            ("completed_at", jstr(v, "completed_at", "-").to_string()),
            ("error", jstr(v, "error", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}
