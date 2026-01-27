//! Agentic Management Server
//!
//! High-performance gRPC server for coordinating agent VMs.
//! Handles agent registration, command dispatch, and output streaming.

use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tonic::transport::Server;
use tracing::info;

mod config;
mod grpc;
mod registry;
mod auth;
mod dispatch;
mod output;
mod ws;
mod http;

use config::ServerConfig;
use grpc::AgentServiceImpl;
use registry::AgentRegistry;
use auth::SecretStore;
use dispatch::CommandDispatcher;
use output::OutputAggregator;
use ws::WebSocketHub;
use http::HttpServer;

pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("agentic_management=info".parse()?)
                .add_directive("tonic=info".parse()?)
        )
        .init();

    // Load configuration
    let config = ServerConfig::from_env()?;

    // Initialize components
    let registry = Arc::new(AgentRegistry::new());
    let secrets = Arc::new(SecretStore::new(&config.secrets_dir)?);
    let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));
    let output_agg = Arc::new(OutputAggregator::default());

    // Create gRPC service
    let service = AgentServiceImpl::new(
        registry.clone(),
        secrets.clone(),
        dispatcher.clone(),
        output_agg.clone(),
    );

    // Build gRPC server address
    let grpc_addr: SocketAddr = config.listen_addr.parse()?;
    info!("Starting gRPC server on {}", grpc_addr);
    info!("Secrets directory: {}", config.secrets_dir);

    // WebSocket server address (port + 1)
    let ws_port = grpc_addr.port() + 1;
    let ws_addr: SocketAddr = format!("{}:{}", grpc_addr.ip(), ws_port).parse()?;
    info!("Starting WebSocket server on ws://{}", ws_addr);

    // Start WebSocket server in background
    let ws_hub = WebSocketHub::new(
        ws_addr,
        output_agg.clone(),
        registry.clone(),
        dispatcher.clone(),
    );
    tokio::spawn(async move {
        if let Err(e) = ws_hub.run().await {
            tracing::error!("WebSocket server error: {}", e);
        }
    });

    // HTTP dashboard server address (port + 2)
    let http_port = grpc_addr.port() + 2;
    let http_addr: SocketAddr = format!("{}:{}", grpc_addr.ip(), http_port).parse()?;
    info!("Starting HTTP dashboard on http://{}", http_addr);

    // Start HTTP server in background
    let http_server = HttpServer::new(
        http_addr,
        registry.clone(),
        output_agg.clone(),
        dispatcher.clone(),
    );
    tokio::spawn(async move {
        if let Err(e) = http_server.run().await {
            tracing::error!("HTTP server error: {}", e);
        }
    });

    // Start gRPC server (blocking)
    Server::builder()
        .add_service(proto::agent_service_server::AgentServiceServer::new(service))
        .serve(grpc_addr)
        .await?;

    Ok(())
}
