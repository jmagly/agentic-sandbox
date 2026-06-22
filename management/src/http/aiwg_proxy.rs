//! AIWG companion endpoints — manifest CRUD and remote aiwg exec.
//!
//! These are the sandbox-side counterparts to the AIWG proxy layer.
//!
//! This module is a legacy direct-runtime SSH bridge for dev/break-glass
//! diagnostics only. It predates ADR-029 gateway-mediated SSH access and must
//! not be presented as the managed-profile SSH path. The handlers are disabled
//! by default and require `AGENTIC_ENABLE_DIRECT_SSH_AIWG_PROXY=1`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{error, warn};

use super::server::AppState;

const DIRECT_SSH_AIWG_PROXY_ENV: &str = "AGENTIC_ENABLE_DIRECT_SSH_AIWG_PROXY";

fn direct_ssh_aiwg_proxy_enabled_value(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn direct_ssh_aiwg_proxy_enabled() -> bool {
    direct_ssh_aiwg_proxy_enabled_value(std::env::var(DIRECT_SSH_AIWG_PROXY_ENV).ok().as_deref())
}

fn ensure_direct_ssh_aiwg_proxy_enabled() -> Result<(), axum::response::Response> {
    if direct_ssh_aiwg_proxy_enabled() {
        Ok(())
    } else {
        Err(direct_ssh_aiwg_proxy_disabled_response())
    }
}

fn direct_ssh_aiwg_proxy_disabled_response() -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "legacy direct-runtime AIWG SSH proxy is disabled",
            "detail": format!(
                "set {}=1 only for dev/break-glass diagnostics; managed-profile SSH must use the gateway-mediated path",
                DIRECT_SSH_AIWG_PROXY_ENV
            ),
            "path": "legacy_direct_runtime_aiwg_proxy"
        })),
    )
        .into_response()
}

// ── SSH helpers ───────────────────────────────────────────────────────────────

/// Resolve the SSH private key path for a given VM name.
///
/// Mirrors the fallback chain in scripts/deploy-agent.sh:
///   1. ~/.config/agentic-sandbox/secrets/ssh-keys/{vm_name}
///   2. ~/.ssh/agentic_ed25519
fn ssh_key_path(vm_name: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let preferred = std::path::PathBuf::from(format!(
        "{}/.config/agentic-sandbox/secrets/ssh-keys/{}",
        home, vm_name
    ));
    if preferred.exists() {
        return Some(preferred);
    }
    let fallback = std::path::PathBuf::from(format!("{}/.ssh/agentic_ed25519", home));
    if fallback.exists() {
        return Some(fallback);
    }
    None
}

/// Run a single command on a remote agent VM via SSH.
/// Returns `(stdout, stderr, success)`.
async fn ssh_exec(ip: &str, vm_name: &str, remote_cmd: &str) -> Result<String, String> {
    let key = ssh_key_path(vm_name).ok_or_else(|| format!("no SSH key found for {}", vm_name))?;

    warn!(
        path = "legacy_direct_runtime_aiwg_proxy",
        vm_name = %vm_name,
        "executing dev/break-glass direct-runtime AIWG SSH command"
    );

    let output = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "ConnectTimeout=10",
            "-i",
            key.to_str().unwrap_or_default(),
            &format!("agent@{}", ip),
            remote_cmd,
        ])
        .output()
        .await
        .map_err(|e| format!("failed to spawn ssh: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Push content to a remote file via SSH + stdin redirection.
async fn ssh_write_file(
    ip: &str,
    vm_name: &str,
    remote_path: &str,
    content: &[u8],
) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let key = ssh_key_path(vm_name).ok_or_else(|| format!("no SSH key found for {}", vm_name))?;

    warn!(
        path = "legacy_direct_runtime_aiwg_proxy",
        vm_name = %vm_name,
        remote_path = %remote_path,
        "writing via dev/break-glass direct-runtime AIWG SSH proxy"
    );

    let mut child = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "ConnectTimeout=10",
            "-i",
            key.to_str().unwrap_or_default(),
            &format!("agent@{}", ip),
            &format!(
                "mkdir -p \"$(dirname '{}')\" && cat > '{}'",
                remote_path, remote_path
            ),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn ssh: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(content)
            .await
            .map_err(|e| format!("stdin write: {}", e))?;
    }

    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("wait: {}", e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

// ── Platform resolution ───────────────────────────────────────────────────────

/// Map a platform name to its agent manifest directory on the remote VM.
///
/// These paths match the per-provider deployment structure used by AIWG:
///   claude  → ~/.claude/agents/
///   copilot → ~/.github/agents/
///   cursor  → ~/.cursor/agents/
fn platform_dir(platform: &str) -> Option<&'static str> {
    match platform {
        "claude" => Some(".claude/agents"),
        "copilot" => Some(".github/agents"),
        "cursor" => Some(".cursor/agents"),
        _ => None,
    }
}

// ── Agent resolution helper ───────────────────────────────────────────────────

struct AgentConn {
    ip: String,
    vm_name: String,
}

fn resolve_agent(
    state: &AppState,
    id: &str,
) -> Result<AgentConn, (StatusCode, Json<serde_json::Value>)> {
    match state.registry.get(id) {
        Some(agent) => Ok(AgentConn {
            ip: agent.registration.ip_address.clone(),
            vm_name: agent.registration.hostname.clone(),
        }),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("agent '{}' not found", id) })),
        )),
    }
}

// ── Manifest list ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ManifestEntry {
    pub name: String,
    pub sha256: String,
}

#[derive(Serialize)]
pub struct ManifestList {
    pub platform: String,
    pub manifests: Vec<ManifestEntry>,
}

/// GET /api/v1/agents/{id}/manifests/{platform}
///
/// Lists all `.md` agent manifests deployed on the VM under the platform directory.
pub async fn list_manifests(
    State(state): State<AppState>,
    Path((id, platform)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(response) = ensure_direct_ssh_aiwg_proxy_enabled() {
        return response;
    }

    let dir = match platform_dir(&platform) {
        Some(d) => d,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("unknown platform '{}'", platform) })),
            )
                .into_response()
        }
    };

    let conn = match resolve_agent(&state, &id) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    // List files and compute sha256 in one pass; gracefully handle missing dir.
    let cmd = format!(
        r#"if [ -d ~/{dir} ]; then cd ~/{dir} && find . -maxdepth 1 -name '*.md' -printf '%f\n' 2>/dev/null | sort | xargs -r sha256sum 2>/dev/null || true; fi"#,
        dir = dir
    );

    match ssh_exec(&conn.ip, &conn.vm_name, &cmd).await {
        Ok(stdout) => {
            let mut manifests: Vec<ManifestEntry> = Vec::new();
            for line in stdout.lines() {
                let parts: Vec<&str> = line.splitn(2, "  ").collect();
                if parts.len() == 2 {
                    let sha256 = parts[0].trim().to_string();
                    let name = parts[1].trim().trim_start_matches("./").to_string();
                    if !name.is_empty() {
                        manifests.push(ManifestEntry { name, sha256 });
                    }
                }
            }
            Json(
                serde_json::to_value(ManifestList {
                    platform,
                    manifests,
                })
                .unwrap(),
            )
            .into_response()
        }
        Err(e) => {
            error!(agent = %id, %e, "list_manifests SSH failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response()
        }
    }
}

// ── Manifest get ──────────────────────────────────────────────────────────────

/// GET /api/v1/agents/{id}/manifests/{platform}/{name}
///
/// Returns the raw markdown content of a single agent manifest.
pub async fn get_manifest(
    State(state): State<AppState>,
    Path((id, platform, name)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if let Err(response) = ensure_direct_ssh_aiwg_proxy_enabled() {
        return response;
    }

    let dir = match platform_dir(&platform) {
        Some(d) => d,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("unknown platform '{}'", platform) })),
            )
                .into_response()
        }
    };

    let conn = match resolve_agent(&state, &id) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let filename = ensure_md_extension(&name);
    let remote_path = format!("~/{}/{}", dir, filename);
    let cmd = format!("cat '{}'", remote_path);

    match ssh_exec(&conn.ip, &conn.vm_name, &cmd).await {
        Ok(content) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/markdown; charset=utf-8",
            )],
            content,
        )
            .into_response(),
        Err(e) => {
            warn!(agent = %id, manifest = %name, %e, "get_manifest SSH failed");
            let status = if e.contains("No such file") || e.contains("no such file") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, Json(serde_json::json!({ "error": e }))).into_response()
        }
    }
}

// ── Manifest push ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PushManifestRequest {
    pub content: String,
}

/// POST /api/v1/agents/{id}/manifests/{platform}/{name}
///
/// Writes (creates or replaces) an agent manifest on the VM.
pub async fn push_manifest(
    State(state): State<AppState>,
    Path((id, platform, name)): Path<(String, String, String)>,
    Json(body): Json<PushManifestRequest>,
) -> impl IntoResponse {
    if let Err(response) = ensure_direct_ssh_aiwg_proxy_enabled() {
        return response;
    }

    let dir = match platform_dir(&platform) {
        Some(d) => d,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("unknown platform '{}'", platform) })),
            )
                .into_response()
        }
    };

    let conn = match resolve_agent(&state, &id) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let filename = ensure_md_extension(&name);
    let remote_path = format!("/home/agent/{}/{}", dir, filename);

    match ssh_write_file(
        &conn.ip,
        &conn.vm_name,
        &remote_path,
        body.content.as_bytes(),
    )
    .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "platform": platform,
                "name": filename,
            })),
        )
            .into_response(),
        Err(e) => {
            error!(agent = %id, manifest = %name, %e, "push_manifest SSH failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response()
        }
    }
}

// ── AIWG exec ─────────────────────────────────────────────────────────────────

/// Allowlisted aiwg subcommands that may be invoked remotely.
///
/// This must remain conservative: only read-only or idempotent ops.
/// Destructive subcommands (remove, purge, reset) are explicitly excluded.
const AIWG_ALLOWLIST: &[&str] = &[
    "status",
    "doctor",
    "sync",
    "use",
    "list",
    "activity-log",
    "runtime-info",
    "hook-enable",
    "hook-disable",
];

#[derive(Deserialize)]
pub struct AiwgExecRequest {
    pub subcommand: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Serialize)]
pub struct AiwgExecResponse {
    pub ok: bool,
    pub stdout: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    pub exit_code: i32,
}

/// POST /api/v1/agents/{id}/aiwg/exec
///
/// Runs an allowlisted `aiwg` subcommand on the agent VM via SSH.
pub async fn aiwg_exec(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AiwgExecRequest>,
) -> impl IntoResponse {
    if let Err(response) = ensure_direct_ssh_aiwg_proxy_enabled() {
        return response;
    }

    // Allowlist check
    if !AIWG_ALLOWLIST.contains(&body.subcommand.as_str()) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!("subcommand '{}' is not allowed", body.subcommand),
                "allowed": AIWG_ALLOWLIST,
            })),
        )
            .into_response();
    }

    // Validate individual args — no shell metacharacters
    for arg in &body.args {
        if arg
            .chars()
            .any(|c| matches!(c, '`' | '$' | ';' | '&' | '|' | '>' | '<' | '\n' | '\r'))
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({ "error": format!("invalid character in arg: {:?}", arg) }),
                ),
            )
                .into_response();
        }
    }

    let conn = match resolve_agent(&state, &id) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    // Build the quoted arg list for the remote shell
    let quoted_args: Vec<String> = body
        .args
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
        .collect();
    let remote_cmd = format!(
        r#"PATH="$HOME/.local/bin:$PATH" aiwg {} {}"#,
        body.subcommand,
        quoted_args.join(" ")
    );

    let key = match ssh_key_path(&conn.vm_name) {
        Some(k) => k,
        None => return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("no SSH key found for {}", conn.vm_name) })),
        )
            .into_response(),
    };

    warn!(
        path = "legacy_direct_runtime_aiwg_proxy",
        agent = %id,
        cmd = %body.subcommand,
        "executing dev/break-glass direct-runtime AIWG SSH command"
    );

    let output = match Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "ConnectTimeout=10",
            "-i",
            key.to_str().unwrap_or_default(),
            &format!("agent@{}", conn.ip),
            &remote_cmd,
        ])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            error!(agent = %id, cmd = %body.subcommand, %e, "aiwg_exec spawn failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("failed to spawn ssh: {}", e) })),
            )
                .into_response();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    let ok = output.status.success();

    if !ok {
        warn!(agent = %id, cmd = %body.subcommand, %exit_code, %stderr, "aiwg_exec remote failure");
    }

    (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        },
        Json(
            serde_json::to_value(AiwgExecResponse {
                ok,
                stdout,
                stderr,
                exit_code,
            })
            .unwrap(),
        ),
    )
        .into_response()
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn ensure_md_extension(name: &str) -> String {
    if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{}.md", name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_ssh_aiwg_proxy_gate_is_opt_in() {
        assert!(!direct_ssh_aiwg_proxy_enabled_value(None));
        assert!(!direct_ssh_aiwg_proxy_enabled_value(Some("")));
        assert!(!direct_ssh_aiwg_proxy_enabled_value(Some("0")));
        assert!(!direct_ssh_aiwg_proxy_enabled_value(Some("false")));

        assert!(direct_ssh_aiwg_proxy_enabled_value(Some("1")));
        assert!(direct_ssh_aiwg_proxy_enabled_value(Some("true")));
        assert!(direct_ssh_aiwg_proxy_enabled_value(Some("YES")));
        assert!(direct_ssh_aiwg_proxy_enabled_value(Some(" on ")));
    }

    #[test]
    fn direct_ssh_aiwg_proxy_disabled_response_names_legacy_path() {
        let response = direct_ssh_aiwg_proxy_disabled_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn ensure_md_extension_appends_extension_once() {
        assert_eq!(ensure_md_extension("agent"), "agent.md");
        assert_eq!(ensure_md_extension("agent.md"), "agent.md");
    }
}
