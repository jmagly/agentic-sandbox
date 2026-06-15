//! v2 Admin/Fleet HTTP API (Surface 1 per ADR-022).
//!
//! Mounted under `/api/v2/admin/...`. Implements the OpenAPI 3.1 spec at
//! `docs/contracts/admin-api.openapi.yaml`. This is the operator-facing
//! fleet-management surface — distinct from the per-instance A2A surface
//! (Surface 2) and from observability (Surface 3).
//!
//! ## Implementation strategy
//!
//! Parallel routing: v1 (`/api/v1/...`) continues to work unchanged.
//! v2 handlers either:
//!
//! 1. Reuse v1 logic by calling into shared registry/service code, then
//!    adapt the response shape to match the v2 OpenAPI contract; or
//! 2. Implement fresh logic for v2-only endpoints (storage by scope/path,
//!    operations envelope, SSE streaming).
//!
//! All non-2xx responses use the RFC 7807 `application/problem+json`
//! envelope defined in `docs/contracts/admin-api/error-envelope.schema.json`.
//!
//! Issue: #215.

use axum::{
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{header, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use super::operations::{Operation, OperationType};
use super::server::AppState;
use crate::host_runtime::{HostProvisionRequest, HostSupervisorError};

// ─── Error envelope (RFC 7807 problem+json) ──────────────────────────────

/// Build the v2 admin router. Mounted at `/api/v2/admin`.
pub fn router() -> Router<AppState> {
    Router::new()
        // Instances
        .route("/instances", get(list_instances).post(provision_instance))
        .route("/instances/{id}", get(get_instance))
        // Lifecycle
        .route("/instances/{id}/start", post(start_instance))
        .route("/instances/{id}/stop", post(stop_instance))
        .route("/instances/{id}/destroy", post(destroy_instance))
        .route("/instances/{id}/restart", post(restart_instance))
        .route("/instances/{id}/reprovision", post(reprovision_instance))
        .route(
            "/instances/{id}/rotate-secret",
            post(rotate_instance_secret_gone),
        )
        // Operations
        .route("/operations/{id}", get(get_operation))
        // Storage — note `{path}` is greedy via wildcard.
        .route(
            "/storage/{scope}/{*path}",
            get(get_storage_object)
                .put(put_storage_object)
                .delete(delete_storage_object),
        )
        // Container images
        .route("/container-images", get(list_container_images))
        // Loadouts
        .route("/loadouts", get(list_loadouts).post(create_loadout))
        // Streaming
        .route("/logs", get(stream_logs))
        .route("/events", get(stream_events))
        // Deprecation observability (#250) — snapshot of the v1 hit
        // counter wired into AppState by `HttpServer::run`, plus the
        // canonical v1→v2 path map and the configured Sunset date.
        .route("/deprecation/v1-counters", get(get_v1_counters))
}

// ─── Deprecation observability (#250) ────────────────────────────────────

/// Response body for `/api/v2/admin/deprecation/v1-counters`. Mirrors the
/// shape the dashboard's `DeprecationTracker` consumes: a snapshot of the
/// per-path hit counter, plus the canonical v1→v2 map and the configured
/// Sunset / successor-version metadata.
#[derive(Serialize)]
pub struct V1CountersResponse {
    /// RFC 7231 IMF-fixdate (mirrors the `Sunset:` response header).
    pub sunset_date: String,
    /// URL of the migration guide (mirrors the `Link: rel="successor-version"`
    /// response header).
    pub successor_url: String,
    /// Canonical v1 path template → v2 successor (or semantic-shift note).
    /// Same data as [`super::compat_v1::path_map`].
    pub path_map: std::collections::HashMap<String, String>,
    /// Per-path hit counts since process start (cumulative). Empty until
    /// the first v1 request lands.
    pub counts: std::collections::HashMap<String, u64>,
}

/// GET /api/v2/admin/deprecation/v1-counters
///
/// Returns the live snapshot of [`V1Counter`](super::compat_v1::V1Counter)
/// alongside the canonical path map and the configured Sunset metadata.
/// `503` (with an RFC 7807 envelope) when the counter wasn't plumbed into
/// `AppState` — that only happens in test harnesses constructed by hand.
pub async fn get_v1_counters(State(state): State<AppState>) -> Response {
    let Some(counter) = state.v1_counter.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "v1_counter.unavailable",
            "v1 hit counter not initialized",
            Some("AppState.v1_counter is None; CompatLayer not wired.".to_string()),
            None,
        );
    };

    let path_map: std::collections::HashMap<String, String> = super::compat_v1::path_map()
        .iter()
        .map(|(v1, v2)| (v1.to_string(), v2.to_string()))
        .collect();

    let body = V1CountersResponse {
        sunset_date: super::compat_v1::DEFAULT_SUNSET.to_string(),
        successor_url: super::compat_v1::DEFAULT_LINK.to_string(),
        path_map,
        counts: counter.snapshot(),
    };

    (StatusCode::OK, Json(body)).into_response()
}

/// Build an RFC 7807 problem+json error response.
fn error_response(
    status: StatusCode,
    code: &str,
    title: &str,
    detail: impl Into<Option<String>>,
    instance_uri: impl Into<Option<String>>,
) -> Response {
    let mut body = json!({
        "type": format!("https://agentic-sandbox.example/problems/{}", code.replace('.', "-")),
        "title": title,
        "status": status.as_u16(),
        "code": code,
    });
    if let Some(d) = detail.into() {
        body["detail"] = Value::String(d);
    }
    if let Some(uri) = instance_uri.into() {
        body["instance"] = Value::String(uri);
    }

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/problem+json")
        .body(Body::from(serde_json::to_vec(&body).unwrap_or_default()))
        .unwrap()
}

fn err_not_found(resource: &str, id: &str, uri: String) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        &format!("{}.not_found", resource),
        &format!("{} not found", capitalize(resource)),
        Some(format!("No {} with id '{}' exists.", resource, id)),
        Some(uri),
    )
}

fn err_validation(detail: &str) -> Response {
    error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation.failed",
        "Validation failed",
        Some(detail.to_string()),
        None,
    )
}

fn err_bad_request(detail: &str) -> Response {
    error_response(
        StatusCode::BAD_REQUEST,
        "request.malformed",
        "Malformed request",
        Some(detail.to_string()),
        None,
    )
}

fn err_storage_scope_disabled(scope: &str) -> Response {
    error_response(
        StatusCode::FORBIDDEN,
        "storage.scope_disabled",
        "Storage scope disabled",
        Some(format!(
            "Scope '{}' is read-only; writes and deletes are not permitted.",
            scope
        )),
        None,
    )
}

fn err_internal(detail: &str) -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal.unavailable",
        "Internal error",
        Some(detail.to_string()),
        None,
    )
}

fn err_service_unavailable(detail: &str) -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "storage.unavailable",
        "Storage unavailable",
        Some(detail.to_string()),
        None,
    )
}

fn err_gone(detail: &str) -> Response {
    error_response(
        StatusCode::GONE,
        "legacy_agent_secret.retired",
        "Legacy agent shared-secret retired",
        Some(detail.to_string()),
        None,
    )
}

fn err_not_implemented(detail: &str) -> Response {
    error_response(
        StatusCode::NOT_IMPLEMENTED,
        "runtime.not_implemented",
        "Runtime not implemented",
        Some(detail.to_string()),
        None,
    )
}

fn host_supervisor_error(err: HostSupervisorError) -> String {
    match err {
        HostSupervisorError::Unavailable(detail)
        | HostSupervisorError::Rejected(detail)
        | HostSupervisorError::Failed(detail) => detail,
    }
}

fn is_host_instance(state: &AppState, instance_id: &str) -> bool {
    state
        .executor_instance_registry
        .as_ref()
        .and_then(|reg| reg.get(instance_id))
        .is_some_and(|ctx| {
            ctx.runtime_kind == agentic_sandbox_executor::instance::RuntimeKind::Host
        })
}

fn err_vm_error(err: &super::vms::VmError) -> Response {
    if let Some(retry_after_seconds) = err.retry_after_seconds() {
        let mut response = error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "libvirt.unresponsive",
            "Libvirt unresponsive",
            Some(err.to_string()),
            None,
        );
        if let Ok(value) = header::HeaderValue::from_str(&retry_after_seconds.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return response;
    }
    err_internal(&err.to_string())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ─── v2 schema shapes ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct Instance {
    id: String,
    name: String,
    runtime: String, // "qemu" | "docker"
    state: String,   // matches InstanceState enum
    agent_card_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    loadout: Option<String>,
    created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<InstanceNetwork>,
}

#[derive(Debug, Serialize)]
struct InstanceNetwork {
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh_port: Option<u32>,
}

#[derive(Debug, Serialize)]
struct InstancesList {
    items: Vec<Instance>,
}

#[derive(Debug, Deserialize)]
struct ListInstancesQuery {
    state: Option<String>,
    runtime: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProvisionRequest {
    name: String,
    runtime: String,
    #[serde(default)]
    loadout: Option<String>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    agentshare: bool,
    #[serde(default)]
    start: bool,
    /// Optional launch cwd for host-backed instances.
    #[serde(default)]
    working_dir: Option<PathBuf>,
    /// Docker bind mounts as `host_path:container_path` strings.
    #[serde(default)]
    mounts: Vec<String>,
    /// Extra Docker labels to attach to container-backed instances.
    #[serde(default)]
    labels: HashMap<String, String>,
    /// Optional startup profile to execute after the agent reaches Ready.
    #[serde(default)]
    startup_profile_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct OperationStatusV2 {
    id: String,
    kind: String,
    state: String,
    created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

#[derive(Debug, Serialize)]
struct StorageObject {
    scope: String,
    path: String,
    media_type: String,
    size_bytes: u64,
    sha256: String,
    modified_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_base64: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StorageObjectWrite {
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    content_base64: Option<String>,
}

#[derive(Debug, Serialize)]
struct ContainerImageV2 {
    name: String,
    reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
struct LoadoutV2 {
    name: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadoutCreateRequest {
    name: String,
    manifest: String,
}

#[derive(Debug, Serialize)]
struct LogEntryV2 {
    timestamp: DateTime<Utc>,
    level: String,
    target: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct EventV2 {
    id: String,
    kind: String,
    timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    #[serde(default)]
    follow: bool,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

// ─── Adapters ────────────────────────────────────────────────────────────

/// Map v1 VM state → v2 InstanceState enum.
fn v1_vmstate_to_v2(state: &super::vms::VmState) -> &'static str {
    use super::vms::VmState;
    match state {
        VmState::Running => "running",
        VmState::Stopped => "stopped",
        VmState::Paused => "stopped",
        VmState::Shutdown => "stopping",
        VmState::Crashed => "failed",
        VmState::Suspended => "stopped",
        VmState::Unknown => "failed",
    }
}

/// Build a v2 Instance from a v1 VmInfo plus optional registry data.
fn build_instance_from_vm(vm: &super::vms::VmInfo, base_url: &str) -> Instance {
    let agent_card_url = format!(
        "{}/agents/{}/.well-known/agent-card.json",
        base_url, vm.uuid
    );
    Instance {
        id: vm.uuid.clone(),
        name: vm.name.clone(),
        runtime: "qemu".to_string(),
        state: v1_vmstate_to_v2(&vm.state).to_string(),
        agent_card_url,
        loadout: None,
        created_at: Utc::now(), // v1 VmInfo doesn't track creation time
        network: vm.ip_address.as_ref().map(|ip| InstanceNetwork {
            ip: Some(ip.clone()),
            ssh_port: Some(22),
        }),
    }
}

/// Default base URL for the AgentCard. Production deployments should
/// expose this via configuration; for now we use the bind address.
fn default_base_url() -> String {
    "https://localhost:8122".to_string()
}

/// Map v1 OperationType → v2 OperationKind string.
fn v1_optype_to_v2(t: &OperationType) -> &'static str {
    match t {
        OperationType::VmCreate => "instance.provision",
        OperationType::VmDelete => "instance.destroy",
        OperationType::VmRestart => "instance.restart",
    }
}

/// Map v1 OperationState → v2 OperationState string.
fn v1_opstate_to_v2(s: &super::operations::OperationState) -> &'static str {
    use super::operations::OperationState;
    match s {
        OperationState::Pending => "pending",
        OperationState::Running => "running",
        OperationState::Completed => "succeeded",
        OperationState::Failed { .. } => "failed",
    }
}

fn op_to_v2(op: &Operation) -> OperationStatusV2 {
    let error = match &op.state {
        super::operations::OperationState::Failed { error } => Some(json!({
            "type": "about:blank",
            "title": "Operation failed",
            "status": 500,
            "code": "operation.failed",
            "detail": error,
        })),
        _ => None,
    };
    OperationStatusV2 {
        id: op.id.clone(),
        kind: v1_optype_to_v2(&op.op_type).to_string(),
        state: v1_opstate_to_v2(&op.state).to_string(),
        created_at: op.created_at,
        completed_at: op.completed_at,
        result: op.result.clone(),
        error,
    }
}

/// Insert a synthetic "succeeded" operation for v1 endpoints that
/// don't go through the OperationStore. Returns (op_id, op_status_json).
fn synth_succeeded_op(
    state: &AppState,
    kind: &'static str,
    target: String,
    result: Option<Value>,
) -> (String, Value) {
    let op_type = match kind {
        "instance.destroy" => OperationType::VmDelete,
        "instance.restart" => OperationType::VmRestart,
        _ => OperationType::VmCreate,
    };
    let mut op = Operation::new(op_type, target);
    op.state = super::operations::OperationState::Completed;
    op.completed_at = Some(Utc::now());
    op.progress_percent = 100;
    op.result = result;

    let id = op.id.clone();
    if let Some(store) = state.operation_store.as_ref() {
        store.insert(op.clone());
    }
    let v2 = op_to_v2(&op);
    (id, serde_json::to_value(&v2).unwrap_or_default())
}

// ─── Handlers: instances ─────────────────────────────────────────────────

async fn list_instances(
    State(state): State<AppState>,
    Query(q): Query<ListInstancesQuery>,
) -> Response {
    // Reuse v1 list_vms logic but adapt response shape.
    let registry = state.registry.clone();
    let result = super::vms::libvirt_read(
        "admin_v2.instances.list",
        move || -> Result<Vec<super::vms::VmInfo>, super::vms::VmError> {
            let conn = super::vms::connect_libvirt()?;
            let domains = conn.list_all_domains(0).map_err(|e| {
                super::vms::VmError::LibvirtError(format!("Failed to list domains: {}", e))
            })?;
            let mut vms = Vec::new();
            for domain in domains {
                let name = match domain.get_name() {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                // v1 listed only "agent-" prefixed VMs. v2 admin is the
                // orchestrator inventory, so also include any libvirt domain
                // that has called back and is present in the AgentRegistry.
                if !name.starts_with("agent-") && registry.get(&name).is_none() {
                    continue;
                }
                let vm_state = match super::vms::get_domain_state(&domain) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let info = domain.get_info();
                let domain_uuid = domain.get_uuid_string().unwrap_or_default();
                if let Ok(info) = info {
                    let agent_entry = registry.get(&name);
                    let ip = agent_entry
                        .as_ref()
                        .map(|a| a.registration.ip_address.clone());
                    let uuid = agent_entry
                        .as_ref()
                        .map(|a| a.instance_id.clone())
                        .filter(|id| !id.is_empty())
                        .unwrap_or(domain_uuid);
                    vms.push(super::vms::VmInfo {
                        name,
                        state: vm_state,
                        uuid,
                        vcpus: info.nr_virt_cpu,
                        memory_mb: info.max_mem / 1024,
                        ip_address: ip,
                        uptime_seconds: None,
                    });
                }
            }
            Ok(vms)
        },
    )
    .await;

    let vms = match result {
        Ok(v) => v,
        Err(e) => return err_vm_error(&e),
    };

    let base_url = default_base_url();
    let mut items: Vec<Instance> = vms
        .iter()
        .map(|v| build_instance_from_vm(v, &base_url))
        .collect();

    // #268: also include docker-backed instances. v2 admin had been
    // returning libvirt VMs only, so a provisioned container never
    // appeared in /api/v2/admin/instances even when the operation
    // reported succeeded. Look up the canonical instance_id via the
    // executor InstanceRegistry where available (containers register
    // there at provision time); fall back to the container name when
    // the registry isn't mounted.
    if let Ok(containers) = crate::docker_runtime::list_containers().await {
        for c in &containers {
            // AGENT_ID = container name (set at provision time), so the
            // AgentRegistry entry — when the container has connected back —
            // exposes the canonical UUIDv7 via `instance_id`. Containers
            // that exited before registration (the failure case this issue
            // fixes at provisioning time) still appear in the inventory
            // using the container name so operators can see the failure.
            let instance_id = state
                .registry
                .get(&c.name)
                .map(|entry| entry.value().instance_id.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| c.name.clone());
            items.push(build_instance_from_container(&instance_id, c, &base_url));
        }
    }

    // Apply filters
    if let Some(s) = q.state.as_deref() {
        items.retain(|i| i.state == s);
    }
    if let Some(r) = q.runtime.as_deref() {
        items.retain(|i| i.runtime == r);
    }

    Json(InstancesList { items }).into_response()
}

/// #268: Build a v2 Instance from a docker ContainerInfo. State mapping
/// mirrors `v1_vmstate_to_v2` semantics so dashboard filters work
/// identically across runtimes:
///   running → "running" ; stopped → "stopped" ; other → as-is.
fn build_instance_from_container(
    instance_id: &str,
    c: &crate::docker_runtime::ContainerInfo,
    base_url: &str,
) -> Instance {
    let state = match &c.status {
        crate::docker_runtime::ContainerStatus::Running => "running".to_string(),
        crate::docker_runtime::ContainerStatus::Stopped => "stopped".to_string(),
        crate::docker_runtime::ContainerStatus::Other(s) => s.clone(),
    };
    let agent_card_url = format!(
        "{}/agents/{}/.well-known/agent-card.json",
        base_url, instance_id
    );
    Instance {
        id: instance_id.to_string(),
        name: c.name.clone(),
        runtime: "docker".to_string(),
        state,
        agent_card_url,
        loadout: None,
        created_at: Utc::now(),
        network: None,
    }
}

/// Resolve the path to `provision-vm.sh`. Honors the `AIWG_PROVISION_VM_SCRIPT`
/// environment variable for test fixtures; otherwise falls back to the same
/// search order used by `vms_extended::find_provision_script`. Returns a
/// best-effort `PathBuf` even if the file does not exist on disk — the
/// spawned task surfaces a useful error in that case via stderr capture.
fn provision_vm_script_path() -> PathBuf {
    if let Ok(p) = std::env::var("AIWG_PROVISION_VM_SCRIPT") {
        return PathBuf::from(p);
    }
    let rel = "images/qemu/provision-vm.sh";
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for candidate in [
        cwd.join("..").join(rel),
        cwd.join(rel),
        PathBuf::from("/opt/agentic-sandbox").join(rel),
    ] {
        if candidate.exists() {
            return candidate;
        }
    }
    // Default fallback: relative path, lets the spawn error report it.
    PathBuf::from(format!("./{}", rel))
}

#[cfg(test)]
fn parse_docker_mount_specs(specs: &[String]) -> Result<Vec<(String, String)>, String> {
    specs
        .iter()
        .map(|mount| {
            let mut parts = mount.splitn(2, ':');
            let host = parts.next().unwrap_or_default().trim();
            let container = parts.next().unwrap_or_default().trim();
            if host.is_empty() || container.is_empty() {
                return Err(format!(
                    "invalid docker mount '{mount}'; expected host_path:container_path"
                ));
            }
            if !container.starts_with('/') {
                return Err(format!(
                    "invalid docker mount '{mount}'; container path must be absolute"
                ));
            }
            Ok((host.to_string(), container.to_string()))
        })
        .collect()
}

#[cfg(test)]
fn has_workspace_mount(mounts: &[(String, String)]) -> bool {
    mounts
        .iter()
        .any(|(_, container)| container == "/workspace")
}

fn bootstrap_trust_domain() -> String {
    std::env::var("AGENTIC_GRPC_LOCAL_CA_TRUST_DOMAIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "sandbox.agentic.local".to_string())
}

fn bootstrap_token_ttl() -> Duration {
    const DEFAULT_TTL_SECS: u64 = 10 * 60;
    let secs = std::env::var("AGENTIC_BOOTSTRAP_TOKEN_TTL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TTL_SECS);
    Duration::from_secs(secs)
}

fn bootstrap_spiffe_id(instance_id: &str) -> String {
    format!(
        "spiffe://{}/agent/{}",
        bootstrap_trust_domain(),
        instance_id
    )
}

fn provision_output_excerpt(bytes: &[u8], bootstrap_token: Option<&str>, limit: usize) -> String {
    let mut text = String::from_utf8_lossy(bytes).to_string();
    if let Some(token) = bootstrap_token.filter(|token| !token.is_empty()) {
        text = text.replace(token, "[REDACTED_BOOTSTRAP_TOKEN]");
    }
    text.chars().take(limit).collect()
}

async fn provision_instance(
    State(state): State<AppState>,
    Json(req): Json<ProvisionRequest>,
) -> Response {
    // Validate runtime
    if req.runtime != "qemu" && req.runtime != "docker" && req.runtime != "host" {
        return err_validation(&format!(
            "runtime must be 'qemu', 'docker', or 'host', got '{}'",
            req.runtime
        ));
    }
    // Validate name
    let name_re = regex::Regex::new(r"^[a-z][a-z0-9-]{1,62}$").unwrap();
    if !name_re.is_match(&req.name) {
        return err_validation("name must match ^[a-z][a-z0-9-]{1,62}$");
    }
    if req.runtime == "host" && state.host_runtime_supervisor.is_none() {
        return err_not_implemented(
            "host runtime requires the durable host supervisor/daemon tracked by agentic-sandbox#460 before provisioning can run safely",
        );
    }
    let startup_profile_id = req
        .startup_profile_id
        .as_ref()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    if let Some(id) = startup_profile_id.as_deref() {
        if state.startup_profiles.get(id).is_err() {
            return err_not_found(
                "startup_profile",
                id,
                format!("/api/v2/startup-profiles/{}", id),
            );
        }
    }

    let store = match state.operation_store.as_ref() {
        Some(s) => s.clone(),
        None => return err_internal("operation store unavailable"),
    };

    // #252: generate the canonical instance UUIDv7 BEFORE the idempotency
    // check so the operation row carries it. Subsequent dispatches see
    // the same op via `find_active_by_target` (key is `req.name`) and
    // therefore reuse the same instance_id automatically.
    let instance_id = uuid::Uuid::now_v7().to_string();

    // Idempotency: if a pending/running provision is already in flight for
    // this instance name, return that op instead of starting another.
    if let Some(existing) = store.find_active_by_target(&req.name, &OperationType::VmCreate) {
        let v2 = op_to_v2(&existing);
        let location = format!("/api/v2/admin/operations/{}", existing.id);
        let body = serde_json::to_vec(&v2).unwrap_or_default();
        return Response::builder()
            .status(StatusCode::ACCEPTED)
            .header(header::LOCATION, location)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
    }

    if let Some(profile_id) = startup_profile_id.as_deref() {
        if let Err(err) = state
            .startup_profiles
            .bind_instance_profile(&instance_id, profile_id)
        {
            tracing::warn!(
                instance = %req.name,
                instance_id = %instance_id,
                startup_profile_id = %profile_id,
                error = %err,
                "failed to bind startup profile to provisioned instance"
            );
            return match err {
                crate::startup_profiles::StartupProfileError::NotFound(_) => err_not_found(
                    "startup_profile",
                    profile_id,
                    format!("/api/v2/startup-profiles/{}", profile_id),
                ),
                _ => err_internal(&format!("failed to bind startup profile: {}", err)),
            };
        }
    }

    // Record pending, then transition to running before spawning the worker.
    // The HTTP response body reflects the pending snapshot so callers see a
    // stable initial state; subsequent GET /operations/{id} shows running →
    // succeeded/failed as the spawned task progresses.
    let mut op = Operation::new(OperationType::VmCreate, req.name.clone());
    op.state = super::operations::OperationState::Pending;
    let op_id = op.id.clone();
    let response_snapshot = op.clone();
    store.insert(op);
    store.update_state(&op_id, super::operations::OperationState::Running);

    // Spawn the runtime-specific provisioning worker. The v2 response body
    // still reports the snapshot taken before the transition to `running`
    // so the caller observes the operation_id immediately; subsequent
    // GET /operations/{id} reflects live state.
    let op_id_task = op_id.clone();
    let store_task = store.clone();
    let req_name = req.name.clone();
    let runtime = req.runtime.clone();
    let loadout = req.loadout.clone();
    let profile = req.profile.clone();
    let image = req.image.clone();
    let agentshare = req.agentshare;
    let start = req.start;
    let working_dir = req.working_dir.clone();
    let startup_profile_id_for_task = startup_profile_id.clone();
    let registry = state.registry.clone();
    let inst_id_task = instance_id.clone();
    let startup_profiles_for_task = state.startup_profiles.clone();
    // #252: capture executor handles for post-success InstanceContext
    // registration. These are `None` when the executor surface wasn't
    // mounted (e.g. unit tests without an executor binding).
    let exec_registry = state.executor_instance_registry.clone();
    let signing_keys_dir = state.executor_signing_keys_dir.clone();
    let bootstrap_store_for_task = state.bootstrap_token_store.clone();
    let host_supervisor_for_task = state.host_runtime_supervisor.clone();
    let runtime_kind_for_ctx = match runtime.as_str() {
        "docker" => agentic_sandbox_executor::instance::RuntimeKind::Container,
        "host" => agentic_sandbox_executor::instance::RuntimeKind::Host,
        _ => agentic_sandbox_executor::instance::RuntimeKind::Vm,
    };
    let loadout_for_ctx = loadout.clone();
    let image_for_ctx = image.clone();

    tokio::spawn(async move {
        let adapter_command_supported_for_ctx =
            runtime_kind_for_ctx != agentic_sandbox_executor::instance::RuntimeKind::Container;
        let result: Result<serde_json::Value, String> = match runtime.as_str() {
            "qemu" => 'qemu_branch: {
                let script = provision_vm_script_path();
                let mut cmd = tokio::process::Command::new(&script);
                let bootstrap_token = if let Some(store) = bootstrap_store_for_task.as_ref() {
                    let spiffe_id = bootstrap_spiffe_id(&inst_id_task);
                    match store.issue(&inst_id_task, &spiffe_id, bootstrap_token_ttl()) {
                        Ok(issued) => {
                            cmd.env("AGENT_BOOTSTRAP_TOKEN", &issued.token);
                            cmd.env("AGENT_BOOTSTRAP_SPIFFE_ID", &issued.spiffe_id);
                            cmd.env(
                                "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS",
                                issued.expires_at_unix_ms.to_string(),
                            );
                            Some(issued)
                        }
                        Err(e) => {
                            break 'qemu_branch Err(format!(
                                "failed to issue bootstrap token: {}",
                                e
                            ));
                        }
                    }
                } else {
                    None
                };
                if let Some(lo) = loadout.as_deref() {
                    cmd.arg("--loadout").arg(lo);
                } else if let Some(p) = profile.as_deref() {
                    cmd.arg("--profile").arg(p);
                }
                if agentshare {
                    cmd.arg("--agentshare");
                }
                if start {
                    cmd.arg("--start");
                }
                // #252: pass the canonical UUIDv7 so cloud-init can write
                // AGENT_INSTANCE_ID into /etc/agentic-sandbox/agent.env.
                cmd.arg("--instance-id").arg(&inst_id_task);
                cmd.arg(&req_name);

                tracing::info!(
                    instance = %req_name,
                    instance_id = %inst_id_task,
                    operation = %op_id_task,
                    script = %script.display(),
                    "v2 admin: spawning provision-vm.sh"
                );

                match cmd.output().await {
                    Ok(out) if out.status.success() => Ok(json!({
                        "instance_id": inst_id_task,
                        "name": req_name,
                        "runtime": "qemu",
                        "provisioned": true,
                        "startup_profile_id": startup_profile_id_for_task,
                        "bootstrap_token_issued": bootstrap_token.is_some(),
                        "bootstrap_spiffe_id": bootstrap_token
                            .as_ref()
                            .map(|issued| issued.spiffe_id.clone()),
                        "bootstrap_token_expires_at_unix_ms": bootstrap_token
                            .as_ref()
                            .map(|issued| issued.expires_at_unix_ms),
                        "stdout_excerpt": provision_output_excerpt(
                            &out.stdout,
                            bootstrap_token.as_ref().map(|issued| issued.token.as_str()),
                            512,
                        ),
                    })),
                    Ok(out) => {
                        let stderr = provision_output_excerpt(
                            &out.stderr,
                            bootstrap_token.as_ref().map(|issued| issued.token.as_str()),
                            4096,
                        );
                        Err(format!(
                            "provision-vm.sh exited with code {}: {}",
                            out.status.code().unwrap_or(-1),
                            stderr
                        ))
                    }
                    Err(e) => Err(format!("failed to spawn provision-vm.sh: {}", e)),
                }
            }
            "docker" => Err(
                "docker provisioning requires secure transport material; legacy AGENT_SECRET bootstrap was retired in #412"
                    .to_string(),
            ),
            "host" => {
                let Some(supervisor) = host_supervisor_for_task.as_ref() else {
                    return store_task.mark_failed(
                        &op_id_task,
                        "host runtime requires the durable host supervisor/daemon tracked by agentic-sandbox#460".to_string(),
                    );
                };
                let request = HostProvisionRequest {
                    instance_id: inst_id_task.clone(),
                    name: req_name.clone(),
                    loadout: loadout.clone(),
                    profile: profile.clone(),
                    image_ref: image.clone(),
                    agentshare,
                    start,
                    working_dir,
                    labels: req.labels.clone(),
                    startup_profile_id: startup_profile_id_for_task.clone(),
                };
                supervisor
                    .provision(request)
                    .await
                    .map_err(host_supervisor_error)
                    .and_then(|provisioned| {
                        if provisioned.instance_id != inst_id_task {
                            return Err(format!(
                                "host supervisor returned mismatched instance_id: expected {}, got {}",
                                inst_id_task, provisioned.instance_id
                            ));
                        }
                        Ok(json!({
                            "instance_id": provisioned.instance_id,
                            "name": provisioned.name,
                            "runtime": "host",
                            "provisioned": true,
                            "startup_profile_id": startup_profile_id_for_task,
                            "supervisor_id": provisioned.supervisor_id,
                            "host_endpoint": provisioned.host_endpoint,
                            "session_backend": provisioned.session_backend,
                            "watch_agents": provisioned.watch_agents,
                            "isolation": "host",
                        }))
                    })
            }
            other => Err(format!("unsupported runtime: {}", other)),
        };

        match result {
            Ok(v) => {
                let context_host = if runtime_kind_for_ctx
                    == agentic_sandbox_executor::instance::RuntimeKind::Host
                {
                    v.get("host_endpoint")
                        .and_then(|value| value.as_str())
                        .unwrap_or("executor.local")
                        .to_string()
                } else {
                    "executor.local".to_string()
                };
                // #252: register the InstanceContext in the executor's
                // InstanceRegistry so `/agents/{instance_id}/...` routes
                // resolve to a real context. Failure is non-fatal — the
                // provision still succeeded at the underlying runtime,
                // and the operator gets a clear log line.
                if let (Some(reg), Some(key_dir)) =
                    (exec_registry.as_ref(), signing_keys_dir.as_ref())
                {
                    match agentic_sandbox_executor::instance::InstanceContext::new(
                        inst_id_task.clone(),
                        runtime_kind_for_ctx,
                        loadout_for_ctx.unwrap_or_else(|| "agentic-dev".to_string()),
                        image_for_ctx,
                        context_host,
                        key_dir,
                    ) {
                        Ok(ctx) => {
                            ctx.set_adapter_command_supported(adapter_command_supported_for_ctx);
                            reg.insert(std::sync::Arc::new(ctx));
                            tracing::info!(
                                instance_id = %inst_id_task,
                                "registered InstanceContext in executor registry"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                instance_id = %inst_id_task,
                                error = %e,
                                "failed to build InstanceContext; /agents/* will 404 until next provision"
                            );
                        }
                    }
                } else {
                    tracing::debug!(
                        instance_id = %inst_id_task,
                        "executor surface not mounted; skipping InstanceRegistry insert"
                    );
                }
                store_task.mark_completed(&op_id_task, Some(v));
                let _ = registry; // keep the agent_registry clone alive
            }
            Err(e) => {
                tracing::warn!(
                    instance = %req_name,
                    operation = %op_id_task,
                    error = %e,
                    "v2 admin: provision failed"
                );
                if startup_profile_id_for_task.is_some() {
                    if let Err(unbind_err) =
                        startup_profiles_for_task.unbind_instance_profile(&inst_id_task)
                    {
                        tracing::warn!(
                            instance = %req_name,
                            instance_id = %inst_id_task,
                            error = %unbind_err,
                            "failed to remove startup profile binding after provision failure"
                        );
                    }
                }
                store_task.mark_failed(&op_id_task, e);
            }
        }
    });

    let v2 = op_to_v2(&response_snapshot);
    let location = format!("/api/v2/admin/operations/{}", op_id);
    // Include the assigned instance_id in the response envelope so the caller
    // doesn't have to wait for the operation to terminate to learn it.
    let mut body_val = serde_json::to_value(&v2).unwrap_or_default();
    if let Some(obj) = body_val.as_object_mut() {
        obj.insert(
            "instance_id".to_string(),
            serde_json::Value::String(instance_id),
        );
        if let Some(profile_id) = startup_profile_id {
            obj.insert(
                "startup_profile_id".to_string(),
                serde_json::Value::String(profile_id),
            );
        }
    }
    let body = serde_json::to_vec(&body_val).unwrap_or_default();
    Response::builder()
        .status(StatusCode::ACCEPTED)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn get_instance(State(state): State<AppState>, AxPath(id): AxPath<String>) -> Response {
    let registry = state.registry.clone();
    let id_blk = id.clone();
    let result = super::vms::libvirt_read(
        "admin_v2.instances.get",
        move || -> Result<super::vms::VmInfo, super::vms::VmError> {
            let conn = super::vms::connect_libvirt()?;
            let domain = super::vms::get_domain(&conn, &id_blk)?;
            let name = domain
                .get_name()
                .map_err(|e| super::vms::VmError::LibvirtError(e.to_string()))?;
            let vm_state = super::vms::get_domain_state(&domain)?;
            let uuid = domain.get_uuid_string().unwrap_or_default();
            let info = domain
                .get_info()
                .map_err(|e| super::vms::VmError::LibvirtError(e.to_string()))?;
            let ip = registry
                .get(&name)
                .map(|a| a.registration.ip_address.clone());
            Ok(super::vms::VmInfo {
                name,
                state: vm_state,
                uuid,
                vcpus: info.nr_virt_cpu,
                memory_mb: info.max_mem / 1024,
                ip_address: ip,
                uptime_seconds: None,
            })
        },
    )
    .await;

    match result {
        Ok(vm) => {
            let inst = build_instance_from_vm(&vm, &default_base_url());
            Json(inst).into_response()
        }
        Err(e) => match e {
            super::vms::VmError::NotFound(_) => {
                err_not_found("instance", &id, format!("/api/v2/admin/instances/{}", id))
            }
            other => err_vm_error(&other),
        },
    }
}

// ─── Handlers: lifecycle ─────────────────────────────────────────────────

async fn start_instance(State(state): State<AppState>, AxPath(id): AxPath<String>) -> Response {
    let id_blk = id.clone();
    let result = super::vms::libvirt_write(
        "admin_v2.instances.start",
        move || -> Result<(), super::vms::VmError> {
            let conn = super::vms::connect_libvirt()?;
            let domain = super::vms::get_domain(&conn, &id_blk)?;
            let s = super::vms::get_domain_state(&domain)?;
            if s != super::vms::VmState::Running {
                domain
                    .create()
                    .map_err(|e| super::vms::VmError::LibvirtError(e.to_string()))?;
            }
            Ok(())
        },
    )
    .await;

    match result {
        Ok(_) => {
            let (_, op_body) = synth_succeeded_op(&state, "instance.start", id.clone(), None);
            let location = format!(
                "/api/v2/admin/operations/{}",
                op_body.get("id").and_then(|v| v.as_str()).unwrap_or("")
            );
            Response::builder()
                .status(StatusCode::ACCEPTED)
                .header(header::LOCATION, location)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&op_body).unwrap_or_default()))
                .unwrap()
        }
        Err(e) => match e {
            super::vms::VmError::NotFound(_) => err_not_found(
                "instance",
                &id,
                format!("/api/v2/admin/instances/{}/start", id),
            ),
            other => err_vm_error(&other),
        },
    }
}

async fn stop_instance(State(state): State<AppState>, AxPath(id): AxPath<String>) -> Response {
    if is_host_instance(&state, &id) {
        let Some(supervisor) = state.host_runtime_supervisor.as_ref() else {
            return err_not_implemented(
                "host runtime lifecycle requires a configured host supervisor/daemon",
            );
        };
        return match supervisor.stop(&id).await {
            Ok(result) => {
                if let Some(ctx) = state
                    .executor_instance_registry
                    .as_ref()
                    .and_then(|reg| reg.get(&id))
                {
                    ctx.set_ready(false);
                }
                let (_, op_body) = synth_succeeded_op(
                    &state,
                    "instance.stop",
                    id.clone(),
                    Some(json!({
                        "instance_id": result.instance_id,
                        "runtime": "host",
                        "state": result.state,
                        "supervisor_id": result.supervisor_id,
                        "watch_agents": result.watch_agents,
                    })),
                );
                let location = format!(
                    "/api/v2/admin/operations/{}",
                    op_body.get("id").and_then(|v| v.as_str()).unwrap_or("")
                );
                Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .header(header::LOCATION, location)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&op_body).unwrap_or_default()))
                    .unwrap()
            }
            Err(err) => err_internal(&host_supervisor_error(err)),
        };
    }

    let id_blk = id.clone();
    let result = super::vms::libvirt_write(
        "admin_v2.instances.stop",
        move || -> Result<(), super::vms::VmError> {
            let conn = super::vms::connect_libvirt()?;
            let domain = super::vms::get_domain(&conn, &id_blk)?;
            let s = super::vms::get_domain_state(&domain)?;
            if s == super::vms::VmState::Stopped {
                return Ok(());
            }
            domain
                .shutdown()
                .map_err(|e| super::vms::VmError::LibvirtError(e.to_string()))?;
            Ok(())
        },
    )
    .await;

    match result {
        Ok(_) => {
            let (_, op_body) = synth_succeeded_op(&state, "instance.stop", id.clone(), None);
            let location = format!(
                "/api/v2/admin/operations/{}",
                op_body.get("id").and_then(|v| v.as_str()).unwrap_or("")
            );
            Response::builder()
                .status(StatusCode::ACCEPTED)
                .header(header::LOCATION, location)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&op_body).unwrap_or_default()))
                .unwrap()
        }
        Err(e) => match e {
            super::vms::VmError::NotFound(_) => err_not_found(
                "instance",
                &id,
                format!("/api/v2/admin/instances/{}/stop", id),
            ),
            other => err_vm_error(&other),
        },
    }
}

async fn destroy_instance(State(state): State<AppState>, AxPath(id): AxPath<String>) -> Response {
    if is_host_instance(&state, &id) {
        let Some(supervisor) = state.host_runtime_supervisor.as_ref() else {
            return err_not_implemented(
                "host runtime lifecycle requires a configured host supervisor/daemon",
            );
        };
        return match supervisor.destroy(&id).await {
            Ok(result) => {
                remove_instance_from_executor(&state, &id);
                let (_, op_body) = synth_succeeded_op(
                    &state,
                    "instance.destroy",
                    id.clone(),
                    Some(json!({
                        "instance_id": result.instance_id,
                        "runtime": "host",
                        "state": result.state,
                        "supervisor_id": result.supervisor_id,
                        "watch_agents": result.watch_agents,
                    })),
                );
                let location = format!(
                    "/api/v2/admin/operations/{}",
                    op_body.get("id").and_then(|v| v.as_str()).unwrap_or("")
                );
                Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .header(header::LOCATION, location)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&op_body).unwrap_or_default()))
                    .unwrap()
            }
            Err(err) => err_internal(&host_supervisor_error(err)),
        };
    }

    let id_blk = id.clone();
    let result = super::vms::libvirt_write(
        "admin_v2.instances.destroy",
        move || -> Result<(), super::vms::VmError> {
            let conn = super::vms::connect_libvirt()?;
            let domain = super::vms::get_domain(&conn, &id_blk)?;
            let s = super::vms::get_domain_state(&domain)?;
            if s != super::vms::VmState::Stopped {
                domain
                    .destroy()
                    .map_err(|e| super::vms::VmError::LibvirtError(e.to_string()))?;
            }
            domain
                .undefine()
                .map_err(|e| super::vms::VmError::LibvirtError(e.to_string()))?;
            Ok(())
        },
    )
    .await;

    match result {
        Ok(_) => {
            // #252: drain the instance from the executor's registry and
            // best-effort delete its signing-key directory so a future
            // re-provision under the same id starts fresh.
            remove_instance_from_executor(&state, &id);

            let (_, op_body) = synth_succeeded_op(&state, "instance.destroy", id.clone(), None);
            let location = format!(
                "/api/v2/admin/operations/{}",
                op_body.get("id").and_then(|v| v.as_str()).unwrap_or("")
            );
            Response::builder()
                .status(StatusCode::ACCEPTED)
                .header(header::LOCATION, location)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&op_body).unwrap_or_default()))
                .unwrap()
        }
        Err(e) => match e {
            super::vms::VmError::NotFound(_) => err_not_found(
                "instance",
                &id,
                format!("/api/v2/admin/instances/{}/destroy", id),
            ),
            other => err_vm_error(&other),
        },
    }
}

/// Remove an instance from the executor's `InstanceRegistry` and best-effort
/// delete its on-disk signing key directory (#252). Safe to call when the
/// executor surface isn't mounted — both `executor_instance_registry` and
/// `executor_signing_keys_dir` are `None` in that case.
pub(super) fn remove_instance_from_executor(state: &AppState, instance_id: &str) {
    if let Some(reg) = state.executor_instance_registry.as_ref() {
        let removed = reg.remove(instance_id).is_some();
        if removed {
            tracing::info!(
                instance_id = %instance_id,
                "removed InstanceContext from executor registry"
            );
        }
    }
    if let Some(key_dir) = state.executor_signing_keys_dir.as_ref() {
        let inst_dir = key_dir.join(instance_id);
        if inst_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&inst_dir) {
                tracing::warn!(
                    instance_id = %instance_id,
                    path = %inst_dir.display(),
                    error = %e,
                    "failed to remove signing-key directory; will leak on disk"
                );
            }
        }
    }
}

async fn restart_instance(State(state): State<AppState>, AxPath(id): AxPath<String>) -> Response {
    let id_blk = id.clone();
    let result = super::vms::libvirt_write(
        "admin_v2.instances.restart",
        move || -> Result<(), super::vms::VmError> {
            let conn = super::vms::connect_libvirt()?;
            let domain = super::vms::get_domain(&conn, &id_blk)?;
            let s = super::vms::get_domain_state(&domain)?;
            if s == super::vms::VmState::Running {
                domain
                    .reboot(0)
                    .map_err(|e| super::vms::VmError::LibvirtError(e.to_string()))?;
            } else {
                domain
                    .create()
                    .map_err(|e| super::vms::VmError::LibvirtError(e.to_string()))?;
            }
            Ok(())
        },
    )
    .await;

    match result {
        Ok(_) => {
            let (_, op_body) = synth_succeeded_op(&state, "instance.restart", id.clone(), None);
            let location = format!(
                "/api/v2/admin/operations/{}",
                op_body.get("id").and_then(|v| v.as_str()).unwrap_or("")
            );
            Response::builder()
                .status(StatusCode::ACCEPTED)
                .header(header::LOCATION, location)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&op_body).unwrap_or_default()))
                .unwrap()
        }
        Err(e) => match e {
            super::vms::VmError::NotFound(_) => err_not_found(
                "instance",
                &id,
                format!("/api/v2/admin/instances/{}/restart", id),
            ),
            other => err_vm_error(&other),
        },
    }
}

#[derive(Debug, Deserialize)]
struct ReprovisionRequest {
    #[serde(default)]
    loadout: Option<String>,
}

async fn reprovision_instance(
    State(state): State<AppState>,
    AxPath(id): AxPath<String>,
    body: Option<Json<ReprovisionRequest>>,
) -> Response {
    let _ = body; // loadout argument is forwarded to the v1 pipeline when integrated
    let mut op = Operation::new(OperationType::VmCreate, id.clone());
    op.state = super::operations::OperationState::Pending;
    let op_id = op.id.clone();
    if let Some(store) = state.operation_store.as_ref() {
        store.insert(op.clone());
    }
    // Kick off the reprovision-vm.sh script asynchronously. Mirrors the v1
    // handler in server.rs::agent_reprovision_handler.
    let script_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("scripts/reprovision-vm.sh");
    if let Some(store) = state.operation_store.as_ref() {
        let store = store.clone();
        let op_id_task = op_id.clone();
        let vm_name = id.clone();
        if script_path.exists() {
            tokio::spawn(async move {
                let output = tokio::process::Command::new("bash")
                    .arg(&script_path)
                    .arg(&vm_name)
                    .output()
                    .await;
                match output {
                    Ok(o) if o.status.success() => store.mark_completed(
                        &op_id_task,
                        Some(json!({"instance_id": vm_name, "reprovisioned": true})),
                    ),
                    Ok(o) => store.mark_failed(
                        &op_id_task,
                        format!("reprovision failed: {}", String::from_utf8_lossy(&o.stderr)),
                    ),
                    Err(e) => store.mark_failed(&op_id_task, format!("script error: {}", e)),
                }
            });
        } else {
            store.mark_failed(&op_id, "reprovision-vm.sh not found".to_string());
        }
    }
    let v2 = op_to_v2(&op);
    let location = format!("/api/v2/admin/operations/{}", op_id);
    Response::builder()
        .status(StatusCode::ACCEPTED)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&v2).unwrap_or_default()))
        .unwrap()
}

// ─── Handlers: retired legacy secrets ────────────────────────────────────

async fn rotate_instance_secret_gone(AxPath(_id): AxPath<String>) -> Response {
    err_gone("legacy agent shared-secret rotation was retired; use transport identity credentials")
}

// ─── Handlers: operations ────────────────────────────────────────────────

async fn get_operation(State(state): State<AppState>, AxPath(id): AxPath<String>) -> Response {
    let store = match state.operation_store.as_ref() {
        Some(s) => s.clone(),
        None => return err_internal("operation store unavailable"),
    };
    match store.get(&id) {
        Some(op) => Json(op_to_v2(&op)).into_response(),
        None => err_not_found("operation", &id, format!("/api/v2/admin/operations/{}", id)),
    }
}

// ─── Handlers: storage ───────────────────────────────────────────────────

/// Resolve a storage path to an absolute filesystem path under the
/// agentshare / tasks roots. Refuses path traversal.
fn resolve_storage_path(state: &AppState, scope: &str, rel: &str) -> Result<PathBuf, Response> {
    let rel = rel.trim_start_matches('/');
    if rel.contains("..") {
        return Err(err_bad_request("path may not contain '..'"));
    }
    match scope {
        "global" => {
            let root = state
                .agentshare_root
                .as_ref()
                .ok_or_else(|| err_service_unavailable("agentshare root not configured"))?;
            Ok(PathBuf::from(root).join("global-ro").join(rel))
        }
        "inbox" => {
            let root = state
                .agentshare_root
                .as_ref()
                .ok_or_else(|| err_service_unavailable("agentshare root not configured"))?;
            // First path segment is the instance id; rewrite to `<id>-inbox/...`
            let mut parts = rel.splitn(2, '/');
            let instance = parts.next().unwrap_or("");
            if instance.is_empty() || instance.contains('/') || instance == ".." {
                return Err(err_bad_request("inbox path must begin with instance id"));
            }
            let tail = parts.next().unwrap_or("");
            Ok(PathBuf::from(root)
                .join(format!("{}-inbox", instance))
                .join(tail))
        }
        "outbox" => {
            let root = state
                .tasks_root
                .as_ref()
                .ok_or_else(|| err_service_unavailable("tasks root not configured"))?;
            let mut parts = rel.splitn(2, '/');
            let task = parts.next().unwrap_or("");
            if task.is_empty() || task.contains('/') || task == ".." {
                return Err(err_bad_request("outbox path must begin with task id"));
            }
            let tail = parts.next().unwrap_or("");
            Ok(PathBuf::from(root).join(task).join("outbox").join(tail))
        }
        other => Err(err_bad_request(&format!(
            "unknown storage scope: {}",
            other
        ))),
    }
}

async fn get_storage_object(
    State(state): State<AppState>,
    AxPath((scope, path)): AxPath<(String, String)>,
) -> Response {
    let fs_path = match resolve_storage_path(&state, &scope, &path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let bytes = match tokio::fs::read(&fs_path).await {
        Ok(b) => b,
        Err(_) => {
            return err_not_found(
                "object",
                &path,
                format!("/api/v2/admin/storage/{}/{}", scope, path),
            )
        }
    };
    let meta = match tokio::fs::metadata(&fs_path).await {
        Ok(m) => m,
        Err(e) => return err_internal(&format!("stat failed: {}", e)),
    };
    let modified: DateTime<Utc> = meta
        .modified()
        .ok()
        .and_then(|t| Some(DateTime::<Utc>::from(t)))
        .unwrap_or_else(Utc::now);
    let sha = {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    };
    let media_type = mime_guess::from_path(&fs_path)
        .first_or_octet_stream()
        .to_string();
    // Best-effort text decode
    let (content, content_base64) = match std::str::from_utf8(&bytes) {
        Ok(s) if !media_type.starts_with("application/octet-stream") => (Some(s.to_string()), None),
        _ => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            (None, Some(b64))
        }
    };
    let obj = StorageObject {
        scope,
        path,
        media_type,
        size_bytes: bytes.len() as u64,
        sha256: sha,
        modified_at: modified,
        content,
        content_base64,
    };
    Json(obj).into_response()
}

async fn put_storage_object(
    State(state): State<AppState>,
    AxPath((scope, path)): AxPath<(String, String)>,
    Json(body): Json<StorageObjectWrite>,
) -> Response {
    if scope == "global" {
        return err_storage_scope_disabled(&scope);
    }
    let fs_path = match resolve_storage_path(&state, &scope, &path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let bytes: Vec<u8> = match (body.content, body.content_base64) {
        (Some(s), _) => s.into_bytes(),
        (None, Some(b64)) => {
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
                Ok(b) => b,
                Err(e) => return err_validation(&format!("invalid base64: {}", e)),
            }
        }
        (None, None) => return err_validation("body must include 'content' or 'content_base64'"),
    };
    if let Some(parent) = fs_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return err_internal(&format!("mkdir failed: {}", e));
        }
    }
    let existed = tokio::fs::metadata(&fs_path).await.is_ok();
    if let Err(e) = tokio::fs::write(&fs_path, &bytes).await {
        return err_internal(&format!("write failed: {}", e));
    }
    let sha = {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    };
    let obj = StorageObject {
        scope: scope.clone(),
        path: path.clone(),
        media_type: body
            .media_type
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        size_bytes: bytes.len() as u64,
        sha256: sha,
        modified_at: Utc::now(),
        content: None,
        content_base64: None,
    };
    let status = if existed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    let body = serde_json::to_vec(&obj).unwrap_or_default();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn delete_storage_object(
    State(state): State<AppState>,
    AxPath((scope, path)): AxPath<(String, String)>,
) -> Response {
    if scope == "global" {
        return err_storage_scope_disabled(&scope);
    }
    let fs_path = match resolve_storage_path(&state, &scope, &path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if tokio::fs::metadata(&fs_path).await.is_err() {
        return err_not_found(
            "object",
            &path,
            format!("/api/v2/admin/storage/{}/{}", scope, path),
        );
    }
    if let Err(e) = tokio::fs::remove_file(&fs_path).await {
        return err_internal(&format!("delete failed: {}", e));
    }
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .unwrap()
}

// ─── Handlers: container-images & loadouts ──────────────────────────────

async fn list_container_images() -> Response {
    // Reuse v1 static catalog.
    let v1 = super::container_images::list_container_images().await;
    let v2_items: Vec<ContainerImageV2> =
        v1.0.images
            .iter()
            .map(|img| ContainerImageV2 {
                name: img.label.to_string(),
                reference: img.image_ref.to_string(),
                digest: None,
                size_bytes: None,
            })
            .collect();
    Json(json!({"items": v2_items})).into_response()
}

async fn list_loadouts() -> Response {
    // Scan the loadout profiles directory directly. Mirrors v1
    // loadouts::list_loadouts but returns the v2 shape.
    let dir = [
        "images/qemu/loadouts/profiles",
        "../images/qemu/loadouts/profiles",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.is_dir());
    let dir = match dir {
        Some(d) => d,
        None => return Json(json!({"items": []})).into_response(),
    };
    let mut items: Vec<LoadoutV2> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                    let name = yaml
                        .get("metadata")
                        .and_then(|m| m.get("name"))
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            path.file_stem()
                                .and_then(|s| s.to_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_default();
                    let version = yaml
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("1")
                        .to_string();
                    let runtime = yaml
                        .get("runtime")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let description = yaml
                        .get("metadata")
                        .and_then(|m| m.get("description"))
                        .and_then(|d| d.as_str())
                        .map(|s| s.to_string());
                    items.push(LoadoutV2 {
                        name,
                        version,
                        runtime,
                        description,
                        manifest: None,
                    });
                }
            }
        }
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Json(json!({"items": items})).into_response()
}

async fn create_loadout(Json(req): Json<LoadoutCreateRequest>) -> Response {
    // Validate YAML parses
    if serde_yaml::from_str::<serde_yaml::Value>(&req.manifest).is_err() {
        return err_validation("manifest must be valid YAML");
    }
    let v2 = LoadoutV2 {
        name: req.name.clone(),
        version: "1".to_string(),
        runtime: None,
        description: None,
        manifest: Some(req.manifest),
    };
    let body = serde_json::to_vec(&v2).unwrap_or_default();
    Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

// ─── Handlers: streaming ─────────────────────────────────────────────────

async fn stream_logs(Query(q): Query<StreamQuery>) -> Response {
    if !q.follow {
        // Snapshot
        let limit = q.limit.unwrap_or(200).min(5000);
        let entries = crate::telemetry::log_buffer::snapshot(limit);
        let items: Vec<LogEntryV2> = entries
            .into_iter()
            .filter(|e| {
                q.level
                    .as_deref()
                    .map(|lvl| e.level.eq_ignore_ascii_case(lvl))
                    .unwrap_or(true)
            })
            .filter(|e| {
                q.target
                    .as_deref()
                    .map(|t| e.target.contains(t))
                    .unwrap_or(true)
            })
            .map(|e| LogEntryV2 {
                timestamp: e.timestamp,
                level: e.level.to_string(),
                target: e.target,
                message: e.message,
            })
            .collect();
        return Json(json!({"items": items})).into_response();
    }
    // SSE: emit periodic snapshots of new entries.
    // Without a tracing-layer event broadcast we tail the ring buffer.
    let stream = async_stream::stream! {
        let mut seen = 0u64;
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            let snap = crate::telemetry::log_buffer::snapshot(50);
            for entry in snap.into_iter().rev() {
                seen += 1;
                let v2 = LogEntryV2 {
                    timestamp: entry.timestamp,
                    level: entry.level.to_string(),
                    target: entry.target,
                    message: entry.message,
                };
                let data = serde_json::to_string(&v2).unwrap_or_default();
                yield Ok::<_, Infallible>(
                    SseEvent::default().id(seen.to_string()).event("log").data(data),
                );
            }
        }
    };
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response()
}

async fn stream_events(Query(q): Query<StreamQuery>) -> Response {
    let store = super::events::get_event_store();
    if !q.follow {
        let limit = q.limit.unwrap_or(200).min(5000);
        let all = store.get_all_events(limit).await;
        let items: Vec<EventV2> = all
            .into_iter()
            .filter(|e| {
                q.kind
                    .as_deref()
                    .map(|k| {
                        let kind = format!("{}", e.event_type);
                        if let Some(prefix) = k.strip_suffix(".*") {
                            kind.starts_with(prefix)
                        } else {
                            kind == k
                        }
                    })
                    .unwrap_or(true)
            })
            .map(|e| EventV2 {
                id: format!("ev_{}", uuid::Uuid::new_v4().simple()),
                kind: format!("{}", e.event_type),
                timestamp: e.timestamp,
                subject: Some(format!("instance/{}", e.vm_name)),
                data: serde_json::to_value(&e.details).ok(),
            })
            .collect();
        return Json(json!({"items": items})).into_response();
    }

    // SSE: subscribe to the broadcast channel.
    let mut rx = store.subscribe();
    let stream = async_stream::stream! {
        let mut seq = 0u64;
        loop {
            match rx.recv().await {
                Ok(e) => {
                    seq += 1;
                    let kind = format!("{}", e.event_type);
                    let v2 = EventV2 {
                        id: format!("ev_{}", uuid::Uuid::new_v4().simple()),
                        kind: kind.clone(),
                        timestamp: e.timestamp,
                        subject: Some(format!("instance/{}", e.vm_name)),
                        data: serde_json::to_value(&e.details).ok(),
                    };
                    let data = serde_json::to_string(&v2).unwrap_or_default();
                    yield Ok::<_, Infallible>(
                        SseEvent::default().id(seq.to_string()).event(&kind).data(data),
                    );
                }
                Err(_) => break,
            }
        }
    };
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response()
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::sync::Mutex;
    use tower::ServiceExt;

    /// Serializes env-var mutation across tests that set
    /// `AIWG_PROVISION_VM_SCRIPT`. Tests acquire this guard for the lifetime
    /// of any code path that depends on the env var being a specific value.
    static PROVISION_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Absolute path to a fixture script under `management/tests/fixtures/`.
    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    fn test_state() -> AppState {
        use crate::dispatch::CommandDispatcher;
        use crate::output::OutputAggregator;
        use crate::registry::AgentRegistry;
        use std::sync::Arc;
        let registry = Arc::new(AgentRegistry::new());
        AppState {
            registry: registry.clone(),
            output_agg: Arc::new(OutputAggregator::new(64)),
            dispatcher: Arc::new(CommandDispatcher::new(registry)),
            orchestrator: None,
            metrics: None,
            operation_store: Some(Arc::new(super::super::operations::OperationStore::new())),
            audit_logger: None,
            credential_broker: Arc::new(crate::credentials::CredentialBroker::new_in_memory()),
            startup_profiles: Arc::new(
                crate::startup_profiles::StartupProfileStore::new_in_memory(),
            ),
            bootstrap_token_store: None,
            grpc_local_ca: None,
            screen_registry: None,
            hitl_store: None,
            aiwg_handle: None,
            mission_store: None,
            session_registry: None,
            agentshare_root: None,
            tasks_root: None,
            operator_auth: None,
            mtls_config: super::super::operator_auth::MtlsConfig::default(),
            unix_peer_creds_config: super::super::operator_auth::UnixPeerCredsConfig::default(),
            executor_instance_registry: None,
            executor_signing_keys_dir: None,
            executor_idempotency: None,
            host_runtime_supervisor: None,
            v1_counter: None,
        }
    }

    fn app() -> Router {
        app_with_state(test_state())
    }

    fn app_with_state(state: AppState) -> Router {
        Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state)
    }

    #[test]
    fn docker_mount_specs_require_host_and_absolute_container_path() {
        let mounts = parse_docker_mount_specs(&[
            "/srv/agent-ops:/workspace".to_string(),
            "/tmp/cache:/cache".to_string(),
        ])
        .expect("valid mounts");
        assert_eq!(
            mounts,
            vec![
                ("/srv/agent-ops".to_string(), "/workspace".to_string()),
                ("/tmp/cache".to_string(), "/cache".to_string())
            ]
        );
        assert!(has_workspace_mount(&mounts));

        let err = parse_docker_mount_specs(&["/srv/agent-ops:workspace".to_string()])
            .expect_err("relative container path should fail");
        assert!(err.contains("container path must be absolute"));

        let err = parse_docker_mount_specs(&["/srv/agent-ops".to_string()])
            .expect_err("missing container path should fail");
        assert!(err.contains("expected host_path:container_path"));
    }

    #[test]
    fn provision_request_accepts_docker_mounts_and_labels() {
        let req: ProvisionRequest = serde_json::from_value(json!({
            "name": "m011-codex-adapter-smoke",
            "runtime": "docker",
            "image": "agentic/codex:latest",
            "agentshare": true,
            "startup_profile_id": "startup_codex",
            "mounts": ["/srv/agent-ops:/workspace"],
            "labels": {
                "mission": "M011",
                "cycle": "009"
            }
        }))
        .expect("request should deserialize");

        assert_eq!(req.mounts, vec!["/srv/agent-ops:/workspace"]);
        assert_eq!(req.labels.get("mission").map(String::as_str), Some("M011"));
        assert_eq!(req.startup_profile_id.as_deref(), Some("startup_codex"));
        assert!(req.agentshare);
    }

    async fn body_bytes(resp: Response) -> Vec<u8> {
        to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn list_instances_returns_array() {
        // Without libvirt in the test environment this will fail
        // gracefully — verify the response is JSON either way.
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/instances")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // We accept either 200 (when libvirt present) or 500 (no libvirt).
        // Either way, response must be valid JSON.
        let status = resp.status();
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).expect("response must be JSON");
        if status == StatusCode::OK {
            assert!(v.get("items").is_some(), "expected items key");
        } else {
            // Error envelope shape
            assert_eq!(v["status"], status.as_u16());
            assert!(v.get("code").is_some());
        }
    }

    #[tokio::test]
    async fn get_instance_404_when_unknown_returns_problem_json() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/instances/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 404 expected (libvirt absent or instance missing both yield not_found path)
        let status = resp.status();
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::INTERNAL_SERVER_ERROR,
            "got {}",
            status
        );
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).expect("problem+json body");
        // RFC 7807 fields
        assert!(v["type"].is_string());
        assert!(v["title"].is_string());
        assert_eq!(v["status"], status.as_u16());
        assert!(v["code"].is_string());
    }

    #[tokio::test]
    async fn provision_instance_returns_202_with_location() {
        // Point at the success fixture so we don't actually try to spawn
        // images/qemu/provision-vm.sh on the host running the tests.
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));
        let body = json!({
            "name": "agent-test-99",
            "runtime": "qemu",
        });
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert!(resp.headers().contains_key("location"));
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["id"].is_string());
        assert_eq!(v["kind"], "instance.provision");
        assert_eq!(v["state"], "pending");
    }

    #[tokio::test]
    async fn provision_instance_rejects_missing_startup_profile() {
        let body = json!({
            "name": "agent-test-97",
            "runtime": "qemu",
            "startup_profile_id": "missing_profile"
        });
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "startup_profile.not_found");
    }

    #[tokio::test]
    async fn provision_instance_accepts_existing_startup_profile() {
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));
        let state = test_state();
        state
            .startup_profiles
            .create(
                serde_json::from_value(json!({
                    "id": "startup_codex",
                    "trigger": "on_instance_ready",
                    "session": {
                        "command": "agentic-codex-automation --profile startup_codex",
                        "workdir": "/home/agent/workspace"
                    },
                    "credential_refs": [
                        {
                            "id": "cred_openai_api",
                            "provider": "codex",
                            "allowed_use": "provider_api",
                            "target": { "type": "env", "name": "OPENAI_API_KEY" }
                        }
                    ]
                }))
                .unwrap(),
            )
            .unwrap();
        let startup_profiles = state.startup_profiles.clone();
        let body = json!({
            "name": "agent-test-96",
            "runtime": "qemu",
            "startup_profile_id": "startup_codex"
        });
        let resp = app_with_state(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["startup_profile_id"], "startup_codex");
        let instance_id = v["instance_id"].as_str().expect("instance_id");
        assert_eq!(
            startup_profiles.bound_profile_id(instance_id).as_deref(),
            Some("startup_codex")
        );
    }

    #[tokio::test]
    async fn provision_instance_validation_error_for_bad_name() {
        let body = json!({
            "name": "BAD NAME WITH SPACES",
            "runtime": "qemu",
        });
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "validation.failed");
    }

    #[tokio::test]
    async fn provision_instance_validation_error_for_bad_runtime() {
        let body = json!({
            "name": "agent-ok",
            "runtime": "vbox",
        });
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn lifecycle_op_returns_202_or_404() {
        // We don't have a real instance — lifecycle should 404 cleanly.
        for op in &["start", "stop", "restart", "destroy"] {
            let resp = app()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/v2/admin/instances/no-such-vm/{}", op))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            // Either 404 (libvirt found but unknown) or 500 (no libvirt).
            assert!(
                resp.status() == StatusCode::NOT_FOUND
                    || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
                "{} returned {}",
                op,
                resp.status()
            );
        }
    }

    #[tokio::test]
    async fn rotate_secret_returns_gone_after_legacy_secret_retirement() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances/abc/rotate-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::GONE);
        let bytes = body_bytes(resp).await;
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body.get("code").and_then(Value::as_str),
            Some("legacy_agent_secret.retired")
        );
    }

    #[tokio::test]
    async fn get_operation_404_when_unknown() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/operations/op-not-real")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "operation.not_found");
    }

    #[tokio::test]
    async fn get_operation_returns_status() {
        // Insert one then look it up.
        let state = test_state();
        let op = Operation::new(OperationType::VmCreate, "test-vm".to_string());
        let op_id = op.id.clone();
        state.operation_store.as_ref().unwrap().insert(op);
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v2/admin/operations/{}", op_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["id"], op_id);
        assert_eq!(v["kind"], "instance.provision");
        assert_eq!(v["state"], "pending");
    }

    #[tokio::test]
    async fn storage_global_write_rejected() {
        let body = json!({"content": "hi", "media_type": "text/plain"});
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v2/admin/storage/global/somefile.txt")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "storage.scope_disabled");
    }

    #[tokio::test]
    async fn storage_global_delete_rejected() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v2/admin/storage/global/somefile.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "storage.scope_disabled");
    }

    #[tokio::test]
    async fn storage_inbox_round_trip() {
        // Configure a temp agentshare root.
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state();
        state.agentshare_root = Some(dir.path().to_string_lossy().to_string());
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        // PUT
        let body = json!({"content": "hello world", "media_type": "text/plain"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v2/admin/storage/inbox/instance-abc/missions/m-001.json")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::CREATED || resp.status() == StatusCode::OK,
            "got {}",
            resp.status()
        );

        // GET
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/storage/inbox/instance-abc/missions/m-001.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["scope"], "inbox");
        assert_eq!(v["content"], "hello world");

        // DELETE
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v2/admin/storage/inbox/instance-abc/missions/m-001.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // GET again → 404
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/storage/inbox/instance-abc/missions/m-001.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn container_images_returns_items() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/container-images")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["items"].is_array());
        let items = v["items"].as_array().unwrap();
        assert!(!items.is_empty());
        assert!(items[0]["name"].is_string());
        assert!(items[0]["reference"].is_string());
    }

    #[tokio::test]
    async fn loadouts_get_returns_items() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/loadouts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["items"].is_array());
    }

    #[tokio::test]
    async fn loadouts_post_validates_yaml() {
        // Bad YAML
        let body = json!({"name": "bad", "manifest": ":\n  - this is\n    bad indentation: also\nbroken: ["});
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/loadouts")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn loadouts_post_accepts_valid_yaml() {
        let body = json!({
            "name": "profiles/test.yaml",
            "manifest": "version: '1'\nruntime: qemu\nprofile: basic\n",
        });
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/loadouts")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["name"], "profiles/test.yaml");
    }

    #[tokio::test]
    async fn logs_snapshot_returns_items_array() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/logs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["items"].is_array());
    }

    #[tokio::test]
    async fn events_snapshot_returns_items_array() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["items"].is_array());
    }

    #[tokio::test]
    async fn error_envelope_has_rfc7807_shape() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/operations/x")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        // RFC 7807 required fields
        assert!(v["type"].is_string(), "type field");
        assert!(v["title"].is_string(), "title field");
        assert_eq!(v["status"], 404);
        assert!(v["code"].is_string(), "code extension");
        assert!(v["instance"].is_string(), "instance extension");
    }

    // ─── #250 Deprecation observability ──────────────────────────────────

    /// Build an app with a real `V1Counter` plumbed into `AppState` so
    /// `/deprecation/v1-counters` returns a 200 instead of the 503 path.
    fn app_with_v1_counter() -> (Router, std::sync::Arc<super::super::compat_v1::V1Counter>) {
        let counter = super::super::compat_v1::V1Counter::new();
        let mut state = test_state();
        state.v1_counter = Some(counter.clone());
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);
        (app, counter)
    }

    #[tokio::test]
    async fn deprecation_counters_503_when_unwired() {
        // test_state() returns v1_counter: None, so the endpoint should
        // surface a 503 with the RFC 7807 envelope.
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/deprecation/v1-counters")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
    }

    #[tokio::test]
    async fn deprecation_counters_returns_snapshot_when_wired() {
        let (app, counter) = app_with_v1_counter();
        // Seed two distinct paths so the response has a non-empty count map.
        counter.inc("/api/v1/agents");
        counter.inc("/api/v1/agents");
        counter.inc("/api/v1/vms");

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/deprecation/v1-counters")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        // Sunset + successor metadata mirror compat_v1 constants.
        assert_eq!(v["sunset_date"], super::super::compat_v1::DEFAULT_SUNSET);
        assert_eq!(v["successor_url"], super::super::compat_v1::DEFAULT_LINK);
        // path_map carries the canonical v1→v2 entries.
        assert_eq!(v["path_map"]["/api/v1/agents"], "/api/v2/admin/instances");
        // Counts reflect what we seeded.
        assert_eq!(v["counts"]["/api/v1/agents"], 2);
        assert_eq!(v["counts"]["/api/v1/vms"], 1);
    }

    #[tokio::test]
    async fn deprecation_counters_path_map_includes_semantic_shifts() {
        // The map should surface the non-1:1 entries (sessions/dispatch,
        // ws/missions, hitl) so the dashboard can show "no v2 equivalent —
        // semantic migration" rows without hard-coding them.
        let (app, _) = app_with_v1_counter();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v2/admin/deprecation/v1-counters")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let pm = &v["path_map"];
        assert!(pm["/api/v1/sessions/{id}/dispatch"].is_string());
        assert!(pm["/api/v1/hitl/{id}"].is_string());
        assert!(pm["/api/v1/ws/missions/{id}"].is_string());
    }

    /// Drive an instance-provision POST against an app backed by the shared
    /// operation store, then poll the operation_id until it reaches a
    /// terminal state or the timeout expires.
    async fn poll_until_terminal(
        store: std::sync::Arc<super::super::operations::OperationStore>,
        op_id: &str,
    ) -> super::super::operations::Operation {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(op) = store.get(op_id) {
                use super::super::operations::OperationState;
                if matches!(
                    op.state,
                    OperationState::Completed | OperationState::Failed { .. }
                ) {
                    return op;
                }
            }
            if std::time::Instant::now() > deadline {
                panic!("operation {} did not reach terminal state in 5s", op_id);
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn provision_instance_real_spawn_succeeds() {
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));

        let state = test_state();
        let store = state.operation_store.as_ref().unwrap().clone();
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let body = json!({
            "name": "agent-spawn-ok",
            "runtime": "qemu",
            "loadout": "profiles/basic.yaml",
            "start": true,
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let op_id = v["id"].as_str().expect("op id").to_string();

        let terminal = poll_until_terminal(store, &op_id).await;
        use super::super::operations::OperationState;
        assert!(
            matches!(terminal.state, OperationState::Completed),
            "expected Completed, got {:?}",
            terminal.state
        );
        let result = terminal.result.expect("result body");
        // #252: instance_id is now the canonical UUIDv7 assigned at
        // provision time; the human-friendly name moved to `name`.
        assert_eq!(result["name"], "agent-spawn-ok");
        let inst_id = result["instance_id"].as_str().expect("instance_id");
        assert!(uuid::Uuid::parse_str(inst_id).is_ok(), "{}", inst_id);
        assert_eq!(result["runtime"], "qemu");
        assert_eq!(result["provisioned"], true);
    }

    #[tokio::test]
    async fn provision_instance_host_runtime_requires_supervisor() {
        let state = test_state();
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let body = json!({
            "name": "agent-host",
            "runtime": "host",
            "loadout": "profiles/basic.yaml",
            "start": true,
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "runtime.not_implemented");
        assert!(v["detail"]
            .as_str()
            .unwrap()
            .contains("durable host supervisor/daemon"));
    }

    struct MockHostSupervisor;

    #[async_trait::async_trait]
    impl crate::host_runtime::HostRuntimeSupervisor for MockHostSupervisor {
        async fn provision(
            &self,
            req: crate::host_runtime::HostProvisionRequest,
        ) -> Result<
            crate::host_runtime::HostProvisionedInstance,
            crate::host_runtime::HostSupervisorError,
        > {
            assert_eq!(req.name, "agent-host-supervised");
            assert_eq!(req.labels.get("role").map(String::as_str), Some("watch"));
            assert!(req.working_dir.as_deref().is_some_and(|path| path.is_dir()));
            Ok(crate::host_runtime::HostProvisionedInstance {
                instance_id: req.instance_id,
                name: req.name,
                supervisor_id: "host-supervisor-local".to_string(),
                host_endpoint: "host.local".to_string(),
                session_backend: crate::host_runtime::HostSessionBackend::Native,
                watch_agents: vec!["watch-a".to_string(), "watch-b".to_string()],
            })
        }

        async fn stop(
            &self,
            instance_id: &str,
        ) -> Result<
            crate::host_runtime::HostLifecycleResult,
            crate::host_runtime::HostSupervisorError,
        > {
            Ok(crate::host_runtime::HostLifecycleResult {
                instance_id: instance_id.to_string(),
                supervisor_id: "host-supervisor-local".to_string(),
                state: crate::host_runtime::HostLifecycleState::Stopped,
                watch_agents: vec!["watch-a".to_string(), "watch-b".to_string()],
            })
        }

        async fn destroy(
            &self,
            instance_id: &str,
        ) -> Result<
            crate::host_runtime::HostLifecycleResult,
            crate::host_runtime::HostSupervisorError,
        > {
            Ok(crate::host_runtime::HostLifecycleResult {
                instance_id: instance_id.to_string(),
                supervisor_id: "host-supervisor-local".to_string(),
                state: crate::host_runtime::HostLifecycleState::Destroyed,
                watch_agents: vec!["watch-a".to_string(), "watch-b".to_string()],
            })
        }
    }

    #[tokio::test]
    async fn provision_instance_host_runtime_uses_configured_supervisor() {
        let (mut state, reg, _tmp) = test_state_with_executor();
        let cwd = tempfile::tempdir().expect("cwd");
        let store = state.operation_store.as_ref().unwrap().clone();
        state.host_runtime_supervisor = Some(std::sync::Arc::new(MockHostSupervisor));
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let body = json!({
            "name": "agent-host-supervised",
            "runtime": "host",
            "loadout": "profiles/basic.yaml",
            "start": true,
            "working_dir": cwd.path(),
            "labels": {"role": "watch"}
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let op_id = v["id"].as_str().expect("op id").to_string();
        let instance_id = v["instance_id"].as_str().expect("instance id").to_string();

        let terminal = poll_until_terminal(store, &op_id).await;
        assert!(matches!(
            terminal.state,
            super::super::operations::OperationState::Completed
        ));
        let result = terminal.result.expect("host supervisor result");
        assert_eq!(result["runtime"], "host");
        assert_eq!(result["supervisor_id"], "host-supervisor-local");
        assert_eq!(result["session_backend"], "native");
        assert_eq!(result["watch_agents"].as_array().unwrap().len(), 2);

        let ctx = reg.get(&instance_id).expect("host InstanceContext");
        assert_eq!(
            ctx.runtime_kind,
            agentic_sandbox_executor::instance::RuntimeKind::Host
        );
        assert_eq!(ctx.loadout, "profiles/basic.yaml");
        assert_eq!(ctx.host, "host.local");
    }

    fn insert_host_context(
        reg: &agentic_sandbox_executor::instance::InstanceRegistry,
        tmp: &tempfile::TempDir,
        instance_id: &str,
    ) -> std::sync::Arc<agentic_sandbox_executor::instance::InstanceContext> {
        let ctx = std::sync::Arc::new(
            agentic_sandbox_executor::instance::InstanceContext::new(
                instance_id,
                agentic_sandbox_executor::instance::RuntimeKind::Host,
                "profiles/basic.yaml",
                None,
                "host.local",
                tmp.path(),
            )
            .expect("host ctx"),
        );
        reg.insert(ctx.clone());
        ctx
    }

    #[tokio::test]
    async fn stop_instance_host_runtime_uses_configured_supervisor() {
        let (mut state, reg, tmp) = test_state_with_executor();
        state.host_runtime_supervisor = Some(std::sync::Arc::new(MockHostSupervisor));
        let ctx = insert_host_context(&reg, &tmp, "inst-host-stop");
        assert!(ctx.is_ready());
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances/inst-host-stop/stop")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert!(
            !ctx.is_ready(),
            "host stop should mark executor context unready"
        );
        let body: Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        assert_eq!(body["result"]["runtime"], "host");
        assert_eq!(body["result"]["state"], "stopped");
        assert_eq!(body["result"]["supervisor_id"], "host-supervisor-local");
        assert_eq!(body["result"]["watch_agents"].as_array().unwrap().len(), 2);
        assert!(
            reg.get("inst-host-stop").is_some(),
            "stop preserves context"
        );
    }

    #[tokio::test]
    async fn destroy_instance_host_runtime_uses_configured_supervisor_and_drains_context() {
        let (mut state, reg, tmp) = test_state_with_executor();
        state.host_runtime_supervisor = Some(std::sync::Arc::new(MockHostSupervisor));
        insert_host_context(&reg, &tmp, "inst-host-destroy");
        assert!(reg.get("inst-host-destroy").is_some());
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances/inst-host-destroy/destroy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let body: Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
        assert_eq!(body["result"]["runtime"], "host");
        assert_eq!(body["result"]["state"], "destroyed");
        assert_eq!(body["result"]["supervisor_id"], "host-supervisor-local");
        assert!(
            reg.get("inst-host-destroy").is_none(),
            "destroy must drain host InstanceContext"
        );
        assert!(
            !tmp.path().join("inst-host-destroy").exists(),
            "destroy must remove signing-key state"
        );
    }

    #[tokio::test]
    async fn provision_instance_issues_bootstrap_token_when_store_configured() {
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));
        std::env::set_var("AGENTIC_BOOTSTRAP_TOKEN_TTL_SECS", "120");

        let mut state = test_state();
        let token_dir = tempfile::tempdir().expect("token dir");
        let token_store = std::sync::Arc::new(
            crate::bootstrap_enrollment::BootstrapTokenStore::load_or_create(token_dir.path())
                .expect("bootstrap store"),
        );
        state.bootstrap_token_store = Some(token_store);
        let store = state.operation_store.as_ref().unwrap().clone();
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let body = json!({
            "name": "agent-bootstrap-ok",
            "runtime": "qemu",
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let op_id = v["id"].as_str().expect("op id").to_string();
        let inst_id = v["instance_id"].as_str().expect("instance_id").to_string();

        let terminal = poll_until_terminal(store, &op_id).await;
        use super::super::operations::OperationState;
        assert!(
            matches!(terminal.state, OperationState::Completed),
            "expected Completed, got {:?}",
            terminal.state
        );
        let result = terminal.result.expect("result body");
        assert_eq!(result["bootstrap_token_issued"], true);
        assert_eq!(
            result["bootstrap_spiffe_id"],
            format!("spiffe://sandbox.agentic.local/agent/{inst_id}")
        );
        assert!(result["bootstrap_token_expires_at_unix_ms"].is_u64());
        let stdout = result["stdout_excerpt"].as_str().unwrap_or_default();
        assert!(stdout.contains("bootstrap_token_env=set"), "{stdout}");
        assert!(
            stdout.contains("bootstrap_token_raw=[REDACTED_BOOTSTRAP_TOKEN]"),
            "{stdout}"
        );
        assert!(
            stdout.contains(&format!(
                "bootstrap_spiffe_id=spiffe://sandbox.agentic.local/agent/{inst_id}"
            )),
            "{stdout}"
        );

        let persisted = std::fs::read_to_string(token_dir.path().join("bootstrap-tokens.json"))
            .expect("persisted token store");
        assert!(
            persisted.contains(&format!("spiffe://sandbox.agentic.local/agent/{inst_id}")),
            "{persisted}"
        );

        // The plaintext token is intentionally not returned by the operation
        // result or echoed by the provision fixture.
        assert!(!result.to_string().contains("AGENT_BOOTSTRAP_TOKEN="));
        assert!(!persisted.contains("bootstrap_token_env=set"));

        std::env::remove_var("AGENTIC_BOOTSTRAP_TOKEN_TTL_SECS");
    }

    #[tokio::test]
    async fn provision_instance_real_spawn_failure() {
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "AIWG_PROVISION_VM_SCRIPT",
            fixture("fake-provision-vm-fail.sh"),
        );

        let state = test_state();
        let store = state.operation_store.as_ref().unwrap().clone();
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let body = json!({
            "name": "agent-spawn-fail",
            "runtime": "qemu",
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let op_id = v["id"].as_str().expect("op id").to_string();

        let terminal = poll_until_terminal(store, &op_id).await;
        use super::super::operations::OperationState;
        match terminal.state {
            OperationState::Failed { error } => {
                assert!(
                    error.contains("provision-vm.sh exited"),
                    "missing exit-code prefix: {}",
                    error
                );
                assert!(
                    error.contains("simulated provision failure"),
                    "stderr not captured in error: {}",
                    error
                );
                assert!(error.len() <= 4096 + 64, "stderr should be truncated");
            }
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn provision_instance_idempotent_under_pending() {
        // Acquire the env lock for the duration of the test; both POSTs run
        // against the same fake script so the first task is still in flight
        // when the second arrives (sleep 0.05s gives us enough window).
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));

        let state = test_state();
        let store = state.operation_store.as_ref().unwrap().clone();
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let body = json!({
            "name": "agent-idempo",
            "runtime": "qemu",
        });

        let resp1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::ACCEPTED);
        let bytes1 = body_bytes(resp1).await;
        let v1: Value = serde_json::from_slice(&bytes1).unwrap();
        let op_id_1 = v1["id"].as_str().expect("op id 1").to_string();

        // Immediately fire the second request; the spawned task is still
        // running because the fake script sleeps 0.05s before exiting.
        let resp2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::ACCEPTED);
        let bytes2 = body_bytes(resp2).await;
        let v2: Value = serde_json::from_slice(&bytes2).unwrap();
        let op_id_2 = v2["id"].as_str().expect("op id 2").to_string();

        assert_eq!(
            op_id_1, op_id_2,
            "second POST should return the same operation_id while first is pending/running"
        );

        // Drain the in-flight task so it doesn't leak into other tests.
        let _ = poll_until_terminal(store, &op_id_1).await;
    }

    #[tokio::test]
    async fn provision_instance_idempotent_duplicate_does_not_issue_second_bootstrap_token() {
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));

        let mut state = test_state();
        let token_dir = tempfile::tempdir().expect("token dir");
        state.bootstrap_token_store = Some(std::sync::Arc::new(
            crate::bootstrap_enrollment::BootstrapTokenStore::load_or_create(token_dir.path())
                .expect("bootstrap store"),
        ));
        let store = state.operation_store.as_ref().unwrap().clone();
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let body = json!({
            "name": "agent-bootstrap-idempo",
            "runtime": "qemu",
        });

        let resp1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::ACCEPTED);
        let v1: Value = serde_json::from_slice(&body_bytes(resp1).await).unwrap();
        let op_id = v1["id"].as_str().expect("op id").to_string();

        let resp2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::ACCEPTED);
        let v2: Value = serde_json::from_slice(&body_bytes(resp2).await).unwrap();
        assert_eq!(v2["id"], v1["id"]);

        let _ = poll_until_terminal(store, &op_id).await;

        let persisted = std::fs::read_to_string(token_dir.path().join("bootstrap-tokens.json"))
            .expect("persisted token store");
        let records = persisted.matches("\"token_hash\"").count();
        assert_eq!(records, 1, "{persisted}");
    }

    // ─── #252 wire-up tests ───────────────────────────────────────────────

    /// Helper: build AppState with an attached (empty) executor instance
    /// registry + temp signing-keys dir, returning both so tests can
    /// inspect the registry directly.
    fn test_state_with_executor() -> (
        AppState,
        agentic_sandbox_executor::instance::InstanceRegistry,
        tempfile::TempDir,
    ) {
        let mut state = test_state();
        let reg = agentic_sandbox_executor::instance::InstanceRegistry::new();
        let tmp = tempfile::tempdir().expect("tempdir");
        state.executor_instance_registry = Some(reg.clone());
        state.executor_signing_keys_dir = Some(tmp.path().to_path_buf());
        (state, reg, tmp)
    }

    #[tokio::test]
    async fn provision_instance_generates_instance_id_upfront() {
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));

        let body = json!({
            "name": "agent-upfront",
            "runtime": "qemu",
        });
        let resp = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let inst = v["instance_id"].as_str().expect("instance_id in response");
        let parsed = uuid::Uuid::parse_str(inst).expect("valid uuid");
        // UUIDv7 has version 7 nibble in the high half of timestamp_lo.
        assert_eq!(parsed.get_version_num(), 7, "expected UUIDv7, got {}", inst);
    }

    #[tokio::test]
    async fn provision_instance_populates_executor_registry() {
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));

        let (state, reg, _tmp) = test_state_with_executor();
        let store = state.operation_store.as_ref().unwrap().clone();
        let app = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);

        let body = json!({
            "name": "agent-populates",
            "runtime": "qemu",
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let op_id = v["id"].as_str().expect("op id").to_string();
        let inst_id = v["instance_id"].as_str().expect("instance_id").to_string();

        // Wait for the spawned worker to finish.
        let _ = poll_until_terminal(store, &op_id).await;

        // The InstanceRegistry must now contain the assigned instance_id.
        assert!(
            reg.get(&inst_id).is_some(),
            "InstanceRegistry should contain {inst_id} after successful provision; ids={:?}",
            reg.list_ids()
        );
    }

    #[tokio::test]
    async fn provision_then_404_on_unknown_instance() {
        let _g = PROVISION_ENV_LOCK.lock().unwrap();
        std::env::set_var("AIWG_PROVISION_VM_SCRIPT", fixture("fake-provision-vm.sh"));

        let (state, reg, _tmp) = test_state_with_executor();
        let store = state.operation_store.as_ref().unwrap().clone();
        // Build an executor router that uses the SAME instance registry.
        let task_store = std::sync::Arc::new(
            agentic_sandbox_executor::store::task_store::TaskStore::open_in_memory()
                .expect("in-memory task store"),
        );
        let idem = std::sync::Arc::new(
            agentic_sandbox_executor::store::idempotency::IdempotencyCache::new(task_store.clone()),
        );
        let exec_router =
            agentic_sandbox_executor::bindings::rest::router(reg.clone(), task_store, idem);
        let admin_router = Router::new()
            .nest("/api/v2/admin", super::router())
            .with_state(state);
        let app = admin_router.merge(exec_router);

        let body = json!({ "name": "agent-route-a", "runtime": "qemu" });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/admin/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = body_bytes(resp).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        let op_id = v["id"].as_str().unwrap().to_string();
        let _ = poll_until_terminal(store, &op_id).await;

        // Query a DIFFERENT instance_id — must 404. The InstanceLayer
        // middleware returns problem+json with `type=instance.not_found`,
        // but in this test harness we only assert the status code so the
        // test is resilient to merge ordering between the admin and
        // executor routers (axum's `Router::merge` doesn't guarantee that
        // layered middleware fires on unknown sub-paths the same way as
        // in the production binary). The status-code contract is the
        // load-bearing piece per the issue body.
        let bogus = uuid::Uuid::now_v7().to_string();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/agents/{}/.well-known/agent-card.json", bogus))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn destroy_helper_removes_from_executor_registry() {
        // Exercise `remove_instance_from_executor` directly. The
        // destroy_instance HTTP route requires libvirt, which isn't
        // available in unit tests, so we test the helper in isolation.
        let (state, reg, tmp) = test_state_with_executor();
        let inst_id = "inst-destroy-target";
        let ctx = std::sync::Arc::new(
            agentic_sandbox_executor::instance::InstanceContext::new(
                inst_id,
                agentic_sandbox_executor::instance::RuntimeKind::Vm,
                "agentic-dev",
                None,
                "executor.local",
                tmp.path(),
            )
            .expect("ctx"),
        );
        reg.insert(ctx);
        assert!(reg.get(inst_id).is_some());
        // signing key dir created by InstanceContext::new.
        let key_dir = tmp.path().join(inst_id);
        assert!(
            key_dir.exists(),
            "signing-key dir should exist after construct"
        );

        super::remove_instance_from_executor(&state, inst_id);

        assert!(reg.get(inst_id).is_none(), "registry should drain");
        assert!(
            !key_dir.exists(),
            "signing-key dir should be deleted: {}",
            key_dir.display()
        );
    }
}
