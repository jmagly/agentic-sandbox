//! Docker runtime monitoring for container lifecycle, cleanup, and metrics.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};

use crate::http::events;
use crate::telemetry::Metrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockerHostPlatform {
    Linux,
    Macos,
    Other,
}

pub(crate) fn docker_host_platform() -> DockerHostPlatform {
    match std::env::consts::OS {
        "linux" => DockerHostPlatform::Linux,
        "macos" => DockerHostPlatform::Macos,
        _ => DockerHostPlatform::Other,
    }
}

pub(crate) fn docker_host_preserves_uds_peer_uid() -> bool {
    platform_preserves_uds_peer_uid(docker_host_platform())
}

fn platform_preserves_uds_peer_uid(platform: DockerHostPlatform) -> bool {
    platform == DockerHostPlatform::Linux
}

pub fn stage_managed_container_bootstrap_ca(
    root: &str,
    instance_id: &str,
    ca_pem: &str,
) -> Result<(String, String), String> {
    use std::os::unix::fs::PermissionsExt;

    let dir = PathBuf::from(root)
        .join("instances")
        .join(instance_id)
        .join("bootstrap");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("failed to create Docker bootstrap CA directory: {error}"))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("failed to protect Docker bootstrap CA directory: {error}"))?;
    let path = dir.join("enrollment-ca.pem");
    let temporary = dir.join(format!(".enrollment-ca.{}.tmp", uuid::Uuid::now_v7()));
    std::fs::write(&temporary, ca_pem)
        .map_err(|error| format!("failed to stage Docker bootstrap CA: {error}"))?;
    // This is a public trust anchor. The control process must be able to read
    // the bind-mounted file before it generates its private key in-container.
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o444))
        .map_err(|error| format!("failed to publish readable Docker bootstrap CA: {error}"))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("failed to publish Docker bootstrap CA: {error}"))?;
    Ok((
        path.to_string_lossy().to_string(),
        "/run/agentic-sandbox/enrollment-ca.pem:ro".to_string(),
    ))
}

/// Sanitized Docker CLI availability reported at the API integration boundary.
/// Raw CLI output is intentionally not retained: it can contain daemon paths,
/// registry names, or credential-helper diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerAvailability {
    pub available: bool,
    pub architecture: Option<String>,
    pub unavailable_code: Option<String>,
    pub unavailable_reason: Option<String>,
}

fn docker_command() -> PathBuf {
    std::env::var_os("AGENTIC_DOCKER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docker"))
}

/// Probe the active Docker CLI context and daemon without exposing command
/// output. A successful `docker version` proves both CLI and daemon access.
pub async fn probe_docker() -> DockerAvailability {
    let output = match Command::new(docker_command())
        .args(["version", "--format", "{{.Server.Arch}}"])
        .output()
        .await
    {
        Ok(output) => output,
        Err(_) => {
            return DockerAvailability {
                available: false,
                architecture: None,
                unavailable_code: Some("docker.cli_unavailable".to_string()),
                unavailable_reason: Some(
                    "Docker CLI is not available on the management host".to_string(),
                ),
            };
        }
    };

    if !output.status.success() {
        return DockerAvailability {
            available: false,
            architecture: None,
            unavailable_code: Some("docker.daemon_unavailable".to_string()),
            unavailable_reason: Some(
                "The active Docker CLI context cannot reach a Docker daemon".to_string(),
            ),
        };
    }

    let architecture = String::from_utf8_lossy(&output.stdout).trim().to_string();
    DockerAvailability {
        available: true,
        architecture: (!architecture.is_empty()).then_some(architecture),
        unavailable_code: None,
        unavailable_reason: None,
    }
}

#[derive(Debug, Clone)]
pub struct DockerMonitorConfig {
    pub enabled: bool,
    pub poll_interval_secs: u64,
    pub orphaned_age_secs: u64,
}

impl Default for DockerMonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 30,
            orphaned_age_secs: 3600,
        }
    }
}

impl DockerMonitorConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("DOCKER_MONITOR_ENABLED")
            .map(|v| v.to_lowercase() != "false" && v != "0")
            .unwrap_or(true);
        let poll_interval_secs = std::env::var("DOCKER_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        let orphaned_age_secs = std::env::var("DOCKER_ORPHANED_AGE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        Self {
            enabled,
            poll_interval_secs,
            orphaned_age_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerStatus {
    Running,
    Stopped,
    Other(String),
}

impl std::fmt::Display for ContainerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerStatus::Running => write!(f, "running"),
            ContainerStatus::Stopped => write!(f, "stopped"),
            ContainerStatus::Other(s) => write!(f, "{}", s),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub id: String,
    pub image: String,
    pub name: String,
    pub status: ContainerStatus,
    pub finished_at: Option<DateTime<Utc>>,
    pub labels: HashMap<String, String>,
}

fn parse_labels(labels: &str) -> HashMap<String, String> {
    labels
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let mut kv = part.splitn(2, '=');
            let key = kv.next()?.trim();
            if key.is_empty() {
                return None;
            }
            let value = kv.next().unwrap_or_default().trim();
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn parse_status(status: &str) -> ContainerStatus {
    if status.starts_with("Up ") {
        ContainerStatus::Running
    } else if status.starts_with("Exited")
        || status.starts_with("Created")
        || status.starts_with("Dead")
    {
        ContainerStatus::Stopped
    } else {
        ContainerStatus::Other(status.to_string())
    }
}

pub async fn list_containers() -> Result<Vec<ContainerInfo>, String> {
    let output = Command::new(docker_command())
        .args([
            "ps",
            "-a",
            "--filter",
            "label=agentic-sandbox=true",
            "--format",
            "{{.ID}}|{{.Image}}|{{.Names}}|{{.Status}}|{{.Labels}}",
        ])
        .output()
        .await
        .map_err(|e| format!("failed to run docker ps: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "docker ps failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let mut containers = Vec::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(5, '|').collect();
        if parts.len() != 5 {
            continue;
        }
        let id = parts[0].trim().to_string();
        let image = parts[1].trim().to_string();
        let name = parts[2].trim().to_string();
        let status_raw = parts[3].trim();
        let status = parse_status(status_raw);
        let labels = parse_labels(parts[4]);

        let finished_at = if status == ContainerStatus::Stopped {
            inspect_finished_at(&id).await
        } else {
            None
        };

        containers.push(ContainerInfo {
            id,
            image,
            name,
            status,
            finished_at,
            labels,
        });
    }

    Ok(containers)
}

async fn inspect_finished_at(container_id: &str) -> Option<DateTime<Utc>> {
    let output = Command::new(docker_command())
        .args(["inspect", "--format", "{{.State.FinishedAt}}", container_id])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let ts = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ts.is_empty() || ts == "0001-01-01T00:00:00Z" {
        return None;
    }

    DateTime::parse_from_rfc3339(&ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub async fn remove_container(container_id: &str) -> Result<(), String> {
    let managed_network = Command::new(docker_command())
        .args([
            "inspect",
            "--format",
            "{{ index .Config.Labels \"agentic-managed-network\" }}",
            container_id,
        ])
        .output()
        .await
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|network| !network.is_empty() && network != "<no value>");
    let output = Command::new(docker_command())
        .args(["rm", "-f", container_id])
        .output()
        .await
        .map_err(|e| format!("failed to run docker rm: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    if let Some(network) = managed_network {
        let _ = Command::new(docker_command())
            .args(["network", "rm", &network])
            .output()
            .await;
    }
    Ok(())
}

/// Spawn options for `spawn_container`. Mirrors the smallest useful
/// subset of `docker run` flags. Future: resource limits (`--memory`,
/// `--cpus`), security opts, capability drops — track as Section F gaps.
#[derive(Debug, Clone, Default)]
pub struct SpawnOpts {
    pub env: Vec<(String, String)>,
    /// Extra Docker labels as `(key, value)`. The managed
    /// `agentic-sandbox=true` label is always added by `spawn_container`.
    pub labels: Vec<(String, String)>,
    /// Bind mounts as `(host_path, container_path)`. Mounted RW.
    pub mounts: Vec<(String, String)>,
    /// Optional network mode (`bridge`, `host`, custom name).
    pub network: Option<String>,
    /// Optional command + args overriding the image's default.
    pub cmd: Vec<String>,
    /// Unique host-visible uid used by the long-lived transport/control
    /// process. When set, the entrypoint starts as root with only SETUID and
    /// SETGID, then re-execs the agent under this uid. Workload children run
    /// under the fixed image uid and clear all capabilities before exec.
    pub control_uid: Option<u32>,
}

const CONTAINER_RUNTIME_USER: &str = "10001:10001";
pub const MANAGED_CONTAINER_WORKLOAD_UID: u32 = 10001;
const CONTAINER_WORKLOAD_UID: u32 = MANAGED_CONTAINER_WORKLOAD_UID;
const CONTAINER_WORKLOAD_GID: u32 = 10001;
const MANAGED_CONTROL_UID_BASE: u32 = 200_000;
const MANAGED_CONTROL_UID_SPAN: u32 = 600_000;
pub const MANAGED_CONTAINER_UDS_PATH: &str = "/run/agentic-sandbox/agent-grpc.sock";
const BOOTSTRAP_TOKEN_ENV: &str = "AGENT_BOOTSTRAP_TOKEN";
const BOOTSTRAP_TOKEN_EXPIRY_ENV: &str = "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS";
const BOOTSTRAP_TOKEN_FILE_ENV: &str = "AGENT_BOOTSTRAP_INPUT_FILE";
const BOOTSTRAP_TOKEN_FILE: &str = "/run/agentic-runtime/token";
const BOOTSTRAP_TLS_DIR: &str = "/var/lib/agentic-control/grpc-mtls";

pub fn managed_control_uid(instance_id: &str) -> Result<u32, String> {
    let uuid = uuid::Uuid::parse_str(instance_id)
        .map_err(|error| format!("invalid Docker instance id for control uid: {error}"))?;
    let bytes = uuid.as_bytes();
    let value = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    Ok(MANAGED_CONTROL_UID_BASE + (value % MANAGED_CONTROL_UID_SPAN))
}

/// Return whether a container control uid belongs to the deterministic range
/// reserved for managed Docker identities. Inventory consumers use this
/// boolean rather than receiving the host uid itself.
pub fn is_managed_control_uid(uid: u32) -> bool {
    (MANAGED_CONTROL_UID_BASE..MANAGED_CONTROL_UID_BASE + MANAGED_CONTROL_UID_SPAN).contains(&uid)
}

pub async fn managed_grpc_uds_host_path() -> Result<PathBuf, String> {
    let value = std::env::var("AGENTIC_GRPC_UDS_EFFECTIVE")
        .map_err(|_| "managed Docker UDS transport is unavailable".to_string())?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("managed Docker UDS transport path must be absolute".to_string());
    }
    // HTTP starts just before the gRPC listeners. Bound the startup race so a
    // provision request arriving in that narrow window waits for the socket
    // instead of failing or falling back to a weaker transport.
    for _ in 0..250 {
        if path.exists() {
            return Ok(path);
        }
        sleep(Duration::from_millis(20)).await;
    }
    Err("managed Docker UDS transport socket is not ready".to_string())
}

fn managed_network_create_args(platform: DockerHostPlatform, network: &str) -> Vec<String> {
    let mut args = vec![
        "network".into(),
        "create".into(),
        "--label".into(),
        "agentic-sandbox=true".into(),
    ];
    if platform == DockerHostPlatform::Macos {
        args.extend([
            "--label".into(),
            "agentic-egress-policy=unrestricted-platform-compatibility".into(),
        ]);
    } else {
        args.extend([
            "--internal".into(),
            "--label".into(),
            "agentic-egress-policy=default-deny".into(),
        ]);
    }
    args.push(network.into());
    args
}

fn build_run_args(
    platform: DockerHostPlatform,
    name: &str,
    image: &str,
    opts: &SpawnOpts,
) -> Result<Vec<String>, String> {
    let has_bootstrap_token = opts.env.iter().any(|(key, _)| key == BOOTSTRAP_TOKEN_ENV);
    if platform == DockerHostPlatform::Macos && opts.network.as_deref() == Some("host") {
        return Err(
            "Docker Desktop does not provide Linux host-network semantics; use bridge networking and host.docker.internal"
                .to_string(),
        );
    }
    for (host, container) in &opts.mounts {
        let host_path = std::path::Path::new(host);
        if !host_path.is_absolute() {
            return Err(format!(
                "Docker bind-mount host path must be absolute: {host}"
            ));
        }
        if !std::path::Path::new(container).is_absolute() {
            return Err(format!(
                "Docker bind-mount container path must be absolute: {container}"
            ));
        }
        if !host_path.exists() {
            let suffix = if platform == DockerHostPlatform::Macos {
                "; create it first and allow its parent in Docker Desktop Settings > Resources > File Sharing"
            } else {
                "; create it before provisioning"
            };
            return Err(format!(
                "Docker bind-mount host path does not exist: {host}{suffix}"
            ));
        }
    }

    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--label".into(),
        "agentic-sandbox=true".into(),
        "--name".into(),
        name.into(),
    ];
    args.extend([
        "--user".into(),
        if opts.control_uid.is_some() {
            "0:0".into()
        } else {
            CONTAINER_RUNTIME_USER.into()
        },
        "--cap-drop".into(),
        "ALL".into(),
    ]);
    if let Some(control_uid) = opts.control_uid {
        args.extend([
            "--cap-add".into(),
            "SETUID".into(),
            "--cap-add".into(),
            "SETGID".into(),
            "-e".into(),
            format!("AGENT_CONTROL_UID={control_uid}"),
            "-e".into(),
            format!("AGENT_CONTROL_GID={control_uid}"),
            "-e".into(),
            format!("AGENT_WORKLOAD_UID={CONTAINER_WORKLOAD_UID}"),
            "-e".into(),
            format!("AGENT_WORKLOAD_GID={CONTAINER_WORKLOAD_GID}"),
        ]);
        if has_bootstrap_token {
            // Root needs CHOWN only while the entrypoint creates the durable,
            // control-UID-owned mTLS directory. The setpriv re-exec retains
            // only SETUID/SETGID, and no-new-privileges prevents reacquisition.
            args.extend(["--cap-add".into(), "CHOWN".into()]);
        }
    }
    args.extend([
        "--security-opt".into(),
        "no-new-privileges:true".into(),
        "-e".into(),
        "HOME=/home/agent".into(),
    ]);
    if platform == DockerHostPlatform::Linux {
        args.extend([
            "--add-host".into(),
            "host.docker.internal:host-gateway".into(),
        ]);
    }
    if opts.control_uid.is_some() || has_bootstrap_token {
        args.extend([
            "--tmpfs".into(),
            format!(
                "/run/agentic-runtime:rw,noexec,nosuid,nodev,mode=0700,uid={},gid={}",
                opts.control_uid.unwrap_or(CONTAINER_WORKLOAD_UID),
                opts.control_uid.unwrap_or(CONTAINER_WORKLOAD_GID)
            ),
            "-e".into(),
            "AGENT_SETUP_SENTINEL=/run/agentic-runtime/setup-complete".into(),
        ]);
    }
    if has_bootstrap_token {
        args.extend([
            "-e".into(),
            format!("{BOOTSTRAP_TOKEN_FILE_ENV}={BOOTSTRAP_TOKEN_FILE}"),
            "-e".into(),
            format!("AGENT_BOOTSTRAP_TLS_DIR={BOOTSTRAP_TLS_DIR}"),
        ]);
    }
    for (k, v) in &opts.env {
        if k == BOOTSTRAP_TOKEN_ENV
            || k == BOOTSTRAP_TOKEN_EXPIRY_ENV
            || k == "AGENT_CONTROL_UID"
            || k == "AGENT_CONTROL_GID"
            || k == "AGENT_WORKLOAD_UID"
            || k == "AGENT_WORKLOAD_GID"
            || (k == "AGENT_BOOTSTRAP_TLS_DIR"
                && opts.env.iter().any(|(key, _)| key == BOOTSTRAP_TOKEN_ENV))
        {
            continue;
        }
        args.push("-e".into());
        args.push(format!("{}={}", k, v));
    }
    for (k, v) in &opts.labels {
        args.push("--label".into());
        args.push(format!("{}={}", k, v));
    }
    for (host, ctn) in &opts.mounts {
        args.push("-v".into());
        args.push(format!("{}:{}", host, ctn));
    }
    if let Some(net) = &opts.network {
        args.push("--network".into());
        args.push(net.clone());
    }
    args.push(image.into());
    args.extend(opts.cmd.iter().cloned());
    Ok(args)
}

/// Spawn a container in detached mode tagged with our `agentic-sandbox=true`
/// label so the existing monitor + cleanup loop find it. Returns the
/// container ID. The caller is responsible for emitting the
/// `container.created` event on success — the monitor will pick it up
/// on its next tick anyway, but emitting from the spawn site closes the
/// observability gap noted in #173 Section F.
pub async fn spawn_container(name: &str, image: &str, opts: &SpawnOpts) -> Result<String, String> {
    let platform = docker_host_platform();
    let mut effective_opts = opts.clone();
    let mut created_network = None;
    if effective_opts.network.is_none() {
        let safe_name = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .take(32)
            .collect::<String>();
        let network = format!(
            "agentic-{safe_name}-{}",
            &uuid::Uuid::now_v7().simple().to_string()[..12]
        );
        if platform == DockerHostPlatform::Macos {
            warn!(
                network = %network,
                "Docker Desktop managed network permits external routing so host.docker.internal remains reachable; treat this runtime as T0"
            );
        }
        let output = Command::new(docker_command())
            .args(managed_network_create_args(platform, &network))
            .output()
            .await
            .map_err(|e| format!("failed to create isolated Docker network: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "failed to create isolated Docker network: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        effective_opts.network = Some(network.clone());
        effective_opts
            .labels
            .push(("agentic-managed-network".into(), network.clone()));
        created_network = Some(network);
    }
    let args = match build_run_args(platform, name, image, &effective_opts) {
        Ok(args) => args,
        Err(error) => {
            if let Some(network) = created_network {
                let _ = Command::new(docker_command())
                    .args(["network", "rm", &network])
                    .output()
                    .await;
            }
            return Err(error);
        }
    };

    let output = Command::new(docker_command())
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("failed to run docker run: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if platform == DockerHostPlatform::Macos
            && (stderr.contains("Mounts denied") || stderr.contains("is not shared"))
        {
            return Err(
                "Docker Desktop denied a bind mount; allow the host path in Settings > Resources > File Sharing"
                    .to_string(),
            );
        }
        if let Some(network) = created_network {
            let _ = Command::new(docker_command())
                .args(["network", "rm", &network])
                .output()
                .await;
        }
        return Err(stderr);
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if let Some((_, token)) = opts.env.iter().find(|(key, _)| key == BOOTSTRAP_TOKEN_ENV) {
        let exec_user = opts
            .control_uid
            .map(|uid| format!("{uid}:{uid}"))
            .unwrap_or_else(|| CONTAINER_RUNTIME_USER.to_string());
        let mut child = match Command::new(docker_command())
            .args([
                "exec",
                "-i",
                "--user",
                &exec_user,
                &id,
                "sh",
                "-c",
                "umask 077; cat > \"$AGENT_BOOTSTRAP_INPUT_FILE\"",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = remove_container(&id).await;
                return Err(format!("failed to provision bootstrap token: {error}"));
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(token.as_bytes()).await {
                let _ = remove_container(&id).await;
                return Err(format!("failed to stream bootstrap token: {error}"));
            }
        }
        let provisioned = match child.wait_with_output().await {
            Ok(output) => output,
            Err(error) => {
                let _ = remove_container(&id).await;
                return Err(format!(
                    "failed to wait for bootstrap token provisioning: {error}"
                ));
            }
        };
        if !provisioned.status.success() {
            let _ = remove_container(&id).await;
            return Err(format!(
                "failed to provision bootstrap token: {}",
                String::from_utf8_lossy(&provisioned.stderr).trim()
            ));
        }
    }
    Ok(id)
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    #[test]
    fn only_native_linux_docker_uses_uds_peer_uid_identity() {
        assert!(platform_preserves_uds_peer_uid(DockerHostPlatform::Linux));
        assert!(!platform_preserves_uds_peer_uid(DockerHostPlatform::Macos));
        assert!(!platform_preserves_uds_peer_uid(DockerHostPlatform::Other));
    }

    #[test]
    fn linux_adds_host_gateway_but_macos_uses_native_dns() {
        let opts = SpawnOpts::default();
        let linux = build_run_args(DockerHostPlatform::Linux, "agent-a", "image", &opts).unwrap();
        let macos = build_run_args(DockerHostPlatform::Macos, "agent-a", "image", &opts).unwrap();
        assert!(linux
            .iter()
            .any(|arg| arg == "host.docker.internal:host-gateway"));
        assert!(!macos.iter().any(|arg| arg == "--add-host"));
    }

    #[test]
    fn managed_containers_use_non_root_capability_free_no_new_privileges_defaults() {
        let args = build_run_args(
            DockerHostPlatform::Linux,
            "agent-a",
            "image",
            &SpawnOpts::default(),
        )
        .unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("--user 10001:10001"));
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--security-opt no-new-privileges:true"));
    }

    #[test]
    fn separated_managed_containers_use_unique_control_and_fixed_workload_ids() {
        let opts = SpawnOpts {
            control_uid: Some(240_404),
            ..SpawnOpts::default()
        };
        let args = build_run_args(DockerHostPlatform::Linux, "agent-a", "image", &opts).unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("--user 0:0"));
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--cap-add SETUID --cap-add SETGID"));
        assert!(joined.contains("--security-opt no-new-privileges:true"));
        assert!(joined.contains("AGENT_CONTROL_UID=240404"));
        assert!(joined.contains("AGENT_WORKLOAD_UID=10001"));
        assert!(joined.contains(
            "/run/agentic-runtime:rw,noexec,nosuid,nodev,mode=0700,uid=240404,gid=240404"
        ));
        assert!(joined.contains("AGENT_SETUP_SENTINEL=/run/agentic-runtime/setup-complete"));
    }

    #[test]
    fn linux_managed_networks_are_internal_and_record_default_deny_egress() {
        let args = managed_network_create_args(DockerHostPlatform::Linux, "agentic-test-network");
        assert!(args.iter().any(|arg| arg == "--internal"));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--label", "agentic-egress-policy=default-deny"]));
    }

    #[test]
    fn macos_managed_networks_preserve_host_callback_and_record_t0_posture() {
        let args = managed_network_create_args(DockerHostPlatform::Macos, "agentic-test-network");
        assert!(!args.iter().any(|arg| arg == "--internal"));
        assert!(args.windows(2).any(|pair| {
            pair == [
                "--label",
                "agentic-egress-policy=unrestricted-platform-compatibility",
            ]
        }));
    }

    #[test]
    fn bootstrap_token_is_replaced_by_a_tmpfs_file_reference() {
        let opts = SpawnOpts {
            env: vec![
                (BOOTSTRAP_TOKEN_ENV.into(), "must-not-leak".into()),
                (BOOTSTRAP_TOKEN_EXPIRY_ENV.into(), "1900000000000".into()),
            ],
            control_uid: Some(240_404),
            ..SpawnOpts::default()
        };
        let args = build_run_args(DockerHostPlatform::Linux, "agent-a", "image", &opts).unwrap();
        let joined = args.join(" ");
        assert!(!joined.contains("must-not-leak"));
        assert!(!joined.contains("AGENT_BOOTSTRAP_TOKEN="));
        assert!(!joined.contains(BOOTSTRAP_TOKEN_EXPIRY_ENV));
        assert!(joined.contains("AGENT_BOOTSTRAP_INPUT_FILE=/run/agentic-runtime/token"));
        assert!(joined.contains(&format!("AGENT_BOOTSTRAP_TLS_DIR={BOOTSTRAP_TLS_DIR}")));
        assert!(!joined.contains("AGENT_BOOTSTRAP_TLS_DIR=/run/agentic-runtime"));
        assert!(joined.contains("noexec,nosuid,nodev"));
        assert!(joined.contains("--cap-add CHOWN"));
        assert!(joined.contains("AGENT_BOOTSTRAP_TLS_DIR=/var/lib/agentic-control/grpc-mtls"));
    }

    #[test]
    fn macos_rejects_host_network_mode() {
        let opts = SpawnOpts {
            network: Some("host".to_string()),
            ..SpawnOpts::default()
        };
        let err = build_run_args(DockerHostPlatform::Macos, "agent-a", "image", &opts).unwrap_err();
        assert!(err.contains("does not provide Linux host-network semantics"));
    }

    #[test]
    fn bind_mounts_fail_before_docker_for_missing_host_paths() {
        let opts = SpawnOpts {
            mounts: vec![(
                "/definitely/missing/agentic-path".to_string(),
                "/workspace".to_string(),
            )],
            ..SpawnOpts::default()
        };
        let err = build_run_args(DockerHostPlatform::Macos, "agent-a", "image", &opts).unwrap_err();
        assert!(err.contains("does not exist"));
        assert!(err.contains("File Sharing"));
    }

    #[test]
    fn docker_process_state_or_absence_can_revoke_but_never_grant_agent_readiness() {
        assert_eq!(
            readiness_update_for_container_status(Some(&ContainerStatus::Stopped)),
            Some(false)
        );
        assert_eq!(
            readiness_update_for_container_status(None),
            Some(false),
            "a container that disappeared between polls is not dispatchable"
        );
        assert_eq!(
            readiness_update_for_container_status(Some(&ContainerStatus::Running)),
            None,
            "authenticated agent registration, not docker start, grants readiness"
        );
        assert_eq!(
            readiness_update_for_container_status(Some(&ContainerStatus::Other("created".into()))),
            None
        );
    }
}

/// Look up a single container by its `--name`. Returns `None` if it
/// doesn't exist OR isn't tagged with our label (we don't surface
/// containers we don't manage).
pub async fn get_container_by_name(name: &str) -> Result<Option<ContainerInfo>, String> {
    let all = list_containers().await?;
    Ok(all.into_iter().find(|c| c.name == name))
}

/// Lifecycle controls. Each is a thin shell over `docker <verb>`.
pub async fn start_container(name: &str) -> Result<(), String> {
    docker_simple_verb("start", name).await
}

pub async fn stop_container(name: &str, timeout_seconds: u64) -> Result<(), String> {
    let timeout = timeout_seconds.to_string();
    let output = Command::new(docker_command())
        .args(["stop", "-t", &timeout, name])
        .output()
        .await
        .map_err(|e| format!("failed to run docker stop: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

async fn docker_simple_verb(verb: &str, name: &str) -> Result<(), String> {
    let output = Command::new(docker_command())
        .args([verb, name])
        .output()
        .await
        .map_err(|e| format!("failed to run docker {verb}: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

/// #268: Helper to flip an instance's readiness flag from the docker
/// monitor. Silently no-ops if the registry is unmounted (e.g. test
/// harness) or the instance_id can't be resolved yet (agent hasn't
/// connected back). The whole point of this hook is to mark dead
/// runtimes as not-ready so `send_message` 503s instead of accepting.
fn set_instance_ready(
    registry: &Option<agentic_sandbox_executor::instance::InstanceRegistry>,
    instance_id: Option<&str>,
    ready: bool,
) {
    let (Some(reg), Some(id)) = (registry, instance_id) else {
        return;
    };
    if let Some(ctx) = reg.get(id) {
        ctx.set_ready(ready);
        debug!(instance_id = %id, ready, "updated instance readiness from docker monitor");
    }
}

/// Docker process state is negative readiness evidence only. A stopped
/// container proves the executor is unavailable, but a running container does
/// not prove that its agent completed bootstrap enrollment and registered over
/// an authenticated transport. Only the gRPC registration path may grant
/// readiness.
fn readiness_update_for_container_status(status: Option<&ContainerStatus>) -> Option<bool> {
    match status {
        None | Some(ContainerStatus::Stopped) => Some(false),
        Some(ContainerStatus::Running | ContainerStatus::Other(_)) => None,
    }
}

pub fn spawn_docker_monitor(
    config: DockerMonitorConfig,
    metrics: Option<Arc<Metrics>>,
    // #268: Optional executor InstanceRegistry + AgentRegistry pair so the
    // monitor can flip the per-instance `ready` flag when a container
    // transitions to stopped. Without these, send_message has no way to
    // tell that the backing runtime is dead and accepts work that will
    // stall forever.
    instance_registry: Option<agentic_sandbox_executor::instance::InstanceRegistry>,
    agent_registry: Option<Arc<crate::registry::AgentRegistry>>,
) {
    if !config.enabled {
        info!("Docker monitor disabled");
        return;
    }

    tokio::spawn(async move {
        let mut previous: HashMap<String, ContainerStatus> = HashMap::new();

        loop {
            match list_containers().await {
                Ok(containers) => {
                    let mut running = 0u64;
                    let mut stopped = 0u64;

                    let mut current: HashMap<String, ContainerStatus> = HashMap::new();
                    for c in &containers {
                        current.insert(c.name.clone(), c.status.clone());
                        match c.status {
                            ContainerStatus::Running => running += 1,
                            ContainerStatus::Stopped => stopped += 1,
                            ContainerStatus::Other(_) => {}
                        }

                        // #268: resolve instance_id from container name. v2
                        // admin provisioning sets AGENT_ID = container name,
                        // so the AgentRegistry exposes the canonical
                        // UUIDv7 once the agent has connected back.
                        let instance_id = agent_registry.as_ref().and_then(|reg| {
                            reg.get(&c.name)
                                .map(|entry| entry.value().instance_id.clone())
                                .filter(|s| !s.is_empty())
                        });

                        // New container
                        if !previous.contains_key(&c.name) {
                            match c.status {
                                ContainerStatus::Running => {
                                    events::add_container_event(
                                        "container.started",
                                        c.name.clone(),
                                    )
                                    .await;
                                }
                                ContainerStatus::Stopped => {
                                    events::add_container_event(
                                        "container.created",
                                        c.name.clone(),
                                    )
                                    .await;
                                }
                                ContainerStatus::Other(_) => {}
                            }
                        } else if let Some(prev_status) = previous.get(&c.name) {
                            if prev_status != &c.status {
                                match c.status {
                                    ContainerStatus::Running => {
                                        events::add_container_event(
                                            "container.started",
                                            c.name.clone(),
                                        )
                                        .await;
                                    }
                                    ContainerStatus::Stopped => {
                                        events::add_container_event(
                                            "container.stopped",
                                            c.name.clone(),
                                        )
                                        .await;
                                    }
                                    ContainerStatus::Other(_) => {}
                                }
                            }
                        }

                        if let Some(ready) = readiness_update_for_container_status(Some(&c.status))
                        {
                            set_instance_ready(&instance_registry, instance_id.as_deref(), ready);
                        }

                        // Orphan cleanup for stopped containers beyond threshold
                        if let (ContainerStatus::Stopped, Some(finished)) =
                            (&c.status, c.finished_at)
                        {
                            let age = Utc::now().signed_duration_since(finished).num_seconds();
                            if age >= config.orphaned_age_secs as i64 {
                                debug!(container = %c.name, age_secs = age, "Cleaning up orphaned container");
                                match remove_container(&c.id).await {
                                    Ok(_) => {
                                        events::add_container_event(
                                            "container.removed",
                                            c.name.clone(),
                                        )
                                        .await;
                                    }
                                    Err(err) => {
                                        warn!(container = %c.name, error = %err, "Failed to remove orphaned container");
                                    }
                                }
                            }
                        }
                    }

                    // Containers that disappeared since last poll
                    for (name, _) in previous.iter() {
                        if !current.contains_key(name) {
                            events::add_container_event("container.removed", name.clone()).await;
                            let instance_id = agent_registry.as_ref().and_then(|reg| {
                                reg.get(name)
                                    .map(|entry| entry.value().instance_id.clone())
                                    .filter(|id| !id.is_empty())
                            });
                            if let Some(ready) = readiness_update_for_container_status(None) {
                                set_instance_ready(
                                    &instance_registry,
                                    instance_id.as_deref(),
                                    ready,
                                );
                            }
                        }
                    }

                    if let Some(m) = metrics.as_ref() {
                        m.set_container_counts(running, stopped);
                    }

                    previous = current;
                }
                Err(err) => {
                    warn!(error = %err, "Docker monitor failed to list containers");
                }
            }

            sleep(Duration::from_secs(config.poll_interval_secs)).await;
        }
    });
}
