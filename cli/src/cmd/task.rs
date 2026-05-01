//! `task` verbs.
//!
//! Backing routes:
//! - `GET    /api/v1/tasks`                               ← `task list` (`--state`, `--limit`, `--offset`)
//! - `GET    /api/v1/tasks/{id}`                          ← `task get <id>`
//! - `GET    /api/v1/tasks/{id}/artifacts`                ← `task artifacts list <id>`
//! - `POST   /api/v1/tasks`                               ← `task submit -f manifest.{yaml,json}` (--wait)
//! - `DELETE /api/v1/tasks/{id}`                          ← `task cancel <id> [--reason]`

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::{jstr, kv, table};

#[allow(unused_imports)]
use std::path::Path as _;

pub async fn list(
    c: &HttpClient,
    state: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
    as_json: bool,
) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(s) = state {
        q.push(("state".into(), s.into()));
    }
    if let Some(l) = limit {
        q.push(("limit".into(), l.to_string()));
    }
    if let Some(o) = offset {
        q.push(("offset".into(), o.to_string()));
    }
    let path = super::with_query("/api/v1/tasks", &q);
    let v: Value = c.get_value(&path).await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("tasks")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|t| {
                vec![
                    jstr(t, "id", "").to_string(),
                    jstr(t, "name", "-").to_string(),
                    jstr(t, "state", "-").to_string(),
                    jstr(t, "created_at", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["ID", "NAME", "STATE", "CREATED"], &rows)
    })
}

pub async fn get(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(&format!("/api/v1/tasks/{}", id)).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("id", jstr(&v, "id", "").to_string()),
            ("name", jstr(&v, "name", "-").to_string()),
            ("state", jstr(&v, "state", "-").to_string()),
            ("created_at", jstr(&v, "created_at", "-").to_string()),
            ("started_at", jstr(&v, "started_at", "-").to_string()),
            ("vm_name", jstr(&v, "vm_name", "-").to_string()),
            ("exit_code", crate::output::jnum(&v, "exit_code")),
        ];
        kv::render(&pairs)
    })
}

pub async fn submit(
    c: &HttpClient,
    file_path: &std::path::Path,
    wait: bool,
    as_json: bool,
) -> Result<()> {
    let raw = std::fs::read_to_string(file_path)
        .map_err(|e| anyhow::anyhow!("reading {}: {}", file_path.display(), e))?;
    // YAML extension ⇒ send as `manifest_yaml`; .json (or anything else)
    // ⇒ parse and send as `manifest`. The server distinguishes via the
    // presence of these two fields (tasks.rs:129-212).
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let body = if ext == "yaml" || ext == "yml" {
        serde_json::json!({ "manifest_yaml": raw })
    } else {
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("parsing JSON manifest: {}", e))?;
        serde_json::json!({ "manifest": parsed })
    };

    let v: Value = c.post_json("/api/v1/tasks", Some(&body)).await?;
    if !wait {
        return super::emit(&v, as_json, || {
            let pairs: Vec<(&str, String)> = vec![
                ("task_id", jstr(&v, "task_id", "-").to_string()),
                ("accepted", crate::output::jnum(&v, "accepted")),
                ("error", jstr(&v, "error", "-").to_string()),
                (
                    "note",
                    "task submitted; pass --wait to block until terminal state".into(),
                ),
            ];
            kv::render(&pairs)
        });
    }
    let task_id = jstr(&v, "task_id", "");
    if task_id.is_empty() {
        anyhow::bail!("server did not return a task_id; got: {}", v);
    }
    // Poll task state until terminal. Tasks aren't operations — they
    // live under /api/v1/tasks/{id} with their own state machine.
    let final_v = wait_for_task(c, task_id).await?;
    super::emit(&final_v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("id", jstr(&final_v, "id", "-").to_string()),
            ("name", jstr(&final_v, "name", "-").to_string()),
            ("state", jstr(&final_v, "state", "-").to_string()),
            ("exit_code", crate::output::jnum(&final_v, "exit_code")),
            ("error", jstr(&final_v, "error", "-").to_string()),
        ];
        kv::render(&pairs)
    })?;
    // Map the terminal task state to the process exit code.
    match jstr(&final_v, "state", "") {
        "completed" | "succeeded" | "success" => Ok(()),
        "failed" | "cancelled" | "canceled" => Err(anyhow::anyhow!(
            "task ended: {} ({})",
            jstr(&final_v, "state", "?"),
            jstr(&final_v, "error", "no error reported")
        )),
        other => Err(anyhow::anyhow!("task ended in unexpected state: {}", other)),
    }
}

async fn wait_for_task(c: &HttpClient, id: &str) -> Result<Value> {
    use std::time::{Duration, Instant};
    let started = Instant::now();
    let deadline = Duration::from_secs(3600); // 1h cap; tasks can run long
    let mut backoff = Duration::from_millis(500);
    loop {
        let v: Value = c.get_value(&format!("/api/v1/tasks/{}", id)).await?;
        let state = jstr(&v, "state", "");
        if matches!(
            state,
            "completed" | "succeeded" | "success" | "failed" | "cancelled" | "canceled"
        ) {
            return Ok(v);
        }
        if started.elapsed() >= deadline {
            return Err(anyhow::Error::new(
                crate::client::http::ClientError::Timeout(deadline),
            ));
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

pub async fn cancel(c: &HttpClient, id: &str, reason: Option<&str>, as_json: bool) -> Result<()> {
    let path = format!("/api/v1/tasks/{}", id);
    let v: Value = match reason {
        Some(r) => {
            let body = serde_json::json!({ "reason": r });
            c.delete_with_body(&path, &body).await?
        }
        None => c.delete_json(&path).await?,
    };
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("task_id", id.into()),
            ("success", crate::output::jnum(&v, "success")),
            ("error", jstr(&v, "error", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}

/// `task logs <id> --follow` — SSE-tail of `/api/v1/tasks/{id}/logs`.
/// Without `--follow` we just print the buffered snapshot; the same
/// route is used for both, distinguished by `?follow=true` if the
/// server supports it (otherwise the SSE stream still emits whatever
/// the route returns once and closes).
pub async fn logs(c: &HttpClient, id: &str, follow: bool) -> Result<()> {
    use crate::client::sse::SseStream;
    use futures_util::StreamExt;
    let path = if follow {
        format!("/api/v1/tasks/{}/logs?follow=true", id)
    } else {
        format!("/api/v1/tasks/{}/logs", id)
    };
    let mut s = SseStream::open(c, &path).await?;
    while let Some(ev) = s.next().await {
        let ev = ev?;
        // Tasks emit JSON-encoded log entries; we pass them through.
        if !ev.data.is_empty() {
            println!("{}", ev.data);
        }
    }
    Ok(())
}

pub async fn artifacts_list(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    let v: Value = c
        .get_value(&format!("/api/v1/tasks/{}/artifacts", id))
        .await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("artifacts")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|a| {
                vec![
                    jstr(a, "name", "").to_string(),
                    crate::output::jnum(a, "size_bytes"),
                    jstr(a, "content_type", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["NAME", "SIZE", "CONTENT_TYPE"], &rows)
    })
}
