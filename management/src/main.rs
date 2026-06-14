//! Agentic Management Server
//!
//! High-performance gRPC server for coordinating agent VMs.
//! Handles agent registration, command dispatch, and output streaming.

use anyhow::Result;
use futures_util::TryStreamExt;
use hyper::rt::{Read as HyperRead, ReadBufCursor, Write as HyperWrite};
use hyper_util::rt::TokioIo;
use std::io;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer},
    server::WebPkiClientVerifier,
    RootCertStore,
};
use tonic::transport::Server;
use tracing::info;

mod agent_message_dispatch;
mod agent_pty_bridge;
mod aiwg_serve;
#[allow(dead_code)]
mod bootstrap_enrollment;
mod config;
mod crash_loop;
mod dispatch;
mod docker_runtime;
mod grpc;
#[allow(dead_code)]
mod grpc_local_ca;
mod heartbeat;
mod hitl;
mod host_runtime;
mod http;
mod identity;
mod libvirt_events;
pub mod orchestrator;
mod output;
mod prompt_detector;
mod registry;
mod screen_state;
pub mod session;
mod systemd;
pub mod telemetry;
#[allow(dead_code)]
mod transport_identity;
mod ws;

use config::ServerConfig;
use dispatch::CommandDispatcher;
use docker_runtime::{spawn_docker_monitor, DockerMonitorConfig};
use grpc::{
    AgentMtlsConnectInfo, AgentServiceImpl, AgentTransportIdentityResolver, AgentVsockConnectInfo,
};
use host_runtime::{
    DaemonHostRuntimeSupervisor, DaemonHostSupervisorConfig, LocalHostRuntimeSupervisor,
    LocalHostSupervisorConfig,
};
use http::HttpServer;
use orchestrator::Orchestrator;
use output::{OutputAggregator, StreamType};
use registry::AgentRegistry;
use screen_state::ScreenRegistry;
use session::SessionRegistry;
use transport_identity::{PeerIdentityMap, TrustDomain};
use ws::WebSocketHub;

#[derive(Debug)]
struct TonicVsockIo {
    inner: TokioIo<tokio_vsock::VsockStream>,
    peer: AgentVsockConnectInfo,
}

impl TonicVsockIo {
    fn new(stream: tokio_vsock::VsockStream) -> Self {
        let peer = AgentVsockConnectInfo::new(stream.peer_addr().ok());
        Self {
            inner: TokioIo::new(stream),
            peer,
        }
    }
}

impl tonic::transport::server::Connected for TonicVsockIo {
    type ConnectInfo = AgentVsockConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.peer
    }
}

impl HyperRead for TonicVsockIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl HyperWrite for TonicVsockIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

impl AsyncRead for TonicVsockIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_read(cx, buf)
    }
}

impl AsyncWrite for TonicVsockIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        tokio::io::AsyncWrite::is_write_vectored(self.inner.inner())
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_write_vectored(cx, bufs)
    }
}

#[derive(Debug)]
struct TonicMtlsIo {
    inner: TokioIo<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>,
    peer: AgentMtlsConnectInfo,
}

impl TonicMtlsIo {
    fn new(
        stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        uri_san: Option<String>,
    ) -> Self {
        Self {
            inner: TokioIo::new(stream),
            peer: AgentMtlsConnectInfo::new(uri_san),
        }
    }
}

impl tonic::transport::server::Connected for TonicMtlsIo {
    type ConnectInfo = AgentMtlsConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.peer.clone()
    }
}

impl HyperRead for TonicMtlsIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl HyperWrite for TonicMtlsIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

impl AsyncRead for TonicMtlsIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_read(cx, buf)
    }
}

impl AsyncWrite for TonicMtlsIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        tokio::io::AsyncWrite::is_write_vectored(self.inner.inner())
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_write_vectored(cx, bufs)
    }
}

#[derive(Clone)]
struct GrpcMtlsConfig {
    listen_addr: SocketAddr,
    server_cert_chain: Vec<CertificateDer<'static>>,
    server_key_der: Vec<u8>,
    server_key_kind: GrpcPrivateKeyKind,
    client_ca: RootCertStore,
}

#[derive(Clone, Copy)]
enum GrpcPrivateKeyKind {
    Pkcs8,
    Pkcs1,
    Sec1,
}

impl GrpcPrivateKeyKind {
    fn into_der(self, bytes: Vec<u8>) -> PrivateKeyDer<'static> {
        match self {
            Self::Pkcs8 => PrivateKeyDer::Pkcs8(bytes.into()),
            Self::Pkcs1 => PrivateKeyDer::Pkcs1(bytes.into()),
            Self::Sec1 => PrivateKeyDer::Sec1(bytes.into()),
        }
    }
}

impl GrpcMtlsConfig {
    fn from_env() -> Result<Option<Self>> {
        let listen = env_string_optional("AGENTIC_GRPC_MTLS_LISTEN");
        let cert = env_string_optional("AGENTIC_GRPC_MTLS_CERT");
        let key = env_string_optional("AGENTIC_GRPC_MTLS_KEY");
        let client_ca = env_string_optional("AGENTIC_GRPC_MTLS_CLIENT_CA");

        if listen.is_none() && cert.is_none() && key.is_none() && client_ca.is_none() {
            return Ok(None);
        }

        let listen = listen.ok_or_else(|| {
            anyhow::anyhow!("AGENTIC_GRPC_MTLS_LISTEN is required when gRPC mTLS is configured")
        })?;
        let cert = cert.ok_or_else(|| {
            anyhow::anyhow!("AGENTIC_GRPC_MTLS_CERT is required when gRPC mTLS is configured")
        })?;
        let key = key.ok_or_else(|| {
            anyhow::anyhow!("AGENTIC_GRPC_MTLS_KEY is required when gRPC mTLS is configured")
        })?;
        let client_ca = client_ca.ok_or_else(|| {
            anyhow::anyhow!("AGENTIC_GRPC_MTLS_CLIENT_CA is required when gRPC mTLS is configured")
        })?;

        let (server_key_kind, server_key_der) = load_private_key(Path::new(&key))?;

        Ok(Some(Self {
            listen_addr: listen.parse()?,
            server_cert_chain: load_certs(Path::new(&cert))?,
            server_key_der,
            server_key_kind,
            client_ca: load_root_store(Path::new(&client_ca))?,
        }))
    }

    fn to_rustls_server_config(&self) -> Result<rustls::ServerConfig> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let verifier = WebPkiClientVerifier::builder(Arc::new(self.client_ca.clone())).build()?;
        let key = self.server_key_kind.into_der(self.server_key_der.clone());
        let mut cfg = rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(self.server_cert_chain.clone(), key)?;
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        Ok(cfg)
    }
}

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
    // Persistence file (#193 closed gap 2) lives next to the identity file so
    // the same backup/migrate story applies. After restart, executor.resync
    // emits the loaded mission_ids so AIWG reconciles in-flight work.
    let mission_store_path = std::path::Path::new(&config.secrets_dir)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/var/lib/agentic-sandbox"))
        .join("missions.json");
    let data_dir = std::path::Path::new(&config.secrets_dir)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/var/lib/agentic-sandbox"))
        .to_path_buf();
    let mission_store = aiwg_serve::MissionStore::load_or_default(mission_store_path);
    http::events::configure_event_archive(data_dir.join("events.jsonl")).await;

    // v2 A2A TaskStore (#205) lives alongside the v1 MissionStore. #208 will
    // wire it into the executor; for now we open it so the schema exists on
    // disk and migration tooling (#207) has a target.
    //
    // #243 lift: the Arcs were previously scoped to the match arm, which
    // meant the bindings dropped before HttpServer::new could pick them up
    // for the v2 executor mount. They now live at function scope so the
    // builder chain below can read them.
    let task_store_path = std::path::Path::new(&config.secrets_dir)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/var/lib/agentic-sandbox"))
        .join("missions.db");
    // TaskStore is wrapped in Arc so the v2 IdempotencyCache (#206) and any
    // future v2 wiring (#208/#210) share the same SQLite connection pool.
    let mut task_store: Option<Arc<aiwg_serve::task_store::TaskStore>> = None;
    let mut idempotency_cache: Option<Arc<aiwg_serve::idempotency::IdempotencyCache>> = None;
    match aiwg_serve::task_store::TaskStore::open(&task_store_path) {
        Ok(store) => {
            tracing::info!(
                "v2 TaskStore opened at {}; v1 MissionStore remains active for compat",
                task_store_path.display()
            );
            let store_arc = Arc::new(store);
            let cache = Arc::new(aiwg_serve::idempotency::IdempotencyCache::new(
                store_arc.clone(),
            ));
            tracing::info!(
                "IdempotencyCache initialized (cap={}, ttl={}s, sweep=60s)",
                cache.max_entries(),
                cache.ttl().num_seconds()
            );
            // Background sweep loop — every 60s, prune past-TTL entries.
            // Errors are logged at warn but never break the loop; the
            // next tick will retry. Wired here (not in handlers yet)
            // because #210 will plug the cache into A2A request paths.
            let sweep_cache = cache.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
                ticker.tick().await; // consume immediate first tick
                loop {
                    ticker.tick().await;
                    match sweep_cache.sweep_expired() {
                        Ok(n) if n > 0 => {
                            tracing::debug!("IdempotencyCache swept {n} expired entries");
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!("IdempotencyCache sweep failed: {e:#}");
                        }
                    }
                }
            });
            task_store = Some(store_arc);
            idempotency_cache = Some(cache);
        }
        Err(e) => {
            tracing::warn!(
                "failed to open v2 TaskStore at {}: {e:#}",
                task_store_path.display()
            );
        }
    };
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
    let bootstrap_tokens = Arc::new(bootstrap_enrollment::BootstrapTokenStore::load_or_create(
        Path::new(&config.secrets_dir).join("bootstrap-enrollment"),
    )?);
    let grpc_local_ca = Arc::new(grpc_local_ca::EmbeddedGrpcCa::load_or_create(
        Path::new(&config.secrets_dir).join("grpc-local-ca"),
        &grpc_local_ca_trust_domain(),
    )?);
    let grpc_uds_path = std::env::var("AGENTIC_GRPC_UDS")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from);
    let grpc_vsock_port = env_u32_optional("AGENTIC_GRPC_VSOCK_PORT")?;
    let grpc_mtls_config = GrpcMtlsConfig::from_env()?;
    let agent_transport_identity = grpc_transport_identity_resolver(
        &sandbox_identity.id,
        grpc_uds_path.is_some(),
        grpc_vsock_port.is_some(),
        grpc_mtls_config.is_some(),
    )?;

    let session_registry =
        Arc::new(SessionRegistry::new().with_transcript_archive(data_dir.join("pty-transcripts")));
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

    // AIWG executor graceful-suspend handler (#193 closed gap 3).
    // On SIGTERM / SIGINT, walk the mission store and emit
    // `mission.suspended` for every non-terminal mission before letting
    // the process exit. Pairs with `mission.reconnected` + `mission.resumed`
    // emitted by executor_ws_loop on the next start, completing the
    // suspended → reconnected → resumed lifecycle when the operator
    // restarts agentic-mgmt.
    if let Some(ref h) = aiwg_handle {
        let handle = h.clone();
        let store = mission_store.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("SIGTERM handler install failed: {}", e);
                    return;
                }
            };
            let mut sigint = match signal(SignalKind::interrupt()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("SIGINT handler install failed: {}", e);
                    return;
                }
            };
            tokio::select! {
                _ = sigterm.recv() => tracing::info!("SIGTERM received — emitting mission.suspended"),
                _ = sigint.recv()  => tracing::info!("SIGINT received — emitting mission.suspended"),
            }
            let owned = store.active_mission_ids();
            if let Some(executor_id) = handle.executor_id() {
                for mission_id in &owned {
                    let checkpoint_id = store
                        .get(mission_id)
                        .and_then(|r| r.checkpoint_id)
                        .unwrap_or_else(|| format!("auto-{}", mission_id));
                    handle.emit_executor(aiwg_serve::ExecutorEvent::mission_suspended(
                        &executor_id,
                        mission_id,
                        &checkpoint_id,
                        "mgmt_server_shutdown",
                    ));
                    store.update_state(mission_id, aiwg_serve::MissionState::Suspended);
                }
                // Give the WS forwarder a brief window to push these out
                // before the runtime tears down. The 250 ms is empirical:
                // typical local WS round-trip is <10 ms, so this leaves
                // headroom for slower paths without making restarts feel
                // sluggish.
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            std::process::exit(0);
        });
    }

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

    // #268: Hoist the executor InstanceRegistry creation so the docker
    // monitor (spawned next) can wire readiness updates into it. The
    // registry was previously created inline inside `executor_surface`
    // below; constructing it here lets us share it with both the monitor
    // and the surface without breaking the existing flow.
    let exec_instance_registry = agentic_sandbox_executor::instance::InstanceRegistry::new();

    // Start Docker container monitor for lifecycle events/cleanup/metrics.
    // #268: pass the executor InstanceRegistry + AgentRegistry so the
    // monitor flips `InstanceContext.ready=false` when a container
    // transitions to stopped — letting `send_message` 503 instead of
    // accepting work that will stall forever.
    let docker_config = DockerMonitorConfig::from_env();
    spawn_docker_monitor(
        docker_config,
        telemetry_guard.metrics.clone(),
        Some(exec_instance_registry.clone()),
        Some(registry.clone()),
    );

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

    // gRPC service constructed below — see "Create gRPC service" after the
    // executor surface decision so we can wire the InstanceRegistry bridge
    // (#317) when the surface is available.

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

    // v2 executor wiring (#208 / #243). When the TaskStore opened
    // successfully build the AgentPtyBridge — which forwards `pty-ws/v1`
    // traffic to agent-rs over the existing gRPC channel — install it as
    // the dispatcher's OutputObserver, and capture the surface that
    // HttpServer::with_executor consumes to mount the canonical A2A
    // router under `/agents/*`. The InstanceRegistry starts empty for
    // v2.0; admin-API provisionInstance follow-ups will populate it.
    let executor_surface = if let (Some(store), Some(cache)) =
        (task_store.as_ref(), idempotency_cache.as_ref())
    {
        use crate::agent_pty_bridge::AgentPtyBridge;
        use crate::http::server::ExecutorSurface;
        use agentic_sandbox_executor::bindings::pty_bridge::PtyBridge;

        let conformance_mode = std::env::var("AIWG_CONFORMANCE_MODE").as_deref() == Ok("1");
        let pty_bridge: Arc<dyn PtyBridge> = if conformance_mode {
            tracing::warn!(
                "AIWG_CONFORMANCE_MODE=1: binding NoOpPtyBridge for deterministic PTY conformance. \
                 Do NOT set this env var in production."
            );
            Arc::new(agentic_sandbox_executor::bindings::pty_bridge::NoOpPtyBridge)
        } else {
            let bridge = Arc::new(AgentPtyBridge::new(registry.clone(), dispatcher.clone()));
            bridge.install_as_observer();
            tracing::info!("AgentPtyBridge installed as OutputObserver on CommandDispatcher");
            bridge
        };

        // Per-instance signing key root (#253). Each provisioned instance
        // persists its Ed25519 keypair under
        // `<secrets_dir>/instances/<instance_id>/signing.pem` so the
        // AgentCard JWS kid stays stable across management-server restarts.
        let signing_keys_dir = std::path::Path::new(&config.secrets_dir).join("instances");

        // #269: production-grade A2A `messages:send` dispatch. The
        // executor defaults to NoOpMessageDispatch (truthful 503); wiring
        // this here makes the seam forward work to the connected agent's
        // gRPC channel via the existing CommandDispatcher pipeline.
        //
        // AIWG_CONFORMANCE_MODE=1 swaps in the test-only AcceptingMessageDispatch
        // so the conformance harness (#220) can exercise the A2A surface
        // without an actual backing runtime. The conformance workflow sets
        // this env var explicitly — production deployments must not.
        let message_dispatch: Arc<
            dyn agentic_sandbox_executor::bindings::message_dispatch::MessageDispatch,
        > = if conformance_mode {
            tracing::warn!(
                "AIWG_CONFORMANCE_MODE=1: binding AcceptingMessageDispatch (test-only). \
                 Do NOT set this env var in production."
            );
            agentic_sandbox_executor::bindings::message_dispatch::accepting()
        } else {
            Arc::new(crate::agent_message_dispatch::AgentMessageDispatch::new(
                registry.clone(),
                dispatcher.clone(),
                store.clone(),
            ))
        };

        // AIWG_CONFORMANCE_MODE=1: pre-register a known InstanceContext so
        // the conformance harness can hit `/agents/<id>/...` without
        // separately provisioning a backing runtime. The fixed instance_id
        // is a deterministic UUIDv7 so the harness URL is stable across runs.
        if conformance_mode {
            use agentic_sandbox_executor::instance::{InstanceContext, RuntimeKind};
            const CONFORMANCE_INSTANCE_ID: &str = "00000000-0000-7000-8000-000000000001";
            let host_for_card = http_addr.to_string();
            let ctx = Arc::new(InstanceContext::new_ephemeral(
                CONFORMANCE_INSTANCE_ID.to_string(),
                RuntimeKind::Container,
                "conformance-mock".to_string(),
                None,
                host_for_card,
            ));
            exec_instance_registry.insert(ctx);
            tracing::warn!(
                instance_id = CONFORMANCE_INSTANCE_ID,
                "AIWG_CONFORMANCE_MODE=1: pre-registered ephemeral instance for conformance harness"
            );
        }

        Some(ExecutorSurface {
            store: store.clone(),
            idem: cache.clone(),
            // #268: reuse the hoisted registry shared with the docker
            // monitor so readiness updates propagate.
            instance_registry: exec_instance_registry.clone(),
            pty_bridge,
            message_dispatch,
            signing_keys_dir,
        })
    } else {
        tracing::warn!(
            "executor surface not mounted: TaskStore unavailable (see earlier warning); /agents/* routes will 404"
        );
        None
    };

    // Create gRPC service. #317: when the executor surface is mounted,
    // wire the InstanceRegistry + signing-key root so every gRPC-registered
    // agent (VM via provision-vm.sh, Docker via legacy POST /containers)
    // becomes a routable v2/A2A instance — not just admin-v2 provisioned
    // ones. Without this bridge, VM-backed agents register in v1 but
    // `/agents/{instance_id}/.well-known/agent-card.json` returns
    // `instance.not_found`.
    let service = {
        let mut svc =
            AgentServiceImpl::new(registry.clone(), dispatcher.clone(), output_agg.clone());
        if let Some(surface) = executor_surface.as_ref() {
            svc = svc.with_executor_registry(
                surface.instance_registry.clone(),
                surface.signing_keys_dir.clone(),
            );
        }
        if let Some(resolver) = agent_transport_identity {
            svc = svc.with_transport_identity_resolver(resolver);
        }
        svc
    };

    // Start HTTP server in background
    let http_server = HttpServer::new(
        http_addr,
        registry.clone(),
        output_agg.clone(),
        dispatcher.clone(),
    )
    .with_orchestrator(orchestrator.clone())
    .with_metrics(telemetry_guard.metrics.clone())
    .with_bootstrap_tokens(bootstrap_tokens)
    .with_grpc_local_ca(grpc_local_ca)
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
    let http_server = if let Some(host_config) = DaemonHostSupervisorConfig::from_env() {
        tracing::info!(
            socket = %host_config.socket_path.display(),
            supervisor_id = %host_config.supervisor_id,
            timeout_secs = host_config.request_timeout.as_secs(),
            "daemon host runtime supervisor enabled"
        );
        http_server
            .with_host_runtime_supervisor(Arc::new(DaemonHostRuntimeSupervisor::new(host_config)))
    } else if let Some(host_config) = LocalHostSupervisorConfig::from_env(grpc_addr.to_string()) {
        tracing::info!(
            root = %host_config.root_dir.display(),
            agent_binary = %host_config.agent_binary.display(),
            management_server = %host_config.management_server,
            "local host runtime supervisor enabled"
        );
        http_server
            .with_host_runtime_supervisor(Arc::new(LocalHostRuntimeSupervisor::new(host_config)))
    } else {
        http_server
    };
    let http_server = if let Some(ref h) = aiwg_handle {
        http_server.with_aiwg_handle(h.clone())
    } else {
        http_server
    };
    // #243: mount the v2 executor router when the TaskStore is available.
    let http_server = if let Some(surface) = executor_surface {
        http_server.with_executor(surface)
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

    // Bind gRPC explicitly so Type=notify reports readiness only after the
    // externally-facing gRPC listener is up. HTTP and WS startup tasks have
    // already been launched above and continue to report their own bind errors.
    let grpc_listener = tokio::net::TcpListener::bind(grpc_addr).await?;
    let grpc_incoming = tokio_stream::wrappers::TcpListenerStream::new(grpc_listener);

    if let Some(path) = grpc_uds_path {
        let uds_service = service.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_grpc_uds(path, uds_service).await {
                tracing::error!(error = %e, "gRPC UDS listener exited");
            }
        });
    }
    if let Some(port) = grpc_vsock_port {
        let vsock_service = service.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_grpc_vsock(port, vsock_service).await {
                tracing::error!(error = %e, "gRPC vsock listener exited");
            }
        });
    }
    if let Some(mtls_config) = grpc_mtls_config {
        let mtls_service = service.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_grpc_mtls(mtls_config, mtls_service).await {
                tracing::error!(error = %e, "gRPC mTLS listener exited");
            }
        });
    }

    let watchdog = systemd::SystemdWatchdog::new();
    if let Err(e) = watchdog.notify_ready() {
        tracing::warn!(error = %e, "systemd READY notification failed");
    }
    watchdog.spawn_ping_loop();

    // Start gRPC server (blocking)
    // Configure aggressive keepalives to detect dead connections quickly
    Server::builder()
        .tcp_keepalive(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_timeout(Some(std::time::Duration::from_secs(20)))
        .add_service(proto::agent_service_server::AgentServiceServer::new(
            service,
        ))
        .serve_with_incoming(grpc_incoming)
        .await?;

    Ok(())
}

fn grpc_transport_identity_resolver(
    sandbox_id: &str,
    uds_enabled: bool,
    vsock_enabled: bool,
    mtls_enabled: bool,
) -> Result<Option<AgentTransportIdentityResolver>> {
    let raw_uds_map = std::env::var("AGENTIC_GRPC_UDS_UID_MAP")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let raw_vsock_map = std::env::var("AGENTIC_GRPC_VSOCK_CID_MAP")
        .ok()
        .filter(|v| !v.trim().is_empty());

    if raw_uds_map.is_none() && uds_enabled {
        anyhow::bail!("AGENTIC_GRPC_UDS_UID_MAP is required when AGENTIC_GRPC_UDS is set");
    }
    if raw_vsock_map.is_none() && vsock_enabled {
        anyhow::bail!("AGENTIC_GRPC_VSOCK_CID_MAP is required when AGENTIC_GRPC_VSOCK_PORT is set");
    }
    if raw_uds_map.is_none() && raw_vsock_map.is_none() && !mtls_enabled {
        return Ok(None);
    }

    let trust_domain = TrustDomain::local_from_sandbox_identity(sandbox_id)?;
    let mut peer_map = PeerIdentityMap::new();

    if let Some(raw_map) = raw_uds_map {
        for entry in raw_map.split(',').map(str::trim).filter(|v| !v.is_empty()) {
            let (uid, instance_id) = entry.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("invalid AGENTIC_GRPC_UDS_UID_MAP entry `{entry}`")
            })?;
            let uid: u32 = uid
                .trim()
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid UDS uid `{uid}`: {e}"))?;
            peer_map.register_uds_uid(uid, instance_id.trim())?;
        }
    }

    if let Some(raw_map) = raw_vsock_map {
        for entry in raw_map.split(',').map(str::trim).filter(|v| !v.is_empty()) {
            let (cid, instance_id) = entry.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("invalid AGENTIC_GRPC_VSOCK_CID_MAP entry `{entry}`")
            })?;
            let cid: u32 = cid
                .trim()
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid vsock CID `{cid}`: {e}"))?;
            peer_map.register_vsock_cid(cid, instance_id.trim())?;
        }
    }

    Ok(Some(AgentTransportIdentityResolver::new(
        trust_domain,
        peer_map,
    )))
}

fn env_u32_optional(name: &str) -> Result<Option<u32>> {
    let Some(value) = std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    value
        .parse()
        .map(Some)
        .map_err(|e| anyhow::anyhow!("invalid {name} value `{value}`: {e}"))
}

fn grpc_local_ca_trust_domain() -> String {
    std::env::var("AGENTIC_GRPC_LOCAL_CA_TRUST_DOMAIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "sandbox.agentic.local".to_string())
}

async fn serve_grpc_uds(path: PathBuf, service: AgentServiceImpl) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let listener = tokio::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))?;
    info!(path = %path.display(), "Starting gRPC UDS listener");

    let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);
    Server::builder()
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_timeout(Some(std::time::Duration::from_secs(20)))
        .add_service(proto::agent_service_server::AgentServiceServer::new(
            service,
        ))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}

async fn serve_grpc_vsock(port: u32, service: AgentServiceImpl) -> Result<()> {
    let addr = tokio_vsock::VsockAddr::new(tokio_vsock::VMADDR_CID_ANY, port);
    let listener = tokio_vsock::VsockListener::bind(addr)?;
    let incoming = listener.incoming().map_ok(TonicVsockIo::new);
    info!(port, "Starting gRPC vsock listener");

    Server::builder()
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_timeout(Some(std::time::Duration::from_secs(20)))
        .add_service(proto::agent_service_server::AgentServiceServer::new(
            service,
        ))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}

async fn serve_grpc_mtls(config: GrpcMtlsConfig, service: AgentServiceImpl) -> Result<()> {
    let server_config = Arc::new(config.to_rustls_server_config()?);
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config);
    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    info!(addr = %config.listen_addr, "Starting gRPC mTLS listener");

    let incoming = async_stream::stream! {
        loop {
            let (tcp, peer_addr) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(e) => {
                    yield Err::<TonicMtlsIo, io::Error>(e);
                    continue;
                }
            };
            let acceptor = acceptor.clone();
            match acceptor.accept(tcp).await {
                Ok(tls) => {
                    let uri_san = {
                        let (_, conn) = tls.get_ref();
                        conn.peer_certificates()
                            .and_then(|certs| certs.first())
                            .and_then(|cert| extract_spiffe_uri_san(cert.as_ref()))
                    };
                    yield Ok::<TonicMtlsIo, io::Error>(TonicMtlsIo::new(tls, uri_san));
                }
                Err(e) => {
                    tracing::debug!(error = %e, peer = %peer_addr, "gRPC mTLS handshake failed");
                    continue;
                }
            }
        }
    };

    Server::builder()
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(10)))
        .http2_keepalive_timeout(Some(std::time::Duration::from_secs(20)))
        .add_service(proto::agent_service_server::AgentServiceServer::new(
            service,
        ))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}

fn env_string_optional(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn load_certs(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut reader = io::BufReader::new(std::fs::File::open(path)?);
    let certs = rustls_pemfile::certs(&mut reader).collect::<std::result::Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no certificates found in {:?}", path),
        ));
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> io::Result<(GrpcPrivateKeyKind, Vec<u8>)> {
    let mut reader = io::BufReader::new(std::fs::File::open(path)?);
    for item in rustls_pemfile::read_all(&mut reader) {
        match item? {
            rustls_pemfile::Item::Pkcs8Key(k) => {
                return Ok((GrpcPrivateKeyKind::Pkcs8, k.secret_pkcs8_der().to_vec()));
            }
            rustls_pemfile::Item::Pkcs1Key(k) => {
                return Ok((GrpcPrivateKeyKind::Pkcs1, k.secret_pkcs1_der().to_vec()));
            }
            rustls_pemfile::Item::Sec1Key(k) => {
                return Ok((GrpcPrivateKeyKind::Sec1, k.secret_sec1_der().to_vec()));
            }
            _ => continue,
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("no usable private key found in {:?}", path),
    ))
}

fn load_root_store(path: &Path) -> io::Result<RootCertStore> {
    let certs = load_certs(path)?;
    let mut store = RootCertStore::empty();
    for cert in certs {
        store.add(cert).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid CA cert: {}", e),
            )
        })?;
    }
    Ok(store)
}

fn extract_spiffe_uri_san(cert_der: &[u8]) -> Option<String> {
    use x509_parser::extensions::GeneralName;

    let (_, parsed) = x509_parser::parse_x509_certificate(cert_der).ok()?;
    let san = parsed.subject_alternative_name().ok().flatten()?;
    san.value.general_names.iter().find_map(|name| match name {
        GeneralName::URI(uri) if uri.starts_with("spiffe://") => Some((*uri).to_string()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static GRPC_MTLS_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn grpc_mtls_env_lock() -> std::sync::MutexGuard<'static, ()> {
        GRPC_MTLS_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("gRPC mTLS env test lock poisoned")
    }

    fn clear_grpc_mtls_env() {
        std::env::remove_var("AGENTIC_GRPC_MTLS_LISTEN");
        std::env::remove_var("AGENTIC_GRPC_MTLS_CERT");
        std::env::remove_var("AGENTIC_GRPC_MTLS_KEY");
        std::env::remove_var("AGENTIC_GRPC_MTLS_CLIENT_CA");
    }

    fn make_uri_san_cert(uri: &str) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .subject_alt_names
            .push(rcgen::SanType::URI(uri.try_into().unwrap()));
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        cert.der().to_vec()
    }

    fn make_cn_only_cert(cn: &str) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(vec![cn.to_string()]).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, cn);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        cert.der().to_vec()
    }

    #[test]
    fn env_u32_optional_parses_and_rejects_invalid_value() {
        const NAME: &str = "AIWG_TEST_U32_PARSE";

        std::env::set_var(NAME, "42");
        assert_eq!(env_u32_optional(NAME).unwrap(), Some(42));

        std::env::set_var(NAME, "nope");
        let err = env_u32_optional(NAME).unwrap_err();

        std::env::remove_var(NAME);
        assert!(err
            .to_string()
            .contains("invalid AIWG_TEST_U32_PARSE value"));
    }

    #[test]
    fn grpc_mtls_config_from_env_disabled_when_no_vars_set() {
        let _guard = grpc_mtls_env_lock();
        clear_grpc_mtls_env();

        assert!(GrpcMtlsConfig::from_env().unwrap().is_none());
    }

    #[test]
    fn grpc_mtls_config_from_env_rejects_partial_config() {
        let _guard = grpc_mtls_env_lock();
        clear_grpc_mtls_env();
        std::env::set_var("AGENTIC_GRPC_MTLS_LISTEN", "127.0.0.1:0");

        let err = match GrpcMtlsConfig::from_env() {
            Err(err) => err,
            Ok(_) => panic!("partial gRPC mTLS config should fail closed"),
        };

        clear_grpc_mtls_env();
        assert!(err
            .to_string()
            .contains("AGENTIC_GRPC_MTLS_CERT is required"));
    }

    #[test]
    fn extract_spiffe_uri_san_uses_uri_san_not_subject_cn() {
        let uri = "spiffe://sandbox.example/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let cert = make_uri_san_cert(uri);

        assert_eq!(extract_spiffe_uri_san(&cert).as_deref(), Some(uri));

        let cn_only = make_cn_only_cert(uri);
        assert_eq!(extract_spiffe_uri_san(&cn_only), None);
    }
}
