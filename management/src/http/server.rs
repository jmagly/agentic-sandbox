//! HTTP server implementation using axum
//!
//! Serves the web dashboard UI and REST API endpoints.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

use super::events;
use super::health;
use super::hitl;
use super::loadout_registry;
use super::loadouts;
use super::operations::{get_operation, OperationStore};
use super::orchestrate;
use super::tasks;
use super::vms;
use super::{create_vm, delete_vm, deploy_agent, restart_vm};
use crate::auth::SecretStore;
use crate::dispatch::CommandDispatcher;
use crate::hitl::HitlStore;
use crate::orchestrator::Orchestrator;
use crate::output::OutputAggregator;
use crate::registry::AgentRegistry;
use crate::screen_state::ScreenRegistry;
use crate::telemetry::Metrics;

/// Embedded static files for the web UI
#[derive(RustEmbed)]
#[folder = "ui/"]
struct Assets;

/// Shared state for HTTP handlers
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<AgentRegistry>,
    pub output_agg: Arc<OutputAggregator>,
    pub dispatcher: Arc<CommandDispatcher>,
    pub orchestrator: Option<Arc<Orchestrator>>,
    pub metrics: Option<Arc<Metrics>>,
    pub operation_store: Option<Arc<OperationStore>>,
    pub secret_store: Option<Arc<SecretStore>>,
    pub screen_registry: Option<Arc<ScreenRegistry>>,
    pub hitl_store: Option<Arc<HitlStore>>,
}

/// HTTP server for the web dashboard
pub struct HttpServer {
    listen_addr: SocketAddr,
    state: AppState,
}

impl HttpServer {
    pub fn new(
        listen_addr: SocketAddr,
        registry: Arc<AgentRegistry>,
        output_agg: Arc<OutputAggregator>,
        dispatcher: Arc<CommandDispatcher>,
    ) -> Self {
        Self {
            listen_addr,
            state: AppState {
                registry,
                output_agg,
                dispatcher,
                orchestrator: None,
                metrics: None,
                operation_store: Some(Arc::new(OperationStore::new())),
                secret_store: None,
                screen_registry: None,
                hitl_store: None,
            },
        }
    }

    /// Set the orchestrator for task management
    pub fn with_orchestrator(mut self, orchestrator: Arc<Orchestrator>) -> Self {
        self.state.orchestrator = Some(orchestrator);
        self
    }

    /// Set the metrics instance for /metrics endpoint
    pub fn with_metrics(mut self, metrics: Option<Arc<Metrics>>) -> Self {
        self.state.metrics = metrics;
        self
    }

    /// Set the secret store for agent authentication
    pub fn with_secrets(mut self, secrets: Arc<SecretStore>) -> Self {
        self.state.secret_store = Some(secrets);
        self
    }

    /// Set the screen registry for the orchestrator WS endpoint
    pub fn with_screen_registry(mut self, registry: Arc<ScreenRegistry>) -> Self {
        self.state.screen_registry = Some(registry);
        self
    }

    /// Set the HITL store for human-in-the-loop endpoints
    pub fn with_hitl_store(mut self, store: Arc<HitlStore>) -> Self {
        self.state.hitl_store = Some(store);
        self
    }

    /// Run the HTTP server
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let app = Router::new()
            // API endpoints
            // Health check endpoints (new standardized endpoints)
            .route("/healthz", get(health::liveness))
            .route("/readyz", get(health::readiness))
            .route("/healthz/deep", get(health::health_detailed))
            // Legacy health endpoints (kept for backwards compatibility)
            .route("/api/health", get(health_handler))
            .route("/api/v1/health", get(health_handler_v1))
            .route("/api/v1/health/ready", get(readiness_handler))
            .route("/api/v1/health/live", get(liveness_handler))
            .route("/api/v1/agents", get(agents_handler))
            .route("/api/v1/agents/{id}", get(agent_detail_handler))
            .route("/api/v1/agents/{id}/start", post(agent_start_handler))
            .route("/api/v1/agents/{id}/stop", post(agent_stop_handler))
            .route("/api/v1/agents/{id}/destroy", post(agent_destroy_handler))
            .route(
                "/api/v1/agents/{id}/reprovision",
                post(agent_reprovision_handler),
            )
            .route("/api/v1/agents/{id}", delete(agent_delete_handler))
            // HITL (Human-in-the-Loop) endpoints
            .route("/api/v1/agents/{id}/hitl", post(hitl::hitl_create))
            .route("/api/v1/hitl", get(hitl::hitl_list))
            .route("/api/v1/hitl/{id}/respond", post(hitl::hitl_respond))
            // VM lifecycle events
            .route(
                "/api/v1/events",
                post(events::receive_event).get(events::list_events),
            )
            // Loadout profiles and registry
            .route("/api/v1/loadouts", get(loadouts::list_loadouts))
            .route("/api/v1/loadouts/{name}", get(loadouts::get_loadout))
            .route("/api/v1/loadout/registry", get(loadout_registry::get_registry))
            // VM control endpoints
            .route("/api/v1/vms", get(vms::list_vms).post(create_vm))
            .route("/api/v1/vms/{name}", get(vms::get_vm).delete(delete_vm))
            .route("/api/v1/vms/{name}/start", post(vms::start_vm))
            .route("/api/v1/vms/{name}/stop", post(vms::stop_vm))
            .route("/api/v1/vms/{name}/destroy", post(vms::destroy_vm))
            .route("/api/v1/vms/{name}/restart", post(restart_vm))
            .route("/api/v1/vms/{name}/deploy-agent", post(deploy_agent))
            // PTY screen observer — orchestrator WS + REST snapshot
            .route(
                "/ws/sessions/{id}/orchestrate",
                get(orchestrate::orchestrate_ws),
            )
            .route(
                "/api/v1/sessions/{id}/screen",
                get(orchestrate::get_screen_snapshot),
            )
            // Operations tracking
            .route("/api/v1/operations/{id}", get(get_operation))
            // Prometheus metrics endpoint
            .route("/metrics", get(metrics_handler))
            // Task orchestration endpoints
            .route(
                "/api/v1/tasks",
                post(tasks::submit_task).get(tasks::list_tasks),
            )
            .route(
                "/api/v1/tasks/{id}",
                get(tasks::get_task).delete(tasks::cancel_task),
            )
            .route("/api/v1/tasks/{id}/logs", get(tasks::stream_task_logs))
            .route("/api/v1/tasks/{id}/artifacts", get(tasks::list_artifacts))
            .route(
                "/api/v1/tasks/{id}/artifacts/{name}",
                get(tasks::download_artifact),
            )
            // Static files (dashboard UI)
            .fallback(static_handler)
            .with_state(self.state);

        let listener = TcpListener::bind(self.listen_addr).await?;
        info!("HTTP dashboard available at http://{}", self.listen_addr);

        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Simple health check endpoint (legacy)
async fn health_handler() -> impl IntoResponse {
    Json(HealthResponseSimple {
        status: "ok".to_string(),
        service: "agentic-management".to_string(),
    })
}

#[derive(Serialize)]
struct HealthResponseSimple {
    status: String,
    service: String,
}

/// Enhanced health check endpoint with metrics
async fn health_handler_v1(State(state): State<AppState>) -> impl IntoResponse {
    let agents = state.registry.list_agents();
    let connected = agents.len() as u64;
    let ready = agents
        .iter()
        .filter(|a| matches!(a.status, crate::proto::AgentStatus::Ready))
        .count() as u64;

    // Get task counts from orchestrator if available
    let (tasks_running, tasks_pending) = if let Some(ref orchestrator) = state.orchestrator {
        let tasks = orchestrator.list_tasks(None).await;
        let running = tasks
            .iter()
            .filter(|t| matches!(t.state, crate::orchestrator::TaskState::Running))
            .count() as u64;
        let pending = tasks
            .iter()
            .filter(|t| matches!(t.state, crate::orchestrator::TaskState::Pending))
            .count() as u64;
        (running, pending)
    } else {
        (0, 0)
    };

    let uptime_seconds = state
        .metrics
        .as_ref()
        .map(|m| m.uptime_seconds())
        .unwrap_or(0);

    Json(HealthResponseV1 {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds,
        agents: AgentCounts { connected, ready },
        tasks: TaskCounts {
            running: tasks_running,
            pending: tasks_pending,
        },
    })
}

#[derive(Serialize)]
struct HealthResponseV1 {
    status: String,
    version: String,
    uptime_seconds: u64,
    agents: AgentCounts,
    tasks: TaskCounts,
}

#[derive(Serialize)]
struct AgentCounts {
    connected: u64,
    ready: u64,
}

#[derive(Serialize)]
struct TaskCounts {
    running: u64,
    pending: u64,
}

/// Kubernetes readiness probe
/// Returns 200 if the server is ready to accept traffic
async fn readiness_handler(State(state): State<AppState>) -> impl IntoResponse {
    // Ready if we have at least one agent connected or if we're just starting up
    let agents = state.registry.list_agents();
    if agents.is_empty() {
        // Still ready even with no agents - the service is operational
        (
            StatusCode::OK,
            Json(ReadinessResponse {
                ready: true,
                reason: "no_agents_but_operational".to_string(),
            }),
        )
    } else {
        (
            StatusCode::OK,
            Json(ReadinessResponse {
                ready: true,
                reason: "agents_connected".to_string(),
            }),
        )
    }
}

#[derive(Serialize)]
struct ReadinessResponse {
    ready: bool,
    reason: String,
}

/// Kubernetes liveness probe
/// Returns 200 if the server is alive
async fn liveness_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(LivenessResponse { alive: true }))
}

#[derive(Serialize)]
struct LivenessResponse {
    alive: bool,
}

/// Prometheus metrics endpoint
async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    match state.metrics {
        Some(ref metrics) => {
            // Update agent status metrics before export
            let agents = state.registry.list_agents();
            let ready = agents
                .iter()
                .filter(|a| matches!(a.status, crate::proto::AgentStatus::Ready))
                .count() as u64;
            let busy = agents
                .iter()
                .filter(|a| matches!(a.status, crate::proto::AgentStatus::Busy))
                .count() as u64;
            metrics.set_agent_status(ready, busy);

            Response::builder()
                .status(StatusCode::OK)
                .header(
                    header::CONTENT_TYPE,
                    "text/plain; version=0.0.4; charset=utf-8",
                )
                .body(Body::from(metrics.prometheus_format()))
                .unwrap()
        }
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from("Metrics not enabled"))
            .unwrap(),
    }
}

/// List connected agents
async fn agents_handler(State(state): State<AppState>) -> impl IntoResponse {
    let agents: Vec<AgentInfo> = state
        .registry
        .list_agents()
        .into_iter()
        .map(|a| {
            let metrics = a.metrics.map(|m| MetricsInfo {
                cpu_percent: m.cpu_percent,
                memory_used_bytes: m.memory_used_bytes,
                memory_total_bytes: m.memory_total_bytes,
                disk_used_bytes: m.disk_used_bytes,
                disk_total_bytes: m.disk_total_bytes,
                load_avg: m.load_avg,
                uptime_seconds: m.uptime_seconds,
            });
            let system_info = a.system_info.map(|s| SystemInfoApi {
                os: s.os,
                kernel: s.kernel,
                cpu_cores: s.cpu_cores,
                memory_bytes: s.memory_bytes,
                disk_bytes: s.disk_bytes,
            });
            let aiwg_frameworks = a
                .aiwg_frameworks
                .into_iter()
                .map(|fw| AiwgFrameworkApi {
                    name: fw.name,
                    providers: fw.providers,
                })
                .collect();
            AgentInfo {
                id: a.id,
                hostname: a.hostname,
                ip_address: a.ip_address,
                profile: a.profile,
                loadout: a.loadout,
                status: format!("{:?}", a.status),
                setup_status: a.setup_status,
                setup_progress_json: a.setup_progress_json,
                connected_at: a.connected_at,
                last_heartbeat: a.last_heartbeat,
                metrics,
                system_info,
                aiwg_frameworks,
            }
        })
        .collect();

    Json(AgentsResponse { agents })
}

#[derive(Serialize)]
struct AgentsResponse {
    agents: Vec<AgentInfo>,
}

#[derive(Serialize)]
pub struct AiwgFrameworkApi {
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
}

#[derive(Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub hostname: String,
    pub ip_address: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub profile: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub loadout: String,
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub setup_status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub setup_progress_json: String,
    pub connected_at: i64,
    pub last_heartbeat: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<MetricsInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_info: Option<SystemInfoApi>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aiwg_frameworks: Vec<AiwgFrameworkApi>,
}

#[derive(Serialize)]
pub struct MetricsInfo {
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub load_avg: Vec<f32>,
    pub uptime_seconds: u64,
}

#[derive(Serialize)]
pub struct SystemInfoApi {
    pub os: String,
    pub kernel: String,
    pub cpu_cores: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
}

/// GET /api/v1/agents/:id - Get single agent details
async fn agent_detail_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.registry.get(&id) {
        Some(agent) => {
            let metrics = agent.metrics.as_ref().map(|m| MetricsInfo {
                cpu_percent: m.cpu_percent,
                memory_used_bytes: m.memory_used_bytes,
                memory_total_bytes: m.memory_total_bytes,
                disk_used_bytes: m.disk_used_bytes,
                disk_total_bytes: m.disk_total_bytes,
                load_avg: m.load_avg.clone(),
                uptime_seconds: m.uptime_seconds,
            });
            let system_info = agent.system_info.as_ref().map(|s| SystemInfoApi {
                os: s.os.clone(),
                kernel: s.kernel.clone(),
                cpu_cores: s.cpu_cores,
                memory_bytes: s.memory_bytes,
                disk_bytes: s.disk_bytes,
            });
            let aiwg_frameworks = agent
                .aiwg_frameworks
                .iter()
                .map(|fw| AiwgFrameworkApi {
                    name: fw.name.clone(),
                    providers: fw.providers.clone(),
                })
                .collect();
            let info = AgentInfo {
                id: agent.agent_id.clone(),
                hostname: agent.registration.hostname.clone(),
                ip_address: agent.registration.ip_address.clone(),
                profile: agent.registration.profile.clone(),
                loadout: agent.registration.loadout.clone(),
                status: format!("{:?}", agent.status),
                setup_status: agent.setup_status.clone(),
                setup_progress_json: agent.setup_progress_json.clone(),
                connected_at: agent.connected_at.timestamp_millis(),
                last_heartbeat: agent.last_heartbeat.timestamp_millis(),
                metrics,
                system_info,
                aiwg_frameworks,
            };
            (StatusCode::OK, Json(serde_json::to_value(info).unwrap())).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("Agent '{}' not found", id)})),
        )
            .into_response(),
    }
}

/// POST /api/v1/agents/:id/start — delegate to VM start
async fn agent_start_handler(Path(id): Path<String>) -> impl IntoResponse {
    vms::start_vm(axum::extract::Path(id)).await.into_response()
}

/// POST /api/v1/agents/:id/stop — delegate to VM stop
async fn agent_stop_handler(Path(id): Path<String>) -> impl IntoResponse {
    vms::stop_vm(axum::extract::Path(id)).await.into_response()
}

/// POST /api/v1/agents/:id/destroy — delegate to VM destroy
async fn agent_destroy_handler(Path(id): Path<String>) -> impl IntoResponse {
    vms::destroy_vm(axum::extract::Path(id))
        .await
        .into_response()
}

/// POST /api/v1/agents/:id/reprovision — run reprovision-vm.sh
async fn agent_reprovision_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    use tokio::process::Command;

    // Find reprovision script
    let script_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("scripts/reprovision-vm.sh");

    if !script_path.exists() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "reprovision-vm.sh not found"})),
        )
            .into_response();
    }

    let store = match state.operation_store.as_ref() {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Operation store unavailable"})),
            )
                .into_response()
        }
    };

    use super::operations::{Operation, OperationType};
    let operation = Operation::new(OperationType::VmCreate, id.clone());
    let op_id = store.insert(operation.clone());
    let op_id_clone = op_id.clone();
    let vm_name = id.clone();
    tokio::spawn(async move {
        let op_id = op_id_clone;
        let output = Command::new("bash")
            .arg(&script_path)
            .arg(&vm_name)
            .output()
            .await;
        match output {
            Ok(o) if o.status.success() => store.mark_completed(
                &op_id,
                Some(serde_json::json!({"vm": {"name": vm_name, "reprovisioned": true}})),
            ),
            Ok(o) => store.mark_failed(
                &op_id,
                format!("reprovision failed: {}", String::from_utf8_lossy(&o.stderr)),
            ),
            Err(e) => store.mark_failed(&op_id, format!("failed to run script: {}", e)),
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"operation_id": op_id, "status": "accepted"})),
    )
        .into_response()
}

/// DELETE /api/v1/agents/:id — destroy VM + undefine + clean up
/// Always forces destroy if running; disk deletion can be requested via ?delete_disk=true
async fn agent_delete_handler(
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let delete_disk = params
        .get("delete_disk")
        .map(|v| v == "true")
        .unwrap_or(false);
    let force = params.get("force").map(|v| v == "true").unwrap_or(true);
    use super::events;
    use super::vms::{connect_libvirt, get_domain, get_domain_state, VmError, VmState};

    let conn = match connect_libvirt() {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let domain = match get_domain(&conn, &id) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let state = match get_domain_state(&domain) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    if state == VmState::Running && !force {
        return VmError::CannotDeleteRunning(id).into_response();
    }

    if state == VmState::Running {
        if let Err(e) = domain.destroy() {
            return VmError::LibvirtError(format!("Failed to destroy VM: {}", e)).into_response();
        }
    }

    // Extract disk path before undefine if needed
    let disk_path = if delete_disk {
        domain.get_xml_desc(0).ok().and_then(|xml| {
            let re = regex::Regex::new(r"<source file='([^']+\.qcow2)'").ok()?;
            re.captures(&xml)?.get(1).map(|m| m.as_str().to_string())
        })
    } else {
        None
    };

    if let Err(e) = domain.undefine() {
        return VmError::LibvirtError(format!("Failed to undefine VM: {}", e)).into_response();
    }

    events::add_libvirt_event(
        "vm.undefined",
        id.clone(),
        chrono::Utc::now(),
        Some("api".to_string()),
        None,
    )
    .await;

    let mut disk_deleted = false;
    if let Some(path) = disk_path {
        if std::path::Path::new(&path).exists() {
            if std::fs::remove_file(&path).is_ok() {
                disk_deleted = true;
            }
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "deleted": true,
            "name": id,
            "disk_deleted": disk_deleted,
        })),
    )
        .into_response()
}

/// Serve static files from embedded assets
async fn static_handler(uri: Uri) -> Response<Body> {
    let path = uri.path().trim_start_matches('/');

    // API paths should never fall through to the dashboard
    if path.starts_with("api/") {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"error":"not found"}"#))
            .unwrap();
    }

    // Default to index.html for root
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let body = Body::from(content.data.to_vec());

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(body)
                .unwrap()
        }
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Not Found"))
            .unwrap(),
    }
}
