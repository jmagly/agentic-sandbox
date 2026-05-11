//! `ops` — long-running operations tracker.
//!
//! Backing routes:
//! - `GET /api/v1/operations/{id}`   ← `ops get <id>`, `ops wait <id> --timeout`
//!
//! Wire format (`management/src/http/operations.rs:182-210`): `OperationState`
//! is serialized with `tag = "status"` and `rename_all = "lowercase"`, then
//! flattened into `OperationResponse`. So the response is:
//! ```json
//! { "id": ..., "type": "vm_create", "status": "completed",
//!   "target": "...", "progress_percent": 100, ... }
//! ```
//! Failed operations include an `error` field at the same level.

use anyhow::Result;
use serde_json::Value;
use std::time::{Duration, Instant};

use crate::client::http::HttpClient;
use crate::output::{jstr, kv};

pub async fn get(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    // v2-first: /api/v2/admin/operations/{id}. v1 legacy: /api/v1/operations/{id}.
    let (v, _via_v1) = c
        .try_v2_then_v1(
            &format!("/api/v2/admin/operations/{}", id),
            &format!("/api/v1/operations/{}", id),
            "GET",
            None,
        )
        .await?;
    render(&v, as_json)
}

/// Public wait used by `ops wait` and by other verbs (`vm create --wait` etc.).
/// Polls until status is terminal (`completed` or `failed`) or timeout elapses.
/// Maps timeout to `ClientError::Timeout` so the global error handler exits 5.
pub async fn wait(c: &HttpClient, id: &str, timeout: Duration, as_json: bool) -> Result<()> {
    let v = wait_inner(c, id, timeout).await?;
    render(&v, as_json)?;
    // Final status governs the process exit code: completed ⇒ Ok, failed ⇒ Err.
    match jstr(&v, "status", "") {
        "completed" => Ok(()),
        "failed" => Err(anyhow::anyhow!(
            "operation failed: {}",
            jstr(&v, "error", "(no error message)")
        )),
        other => Err(anyhow::anyhow!(
            "operation ended in unexpected state: {}",
            other
        )),
    }
}

/// Poll until terminal. Returns the final operation JSON value.
/// Exposed so other verbs can `--wait` on the operations they create
/// (vm create, vm restart, vm reprovision, vm deploy-agent).
pub async fn wait_inner(c: &HttpClient, id: &str, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    let mut backoff = Duration::from_millis(250);
    loop {
        let (v, _via_v1) = c
            .try_v2_then_v1(
                &format!("/api/v2/admin/operations/{}", id),
                &format!("/api/v1/operations/{}", id),
                "GET",
                None,
            )
            .await?;
        let status = jstr(&v, "status", "");
        if matches!(status, "completed" | "failed") {
            return Ok(v);
        }
        if started.elapsed() >= timeout {
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
            ("id", jstr(v, "id", "").to_string()),
            ("type", jstr(v, "type", "-").to_string()),
            ("status", jstr(v, "status", "-").to_string()),
            ("target", jstr(v, "target", "-").to_string()),
            (
                "progress_percent",
                crate::output::jnum(v, "progress_percent"),
            ),
            ("created_at", jstr(v, "created_at", "-").to_string()),
            ("completed_at", jstr(v, "completed_at", "-").to_string()),
            ("error", jstr(v, "error", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}
