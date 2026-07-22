//! Docker runtime monitoring for container lifecycle, cleanup, and metrics.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};

use crate::http::events;
use crate::telemetry::Metrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DockerHostPlatform {
    Linux,
    Macos,
    Other,
}

fn docker_host_platform() -> DockerHostPlatform {
    match std::env::consts::OS {
        "linux" => DockerHostPlatform::Linux,
        "macos" => DockerHostPlatform::Macos,
        _ => DockerHostPlatform::Other,
    }
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
    let output = Command::new(docker_command())
        .args(["rm", "-f", container_id])
        .output()
        .await
        .map_err(|e| format!("failed to run docker rm: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
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
}

fn build_run_args(
    platform: DockerHostPlatform,
    name: &str,
    image: &str,
    opts: &SpawnOpts,
) -> Result<Vec<String>, String> {
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
    if platform == DockerHostPlatform::Linux {
        args.extend([
            "--add-host".into(),
            "host.docker.internal:host-gateway".into(),
        ]);
    }
    for (k, v) in &opts.env {
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
    let args = build_run_args(platform, name, image, opts)?;

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
        return Err(stderr);
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(id)
}

#[cfg(test)]
mod platform_tests {
    use super::*;

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
                                    set_instance_ready(
                                        &instance_registry,
                                        instance_id.as_deref(),
                                        false,
                                    );
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
                                        set_instance_ready(
                                            &instance_registry,
                                            instance_id.as_deref(),
                                            true,
                                        );
                                    }
                                    ContainerStatus::Stopped => {
                                        events::add_container_event(
                                            "container.stopped",
                                            c.name.clone(),
                                        )
                                        .await;
                                        set_instance_ready(
                                            &instance_registry,
                                            instance_id.as_deref(),
                                            false,
                                        );
                                    }
                                    ContainerStatus::Other(_) => {}
                                }
                            }
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
