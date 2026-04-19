//! Agentic Management Server
//!
//! High-performance gRPC server for coordinating agent VMs.
//! Handles agent registration, command dispatch, and output streaming.

use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tonic::transport::Server;
use tracing::info;

mod aiwg_serve;
mod auth;
mod identity;
mod config;
mod crash_loop;
mod dispatch;
mod docker_runtime;
mod grpc;
mod heartbeat;
mod hitl;
mod http;
mod libvirt_events;
pub mod orchestrator;
mod output;
mod prompt_detector;
mod registry;
mod screen_state;
pub mod session;
pub mod telemetry;
mod ws;

use auth::SecretStore;
use config::ServerConfig;
use dispatch::CommandDispatcher;
use docker_runtime::{spawn_docker_monitor, DockerMonitorConfig};
use grpc::AgentServiceImpl;
use http::HttpServer;
use orchestrator::Orchestrator;
use output::{OutputAggregator, StreamType};
use registry::AgentRegistry;
use screen_state::ScreenRegistry;
use session::SessionRegistry;
use ws::WebSocketHub;

pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration first (before telemetry, so env file is loaded)
    let config = ServerConfig::from_env()?;

    // Initialize telemetry (logging, metrics)
    let telemetry_guard = telemetry::init_telemetry(&config.telemetry)?;

    // Startup banner
    let grpc_addr: SocketAddr = config.listen_addr.parse()?;
    let ws_port = grpc_addr.port() + 1;
    let http_port = grpc_addr.port() + 2;
    eprintln!();
    eprintln!("  Agentic Sandbox Management Server");
    eprintln!("  ----------------------------------");
    eprintln!("  gRPC      {}:{}", grpc_addr.ip(), grpc_addr.port());
    eprintln!("  WebSocket {}:{}", grpc_addr.ip(), ws_port);
    eprintln!("  Dashboard http://{}:{}", grpc_addr.ip(), http_port);
    eprintln!("  Secrets   {}", config.secrets_dir);
    eprintln!();

    // Load or generate persistent sandbox identity (UUIDv7, stable across restarts)
    let identity_path = identity::SandboxIdentity::default_path(&config.secrets_dir);
    let sandbox_identity = identity::SandboxIdentity::load_or_create(&identity_path)?;
    info!(instance_id = %sandbox_identity.id, "Sandbox identity loaded");

    // Optionally connect to aiwg serve (non-blocking; no-ops if env var absent)
    let aiwg_handle = aiwg_serve::AiwgServeConfig::from_env(&config.listen_addr, sandbox_identity.id.clone())
        .map(|cfg| aiwg_serve::spawn(cfg, env!("CARGO_PKG_VERSION")));

    // Initialize components
    let registry = {
        let mut r = AgentRegistry::new();
        if let Some(ref h) = aiwg_handle {
            r = r.with_aiwg_serve(h.clone());
        }
        Arc::new(r)
    };
    let secrets = Arc::new(SecretStore::new(&config.secrets_dir)?);
    let session_registry = Arc::new(SessionRegistry::new());
    let dispatcher = Arc::new({
        let mut d = CommandDispatcher::new(registry.clone())
            .with_session_registry(session_registry.clone());
        if let Some(ref h) = aiwg_handle {
            d = d.with_aiwg_serve(h.clone());
        }
        d
    });
    let output_agg = Arc::new(OutputAggregator::default());
    let screen_registry = Arc::new(ScreenRegistry::new());
    let hitl_store = Arc::new({
        let mut store = hitl::HitlStore::new();
        if let Some(ref h) = aiwg_handle {
            store = store.with_aiwg_serve(h.clone());
        }
        store
    });

    // Start heartbeat monitor to detect stale connections
    heartbeat::spawn_heartbeat_monitor(registry.clone());

    // Start libvirt event monitor for VM lifecycle events
    let libvirt_config = libvirt_events::LibvirtMonitorConfig::default();
    let (mut event_rx, _libvirt_handle) = libvirt_events::spawn_libvirt_monitor(libvirt_config);

    // Start Docker container monitor for lifecycle events/cleanup/metrics
    let docker_config = DockerMonitorConfig::from_env();
    spawn_docker_monitor(docker_config, telemetry_guard.metrics.clone());

    // Create crash loop detector channel
    let (crash_event_tx, crash_event_rx) = tokio::sync::mpsc::channel(256);
    let crash_config = crash_loop::CrashLoopConfig::default();
    let (crash_detector, mut crash_notification_rx, _crash_handle) =
        crash_loop::spawn_crash_loop_detector(crash_config, crash_event_rx);

    // Forward crash loop notifications to logs (and later WebSocket)
    tokio::spawn(async move {
        while let Some(notification) = crash_notification_rx.recv().await {
            tracing::warn!(
                vm = %notification.vm_name,
                event = %notification.event_type,
                state = %notification.state,
                message = %notification.message,
                "Crash loop notification"
            );
        }
    });

    // Forward libvirt events to both HTTP event store and crash loop detector
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            // Forward to HTTP event store
            http::events::add_libvirt_event(
                &event.event_type.to_string(),
                event.vm_name.clone(),
                event.timestamp,
                event.reason.clone(),
                event.uptime_seconds,
            )
            .await;

            // Forward to crash loop detector
            let _ = crash_event_tx.send(event).await;
        }
    });

    // Store detector reference for API access (unused warning is fine)
    let _crash_detector = crash_detector;

    // Initialize task orchestrator
    let orchestrator = Arc::new(Orchestrator::new(
        "/srv/agentshare/tasks".to_string(),
        "/srv/agentshare".to_string(),
        registry.clone(),
        dispatcher.clone(),
    ));

    // Create gRPC service
    let service = AgentServiceImpl::new(
        registry.clone(),
        secrets.clone(),
        dispatcher.clone(),
        output_agg.clone(),
    );

    info!("Starting gRPC server on {}", grpc_addr);
    info!("Secrets directory: {}", config.secrets_dir);

    // WebSocket server address (port + 1)
    let ws_addr: SocketAddr = format!("{}:{}", grpc_addr.ip(), ws_port).parse()?;
    info!("Starting WebSocket server on ws://{}", ws_addr);

    // Start WebSocket server in background
    let ws_hub = WebSocketHub::new(
        ws_addr,
        output_agg.clone(),
        registry.clone(),
        dispatcher.clone(),
        session_registry.clone(),
    )
    .with_orchestrator(orchestrator.clone())
    .with_hitl_store(hitl_store.clone());
    tokio::spawn(async move {
        if let Err(e) = ws_hub.run().await {
            tracing::error!("WebSocket server error: {}", e);
        }
    });

    // Spawn background task: feed stdout bytes into the screen registry
    {
        let mut screen_sub = output_agg.subscribe(None, Some(StreamType::Stdout));
        let screen_reg = screen_registry.clone();
        tokio::spawn(async move {
            while let Some(msg) = screen_sub.recv().await {
                screen_reg.process(&msg.command_id, &msg.data);
            }
        });
    }

    // HTTP dashboard server address (port + 2)
    let http_addr: SocketAddr = format!("{}:{}", grpc_addr.ip(), http_port).parse()?;
    info!("Starting HTTP dashboard on http://{}", http_addr);

    // Start HTTP server in background
    let http_server = HttpServer::new(
        http_addr,
        registry.clone(),
        output_agg.clone(),
        dispatcher.clone(),
    )
    .with_orchestrator(orchestrator.clone())
    .with_metrics(telemetry_guard.metrics.clone())
    .with_secrets(secrets.clone())
    .with_screen_registry(screen_registry)
    .with_hitl_store(hitl_store);
    let http_server = http_server.with_session_registry(session_registry.clone());
    let http_server = if let Some(ref h) = aiwg_handle {
        http_server.with_aiwg_handle(h.clone())
    } else {
        http_server
    };
    tokio::spawn(async move {
        if let Err(e) = http_server.run().await {
            tracing::error!("HTTP server error: {}", e);
        }
    });

    // Start gRPC server (blocking)
    // Configure aggressive keepalives to detect dead connections quickly
    Server::builder()
        .tcp_keepalive(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_timeout(Some(std::time::Duration::from_secs(20)))
        .add_service(proto::agent_service_server::AgentServiceServer::new(
            service,
        ))
        .serve(grpc_addr)
        .await?;

    Ok(())
}
