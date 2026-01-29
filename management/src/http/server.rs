//! HTTP server implementation using axum
//!
//! Serves the web dashboard UI and REST API endpoints.

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use rust_embed::RustEmbed;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

use crate::registry::AgentRegistry;
use crate::output::OutputAggregator;
use crate::dispatch::CommandDispatcher;

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
            },
        }
    }

    /// Run the HTTP server
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let app = Router::new()
            // API endpoints
            .route("/api/health", get(health_handler))
            .route("/api/v1/health", get(health_handler))
            .route("/api/v1/agents", get(agents_handler))
            // Static files (dashboard UI)
            .fallback(static_handler)
            .with_state(self.state);

        let listener = TcpListener::bind(self.listen_addr).await?;
        info!("HTTP dashboard available at http://{}", self.listen_addr);

        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Health check endpoint
async fn health_handler() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok".to_string(),
        service: "agentic-management".to_string(),
    })
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    service: String,
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
            AgentInfo {
                id: a.id,
                hostname: a.hostname,
                ip_address: a.ip_address,
                status: format!("{:?}", a.status),
                connected_at: a.connected_at,
                last_heartbeat: a.last_heartbeat,
                metrics,
                system_info,
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
pub struct AgentInfo {
    pub id: String,
    pub hostname: String,
    pub ip_address: String,
    pub status: String,
    pub connected_at: i64,
    pub last_heartbeat: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<MetricsInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_info: Option<SystemInfoApi>,
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
