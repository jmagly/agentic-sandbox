//! gRPC service implementation

use std::pin::Pin;
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_vsock::VsockAddr;
use tonic::metadata::MetadataMap;
use tonic::transport::server::UdsConnectInfo;
use tonic::{Request, Response, Status, Streaming};
use tracing::{error, info, warn, Instrument};
use virt::connect::Connect;

use crate::dispatch::CommandDispatcher;
use crate::http::events::{
    emit_agent_connected, emit_agent_disconnected, emit_agent_registered, emit_session_killed,
    emit_session_preserved, emit_session_query_sent, emit_session_reconcile_complete,
    emit_session_reconcile_failed, emit_session_reconcile_started, emit_session_report_received,
};
use crate::output::{OutputAggregator, StreamType};
use crate::proto::{
    agent_message, agent_service_server::AgentService, management_message, AgentMessage,
    ExecOutput, ExecRequest, ManagementMessage, RegistrationAck, SessionQuery, SessionReconcile,
};
use crate::registry::{AgentRegistry, AgentTransportKind};
use crate::startup_executor::StartupExecutor;
use crate::telemetry::{extract_trace_id, generate_trace_id};
use crate::transport_identity::{PeerIdentityEvidence, PeerIdentityMap, SpiffeId, TrustDomain};

#[derive(Clone, Copy, Debug)]
pub struct AgentVsockConnectInfo {
    addr: Option<VsockAddr>,
}

impl AgentVsockConnectInfo {
    pub fn new(addr: Option<VsockAddr>) -> Self {
        Self { addr }
    }

    fn peer_cid(&self) -> Option<u32> {
        self.addr.map(|addr| addr.cid())
    }
}

#[derive(Clone, Debug)]
pub struct AgentMtlsConnectInfo {
    uri_san: Option<String>,
}

impl AgentMtlsConnectInfo {
    pub fn new(uri_san: Option<String>) -> Self {
        Self { uri_san }
    }

    fn uri_san(&self) -> Option<&str> {
        self.uri_san.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentAuthContext {
    agent_id: String,
    peer_identity: Option<SpiffeId>,
    transport_kind: AgentTransportKind,
}

#[derive(Debug, Clone)]
pub struct AgentTransportIdentityResolver {
    trust_domain: TrustDomain,
    peer_map: PeerIdentityMap,
}

impl AgentTransportIdentityResolver {
    pub fn new(trust_domain: TrustDomain, peer_map: PeerIdentityMap) -> Self {
        Self {
            trust_domain,
            peer_map,
        }
    }

    fn uds_peer_identity(&self, uid: u32) -> Result<SpiffeId, Status> {
        self.peer_map
            .peer_identity(
                PeerIdentityEvidence::UdsPeerCred { uid },
                &self.trust_domain,
            )
            .map_err(|e| Status::unauthenticated(format!("Invalid UDS peer identity: {e}")))
    }

    fn vsock_peer_identity(&self, cid: u32) -> Result<SpiffeId, Status> {
        self.peer_map
            .peer_identity(PeerIdentityEvidence::VsockCid { cid }, &self.trust_domain)
            .map_err(|e| Status::unauthenticated(format!("Invalid vsock peer identity: {e}")))
    }

    fn mtls_peer_identity(&self, uri: &str) -> Result<SpiffeId, Status> {
        self.peer_map
            .peer_identity(
                PeerIdentityEvidence::MtlsUriSan {
                    uri: uri.to_string(),
                },
                &self.trust_domain,
            )
            .map_err(|e| Status::unauthenticated(format!("Invalid mTLS peer identity: {e}")))
    }
}

#[derive(Clone)]
pub struct AgentServiceImpl {
    registry: Arc<AgentRegistry>,
    dispatcher: Arc<CommandDispatcher>,
    output_agg: Arc<OutputAggregator>,
    transport_identity: Option<AgentTransportIdentityResolver>,
    /// Executor InstanceRegistry, populated by admin v2 provision and by the
    /// gRPC registration bridge (#317). `None` when the executor surface
    /// wasn't mounted (e.g. TaskStore unavailable); in that case the
    /// `/agents/*` A2A routes already 404 by design.
    instance_registry: Option<agentic_sandbox_executor::instance::InstanceRegistry>,
    /// Per-instance signing-key root for `InstanceContext::new(...)`.
    /// Paired with `instance_registry` — both Some or both None.
    signing_keys_dir: Option<std::path::PathBuf>,
    startup_executor: Option<Arc<StartupExecutor>>,
}

impl AgentServiceImpl {
    pub fn new(
        registry: Arc<AgentRegistry>,
        dispatcher: Arc<CommandDispatcher>,
        output_agg: Arc<OutputAggregator>,
    ) -> Self {
        Self {
            registry,
            dispatcher,
            output_agg,
            transport_identity: None,
            instance_registry: None,
            signing_keys_dir: None,
            startup_executor: None,
        }
    }

    pub fn with_transport_identity_resolver(
        mut self,
        resolver: AgentTransportIdentityResolver,
    ) -> Self {
        self.transport_identity = Some(resolver);
        self
    }

    /// #317: wire the executor InstanceRegistry + signing-key root so that
    /// every gRPC-registered agent becomes a routable v2/A2A instance,
    /// not just admin-v2 provisioned ones. Both must be supplied together;
    /// the pair mirrors the `executor_instance_registry` /
    /// `executor_signing_keys_dir` fields on the HTTP `AppState`.
    pub fn with_executor_registry(
        mut self,
        instance_registry: agentic_sandbox_executor::instance::InstanceRegistry,
        signing_keys_dir: std::path::PathBuf,
    ) -> Self {
        self.instance_registry = Some(instance_registry);
        self.signing_keys_dir = Some(signing_keys_dir);
        self
    }

    pub fn with_startup_executor(mut self, executor: Arc<StartupExecutor>) -> Self {
        self.startup_executor = Some(executor);
        self
    }

    /// Extract authentication from request metadata.
    ///
    /// UDS/vsock/mTLS listeners pass a transport-derived `peer_identity`.
    /// Plain TCP has no transport identity and is rejected. The legacy
    /// `x-agent-secret` compatibility path was retired in #412; agents must
    /// connect over UDS, vsock, or mTLS so identity is derived from transport
    /// evidence instead of bearer metadata.
    #[allow(clippy::result_large_err)] // Status is standard tonic error type
    fn authenticate(
        &self,
        metadata: &MetadataMap,
        peer_identity: Option<(SpiffeId, AgentTransportKind)>,
    ) -> Result<AgentAuthContext, Status> {
        let agent_id = metadata
            .get("x-agent-id")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("Missing x-agent-id header"))?;

        if let Some((peer_identity, transport_kind)) = peer_identity {
            let claimed_instance_id = metadata
                .get("x-agent-instance-id")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    Status::unauthenticated("Missing x-agent-instance-id for transport identity")
                })?;

            if claimed_instance_id != peer_identity.instance_id() {
                return Err(Status::unauthenticated(
                    "Transport identity does not match x-agent-instance-id",
                ));
            }

            return Ok(AgentAuthContext {
                agent_id: agent_id.to_string(),
                peer_identity: Some(peer_identity),
                transport_kind,
            });
        }

        Err(Status::unauthenticated("Agent transport identity required"))
    }

    #[allow(clippy::result_large_err)] // Status is standard tonic error type
    fn peer_identity_for_request<T>(
        &self,
        request: &Request<T>,
    ) -> Result<Option<(SpiffeId, AgentTransportKind)>, Status> {
        let Some(resolver) = self.transport_identity.as_ref() else {
            return Ok(None);
        };

        if let Some(mtls) = request.extensions().get::<AgentMtlsConnectInfo>() {
            let uri = mtls
                .uri_san()
                .ok_or_else(|| Status::unauthenticated("mTLS URI-SAN identity required"))?;
            return resolver
                .mtls_peer_identity(uri)
                .map(|identity| Some((identity, AgentTransportKind::Mtls)));
        }

        if let Some(vsock) = request.extensions().get::<AgentVsockConnectInfo>() {
            let cid = vsock
                .peer_cid()
                .ok_or_else(|| Status::unauthenticated("vsock peer CID required"))?;
            return resolver
                .vsock_peer_identity(cid)
                .map(|identity| Some((identity, AgentTransportKind::Vsock)));
        }

        let Some(uds) = request.extensions().get::<UdsConnectInfo>() else {
            return Ok(None);
        };

        let Some(peer_cred) = uds.peer_cred.as_ref() else {
            return Err(Status::unauthenticated("UDS peer credentials required"));
        };

        resolver
            .uds_peer_identity(peer_cred.uid())
            .map(|identity| Some((identity, AgentTransportKind::Uds)))
    }
}

#[tonic::async_trait]
impl AgentService for AgentServiceImpl {
    type ConnectStream =
        Pin<Box<dyn futures_util::Stream<Item = Result<ManagementMessage, Status>> + Send>>;

    async fn connect(
        &self,
        request: Request<Streaming<AgentMessage>>,
    ) -> Result<Response<Self::ConnectStream>, Status> {
        // Extract or generate trace ID for this connection
        let trace_id = extract_trace_id(&request).unwrap_or_else(generate_trace_id);

        // Authenticate. TCP/h2c requests have no transport identity; UDS
        // requests carry tonic's SO_PEERCRED-derived UdsConnectInfo extension.
        let peer_identity = self.peer_identity_for_request(&request)?;
        let auth = self.authenticate(request.metadata(), peer_identity)?;
        let agent_id = auth.agent_id;
        let transport_kind = auth.transport_kind;
        if let Some(peer_identity) = auth.peer_identity.as_ref() {
            info!(
                trace_id = %trace_id,
                agent_id = %agent_id,
                peer_identity = %peer_identity,
                "Agent connecting with transport identity"
            );
        }

        // Emit connected event (IP will be updated on registration)
        emit_agent_connected(&agent_id, "pending").await;

        let mut inbound = request.into_inner();

        // Create channel for outbound messages to this agent
        let (tx, rx) = mpsc::channel::<ManagementMessage>(100);

        let registry = self.registry.clone();
        let dispatcher = self.dispatcher.clone();
        let output_agg = self.output_agg.clone();
        let instance_registry = self.instance_registry.clone();
        let signing_keys_dir = self.signing_keys_dir.clone();
        let startup_executor = self.startup_executor.clone();
        let agent_id_clone = agent_id.clone();

        // Create span for this connection
        let span =
            tracing::info_span!("agent_connection", trace_id = %trace_id, agent_id = %agent_id);

        // Spawn task to handle inbound messages
        tokio::spawn(
            async move {
                while let Some(msg) = inbound.next().await {
                    match msg {
                        Ok(msg) => {
                            if let Err(e) = handle_agent_message(
                                &registry,
                                &dispatcher,
                                &output_agg,
                                instance_registry.as_ref(),
                                signing_keys_dir.as_deref(),
                                startup_executor.as_ref(),
                                &agent_id_clone,
                                transport_kind,
                                msg,
                                tx.clone(),
                            )
                            .await
                            {
                                error!("Error handling message from {}: {}", agent_id_clone, e);
                            }
                        }
                        Err(e) => {
                            error!("Stream error from {}: {}", agent_id_clone, e);
                            break;
                        }
                    }
                }

                // Agent disconnected - clean up all sessions and pending commands
                emit_agent_disconnected(&agent_id_clone, None).await;
                dispatcher.cleanup_agent(&agent_id_clone);
                // #317: pull the v2 instance_id from the v1 registry BEFORE
                // unregistering, then drop the matching `InstanceContext`
                // from the executor InstanceRegistry. Order matters: the
                // v1 entry owns the instance_id; if we unregister first
                // we lose the mapping and the v2 entry leaks until the
                // next provision.
                let removed_instance_id =
                    registry.get(&agent_id_clone).map(|a| a.instance_id.clone());
                registry.unregister(&agent_id_clone);
                if let (Some(instance_id), Some(inst_reg)) =
                    (removed_instance_id, instance_registry.as_ref())
                {
                    if inst_reg.remove(&instance_id).is_some() {
                        info!(
                            agent_id = %agent_id_clone,
                            instance_id = %instance_id,
                            "removed InstanceContext from executor registry"
                        );
                    }
                }
                info!("Agent disconnected: {}", agent_id_clone);
            }
            .instrument(span),
        );

        // Return outbound stream
        let outbound = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(outbound.map(Ok))))
    }

    type ExecStream = Pin<Box<dyn futures_util::Stream<Item = Result<ExecOutput, Status>> + Send>>;

    async fn exec(
        &self,
        request: Request<ExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        // Extract or generate trace ID for this exec request
        let trace_id = extract_trace_id(&request).unwrap_or_else(generate_trace_id);
        let req = request.into_inner();
        info!(trace_id = %trace_id, agent_id = %req.agent_id, command = %req.command, "Exec request");
        let agent_id = req.agent_id.clone();

        // Block exec while agent is still provisioning
        if let Some(agent) = self.registry.get(&agent_id) {
            if agent.status == crate::proto::AgentStatus::Provisioning {
                let setup_hint = if agent.setup_status.is_empty() {
                    "setup in progress".to_string()
                } else {
                    agent.setup_status.clone()
                };
                return Err(Status::failed_precondition(format!(
                    "Agent {} is still provisioning ({}). Wait for setup to complete.",
                    agent_id, setup_hint
                )));
            }
        }

        let timeout: u32 = if req.timeout_seconds > 0 {
            req.timeout_seconds as u32
        } else {
            300 // Default 5 minutes
        };

        // Dispatch command through dispatcher
        let result = self
            .dispatcher
            .dispatch(
                &agent_id,
                req.command,
                req.args,
                req.working_dir,
                req.env,
                timeout,
            )
            .await;

        match result {
            Ok((_command_id, output_rx)) => {
                let outbound = ReceiverStream::new(output_rx);
                Ok(Response::new(Box::pin(outbound.map(Ok))))
            }
            Err(e) => {
                error!("Dispatch error: {}", e);
                match e {
                    crate::dispatch::DispatchError::AgentNotFound(_) => {
                        Err(Status::not_found(e.to_string()))
                    }
                    _ => Err(Status::internal(e.to_string())),
                }
            }
        }
    }
}

/// Handle incoming message from agent
#[allow(clippy::too_many_arguments)]
async fn handle_agent_message(
    registry: &Arc<AgentRegistry>,
    dispatcher: &Arc<CommandDispatcher>,
    output_agg: &Arc<OutputAggregator>,
    instance_registry: Option<&agentic_sandbox_executor::instance::InstanceRegistry>,
    signing_keys_dir: Option<&std::path::Path>,
    startup_executor: Option<&Arc<StartupExecutor>>,
    agent_id: &str,
    transport_kind: AgentTransportKind,
    msg: AgentMessage,
    tx: mpsc::Sender<ManagementMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match msg.payload {
        Some(agent_message::Payload::Registration(reg)) => {
            info!(
                "Registration from {}: hostname={}, ip={}",
                agent_id, reg.hostname, reg.ip_address
            );

            // Emit agent registered event
            emit_agent_registered(agent_id, &reg.hostname, &reg.ip_address).await;

            // Register agent
            registry.register_with_transport(reg.clone(), tx.clone(), transport_kind);

            // #317: bridge the v1 AgentRegistry entry to the v2/A2A
            // InstanceRegistry so `/agents/{instance_id}/.well-known/agent-card.json`
            // and the rest of the A2A surface resolve for VM-backed agents
            // (and any agent that connects via gRPC without going through
            // the admin v2 provision path). The admin v2 path
            // (`admin_v2.rs`) already does this for its own provisions; the
            // bridge here closes the gap for legacy provision-vm.sh and
            // docker run flows.
            //
            // Use the canonical `instance_id` assigned by ConnectedAgent::new:
            // if the agent supplied one in the Registration message it's
            // reused; otherwise a fresh UUIDv7 is generated server-side
            // (registry.rs:112-116). Either way, that's the id the v1
            // `/api/v1/agents` listing exposes, so binding it here keeps
            // v1 and v2 in lockstep.
            if let (Some(inst_reg), Some(key_dir)) = (instance_registry, signing_keys_dir) {
                if let Some(assigned_instance_id) =
                    registry.get(agent_id).map(|a| a.instance_id.clone())
                {
                    bridge_register_instance(
                        inst_reg,
                        key_dir,
                        agent_id,
                        &assigned_instance_id,
                        &reg.loadout,
                    );
                }
            }

            // Send acknowledgment
            let ack = RegistrationAck {
                accepted: true,
                message: "Welcome to agentic-sandbox".to_string(),
                heartbeat_interval_seconds: 30,
                config: std::collections::HashMap::new(),
            };
            tx.send(ManagementMessage {
                payload: Some(management_message::Payload::RegistrationAck(ack)),
            })
            .await?;

            // Trigger session reconciliation after registration
            info!(agent_id = %agent_id, "Sending session query for reconciliation");
            let report_all = true;
            let query = SessionQuery {
                report_all,
                session_ids: vec![],
            };
            tx.send(ManagementMessage {
                payload: Some(management_message::Payload::SessionQuery(query)),
            })
            .await?;

            // Emit session query sent event
            emit_session_query_sent(agent_id, report_all).await;
        }

        Some(agent_message::Payload::Heartbeat(hb)) => {
            let became_ready = registry.heartbeat(
                agent_id,
                hb.status,
                hb.cpu_percent,
                hb.memory_used_bytes as u64,
                hb.uptime_seconds as u64,
                hb.setup_status,
                hb.setup_progress_json,
            );
            if became_ready {
                if let Some(inst_reg) = instance_registry {
                    if let Some(instance_id) = registry
                        .get(agent_id)
                        .map(|agent| agent.instance_id.clone())
                    {
                        mark_registered_instance_ready(inst_reg, &instance_id);
                    }
                }
                if let Some(executor) = startup_executor.cloned() {
                    if let Some((instance_id, loadout)) = registry.get(agent_id).map(|agent| {
                        (
                            agent.instance_id.clone(),
                            agent.registration.loadout.clone(),
                        )
                    }) {
                        let agent_id = agent_id.to_string();
                        tokio::spawn(async move {
                            executor
                                .handle_agent_ready(&agent_id, &instance_id, &loadout)
                                .await;
                        });
                    }
                }
            }
        }

        Some(agent_message::Payload::Stdout(chunk)) => {
            // Forward to dispatcher first - only broadcast if command exists
            let should_broadcast = dispatcher
                .handle_stdout(&chunk.stream_id, &chunk.stream_id, chunk.data.clone())
                .await;

            if should_broadcast {
                output_agg.push(
                    agent_id.to_string(),
                    chunk.stream_id,
                    StreamType::Stdout,
                    chunk.data,
                );
            }
        }

        Some(agent_message::Payload::Stderr(chunk)) => {
            // Forward to dispatcher first - only broadcast if command exists
            let should_broadcast = dispatcher
                .handle_stderr(&chunk.stream_id, &chunk.stream_id, chunk.data.clone())
                .await;

            if should_broadcast {
                output_agg.push(
                    agent_id.to_string(),
                    chunk.stream_id,
                    StreamType::Stderr,
                    chunk.data,
                );
            }
        }

        Some(agent_message::Payload::Log(chunk)) => {
            // Forward to output aggregator only
            output_agg.push(
                agent_id.to_string(),
                chunk.stream_id,
                StreamType::Log,
                chunk.data,
            );
        }

        Some(agent_message::Payload::CommandResult(result)) => {
            info!(
                "[{}] Command completed: exit={}, success={}, duration={}ms",
                result.command_id, result.exit_code, result.success, result.duration_ms
            );
            // Notify dispatcher
            dispatcher.handle_result(result);
        }

        Some(agent_message::Payload::Metrics(metrics)) => {
            info!(
                "[{}] Metrics: cpu={:.1}%, mem={}MB, disk={}MB",
                agent_id,
                metrics.cpu_percent,
                metrics.memory_used_bytes / 1024 / 1024,
                metrics.disk_used_bytes / 1024 / 1024
            );
            registry.update_metrics(agent_id, &metrics);
            // Push metrics to output aggregator so WS clients get updates
            let metrics_json = serde_json::json!({
                "agent_id": agent_id,
                "cpu_percent": metrics.cpu_percent,
                "memory_used_bytes": metrics.memory_used_bytes,
                "memory_total_bytes": metrics.memory_total_bytes,
                "disk_used_bytes": metrics.disk_used_bytes,
                "disk_total_bytes": metrics.disk_total_bytes,
                "load_avg": metrics.load_avg,
            });
            output_agg.push(
                agent_id.to_string(),
                "__metrics__".to_string(),
                StreamType::Log,
                format!("\x1b[metrics]{}\x1b[/metrics]", metrics_json).into_bytes(),
            );
        }

        Some(agent_message::Payload::SessionReport(report)) => {
            let session_count = report.sessions.len();
            info!(
                agent_id = %agent_id,
                session_count = session_count,
                "Received session report for reconciliation"
            );

            // Emit session report received event
            emit_session_report_received(agent_id, session_count).await;

            // Extract command IDs from reported sessions
            let reported_ids: Vec<String> = report
                .sessions
                .iter()
                .map(|s| s.command_id.clone())
                .collect();

            // Generate reconciliation instruction
            let (keep, kill, kill_unrecognized) =
                dispatcher.reconcile_sessions(agent_id, &reported_ids);

            // Emit reconcile started event
            emit_session_reconcile_started(agent_id, keep.len(), kill.len()).await;

            let reconcile = SessionReconcile {
                keep_session_ids: keep,
                kill_session_ids: kill,
                kill_unrecognized,
                grace_period_seconds: 5,
            };

            tx.send(ManagementMessage {
                payload: Some(management_message::Payload::SessionReconcile(reconcile)),
            })
            .await?;
        }

        Some(agent_message::Payload::SessionReconcileAck(ack)) => {
            let killed_count = ack.killed_session_ids.len();
            let kept_count = ack.kept_session_ids.len();
            let failed_count = ack.failed_to_kill.len();

            info!(
                agent_id = %agent_id,
                killed = killed_count,
                kept = kept_count,
                failed = failed_count,
                "Session reconciliation acknowledged"
            );

            // Emit individual session events
            for session_id in &ack.killed_session_ids {
                emit_session_killed(agent_id, session_id).await;
            }
            for session_id in &ack.kept_session_ids {
                emit_session_preserved(agent_id, session_id).await;
            }
            for session_id in &ack.failed_to_kill {
                emit_session_reconcile_failed(agent_id, session_id, "kill failed").await;
            }

            // Emit reconcile complete summary event
            emit_session_reconcile_complete(agent_id, kept_count, killed_count, failed_count).await;

            dispatcher.handle_reconcile_ack(
                agent_id,
                &ack.killed_session_ids,
                &ack.kept_session_ids,
                &ack.failed_to_kill,
            );
        }

        None => {
            warn!("Received empty message from {}", agent_id);
        }
    }

    Ok(())
}

fn mark_registered_instance_ready(
    inst_reg: &agentic_sandbox_executor::instance::InstanceRegistry,
    instance_id: &str,
) {
    if let Some(ctx) = inst_reg.get(instance_id) {
        ctx.set_ready(true);
    }
}

/// #317: insert a v2 `InstanceContext` into the executor InstanceRegistry
/// based on a freshly-registered v1 agent. Pulled out of the Registration
/// message handler so it can be unit-tested without standing up the full
/// dispatcher / output-aggregator stack.
///
/// Idempotent on duplicate instance_id: if admin v2 already pre-registered
/// the context (or a previous reconnect bridged it), this is a no-op so the
/// cached AgentCard isn't thrown away.
///
/// `loadout` is the `AgentRegistration.loadout` field. Empty loadout signals
/// a Docker container connecting without going through admin v2 — the legacy
/// container path; non-empty loadout signals a VM provisioned via
/// `provision-vm.sh` (cloud-init always materializes a loadout).
fn bridge_register_instance(
    inst_reg: &agentic_sandbox_executor::instance::InstanceRegistry,
    signing_keys_dir: &std::path::Path,
    agent_id: &str,
    instance_id: &str,
    loadout: &str,
) {
    if inst_reg.get(instance_id).is_some() {
        // Already registered (admin v2 happy path, or a prior reconnect).
        return;
    }
    let runtime_kind = classify_registered_runtime(agent_id, loadout);
    let loadout_for_ctx = if loadout.is_empty() {
        "agentic-dev".to_string()
    } else {
        loadout.to_string()
    };
    match agentic_sandbox_executor::instance::InstanceContext::new(
        instance_id.to_string(),
        runtime_kind,
        loadout_for_ctx,
        None,
        "executor.local".to_string(),
        signing_keys_dir,
    ) {
        Ok(ctx) => {
            inst_reg.insert(std::sync::Arc::new(ctx));
            info!(
                agent_id = %agent_id,
                instance_id = %instance_id,
                "registered InstanceContext in executor registry (gRPC bridge #317)"
            );
        }
        Err(e) => {
            warn!(
                agent_id = %agent_id,
                instance_id = %instance_id,
                error = %e,
                "failed to build InstanceContext from gRPC registration; /agents/{instance_id}/* will 404"
            );
        }
    }
}

fn classify_registered_runtime(
    agent_id: &str,
    loadout: &str,
) -> agentic_sandbox_executor::instance::RuntimeKind {
    if !loadout.is_empty() || libvirt_domain_exists(agent_id) {
        agentic_sandbox_executor::instance::RuntimeKind::Vm
    } else {
        agentic_sandbox_executor::instance::RuntimeKind::Container
    }
}

fn libvirt_domain_exists(agent_id: &str) -> bool {
    let Ok(conn) = Connect::open(Some("qemu:///system")) else {
        return false;
    };
    let Ok(domains) = conn.list_all_domains(0) else {
        return false;
    };
    domains
        .into_iter()
        .any(|domain| domain.get_name().as_deref() == Ok(agent_id))
}

#[cfg(test)]
mod tests {
    //! #317 coverage: gRPC-registered agents (VM and legacy Docker) must
    //! become routable v2/A2A instances. These tests exercise the bridge
    //! helper directly; the full gRPC connect flow is exercised at the
    //! integration test layer.
    use super::*;
    use agentic_sandbox_executor::instance::{InstanceRegistry, RuntimeKind};
    use tonic::Code;

    fn fresh_keys_dir(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("aiwg-test-{}-", label))
            .tempdir()
            .expect("create temp signing-keys dir")
    }

    fn fresh_agent_service() -> (AgentServiceImpl, tempfile::TempDir) {
        let secrets_dir = fresh_keys_dir("secrets");
        let registry = Arc::new(AgentRegistry::new());
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));
        let output_agg = Arc::new(OutputAggregator::new(16));
        (
            AgentServiceImpl::new(registry, dispatcher, output_agg),
            secrets_dir,
        )
    }

    fn auth_metadata(agent_id: &str, secret: Option<&str>) -> MetadataMap {
        let mut metadata = MetadataMap::new();
        metadata.insert("x-agent-id", agent_id.parse().unwrap());
        if let Some(secret) = secret {
            metadata.insert("x-agent-secret", secret.parse().unwrap());
        }
        metadata
    }

    fn auth_metadata_with_instance(agent_id: &str, instance_id: &str) -> MetadataMap {
        let mut metadata = auth_metadata(agent_id, None);
        metadata.insert("x-agent-instance-id", instance_id.parse().unwrap());
        metadata
    }

    fn test_spiffe_id() -> SpiffeId {
        SpiffeId::parse("spiffe://sandbox.example/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1")
            .unwrap()
    }

    fn mtls_peer(identity: SpiffeId) -> Option<(SpiffeId, AgentTransportKind)> {
        Some((identity, AgentTransportKind::Mtls))
    }

    fn transport_resolver_with_vsock_cid(
        cid: u32,
        instance_id: &str,
    ) -> AgentTransportIdentityResolver {
        let mut peer_map = PeerIdentityMap::new();
        peer_map.register_vsock_cid(cid, instance_id).unwrap();
        AgentTransportIdentityResolver::new(TrustDomain::new("sandbox.example").unwrap(), peer_map)
    }

    fn transport_resolver_for_mtls() -> AgentTransportIdentityResolver {
        AgentTransportIdentityResolver::new(
            TrustDomain::new("sandbox.example").unwrap(),
            PeerIdentityMap::new(),
        )
    }

    #[test]
    fn transport_identity_resolver_maps_uds_uid_to_spiffe_identity() {
        let mut peer_map = PeerIdentityMap::new();
        peer_map
            .register_uds_uid(1001, "018fb9f1-3291-7a73-b261-c7de8a2af4d1")
            .unwrap();
        let resolver = AgentTransportIdentityResolver::new(
            TrustDomain::new("sandbox.example").unwrap(),
            peer_map,
        );

        let id = resolver.uds_peer_identity(1001).unwrap();

        assert_eq!(id.instance_id(), "018fb9f1-3291-7a73-b261-c7de8a2af4d1");
        assert_eq!(
            id.as_str(),
            "spiffe://sandbox.example/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1"
        );
        let err = resolver.uds_peer_identity(2002).unwrap_err();
        assert_eq!(err.code(), Code::Unauthenticated);
        assert_eq!(
            err.message(),
            "Invalid UDS peer identity: unknown UDS uid: 2002"
        );
    }

    #[test]
    fn authenticate_rejects_legacy_secret_metadata_without_transport_identity() {
        let (service, _dir) = fresh_agent_service();
        let metadata = auth_metadata("agent-01", Some("s3cr3t"));

        let err = service.authenticate(&metadata, None).unwrap_err();

        assert_eq!(err.code(), Code::Unauthenticated);
        assert_eq!(err.message(), "Agent transport identity required");
    }

    #[test]
    fn authenticate_requires_agent_id_for_both_auth_modes() {
        let (service, _dir) = fresh_agent_service();
        let metadata = MetadataMap::new();

        let err = service
            .authenticate(&metadata, mtls_peer(test_spiffe_id()))
            .unwrap_err();

        assert_eq!(err.code(), Code::Unauthenticated);
        assert_eq!(err.message(), "Missing x-agent-id header");
    }

    #[test]
    fn authenticate_accepts_transport_peer_identity_without_shared_secret() {
        let (service, _dir) = fresh_agent_service();
        let peer_identity = test_spiffe_id();
        let metadata = auth_metadata_with_instance("agent-01", peer_identity.instance_id());

        let auth = service
            .authenticate(&metadata, mtls_peer(peer_identity.clone()))
            .unwrap();

        assert_eq!(auth.agent_id, "agent-01");
        assert_eq!(auth.peer_identity, Some(peer_identity));
    }

    #[test]
    fn authenticate_accepts_transport_identity_when_compat_disabled() {
        let (service, _dir) = fresh_agent_service();
        let peer_identity = test_spiffe_id();
        let metadata = auth_metadata_with_instance("agent-01", peer_identity.instance_id());

        let auth = service
            .authenticate(&metadata, mtls_peer(peer_identity.clone()))
            .unwrap();

        assert_eq!(auth.agent_id, "agent-01");
        assert_eq!(auth.peer_identity, Some(peer_identity));
    }

    #[test]
    fn phase3_acceptance_transport_identity_replaces_legacy_secret_path() {
        let (service, _dir) = fresh_agent_service();

        let legacy_err = service
            .authenticate(&auth_metadata("agent-01", Some("s3cr3t")), None)
            .unwrap_err();

        assert_eq!(legacy_err.code(), Code::Unauthenticated);
        assert_eq!(legacy_err.message(), "Agent transport identity required");

        let peer_identity = test_spiffe_id();
        let transport = service
            .authenticate(
                &auth_metadata_with_instance("agent-02", peer_identity.instance_id()),
                mtls_peer(peer_identity.clone()),
            )
            .unwrap();

        assert_eq!(transport.peer_identity, Some(peer_identity));
    }

    #[test]
    fn phase1_acceptance_transport_identity_ignores_legacy_secret_metadata() {
        let (service, _dir) = fresh_agent_service();
        let peer_identity = test_spiffe_id();
        let mut metadata = auth_metadata_with_instance("agent-01", peer_identity.instance_id());
        metadata.insert("x-agent-secret", "wrong-secret".parse().unwrap());

        let auth = service
            .authenticate(&metadata, mtls_peer(peer_identity.clone()))
            .unwrap();

        assert_eq!(auth.agent_id, "agent-01");
        assert_eq!(auth.peer_identity, Some(peer_identity));
    }

    #[test]
    fn phase3_acceptance_rejects_legacy_secret_by_default() {
        let (service, _dir) = fresh_agent_service();

        let err = service
            .authenticate(&auth_metadata("agent-01", Some("s3cr3t")), None)
            .unwrap_err();

        assert_eq!(err.code(), Code::Unauthenticated);
        assert_eq!(err.message(), "Agent transport identity required");
    }

    #[test]
    fn phase3_acceptance_rejects_unknown_legacy_agent_without_tofu() {
        let (service, _dir) = fresh_agent_service();

        let err = service
            .authenticate(&auth_metadata("agent-unknown", Some("first-secret")), None)
            .unwrap_err();

        assert_eq!(err.code(), Code::Unauthenticated);
        assert_eq!(err.message(), "Agent transport identity required");
    }

    #[test]
    fn peer_identity_for_request_maps_vsock_peer_cid() {
        const INSTANCE_ID: &str = "018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let (svc, _dir) = fresh_agent_service();
        let svc = svc
            .with_transport_identity_resolver(transport_resolver_with_vsock_cid(42, INSTANCE_ID));
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(AgentVsockConnectInfo::new(Some(VsockAddr::new(42, 1234))));

        let (identity, transport_kind) = svc.peer_identity_for_request(&request).unwrap().unwrap();

        assert_eq!(identity.instance_id(), INSTANCE_ID);
        assert_eq!(transport_kind, AgentTransportKind::Vsock);
    }

    #[test]
    fn peer_identity_for_request_rejects_unknown_vsock_peer_cid() {
        const INSTANCE_ID: &str = "018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let (svc, _dir) = fresh_agent_service();
        let svc = svc
            .with_transport_identity_resolver(transport_resolver_with_vsock_cid(42, INSTANCE_ID));
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(AgentVsockConnectInfo::new(Some(VsockAddr::new(43, 1234))));

        let err = svc.peer_identity_for_request(&request).unwrap_err();

        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert!(err.message().contains("Invalid vsock peer identity"));
    }

    #[test]
    fn peer_identity_for_request_maps_mtls_uri_san() {
        const INSTANCE_ID: &str = "018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let (svc, _dir) = fresh_agent_service();
        let svc = svc.with_transport_identity_resolver(transport_resolver_for_mtls());
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(AgentMtlsConnectInfo::new(Some(format!(
                "spiffe://sandbox.example/agent/{INSTANCE_ID}"
            ))));

        let (identity, transport_kind) = svc.peer_identity_for_request(&request).unwrap().unwrap();

        assert_eq!(identity.instance_id(), INSTANCE_ID);
        assert_eq!(identity.trust_domain().as_str(), "sandbox.example");
        assert_eq!(transport_kind, AgentTransportKind::Mtls);
    }

    #[test]
    fn peer_identity_for_request_rejects_mtls_without_uri_san() {
        let (svc, _dir) = fresh_agent_service();
        let svc = svc.with_transport_identity_resolver(transport_resolver_for_mtls());
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(AgentMtlsConnectInfo::new(None));

        let err = svc.peer_identity_for_request(&request).unwrap_err();

        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(err.message(), "mTLS URI-SAN identity required");
    }

    #[test]
    fn peer_identity_for_request_rejects_invalid_mtls_uri_san() {
        let (svc, _dir) = fresh_agent_service();
        let svc = svc.with_transport_identity_resolver(transport_resolver_for_mtls());
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(AgentMtlsConnectInfo::new(Some("https://not-spiffe".into())));

        let err = svc.peer_identity_for_request(&request).unwrap_err();

        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert!(err.message().contains("Invalid mTLS peer identity"));
    }

    #[test]
    fn authenticate_rejects_transport_identity_instance_mismatch() {
        let (service, _dir) = fresh_agent_service();
        let peer_identity = test_spiffe_id();
        let metadata =
            auth_metadata_with_instance("agent-01", "018fb9f2-94a1-7c2d-b0c4-01fd58bb5ec1");

        let err = service
            .authenticate(&metadata, mtls_peer(peer_identity))
            .unwrap_err();

        assert_eq!(err.code(), Code::Unauthenticated);
        assert_eq!(
            err.message(),
            "Transport identity does not match x-agent-instance-id"
        );
    }

    #[test]
    fn bridge_registers_vm_instance_with_loadout_kind() {
        let reg = InstanceRegistry::new();
        let keys = fresh_keys_dir("vm");
        let instance_id = "019e4392-7e61-7582-8d91-936096a14c8a";

        bridge_register_instance(
            &reg,
            keys.path(),
            "agentops-m011-codex-smoke",
            instance_id,
            "profiles/codex-only.yaml",
        );

        let ctx = reg
            .get(instance_id)
            .expect("VM-provisioned agent must land in InstanceRegistry (#317)");
        assert_eq!(ctx.runtime_kind, RuntimeKind::Vm);
        assert_eq!(ctx.loadout, "profiles/codex-only.yaml");
        assert_eq!(ctx.instance_id, instance_id);
    }

    #[test]
    fn bridge_registers_container_instance_when_loadout_empty() {
        let reg = InstanceRegistry::new();
        let keys = fresh_keys_dir("docker");
        let instance_id = "019e4392-7e61-7582-8d91-936096a14c8b";

        bridge_register_instance(&reg, keys.path(), "legacy-docker-agent", instance_id, "");

        let ctx = reg
            .get(instance_id)
            .expect("legacy Docker agents must also bridge into InstanceRegistry");
        assert_eq!(ctx.runtime_kind, RuntimeKind::Container);
        assert_eq!(
            ctx.loadout, "agentic-dev",
            "empty loadout falls back to agentic-dev default"
        );
    }

    #[test]
    fn bridge_is_idempotent_on_duplicate_instance_id() {
        // Admin v2 pre-registered the InstanceContext, then the agent
        // dialed back over gRPC; the bridge must NOT overwrite the
        // existing entry, which would invalidate the cached AgentCard.
        let reg = InstanceRegistry::new();
        let keys = fresh_keys_dir("idem");
        let instance_id = "019e4392-7e61-7582-8d91-936096a14c8c";

        bridge_register_instance(&reg, keys.path(), "agent-1", instance_id, "vm-loadout");
        let first = reg.get(instance_id).expect("first insert lands");
        let first_ptr = std::sync::Arc::as_ptr(&first) as usize;

        bridge_register_instance(
            &reg,
            keys.path(),
            "agent-1",
            instance_id,
            "different-loadout",
        );
        let second = reg.get(instance_id).expect("second call must not erase");
        let second_ptr = std::sync::Arc::as_ptr(&second) as usize;

        assert_eq!(
            first_ptr, second_ptr,
            "idempotent bridge must preserve original InstanceContext"
        );
        assert_eq!(
            second.loadout, "vm-loadout",
            "original loadout preserved across duplicate insert"
        );
    }

    #[test]
    fn ready_heartbeat_marks_preregistered_context_ready_without_replacing_it() {
        let reg = InstanceRegistry::new();
        let keys = fresh_keys_dir("ready");
        let instance_id = "019e4392-7e61-7582-8d91-936096a14c8d";
        bridge_register_instance(&reg, keys.path(), "admin-v2-docker", instance_id, "");
        let ctx = reg.get(instance_id).expect("first insert lands");
        ctx.set_ready(false);
        let original_ptr = std::sync::Arc::as_ptr(&ctx) as usize;

        mark_registered_instance_ready(&reg, instance_id);

        let updated = reg.get(instance_id).expect("context still registered");
        assert!(
            updated.is_ready(),
            "Ready heartbeat should make context routable"
        );
        assert_eq!(
            original_ptr,
            std::sync::Arc::as_ptr(&updated) as usize,
            "readiness update must not replace the signed InstanceContext"
        );
    }
}
