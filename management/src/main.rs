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
mod config;
mod crash_loop;
mod dispatch;
mod docker_runtime;
mod grpc;
mod heartbeat;
mod hitl;
mod http;
mod identity;
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
    // The MissionStore is shared between the aiwg background task (executor.resync,
    // dispatch acceptance) and the HTTP dispatch handler (Pass 3).
    let mission_store = aiwg_serve::MissionStore::new();
    let aiwg_handle =
        aiwg_serve::AiwgServeConfig::from_env(&config.listen_addr, sandbox_identity.id.clone())
            .map(|cfg| aiwg_serve::spawn(cfg, env!("CARGO_PKG_VERSION"), mission_store.clone()));

    // Initialize components
    let registry = {
        let mut r = AgentRegistry::new();
        if let Some(ref h) = aiwg_handle {
            r = r.with_aiwg_serve(h.clone());
        }
        Arc::new(r)
    };
    let secrets = Arc::new(SecretStore::new(&config.secrets_dir)?);

    // SIGHUP → reload agent-hashes.json (in addition to operator-tokens.toml).
    // Required after `provision-vm.sh` rotates a VM's secret: without this,
    // the in-memory hash stays stale until the server restarts and the
    // newly-provisioned agent fails auth with `Unauthenticated`. Mirrors
    // the operator-tokens reload below; both share the same SIGHUP signal.
    {
        let secrets = secrets.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sighup = match signal(SignalKind::hangup()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "failed to install SIGHUP handler for agent-hashes; reload disabled");
                    return;
                }
            };
            while sighup.recv().await.is_some() {
                match secrets.reload() {
                    Ok(()) => tracing::info!("agent-hashes.json reloaded on SIGHUP"),
                    Err(e) => {
                        tracing::error!(error = %e, "agent-hashes reload failed; keeping previous hashes")
                    }
                }
            }
        });
    }

    let session_registry = Arc::new(SessionRegistry::new());
    let dispatcher = Arc::new({
        let mut d = CommandDispatcher::new(registry.clone())
            .with_session_registry(session_registry.clone())
            .with_mission_store(mission_store.clone());
        if let Some(ref h) = aiwg_handle {
            d = d.with_aiwg_serve(h.clone());
        }
        d
    });
    let output_agg = Arc::new(OutputAggregator::default());
    let screen_registry = Arc::new(ScreenRegistry::new());
    let hitl_store = Arc::new({
        let mut store = hitl::HitlStore::new().with_mission_store(mission_store.clone());
        if let Some(ref h) = aiwg_handle {
            store = store.with_aiwg_serve(h.clone());
        }
        store
    });

    // Start heartbeat monitor to detect stale connections
    heartbeat::spawn_heartbeat_monitor(registry.clone());

    // AIWG executor inbound HITL response handler (#193 pass 3).
    // Subscribes to inbound events from aiwg serve and on
    // `mission.hitl_responded` looks up the hitl request and injects the
    // response text into the agent's PTY stdin via the existing flow.
    if let Some(ref h) = aiwg_handle {
        let mut inbound_rx = h.subscribe_inbound();
        let hitl = hitl_store.clone();
        let disp = dispatcher.clone();
        tokio::spawn(async move {
            while let Ok(event) = inbound_rx.recv().await {
                if event.event != "mission.hitl_responded" {
                    continue;
                }
                let Some(data) = event.data.as_ref() else {
                    continue;
                };
                let Some(hitl_id) = data.get("hitl_id").and_then(|v| v.as_str()) else {
                    tracing::warn!("inbound mission.hitl_responded missing hitl_id");
                    continue;
                };
                let text = data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let Some(req) = hitl.resolve(hitl_id) else {
                    tracing::warn!(
                        hitl_id,
                        "inbound HITL response: no matching pending request"
                    );
                    continue;
                };
                let mut bytes = text.into_bytes();
                bytes.push(b'\n');
                if let Err(e) = disp.send_stdin(&req.session_id, bytes).await {
                    tracing::warn!(error = %e, session_id = %req.session_id, "failed to inject inbound HITL response");
                } else {
                    tracing::info!(hitl_id, session_id = %req.session_id, "injected inbound HITL response from aiwg");
                }
            }
        });
    }

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

    // Periodic keyframe injection (#145).
    //
    // Every 30s walk every active session, snapshot the parsed VT screen
    // (`vt100::Screen::contents_formatted()`), and push it as a Keyframe
    // into the session's replay buffer. Late joiners can then replay
    // from this point — `attach()` defaults `replay_from = None` to the
    // most recent keyframe seq. Idle sessions whose screen state hasn't
    // changed still pay the encoding cost; the period is generous to
    // keep that overhead low.
    {
        let session_reg = session_registry.clone();
        let screen_reg = screen_registry.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            // First tick fires immediately; skip it so we don't push a
            // keyframe for a session that just started and has nothing
            // on its screen yet.
            interval.tick().await;
            loop {
                interval.tick().await;
                let summaries = session_reg.list();
                for s in summaries {
                    let Some(state) = screen_reg.get(&s.command_id) else {
                        continue;
                    };
                    // ScreenState uses std::sync::Mutex (vt100 parser
                    // is fully sync). Hold the guard for the encode
                    // step only; release before publish_keyframe so we
                    // don't carry the lock across an await.
                    let bytes = match state.lock() {
                        Ok(guard) => guard.keyframe_bytes(),
                        Err(_) => continue,
                    };
                    if bytes.is_empty() {
                        continue;
                    }
                    session_reg
                        .publish_keyframe(&s.session_id, crate::session::StreamKind::Stdout, bytes)
                        .await;
                }
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
    .with_hitl_store(hitl_store)
    .with_storage_roots(
        "/srv/agentshare".to_string(),
        "/srv/agentshare/tasks".to_string(),
    )
    .with_operator_auth({
        let cfg = crate::http::operator_auth::OperatorAuthConfig::load(
            std::path::Path::new(&config.secrets_dir),
        )
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to load operator-tokens.toml; auth DISABLED");
            None
        });
        // SIGHUP → reload tokens. Atomic swap inside `reload()`; on parse
        // error the previous map stays intact (we keep auth on, not off).
        if let Some(ref auth) = cfg {
            let auth = auth.clone();
            tokio::spawn(async move {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sighup = match signal(SignalKind::hangup()) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(error = %e, "failed to install SIGHUP handler; token reload disabled");
                        return;
                    }
                };
                while sighup.recv().await.is_some() {
                    match auth.reload() {
                        Ok(count) => {
                            crate::http::events::emit_operator_tokens_reloaded(count, true).await;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "operator-tokens.toml reload failed; keeping previous tokens");
                            crate::http::events::emit_operator_tokens_reloaded(0, false).await;
                        }
                    }
                }
            });
        }
        cfg
    })
    .with_uds({
        // UDS is opt-in via env var. Setting AGENTIC_MGMT_UDS to a path
        // (e.g. /run/agentic-mgmt.sock) enables peer-creds-authenticated
        // admin access. Group is configurable via AGENTIC_MGMT_UDS_GROUP
        // (default agentic-admin).
        match std::env::var("AGENTIC_MGMT_UDS").ok() {
            Some(p) if !p.is_empty() => Some(crate::http::uds::UdsConfig {
                path: std::path::PathBuf::from(p),
                group: std::env::var("AGENTIC_MGMT_UDS_GROUP")
                    .unwrap_or_else(|_| "agentic-admin".to_string()),
            }),
            _ => None,
        }
    });
    let http_server = http_server.with_session_registry(session_registry.clone());
    let http_server = http_server.with_mission_store(mission_store.clone());
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

    // HTTP self-watchdog.
    //
    // gRPC on :8120 and HTTP on :8122 share this process but run as
    // separate tasks. A bug in a blocking HTTP handler (see the libvirt
    // spawn_blocking refactor) used to wedge the HTTP task while gRPC
    // kept flowing — process alive, but `/api/v1/*` hung for ~23h before
    // anyone noticed. This task probes `/healthz/http` (a trivial handler
    // that touches zero shared state) and exits the process on sustained
    // failures so the supervisor (systemd / dev.sh) can restart clean.
    {
        let probe_addr = http_addr;
        tokio::spawn(async move {
            // Startup grace period so the HTTP server has time to bind.
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;

            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error=%e, "watchdog: failed to build HTTP client; disabling");
                    return;
                }
            };
            let url = format!("http://{}/healthz/http", probe_addr);
            let mut consecutive_failures: u32 = 0;
            const MAX_FAILURES: u32 = 3;

            loop {
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                match client.get(&url).send().await {
                    Ok(r) if r.status().is_success() => {
                        if consecutive_failures > 0 {
                            tracing::info!("watchdog: HTTP recovered");
                        }
                        consecutive_failures = 0;
                    }
                    Ok(r) => {
                        consecutive_failures += 1;
                        tracing::warn!(
                            status = %r.status(),
                            failures = consecutive_failures,
                            "watchdog: non-success from /healthz/http"
                        );
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        tracing::warn!(
                            error = %e,
                            failures = consecutive_failures,
                            "watchdog: /healthz/http probe failed"
                        );
                    }
                }
                if consecutive_failures >= MAX_FAILURES {
                    tracing::error!(
                        "watchdog: HTTP unresponsive after {} consecutive probes — exiting(1) for supervisor restart",
                        consecutive_failures
                    );
                    // Flush logs before exit.
                    std::process::exit(1);
                }
            }
        });
    }

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
