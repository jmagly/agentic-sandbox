//! Docker runtime monitoring for container lifecycle, cleanup, and metrics.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};

use crate::http::events;
use crate::telemetry::Metrics;

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
    pub name: String,
    pub status: ContainerStatus,
    pub finished_at: Option<DateTime<Utc>>,
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
    let output = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            "label=agentic-sandbox=true",
            "--format",
            "{{.ID}}|{{.Names}}|{{.Status}}",
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
        let parts: Vec<&str> = line.splitn(3, '|').collect();
        if parts.len() != 3 {
            continue;
        }
        let id = parts[0].trim().to_string();
        let name = parts[1].trim().to_string();
        let status_raw = parts[2].trim();
        let status = parse_status(status_raw);

        let finished_at = if status == ContainerStatus::Stopped {
            inspect_finished_at(&id).await
        } else {
            None
        };

        containers.push(ContainerInfo {
            id,
            name,
            status,
            finished_at,
        });
    }

    Ok(containers)
}

async fn inspect_finished_at(container_id: &str) -> Option<DateTime<Utc>> {
    let output = Command::new("docker")
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
    let output = Command::new("docker")
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
    /// Bind mounts as `(host_path, container_path)`. Mounted RW.
    pub mounts: Vec<(String, String)>,
    /// Optional network mode (`bridge`, `host`, custom name).
    pub network: Option<String>,
    /// Optional command + args overriding the image's default.
    pub cmd: Vec<String>,
}

/// Spawn a container in detached mode tagged with our `agentic-sandbox=true`
/// label so the existing monitor + cleanup loop find it. Returns the
/// container ID. The caller is responsible for emitting the
/// `container.created` event on success — the monitor will pick it up
/// on its next tick anyway, but emitting from the spawn site closes the
/// observability gap noted in #173 Section F.
pub async fn spawn_container(name: &str, image: &str, opts: &SpawnOpts) -> Result<String, String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--label".into(),
        "agentic-sandbox=true".into(),
        "--name".into(),
        name.into(),
        // On Linux Docker, `host.docker.internal` doesn't resolve unless the
        // container is started with --add-host pointing at the special
        // host-gateway IP. The agent-entrypoint defaults its
        // MANAGEMENT_SERVER to `host.docker.internal:8120`, so without this
        // the container starts then immediately fails to dial the management
        // server. Adding it unconditionally is safe — Docker no-ops if the
        // platform already resolves it natively (Mac/Windows).
        "--add-host".into(),
        "host.docker.internal:host-gateway".into(),
    ];
    for (k, v) in &opts.env {
        args.push("-e".into());
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
    for c in &opts.cmd {
        args.push(c.clone());
    }

    let output = Command::new("docker")
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("failed to run docker run: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(id)
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
    let output = Command::new("docker")
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
    let output = Command::new("docker")
        .args([verb, name])
        .output()
        .await
        .map_err(|e| format!("failed to run docker {verb}: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

pub fn spawn_docker_monitor(config: DockerMonitorConfig, metrics: Option<Arc<Metrics>>) {
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
