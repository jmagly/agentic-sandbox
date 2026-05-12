//! `vm` verbs.
//!
//! Backing routes:
//! - `GET    /api/v1/vms`                            ← `vm list` (`--state`, `--prefix`)
//! - `GET    /api/v1/vms/{name}`                     ← `vm get <name>`
//! - `POST   /api/v1/vms`                            ← `vm create <name> ...`
//! - `POST   /api/v1/vms/{name}/start`               ← `vm start <name>`
//! - `POST   /api/v1/vms/{name}/stop`                ← `vm stop <name>`
//! - `POST   /api/v1/vms/{name}/restart`             ← `vm restart <name> ...`
//! - `DELETE /api/v1/vms/{name}?delete_disk&force`   ← `vm destroy <name> ...`
//! - `POST   /api/v1/agents/{id}/reprovision`        ← `vm reprovision <name> ...`
//! - `POST   /api/v1/vms/{name}/deploy-agent`        ← `vm deploy-agent <name>`

use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;

use crate::client::http::HttpClient;
use crate::output::{jstr, kv, table};

/// Default --wait timeout for any vm verb that returns an operation_id.
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(600);

pub async fn list(
    c: &HttpClient,
    state: Option<&str>,
    prefix: Option<&str>,
    as_json: bool,
) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(s) = state {
        q.push(("state".into(), s.into()));
    }
    // The server's default prefix filter is `agent-` (the dashboard
    // intentionally scopes its list to agent VMs). Operators using
    // `sandboxctl` typically want every libvirt domain — match
    // `virsh list --all` semantics by sending `prefix=*` unless the
    // operator overrode it. Closes #168 from the CLI side.
    let p = prefix.unwrap_or("*");
    q.push(("prefix".into(), p.into()));
    // v2-first: GET /api/v2/admin/instances. v1 fallback: GET /api/v1/agents
    // (legacy admin route — note the resource rename instances vs agents).
    let v2_path = super::with_query("/api/v2/admin/instances", &q);
    let v1_path = super::with_query("/api/v1/vms", &q);
    let (v, _via_v1) = c.try_v2_then_v1(&v2_path, &v1_path, "GET", None).await?;
    super::emit(&v, as_json, || {
        let arr = v
            .get("vms")
            .and_then(|x| x.as_array())
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|vm| {
                vec![
                    jstr(vm, "name", "").to_string(),
                    jstr(vm, "state", "").to_string(),
                    jstr(vm, "ip_address", "-").to_string(),
                    jstr(vm, "profile", "-").to_string(),
                ]
            })
            .collect();
        table::render(&["NAME", "STATE", "IP", "PROFILE"], &rows)
    })
}

pub async fn get(c: &HttpClient, name: &str, as_json: bool) -> Result<()> {
    let (v, _via_v1) = c
        .try_v2_then_v1(
            &format!("/api/v2/admin/instances/{}", name),
            &format!("/api/v1/vms/{}", name),
            "GET",
            None,
        )
        .await?;
    super::emit(&v, as_json, || {
        let pairs = vec![
            ("name", jstr(&v, "name", "").to_string()),
            ("state", jstr(&v, "state", "").to_string()),
            ("ip_address", jstr(&v, "ip_address", "-").to_string()),
            ("profile", jstr(&v, "profile", "-").to_string()),
            ("loadout", jstr(&v, "loadout", "-").to_string()),
            ("memory_mb", crate::output::jnum(&v, "memory_mb")),
            ("vcpus", crate::output::jnum(&v, "vcpus")),
        ];
        kv::render(
            &pairs
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<Vec<_>>(),
        )
    })
}

/// `vm create` — POST /api/v1/vms with the full create-request body.
/// Server responds with `CreateOperationResponse { operation, vm? }`.
/// `--wait` polls /api/v1/operations/{id} until terminal.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    c: &HttpClient,
    name: &str,
    profile: Option<&str>,
    loadout: Option<&str>,
    vcpus: Option<u32>,
    memory_mb: Option<u32>,
    disk_gb: Option<u32>,
    agentshare: bool,
    start: bool,
    wait: bool,
    as_json: bool,
) -> Result<()> {
    let mut body = serde_json::Map::new();
    body.insert("name".into(), Value::String(name.into()));
    if let Some(p) = profile {
        body.insert("profile".into(), Value::String(p.into()));
    }
    if let Some(l) = loadout {
        body.insert("loadout".into(), Value::String(l.into()));
    }
    if let Some(n) = vcpus {
        body.insert("vcpus".into(), json!(n));
    }
    if let Some(n) = memory_mb {
        body.insert("memory_mb".into(), json!(n));
    }
    if let Some(n) = disk_gb {
        body.insert("disk_gb".into(), json!(n));
    }
    body.insert("agentshare".into(), Value::Bool(agentshare));
    body.insert("start".into(), Value::Bool(start));
    let body = Value::Object(body);

    let (v, _via_v1) = c
        .try_v2_then_v1(
            "/api/v2/admin/instances",
            "/api/v1/vms",
            "POST",
            Some(&body),
        )
        .await?;
    if !wait {
        return super::emit(&v, as_json, || render_op_envelope(&v));
    }
    let op_id = op_id_from_envelope(&v)?;
    let final_v = super::ops::wait_inner(c, &op_id, DEFAULT_WAIT_TIMEOUT).await?;
    super::emit(&final_v, as_json, || render_terminal_op(&final_v))?;
    terminal_to_result(&final_v)
}

pub async fn start(c: &HttpClient, name: &str, as_json: bool) -> Result<()> {
    let (v, _via_v1) = c
        .try_v2_then_v1(
            &format!("/api/v2/admin/instances/{}/start", name),
            &format!("/api/v1/vms/{}/start", name),
            "POST",
            None,
        )
        .await?;
    super::emit(&v, as_json, || render_action(&v))
}

pub async fn stop(c: &HttpClient, name: &str, as_json: bool) -> Result<()> {
    let (v, _via_v1) = c
        .try_v2_then_v1(
            &format!("/api/v2/admin/instances/{}/stop", name),
            &format!("/api/v1/vms/{}/stop", name),
            "POST",
            None,
        )
        .await?;
    super::emit(&v, as_json, || render_action(&v))
}

/// `vm restart` — POST with `{ mode, timeout_seconds }`. Returns
/// `CreateOperationResponse`.
pub async fn restart(
    c: &HttpClient,
    name: &str,
    hard: bool,
    timeout_seconds: u64,
    wait: bool,
    as_json: bool,
) -> Result<()> {
    let body = json!({
        "mode": if hard { "hard" } else { "graceful" },
        "timeout_seconds": timeout_seconds,
    });
    let (v, _via_v1) = c
        .try_v2_then_v1(
            &format!("/api/v2/admin/instances/{}/restart", name),
            &format!("/api/v1/vms/{}/restart", name),
            "POST",
            Some(&body),
        )
        .await?;
    if !wait {
        return super::emit(&v, as_json, || render_op_envelope(&v));
    }
    let op_id = op_id_from_envelope(&v)?;
    let final_v = super::ops::wait_inner(c, &op_id, DEFAULT_WAIT_TIMEOUT).await?;
    super::emit(&final_v, as_json, || render_terminal_op(&final_v))?;
    terminal_to_result(&final_v)
}

/// `vm destroy` — DELETE with `?delete_disk=&force=`. Returns immediate
/// `DeleteVmResponse` (no operation polling on this route).
pub async fn destroy(
    c: &HttpClient,
    name: &str,
    force: bool,
    delete_disk: bool,
    as_json: bool,
) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if force {
        q.push(("force".into(), "true".into()));
    }
    if delete_disk {
        q.push(("delete_disk".into(), "true".into()));
    }
    // v2: POST /admin/instances/{id}/destroy (per OpenAPI). v1 legacy: DELETE /api/v1/vms/{name}.
    // The v2 admin API converged on POST /destroy with query flags. Try v2 POST first; on 404
    // fall through to the v1 DELETE for old servers.
    let v2_path = super::with_query(&format!("/api/v2/admin/instances/{}/destroy", name), &q);
    let v1_path = super::with_query(&format!("/api/v1/vms/{}", name), &q);
    let v: Value = match c.try_v2_then_v1(&v2_path, &v1_path, "POST", None).await {
        Ok((v, _)) => v,
        Err(_) => {
            // Some older v2 servers might use DELETE on instances/{id} directly. Fall through.
            let v1_only = c.delete_json(&v1_path).await?;
            v1_only
        }
    };
    super::emit(&v, as_json, || {
        let pairs: Vec<(&str, String)> = vec![
            ("name", jstr(&v, "name", "").to_string()),
            ("deleted", crate::output::jnum(&v, "deleted")),
            ("disk_deleted", crate::output::jnum(&v, "disk_deleted")),
        ];
        kv::render(&pairs)
    })
}

pub async fn reprovision(c: &HttpClient, name: &str, wait: bool, as_json: bool) -> Result<()> {
    let (v, _via_v1) = c
        .try_v2_then_v1(
            &format!("/api/v2/admin/instances/{}/reprovision", name),
            &format!("/api/v1/agents/{}/reprovision", name),
            "POST",
            None,
        )
        .await?;
    if !wait {
        return super::emit(&v, as_json, || render_flat_op_response(&v));
    }
    // The reprovision response is FLAT: { operation_id, status }.
    let op_id = jstr(&v, "operation_id", "");
    if op_id.is_empty() {
        anyhow::bail!("server did not return an operation_id; got: {}", v);
    }
    let final_v = super::ops::wait_inner(c, op_id, DEFAULT_WAIT_TIMEOUT).await?;
    super::emit(&final_v, as_json, || render_terminal_op(&final_v))?;
    terminal_to_result(&final_v)
}

pub async fn deploy_agent(c: &HttpClient, name: &str, wait: bool, as_json: bool) -> Result<()> {
    let v: Value = c
        .post_json::<Value, ()>(&format!("/api/v1/vms/{}/deploy-agent", name), None)
        .await?;
    if !wait {
        return super::emit(&v, as_json, || render_op_envelope(&v));
    }
    let op_id = op_id_from_envelope(&v)?;
    let final_v = super::ops::wait_inner(c, &op_id, DEFAULT_WAIT_TIMEOUT).await?;
    super::emit(&final_v, as_json, || render_terminal_op(&final_v))?;
    terminal_to_result(&final_v)
}

// ── helpers ──────────────────────────────────────────────────────────────

/// Pull `operation.id` out of `CreateOperationResponse { operation: { id, ... }, vm? }`.
/// (`OperationResponse.id` is the canonical field per `operations.rs:198`.)
pub(crate) fn op_id_from_envelope(v: &Value) -> Result<String> {
    let id = v
        .get("operation")
        .and_then(|op| op.get("id"))
        .and_then(|x| x.as_str());
    match id {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => anyhow::bail!("expected `operation.id` in response, got: {}", v),
    }
}

/// Render the immediate `CreateOperationResponse` (operation accepted; no wait).
fn render_op_envelope(v: &Value) -> String {
    let op = v.get("operation").cloned().unwrap_or(Value::Null);
    let pairs: Vec<(&str, String)> = vec![
        ("operation_id", jstr(&op, "id", "-").to_string()),
        ("type", jstr(&op, "type", "-").to_string()),
        ("status", jstr(&op, "status", "-").to_string()),
        ("target", jstr(&op, "target", "-").to_string()),
        (
            "note",
            "operation accepted; pass --wait to block until terminal".into(),
        ),
    ];
    kv::render(&pairs)
}

/// Render the flat reprovision response `{ operation_id, status }`.
fn render_flat_op_response(v: &Value) -> String {
    let pairs: Vec<(&str, String)> = vec![
        ("operation_id", jstr(v, "operation_id", "-").to_string()),
        ("status", jstr(v, "status", "-").to_string()),
        (
            "note",
            "operation accepted; pass --wait to block until terminal".into(),
        ),
    ];
    kv::render(&pairs)
}

/// Render a terminal Operation (post-wait).
fn render_terminal_op(v: &Value) -> String {
    let pairs: Vec<(&str, String)> = vec![
        ("id", jstr(v, "id", "-").to_string()),
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
}

/// Render a `VmActionResponse` from start/stop. Server returns
/// `{ vm: { name, state }, message? }` per `vms.rs:216`. The state
/// is a serde-tagged enum (e.g. `"running"`, `"stopped"`, `"shutdown"`)
/// — read it as a string. Closes #171 (CLI side).
fn render_action(v: &Value) -> String {
    let vm = v.get("vm").cloned().unwrap_or(Value::Null);
    let pairs: Vec<(&str, String)> = vec![
        ("name", jstr(&vm, "name", "-").to_string()),
        ("state", jstr(&vm, "state", "-").to_string()),
        ("message", jstr(v, "message", "-").to_string()),
    ];
    kv::render(&pairs)
}

/// Convert a terminal-state Operation JSON into Ok/Err so verbs that
/// `--wait` exit with the right code (per acceptance criterion 2).
fn terminal_to_result(v: &Value) -> Result<()> {
    match jstr(v, "status", "") {
        "completed" => Ok(()),
        "failed" => Err(anyhow::anyhow!(
            "operation failed: {}",
            jstr(v, "error", "(no error message)")
        )),
        other => Err(anyhow::anyhow!(
            "operation ended in unexpected state: {}",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_list_table_from_array_response() {
        let v: Value = serde_json::json!([
            {"name": "a", "state": "running", "ip_address": "1.1.1.1", "profile": "basic"},
            {"name": "bb", "state": "stopped", "ip_address": null, "profile": "agentic-dev"}
        ]);
        let arr = v.as_array().cloned().unwrap();
        let rows: Vec<Vec<String>> = arr
            .iter()
            .map(|vm| {
                vec![
                    jstr(vm, "name", "").to_string(),
                    jstr(vm, "state", "").to_string(),
                    jstr(vm, "ip_address", "-").to_string(),
                    jstr(vm, "profile", "-").to_string(),
                ]
            })
            .collect();
        let out = table::render(&["NAME", "STATE", "IP", "PROFILE"], &rows);
        assert!(out.contains("running"));
        assert!(out.contains("stopped"));
        assert!(out.contains("1.1.1.1"));
        // Null IP renders as "-".
        assert!(out.contains("-"));
    }

    #[test]
    fn op_id_from_envelope_extracts_operation_id() {
        let v = serde_json::json!({
            "operation": {
                "id": "op-abc",
                "type": "vm_create",
                "status": "pending",
                "target": "test-vm",
                "progress_percent": 0,
                "created_at": "2026-04-29T12:00:00Z"
            },
            "vm": null
        });
        assert_eq!(op_id_from_envelope(&v).unwrap(), "op-abc");
    }

    #[test]
    fn op_id_from_envelope_errors_on_missing() {
        let v = serde_json::json!({"operation": {}});
        assert!(op_id_from_envelope(&v).is_err());

        let v2 = serde_json::json!({"unrelated": "shape"});
        assert!(op_id_from_envelope(&v2).is_err());
    }

    #[test]
    fn terminal_to_result_maps_status_to_outcome() {
        assert!(terminal_to_result(&serde_json::json!({"status": "completed"})).is_ok());
        let err = terminal_to_result(&serde_json::json!({"status": "failed", "error": "boom"}));
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("boom"));
    }
}
