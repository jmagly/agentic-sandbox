//! gRPC service implementation

use std::pin::Pin;
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{error, info, warn, Instrument};

use crate::auth::SecretStore;
use crate::dispatch::CommandDispatcher;
use crate::output::{OutputAggregator, StreamType};
use crate::proto::{
    agent_message, agent_service_server::AgentService, management_message, AgentMessage,
    ExecOutput, ExecRequest, ManagementMessage, RegistrationAck,
};
use crate::registry::AgentRegistry;
use crate::telemetry::{extract_trace_id, generate_trace_id, TraceId};

pub struct AgentServiceImpl {
    registry: Arc<AgentRegistry>,
    secrets: Arc<SecretStore>,
    dispatcher: Arc<CommandDispatcher>,
    output_agg: Arc<OutputAggregator>,
}

impl AgentServiceImpl {
    pub fn new(
        registry: Arc<AgentRegistry>,
        secrets: Arc<SecretStore>,
        dispatcher: Arc<CommandDispatcher>,
        output_agg: Arc<OutputAggregator>,
    ) -> Self {
        Self {
            registry,
            secrets,
            dispatcher,
            output_agg,
        }
    }

    /// Extract authentication from request metadata
    fn authenticate(&self, request: &Request<Streaming<AgentMessage>>) -> Result<String, Status> {
        let metadata = request.metadata();

        let agent_id = metadata
            .get("x-agent-id")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("Missing x-agent-id header"))?;

        let secret = metadata
            .get("x-agent-secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !self.secrets.verify(agent_id, secret) {
            return Err(Status::unauthenticated("Invalid agent secret"));
        }

        Ok(agent_id.to_string())
    }
}

#[tonic::async_trait]
impl AgentService for AgentServiceImpl {
    type ConnectStream = Pin<Box<dyn futures_util::Stream<Item = Result<ManagementMessage, Status>> + Send>>;

    async fn connect(
        &self,
        request: Request<Streaming<AgentMessage>>,
    ) -> Result<Response<Self::ConnectStream>, Status> {
        // Extract or generate trace ID for this connection
        let trace_id = extract_trace_id(&request).unwrap_or_else(generate_trace_id);

        // Authenticate
        let agent_id = self.authenticate(&request)?;
        info!(trace_id = %trace_id, "Agent connecting: {}", agent_id);

        let mut inbound = request.into_inner();

        // Create channel for outbound messages to this agent
        let (tx, rx) = mpsc::channel::<ManagementMessage>(100);

        let registry = self.registry.clone();
        let dispatcher = self.dispatcher.clone();
        let output_agg = self.output_agg.clone();
        let agent_id_clone = agent_id.clone();

        // Create span for this connection
        let span = tracing::info_span!("agent_connection", trace_id = %trace_id, agent_id = %agent_id);

        // Spawn task to handle inbound messages
        tokio::spawn(async move {
            while let Some(msg) = inbound.next().await {
                match msg {
                    Ok(msg) => {
                        if let Err(e) = handle_agent_message(
                            &registry,
                            &dispatcher,
                            &output_agg,
                            &agent_id_clone,
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

            // Agent disconnected
            registry.unregister(&agent_id_clone);
            info!("Agent disconnected: {}", agent_id_clone);
        }.instrument(span));

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
async fn handle_agent_message(
    registry: &Arc<AgentRegistry>,
    dispatcher: &Arc<CommandDispatcher>,
    output_agg: &Arc<OutputAggregator>,
    agent_id: &str,
    msg: AgentMessage,
    tx: mpsc::Sender<ManagementMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match msg.payload {
        Some(agent_message::Payload::Registration(reg)) => {
            info!(
                "Registration from {}: hostname={}, ip={}",
                agent_id, reg.hostname, reg.ip_address
            );

            // Register agent
            registry.register(reg, tx.clone());

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
        }

        Some(agent_message::Payload::Heartbeat(hb)) => {
            registry.heartbeat(
                agent_id,
                hb.status,
                hb.cpu_percent,
                hb.memory_used_bytes as u64,
                hb.uptime_seconds as u64,
            );
        }

        Some(agent_message::Payload::Stdout(chunk)) => {
            // Forward to dispatcher and output aggregator
            dispatcher
                .handle_stdout(&chunk.stream_id, &chunk.stream_id, chunk.data.clone())
                .await;
            output_agg.push(
                agent_id.to_string(),
                chunk.stream_id,
                StreamType::Stdout,
                chunk.data,
            );
        }

        Some(agent_message::Payload::Stderr(chunk)) => {
            // Forward to dispatcher and output aggregator
            dispatcher
                .handle_stderr(&chunk.stream_id, &chunk.stream_id, chunk.data.clone())
                .await;
            output_agg.push(
                agent_id.to_string(),
                chunk.stream_id,
                StreamType::Stderr,
                chunk.data,
            );
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

        None => {
            warn!("Received empty message from {}", agent_id);
        }
    }

    Ok(())
}
