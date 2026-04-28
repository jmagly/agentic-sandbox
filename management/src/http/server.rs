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
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::timeout::TimeoutLayer;
use tracing::info;

/// Max duration a single HTTP handler may run before the layer returns 408.
/// Keeps one slow handler from wedging the HTTP task forever — if libvirt
/// or another blocking dep stalls longer than this, the request fails fast
/// and the watchdog (see `main.rs`) catches process-level stalls.
const HTTP_HANDLER_TIMEOUT: Duration = Duration::from_secs(30);

use super::aiwg_proxy;
use super::events;
use super::health;
use super::hitl;
use super::loadout_registry;
use super::loadouts;
use super::operations::{get_operation, OperationStore};
use super::orchestrate;
use super::sessions;
use super::storage;
use super::tasks;
use super::vms;
use super::{create_vm, delete_vm, deploy_agent, restart_vm};
use crate::auth::SecretStore;
use crate::aiwg_serve::AiwgServeHandle;
use crate::dispatch::CommandDispatcher;
use crate::hitl::HitlStore;
use crate::orchestrator::Orchestrator;
use crate::output::OutputAggregator;
use crate::registry::AgentRegistry;
use crate::screen_state::ScreenRegistry;
use crate::session::SessionRegistry;
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
    pub aiwg_handle: Option<AiwgServeHandle>,
    pub session_registry: Option<Arc<SessionRegistry>>,
    /// Filesystem root for agentshare (`global-ro/` and `<agent>-inbox/`).
    /// Required by `/api/v1/storage/{global,inbox}` handlers; absent ⇒ 503.
    pub agentshare_root: Option<String>,
    /// Filesystem root for task directories (`<task-id>/outbox/`).
    /// Required by `/api/v1/storage/outbox` handlers; absent ⇒ 503.
    pub tasks_root: Option<String>,
    /// Operator (HTTP/WS) auth. `None` ⇒ auth disabled (back-compat).
    pub operator_auth: Option<Arc<super::operator_auth::OperatorAuthConfig>>,
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
                aiwg_handle: None,
                session_registry: None,
                agentshare_root: None,
                tasks_root: None,
                operator_auth: None,
            },
        }
    }

    /// Configure agentshare and tasks roots so the storage REST endpoints
    /// can serve `/api/v1/storage/*`. When unset those routes return 503.
    pub fn with_storage_roots(mut self, agentshare_root: String, tasks_root: String) -> Self {
        self.state.agentshare_root = Some(agentshare_root);
        self.state.tasks_root = Some(tasks_root);
        self
    }

    /// Enable operator auth for HTTP/WS. `None` keeps the surface open
    /// (back-compat default). When `Some`, requests must present a
    /// matching bearer token; destructive verbs additionally require
    /// the `admin` role.
    pub fn with_operator_auth(
        mut self,
        cfg: Option<Arc<super::operator_auth::OperatorAuthConfig>>,
    ) -> Self {
        self.state.operator_auth = cfg;
        self
    }

    pub fn with_session_registry(mut self, registry: Arc<SessionRegistry>) -> Self {
        self.state.session_registry = Some(registry);
        self
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

    /// Attach the aiwg serve handle for status and reconnect endpoints
    pub fn with_aiwg_handle(mut self, handle: AiwgServeHandle) -> Self {
        self.state.aiwg_handle = Some(handle);
        self
    }

    /// Run the HTTP server
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let app = Router::new()
            // API endpoints
            // Health check endpoints (new standardized endpoints)
            .route("/healthz", get(health::liveness))
            .route("/healthz/http", get(health::http_only))
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
            .route(
                "/api/v1/agents/{id}/rotate-secret",
                post(agent_rotate_secret_handler),
            )
            .route("/api/v1/agents/{id}", delete(agent_delete_handler))
            // HITL (Human-in-the-Loop) endpoints
            .route("/api/v1/agents/{id}/hitl", post(hitl::hitl_create))
            .route("/api/v1/aiwg/status", get(aiwg_status_handler))
            .route("/api/v1/aiwg/reconnect", post(aiwg_reconnect_handler))
            .route(
                "/api/v1/agents/{id}/sessions",
                get(sessions::list_sessions).post(sessions::create_session),
            )
            .route(
                "/api/v1/agents/{id}/sessions/{session}",
                delete(sessions::delete_session),
            )
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
            // Formal session registry endpoints
            .route("/api/v1/sessions", get(session_list_handler))
            .route("/api/v1/sessions/{id}", delete(session_delete_handler))
            .route("/api/v1/sessions/{id}/stream", get(session_stream_handler))
            // Agentshare REST surface (admin-only — gating enforced by
            // future operator-auth middleware; today this surface is open
            // on the same listener as the rest of the API).
            .route(
                "/api/v1/storage/global",
                get(storage::list_global).post(storage::upload_global),
            )
            .route(
                "/api/v1/storage/global/_download",
                get(storage::download_global),
            )
            .route(
                "/api/v1/storage/inbox/{agent_id}",
                get(storage::list_inbox).post(storage::upload_inbox),
            )
            .route(
                "/api/v1/storage/inbox/{agent_id}/_download",
                get(storage::download_inbox),
            )
            .route(
                "/api/v1/storage/outbox/{task_id}",
                get(storage::list_outbox),
            )
            .route(
                "/api/v1/storage/outbox/{task_id}/_download",
                get(storage::download_outbox),
            )
            // AIWG companion endpoints (manifest CRUD + exec proxy)
            .route(
                "/api/v1/agents/{id}/manifests/{platform}",
                get(aiwg_proxy::list_manifests),
            )
            .route(
                "/api/v1/agents/{id}/manifests/{platform}/{name}",
                get(aiwg_proxy::get_manifest).post(aiwg_proxy::push_manifest),
            )
            .route("/api/v1/agents/{id}/aiwg/exec", post(aiwg_proxy::aiwg_exec))
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
            // Per-request timeout so one slow handler can't wedge the HTTP
            // task forever. `TimeoutLayer` times out the response future only,
            // so SSE/WebSocket upgrades (which produce Response headers
            // immediately and then stream) are unaffected.
            .layer(TimeoutLayer::new(HTTP_HANDLER_TIMEOUT))
            // Operator auth — bearer-token middleware that resolves the
            // caller's role into request extensions. Passes through when
            // operator-tokens.toml is absent (back-compat).
            .layer(axum::middleware::from_fn_with_state(
                self.state.clone(),
                super::operator_auth::auth_middleware,
            ))
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

            // Append operator-auth gauges/counters if auth is configured.
            // Kept inline so observability isn't gated on the broader
            // metrics module growing a dependency on http/operator_auth.
            let mut body = metrics.prometheus_format();
            if let Some(auth) = &state.operator_auth {
                body.push_str(
                    "# HELP agentic_operator_tokens_active Number of currently-active operator bearer tokens\n",
                );
                body.push_str("# TYPE agentic_operator_tokens_active gauge\n");
                body.push_str(&format!(
                    "agentic_operator_tokens_active {}\n",
                    auth.active_count()
                ));
                body.push_str(
                    "# HELP agentic_operator_tokens_reloads_total Total successful SIGHUP reloads of operator-tokens.toml\n",
                );
                body.push_str("# TYPE agentic_operator_tokens_reloads_total counter\n");
                body.push_str(&format!(
                    "agentic_operator_tokens_reloads_total {}\n",
                    auth.reload_count()
                ));
            }

            Response::builder()
                .status(StatusCode::OK)
                .header(
                    header::CONTENT_TYPE,
                    "text/plain; version=0.0.4; charset=utf-8",
                )
                .body(Body::from(body))
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
                instance_id: a.instance_id,
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
    /// Stable per-agent UUIDv7 — persists across gRPC reconnects (#917).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instance_id: String,
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
                instance_id: agent.instance_id.clone(),
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
async fn agent_destroy_handler(
    admin: super::operator_auth::RequireAdmin,
    Path(id): Path<String>,
) -> impl IntoResponse {
    vms::destroy_vm(admin, axum::extract::Path(id))
        .await
        .into_response()
}

/// POST /api/v1/agents/:id/reprovision — run reprovision-vm.sh
async fn agent_reprovision_handler(
    _: super::operator_auth::RequireAdmin,
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

/// POST /api/v1/agents/:id/rotate-secret — rotate the per-agent shared
/// secret. Generates a new 32-byte hex secret, stages it via
/// `SecretStore::prepare_rotation`, then SSHes to the VM to write
/// `/etc/agentic-sandbox/agent.env` (mode 0600) and restart the
/// agentic-agent service. Old secret remains valid until the agent
/// reconnects with the new one OR the grace window expires (default
/// 5 minutes; override with `?grace_seconds=N`).
///
/// Returns 202 with `{ operation_id, status: "accepted", deadline_ms }`.
/// CLI polls `/api/v1/operations/{id}` for completion.
async fn agent_rotate_secret_handler(
    _: super::operator_auth::RequireAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    use rand::RngCore;
    use std::time::Duration;
    use tokio::process::Command;

    let secrets = match state.secret_store.as_ref() {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "secret store not configured"})),
            )
                .into_response()
        }
    };
    let op_store = match state.operation_store.as_ref() {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "operation store unavailable"})),
            )
                .into_response()
        }
    };

    let ip_address = match state.registry.get(&id) {
        Some(a) => a.registration.ip_address.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("agent {} not connected; rotation requires the agent to be reachable", id)
                })),
            )
                .into_response()
        }
    };

    let grace_seconds: u64 = params
        .get("grace_seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let grace = Duration::from_secs(grace_seconds);

    // Generate new 32-byte secret.
    let new_secret = {
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        hex::encode(buf)
    };

    let deadline = secrets.prepare_rotation(&id, &new_secret, grace);
    let deadline_ms = chrono::Utc::now().timestamp_millis()
        + (grace_seconds as i64).saturating_mul(1000);

    use super::operations::{Operation, OperationType};
    let operation = Operation::new(OperationType::VmCreate, id.clone());
    let op_id = op_store.insert(operation);
    let op_id_clone = op_id.clone();
    let agent_id = id.clone();
    let secrets_for_task = secrets.clone();

    tokio::spawn(async move {
        let op_id = op_id_clone;
        // Push secret to VM. We write the file with sudo via SSH,
        // chmod 0600, then restart the agent service. If the SSH/restart
        // step fails we roll back the staged rotation so the old secret
        // remains the only valid one.
        let env_contents = format!("AGENT_SECRET={}\n", new_secret);
        let remote_cmd = format!(
            "sudo install -m 600 /dev/stdin /etc/agentic-sandbox/agent.env \
             && sudo systemctl restart agentic-agent"
        );
        let ssh = Command::new("ssh")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(format!("agent@{}", ip_address))
            .arg(&remote_cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let mut child = match ssh {
            Ok(c) => c,
            Err(e) => {
                secrets_for_task.rollback_rotation(&agent_id);
                op_store.mark_failed(&op_id, format!("ssh spawn failed: {}", e));
                return;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            if let Err(e) = stdin.write_all(env_contents.as_bytes()).await {
                secrets_for_task.rollback_rotation(&agent_id);
                op_store.mark_failed(&op_id, format!("ssh stdin write failed: {}", e));
                return;
            }
            drop(stdin);
        }
        let output = match child.wait_with_output().await {
            Ok(o) => o,
            Err(e) => {
                secrets_for_task.rollback_rotation(&agent_id);
                op_store.mark_failed(&op_id, format!("ssh wait failed: {}", e));
                return;
            }
        };
        if !output.status.success() {
            secrets_for_task.rollback_rotation(&agent_id);
            op_store.mark_failed(
                &op_id,
                format!(
                    "remote rotation failed (exit {}): {}",
                    output.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&output.stderr)
                ),
            );
            return;
        }
        // Push succeeded. The rotation will commit on the next successful
        // verify against the new secret (i.e. when the agent reconnects).
        op_store.mark_completed(
            &op_id,
            Some(serde_json::json!({
                "agent_id": agent_id,
                "rotation": "pushed",
                "deadline_ms": deadline_ms,
                "note": "old secret remains valid until agent re-registers with new secret or grace window expires"
            })),
        );
        // Suppress unused-var warning on Instant deadline (used only above).
        let _ = deadline;
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "operation_id": op_id,
            "status": "accepted",
            "deadline_ms": deadline_ms,
            "grace_seconds": grace_seconds,
        })),
    )
        .into_response()
}

/// DELETE /api/v1/agents/:id — destroy VM + undefine + clean up
/// Always forces destroy if running; disk deletion can be requested via ?delete_disk=true
async fn agent_delete_handler(
    _: super::operator_auth::RequireAdmin,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let delete_disk = params
        .get("delete_disk")
        .map(|v| v == "true")
        .unwrap_or(false);
    let force = params.get("force").map(|v| v == "true").unwrap_or(true);
    use super::events;
    use super::vms::{
        connect_libvirt, get_domain, get_domain_state, libvirt_blocking, VmError, VmState,
    };

    let id_blk = id.clone();
    let result = libvirt_blocking(move || -> Result<bool, VmError> {
        let conn = connect_libvirt()?;
        let domain = get_domain(&conn, &id_blk)?;
        let state = get_domain_state(&domain)?;

        if state == VmState::Running && !force {
            return Err(VmError::CannotDeleteRunning(id_blk.clone()));
        }
        if state == VmState::Running {
            domain
                .destroy()
                .map_err(|e| VmError::LibvirtError(format!("Failed to destroy VM: {}", e)))?;
        }

        let disk_path = if delete_disk {
            domain.get_xml_desc(0).ok().and_then(|xml| {
                let re = regex::Regex::new(r"<source file='([^']+\.qcow2)'").ok()?;
                re.captures(&xml)?.get(1).map(|m| m.as_str().to_string())
            })
        } else {
            None
        };

        domain
            .undefine()
            .map_err(|e| VmError::LibvirtError(format!("Failed to undefine VM: {}", e)))?;

        let mut disk_deleted = false;
        if let Some(path) = disk_path {
            if std::path::Path::new(&path).exists() && std::fs::remove_file(&path).is_ok() {
                disk_deleted = true;
            }
        }
        Ok(disk_deleted)
    })
    .await;

    let disk_deleted = match result {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    events::add_libvirt_event(
        "vm.undefined",
        id.clone(),
        chrono::Utc::now(),
        Some("api".to_string()),
        None,
    )
    .await;

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

/// GET /api/v1/aiwg/status
async fn aiwg_status_handler(State(state): State<AppState>) -> impl IntoResponse {
    match &state.aiwg_handle {
        Some(h) => Json(h.conn_state()).into_response(),
        None => Json(serde_json::json!({
            "configured": false,
            "connected": false,
            "endpoint": null,
            "sandbox_id": null,
        }))
        .into_response(),
    }
}

/// POST /api/v1/aiwg/reconnect
async fn aiwg_reconnect_handler(State(state): State<AppState>) -> impl IntoResponse {
    match &state.aiwg_handle {
        Some(h) => {
            h.trigger_reconnect();
            (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "aiwg serve not configured" })),
        )
            .into_response(),
    }
}

// ── Session Registry HTTP handlers ────────────────────────────────────────────

/// GET /api/v1/sessions — list all live sessions.
async fn session_list_handler(State(state): State<AppState>) -> impl IntoResponse {
    match &state.session_registry {
        Some(sr) => Json(sr.list()).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "session registry not available" })),
        )
            .into_response(),
    }
}

/// DELETE /api/v1/sessions/:id — terminate a formal-model session.
///
/// Sends `?signal=TERM|KILL` (default `TERM`) to the underlying PTY via
/// the dispatcher. The `Closed` frame is broadcast through the existing
/// `CommandResult` path when the agent reaps the process — this handler
/// only delivers the signal.
async fn session_delete_handler(
    _: super::operator_auth::RequireAdmin,
    Path(session_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    use crate::dispatch::DispatchError;

    let signal_number = match params
        .get("signal")
        .map(|s| s.to_ascii_uppercase())
        .as_deref()
    {
        None | Some("TERM") | Some("SIGTERM") | Some("15") => 15,
        Some("KILL") | Some("SIGKILL") | Some("9") => 9,
        Some("INT") | Some("SIGINT") | Some("2") => 2,
        Some("HUP") | Some("SIGHUP") | Some("1") => 1,
        Some(other) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("unsupported signal: {}", other),
                    "supported": ["TERM", "KILL", "INT", "HUP"],
                })),
            )
                .into_response();
        }
    };

    if state.session_registry.is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "session registry not available" })),
        )
            .into_response();
    }

    match state
        .dispatcher
        .send_pty_signal_to_session(&session_id, signal_number)
        .await
    {
        Ok(()) => {
            // Best-effort: emit an event for observability. We pull the agent_id
            // from the session summary if available; not load-bearing.
            if let Some(sr) = &state.session_registry {
                let agent_id = sr
                    .list()
                    .into_iter()
                    .find(|s| s.session_id == session_id)
                    .map(|s| s.agent_id)
                    .unwrap_or_default();
                events::emit_session_killed(&agent_id, &session_id).await;
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "session_id": session_id,
                    "signal": signal_number,
                    "status": "signaled",
                })),
            )
                .into_response()
        }
        Err(DispatchError::CommandNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/v1/sessions/:id/stream — SSE stream of SessionFrames.
///
/// Any observer (proxy node, monitoring client) can subscribe here.
/// The session's replay buffer is not replayed automatically; pass
/// `?from=<seq>` to start from a specific sequence number.
async fn session_stream_handler(
    Path(session_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    use axum::response::sse::{Event, Sse};

    let sr = match &state.session_registry {
        Some(sr) => sr.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "session registry not available",
            )
                .into_response();
        }
    };

    let replay_from = params.get("from").and_then(|s| s.parse::<u64>().ok());
    let client_id = format!("sse-{}", uuid::Uuid::new_v4());

    let result = sr
        .attach(
            &session_id,
            client_id,
            crate::session::Role::Observer,
            replay_from,
        )
        .await;

    match result {
        Some((rx, _, _)) => {
            let stream = async_stream::stream! {
                let mut rx = rx;
                while let Some(frame) = rx.recv().await {
                    match serde_json::to_string(&*frame) {
                        Ok(data) => yield Ok::<_, std::convert::Infallible>(
                            Event::default().data(data)
                        ),
                        Err(_) => continue,
                    }
                }
            };
            Sse::new(stream)
                .keep_alive(axum::response::sse::KeepAlive::default())
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
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
