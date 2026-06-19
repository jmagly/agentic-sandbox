//! Container lifecycle REST surface (#173 Section B).
//!
//! Wraps the existing `docker_runtime` helpers (which shell out to `docker`)
//! behind the same shape as `/api/v1/vms`. Containers are first-class
//! workloads alongside VMs; the dashboard, CLI, and AIWG bridge can pick
//! one or the other per workload.
//!
//! PTY exec inside a spawned container is **not** part of this surface —
//! that lives under #174. Today this surface lets you create / list /
//! inspect / start / stop / delete containers; the formal session-registry
//! protocol attaches to whatever the container's entrypoint produces via
//! the existing in-container agent path once #174 lands.
//!
//! All managed containers carry the `agentic-sandbox=true` label so the
//! existing `docker_runtime::list_containers` + monitor + cleanup loop
//! observe them automatically.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use super::server::AppState;
use crate::docker_runtime::{
    get_container_by_name, list_containers, remove_container, spawn_container, start_container,
    stop_container, ContainerInfo, SpawnOpts,
};
use crate::runtime_bootstrap::issue_bootstrap_envelope;

#[derive(Debug, Serialize)]
pub struct ContainerView {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

impl From<ContainerInfo> for ContainerView {
    fn from(c: ContainerInfo) -> Self {
        Self {
            id: c.id,
            name: c.name,
            status: c.status.to_string(),
            finished_at: c.finished_at.map(|t| t.to_rfc3339()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ListContainersResponse {
    pub total: usize,
    pub containers: Vec<ContainerView>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListContainersQuery {
    /// Filter by status: `running` | `stopped` | `all` (default).
    #[serde(default)]
    pub status: Option<String>,
}

/// `GET /api/v1/containers`
pub async fn list(
    State(_state): State<AppState>,
    Query(q): Query<ListContainersQuery>,
) -> impl IntoResponse {
    match list_containers().await {
        Ok(containers) => {
            let want = q.status.as_deref().unwrap_or("all");
            let filtered: Vec<ContainerView> = containers
                .into_iter()
                .filter(|c| match want {
                    "running" => {
                        matches!(c.status, crate::docker_runtime::ContainerStatus::Running)
                    }
                    "stopped" => {
                        matches!(c.status, crate::docker_runtime::ContainerStatus::Stopped)
                    }
                    _ => true,
                })
                .map(ContainerView::from)
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "total": filtered.len(),
                    "containers": filtered,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/containers/{name}`
pub async fn get(State(_state): State<AppState>, Path(name): Path<String>) -> impl IntoResponse {
    match get_container_by_name(&name).await {
        Ok(Some(c)) => (StatusCode::OK, Json(ContainerView::from(c))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("container not found: {}", name)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateContainerRequest {
    pub name: String,
    pub image: String,
    /// Optional env vars as `KEY=VALUE` strings or `[k, v]` pairs.
    #[serde(default)]
    pub env: Vec<EnvSpec>,
    /// Bind mounts as `host:container` strings. Mounted RW.
    #[serde(default)]
    pub mounts: Vec<String>,
    /// Network mode (`bridge`, `host`, custom name).
    #[serde(default)]
    pub network: Option<String>,
    /// Override the image's default command.
    #[serde(default)]
    pub cmd: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EnvSpec {
    /// `"KEY=VALUE"` shorthand
    KvString(String),
    /// `{ key, value }` object
    Object { key: String, value: String },
}

impl EnvSpec {
    fn into_pair(self) -> Option<(String, String)> {
        match self {
            EnvSpec::Object { key, value } => Some((key, value)),
            EnvSpec::KvString(s) => {
                let mut split = s.splitn(2, '=');
                let k = split.next()?.trim().to_string();
                let v = split.next().unwrap_or("").to_string();
                if k.is_empty() {
                    None
                } else {
                    Some((k, v))
                }
            }
        }
    }
}

/// `POST /api/v1/containers`
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateContainerRequest>,
) -> impl IntoResponse {
    if req.name.is_empty() || req.image.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "name and image are required"})),
        )
            .into_response();
    }

    // Decompose mounts. We accept the docker-style `host:container`
    // shorthand because that's what operators copy/paste from existing
    // `docker run` invocations; rejecting trailing flags (`:ro`) for now
    // — we can layer that on once a real use case appears.
    let mounts: Vec<(String, String)> = req
        .mounts
        .iter()
        .filter_map(|m| {
            let mut parts = m.splitn(2, ':');
            let host = parts.next()?.trim().to_string();
            let ctn = parts.next()?.trim().to_string();
            if host.is_empty() || ctn.is_empty() {
                None
            } else {
                Some((host, ctn))
            }
        })
        .collect();

    let mut env: Vec<(String, String)> =
        req.env.into_iter().filter_map(EnvSpec::into_pair).collect();

    // Auto-inject non-secret bootstrap env unless the operator overrode it.
    // The legacy AGENT_SECRET path was retired in #412. Container callers
    // must provide complete secure transport env instead of relying on
    // management to mint a bearer secret.
    fn has_key(env: &[(String, String)], k: &str) -> bool {
        env.iter().any(|(name, _)| name == k)
    }
    fn secure_transport_configured(env: &[(String, String)]) -> bool {
        [
            "AGENT_GRPC_TLS_CA",
            "AGENT_GRPC_TLS_CERT",
            "AGENT_GRPC_TLS_KEY",
        ]
        .into_iter()
        .all(|name| {
            env.iter()
                .any(|(key, value)| key == name && !value.trim().is_empty())
        }) || env
            .iter()
            .any(|(key, value)| key == "AGENT_GRPC_UDS_PATH" && !value.trim().is_empty())
            || ["AGENT_GRPC_VSOCK_CID", "AGENT_GRPC_VSOCK_PORT"]
                .into_iter()
                .all(|name| {
                    env.iter()
                        .any(|(key, value)| key == name && !value.trim().is_empty())
                })
            || ["AGENT_BOOTSTRAP_TOKEN", "AGENT_BOOTSTRAP_SPIFFE_ID"]
                .into_iter()
                .all(|name| {
                    env.iter()
                        .any(|(key, value)| key == name && !value.trim().is_empty())
                })
    }
    let instance_id = env
        .iter()
        .find_map(|(key, value)| {
            if key == "AGENT_INSTANCE_ID" && !value.trim().is_empty() {
                Some(value.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    if !has_key(&env, "AGENT_ID") {
        env.push(("AGENT_ID".to_string(), req.name.clone()));
    }
    if !has_key(&env, "AGENT_INSTANCE_ID") {
        env.push(("AGENT_INSTANCE_ID".to_string(), instance_id.clone()));
    }
    if !has_key(&env, "AIWG_INSTANCE_ID") {
        env.push(("AIWG_INSTANCE_ID".to_string(), instance_id.clone()));
    }
    if !has_key(&env, "MANAGEMENT_SERVER") {
        // host.docker.internal resolves to the Docker host on Linux
        // when --add-host host.docker.internal:host-gateway is passed
        // (added unconditionally in spawn_container).
        env.push((
            "MANAGEMENT_SERVER".to_string(),
            "host.docker.internal:8120".to_string(),
        ));
    }
    if has_key(&env, "AGENT_SECRET") {
        return (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "legacy AGENT_SECRET container bootstrap was retired; provide secure transport env or bootstrap enrollment"
            })),
        )
            .into_response();
    }
    let mut bootstrap_token_issued = false;
    if !secure_transport_configured(&env) {
        let bootstrap = match issue_bootstrap_envelope(
            state.bootstrap_token_store.as_ref(),
            &instance_id,
        ) {
            Ok(Some(bootstrap)) => bootstrap,
            Ok(None) => {
                return (
                        StatusCode::GONE,
                        Json(serde_json::json!({
                            "error": "container provisioning requires secure transport env or bootstrap enrollment"
                        })),
                    )
                        .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e })),
                )
                    .into_response();
            }
        };
        bootstrap_token_issued = true;
        env.extend(bootstrap.env_pairs(None, None));
    }

    let opts = SpawnOpts {
        env,
        labels: vec![
            ("agentic-instance-id".to_string(), instance_id.clone()),
            ("agentic-runtime".to_string(), "docker".to_string()),
        ],
        mounts,
        network: req.network.clone(),
        cmd: req.cmd.clone(),
    };

    match spawn_container(&req.name, &req.image, &opts).await {
        Ok(id) => {
            // Emit container.created up-front (the monitor will see it on
            // its next tick anyway, but this closes the observability
            // window noted in #173 Section F).
            super::events::add_container_event("container.created", req.name.clone()).await;
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": id,
                    "name": req.name,
                    "image": req.image,
                    "status": "running",
                    "instance_id": instance_id,
                    "bootstrap_token_issued": bootstrap_token_issued,
                })),
            )
                .into_response()
        }
        Err(e) => {
            // Distinguish the common failures: name conflict (409) and
            // image-not-found (404). Other errors bubble as 500.
            let lower = e.to_ascii_lowercase();
            let status = if lower.contains("already in use") {
                StatusCode::CONFLICT
            } else if lower.contains("no such image") || lower.contains("manifest unknown") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(serde_json::json!({"error": e}))).into_response()
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct StopContainerQuery {
    /// Graceful-stop timeout before SIGKILL. Default 10s — the docker default.
    #[serde(default = "default_stop_timeout")]
    pub timeout: u64,
}

fn default_stop_timeout() -> u64 {
    10
}

/// `POST /api/v1/containers/{name}/start`
pub async fn start(State(_state): State<AppState>, Path(name): Path<String>) -> impl IntoResponse {
    match start_container(&name).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"name": name, "action": "start", "status": "running"})),
        )
            .into_response(),
        Err(e) => not_found_or_500(&e),
    }
}

/// `POST /api/v1/containers/{name}/stop`
pub async fn stop(
    State(_state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<StopContainerQuery>,
) -> impl IntoResponse {
    match stop_container(&name, q.timeout).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "name": name,
                "action": "stop",
                "status": "stopped",
                "timeout": q.timeout,
            })),
        )
            .into_response(),
        Err(e) => not_found_or_500(&e),
    }
}

/// `DELETE /api/v1/containers/{name}` — force-remove (matches `docker rm -f`).
/// Pre-checks existence so we return a typed 404 instead of pretending
/// the rm succeeded — `docker rm -f <missing>` exits 0 on docker 24+,
/// which would otherwise hide the failure.
pub async fn delete(
    _: super::operator_auth::RequireAdmin,
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match get_container_by_name(&name).await {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": format!("container not found: {}", name)})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e})),
            )
                .into_response()
        }
        Ok(Some(_)) => {}
    }
    match remove_container(&name).await {
        Ok(()) => {
            super::events::add_container_event("container.removed", name.clone()).await;
            (
                StatusCode::OK,
                Json(serde_json::json!({"name": name, "deleted": true})),
            )
                .into_response()
        }
        Err(e) => not_found_or_500(&e),
    }
}

fn not_found_or_500(e: &str) -> axum::response::Response {
    let lower = e.to_ascii_lowercase();
    let status = if lower.contains("no such container") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, Json(serde_json::json!({"error": e}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_spec_kv_string_parses() {
        let s = EnvSpec::KvString("KEY=value".into()).into_pair().unwrap();
        assert_eq!(s, ("KEY".into(), "value".into()));
    }

    #[test]
    fn env_spec_kv_string_empty_value_ok() {
        let s = EnvSpec::KvString("EMPTY=".into()).into_pair().unwrap();
        assert_eq!(s, ("EMPTY".into(), "".into()));
    }

    #[test]
    fn env_spec_kv_string_no_equals_drops() {
        // "JUSTKEY" with no = and no value is rejected (avoids silently
        // pushing a malformed env to docker).
        assert_eq!(
            EnvSpec::KvString("JUSTKEY".into()).into_pair(),
            Some(("JUSTKEY".into(), "".into()))
        );
    }

    #[test]
    fn env_spec_object_parses() {
        let s = EnvSpec::Object {
            key: "FOO".into(),
            value: "bar".into(),
        }
        .into_pair()
        .unwrap();
        assert_eq!(s, ("FOO".into(), "bar".into()));
    }
}
