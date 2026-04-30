//! `agent` verbs.
//!
//! Backing routes:
//! - `GET  /api/v1/agents`                                      ← `agent list`
//! - `GET  /api/v1/agents/{id}`                                 ← `agent get <id>`
//! - `POST /api/v1/agents/{id}/stop`                            ← `agent stop <id>`
//! - `POST /api/v1/agents/{id}/rotate-secret`                   ← `agent rotate-secret <id>`
//! - `GET  /api/v1/agents/{id}/manifests/{platform}`            ← `agent manifests list`
//! - `GET  /api/v1/agents/{id}/manifests/{platform}/{name}`     ← `agent manifests get`
//! - `POST /api/v1/agents/{id}/manifests/{platform}/{name}`     ← `agent manifests push`
//! - WS `agent shell <id>` — convenience: create a session, attach as controller

use anyhow::Result;
use serde_json::Value;

use crate::client::http::HttpClient;
use crate::output::{jstr, kv, table};

pub async fn list(c: &HttpClient, state: Option<&str>, as_json: bool) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(s) = state {
        q.push(("state".into(), s.into()));
    }
    let path = super::with_query("/api/v1/agents", &q);
    let v: Value = c.get_value(&path).await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("agents")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|a| {
                vec![
                    jstr(a, "id", "").to_string(),
                    jstr(a, "hostname", "-").to_string(),
                    jstr(a, "ip_address", "-").to_string(),
                    status_str(a),
                    jstr(a, "profile", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["ID", "HOSTNAME", "IP", "STATUS", "PROFILE"], &rows)
    })
}

pub async fn get(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    let v: Value = c.get_value(&format!("/api/v1/agents/{}", id)).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("id", jstr(&v, "id", "").to_string()),
            ("instance_id", jstr(&v, "instance_id", "-").to_string()),
            ("hostname", jstr(&v, "hostname", "-").to_string()),
            ("ip_address", jstr(&v, "ip_address", "-").to_string()),
            ("status", status_str(&v)),
            ("profile", jstr(&v, "profile", "-").to_string()),
            ("loadout", jstr(&v, "loadout", "-").to_string()),
            ("connected_at", crate::output::jnum(&v, "connected_at")),
            ("last_heartbeat", crate::output::jnum(&v, "last_heartbeat")),
        ];
        kv::render(&pairs)
    })
}

pub async fn manifests_list(c: &HttpClient, id: &str, platform: &str, as_json: bool) -> Result<()> {
    let path = format!("/api/v1/agents/{}/manifests/{}", id, platform);
    let v: Value = c.get_value(&path).await?;
    super::emit(&v, as_json, || {
        let arr = v.as_array().cloned().unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|m| {
                vec![
                    jstr(m, "name", "").to_string(),
                    crate::output::jnum(m, "size"),
                    jstr(m, "modified", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["NAME", "SIZE", "MODIFIED"], &rows)
    })
}

pub async fn manifests_get(
    c: &HttpClient,
    id: &str,
    platform: &str,
    name: &str,
    as_json: bool,
) -> Result<()> {
    let path = format!("/api/v1/agents/{}/manifests/{}/{}", id, platform, name);
    let v: Value = c.get_value(&path).await?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        // Manifests are content blobs; just dump the body.
        if let Some(content) = v.get("content").and_then(|x| x.as_str()) {
            print!("{}", content);
        } else {
            print!("{}", serde_json::to_string_pretty(&v)?);
        }
    }
    Ok(())
}

/// `agent stop <id>` — graceful stop, delegates to `vm stop` server-side.
pub async fn stop(c: &HttpClient, id: &str, as_json: bool) -> Result<()> {
    let v: Value = c
        .post_json::<Value, ()>(&format!("/api/v1/agents/{}/stop", id), None)
        .await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("agent_id", id.into()),
            ("name", jstr(&v, "name", "-").to_string()),
            ("action", jstr(&v, "action", "-").to_string()),
            ("state", jstr(&v, "state", "-").to_string()),
        ];
        kv::render(&pairs)
    })
}

/// `agent manifests push` — POST a raw manifest blob to the AIWG-proxy
/// path on the agent. Body shape: `{ content: <text> }`.
pub async fn manifests_push(
    c: &HttpClient,
    id: &str,
    platform: &str,
    name: &str,
    content: &str,
    as_json: bool,
) -> Result<()> {
    let path = format!("/api/v1/agents/{}/manifests/{}/{}", id, platform, name);
    let body = serde_json::json!({ "content": content });
    let v: Value = c.post_json(&path, Some(&body)).await?;
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("agent_id", id.into()),
            ("platform", platform.into()),
            ("name", name.into()),
            ("ok", crate::output::jnum(&v, "ok")),
            ("bytes", content.len().to_string()),
        ];
        kv::render(&pairs)
    })
}

/// `agent rotate-secret <id> [--wait]` — POST kicks off a rotation;
/// server returns `{ operation_id, deadline_ms, grace_seconds }` (202).
/// Old secret stays valid until the agent re-registers with the new
/// one or the grace window expires.
pub async fn rotate_secret(c: &HttpClient, id: &str, wait: bool, as_json: bool) -> Result<()> {
    let v: Value = c
        .post_json::<Value, ()>(&format!("/api/v1/agents/{}/rotate-secret", id), None)
        .await?;
    if !wait {
        return super::emit(&v, as_json, || {
            let pairs: Vec<(&str, String)> = vec![
                ("agent_id", id.into()),
                ("operation_id", jstr(&v, "operation_id", "-").to_string()),
                ("status", jstr(&v, "status", "-").to_string()),
                ("grace_seconds", crate::output::jnum(&v, "grace_seconds")),
                ("deadline_ms", crate::output::jnum(&v, "deadline_ms")),
                ("note", "rotation accepted; pass --wait to block until terminal".into()),
            ];
            kv::render(&pairs)
        });
    }
    let op_id = jstr(&v, "operation_id", "");
    if op_id.is_empty() {
        anyhow::bail!("server did not return operation_id; got: {}", v);
    }
    let final_v =
        super::ops::wait_inner(c, op_id, std::time::Duration::from_secs(600)).await?;
    super::emit(&final_v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("operation_id", jstr(&final_v, "id", "-").to_string()),
            ("status", jstr(&final_v, "status", "-").to_string()),
            ("error", jstr(&final_v, "error", "-").to_string()),
        ];
        kv::render(&pairs)
    })?;
    match jstr(&final_v, "status", "") {
        "completed" => Ok(()),
        _ => Err(anyhow::anyhow!(
            "rotate-secret failed: {}",
            jstr(&final_v, "error", "(no error message)")
        )),
    }
}

/// `agent shell <id>` — create an interactive session on the agent and
/// attach as controller in one step. Convenience wrapper that uses the
/// legacy POST /api/v1/agents/{id}/sessions path to spawn the PTY, then
/// hands off to `session::attach` with the returned formal session_id.
///
/// The legacy session-create response carries `session_id` (the formal
/// id we attach to). On success we exec straight into the attach loop;
/// on failure we surface the error verbatim.
pub async fn shell(c: &HttpClient, id: &str, command: Option<&str>) -> Result<()> {
    // Reasonable defaults for the spawned PTY. Operator can resize once
    // attached; this is the initial size sent to the agent.
    let (cols, rows) = crate::pty::current_size();
    let body = serde_json::json!({
        "session_name": "sandboxctl",
        "session_type": "interactive",
        "command":   command.unwrap_or("/bin/bash"),
        "args":      Vec::<String>::new(),
        "cols":      cols,
        "rows":      rows,
    });
    let v: Value = c
        .post_json(&format!("/api/v1/agents/{}/sessions", id), Some(&body))
        .await?;
    let sid = jstr(&v, "session_id", "");
    if sid.is_empty() {
        anyhow::bail!(
            "agent shell: server did not return session_id; got: {}",
            v
        );
    }
    super::session::attach(c, sid, true, None).await
}

fn status_str(v: &Value) -> String {
    // The mgmt server emits AgentStatus as either a string or a struct; tolerate both.
    if let Some(s) = v.get("status").and_then(|x| x.as_str()) {
        return s.to_string();
    }
    if let Some(o) = v.get("status").and_then(|x| x.as_object()) {
        if let Some((k, _)) = o.iter().next() {
            return k.clone();
        }
    }
    "-".to_string()
}
