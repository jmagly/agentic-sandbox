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
            .route("/api/v1/health", get(health_handler))
            .route("/api/v1/agents", get(agents_handler))
            // Static files (index.html, app.js, styles.css)
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
        .map(|a| AgentInfo {
            id: a.id,
            hostname: a.hostname,
            ip_address: a.ip_address,
            status: format!("{:?}", a.status),
            connected_at: a.connected_at,
            last_heartbeat: a.last_heartbeat,
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
}

/// Serve static files from embedded assets
async fn static_handler(uri: Uri) -> Response<Body> {
    let path = uri.path().trim_start_matches('/');

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
        None => {
            // Try index.html for SPA routing
            if let Some(content) = Assets::get("index.html") {
                let body = Body::from(content.data.to_vec());
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html")
                    .body(body)
                    .unwrap()
            } else {
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("Not Found"))
                    .unwrap()
            }
        }
    }
}
