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
enum ContainerStatus {
    Running,
    Stopped,
    Other(String),
}

#[derive(Debug, Clone)]
struct ContainerInfo {
    id: String,
    name: String,
    status: ContainerStatus,
    finished_at: Option<DateTime<Utc>>,
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

async fn list_containers() -> Result<Vec<ContainerInfo>, String> {
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

async fn remove_container(container_id: &str) -> Result<(), String> {
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
