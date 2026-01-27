//! WebSocket connection handler

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tracing::{debug, error, info, warn};

use crate::dispatch::CommandDispatcher;
use crate::output::{OutputAggregator, OutputMessage, StreamType};
use crate::registry::AgentRegistry;

/// Client-to-server WebSocket message
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Subscribe to an agent's output (agent_id = "*" for all)
    Subscribe { agent_id: String },
    /// Unsubscribe from an agent's output
    Unsubscribe { agent_id: String },
    /// Ping for keepalive
    Ping { timestamp: i64 },
    /// Send input to agent stdin
    SendInput {
        agent_id: String,
        command_id: String,
        data: String,
    },
    /// Execute a command on an agent
    SendCommand {
        agent_id: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Request list of connected agents
    ListAgents,
}

/// Server-to-client WebSocket message
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Output from an agent
    Output {
        agent_id: String,
        command_id: String,
        stream: String,
        data: String,
        ts: i64,
    },
    /// Subscription confirmed
    Subscribed { agent_id: String },
    /// Unsubscription confirmed
    Unsubscribed { agent_id: String },
    /// Pong response
    Pong { timestamp: i64 },
    /// Error message
    Error { message: String },
    /// List of connected agents
    AgentList { agents: Vec<AgentInfoWs> },
    /// Input sent confirmation
    InputSent { agent_id: String, command_id: String },
    /// Command started
    CommandStarted {
        agent_id: String,
        command_id: String,
        command: String,
    },
}

/// Agent info for WebSocket responses
#[derive(Debug, Serialize)]
pub struct AgentInfoWs {
    pub id: String,
    pub hostname: String,
    pub ip_address: String,
    pub status: String,
    pub connected_at: i64,
    pub last_heartbeat: i64,
}

/// Represents a WebSocket client connection
pub struct WsConnection {
    id: String,
    /// Subscribed agents (empty = none, ["*"] = all)
    subscriptions: Vec<String>,
    registry: Arc<AgentRegistry>,
    dispatcher: Arc<CommandDispatcher>,
}

impl WsConnection {
    pub fn new(
        id: String,
        registry: Arc<AgentRegistry>,
        dispatcher: Arc<CommandDispatcher>,
    ) -> Self {
        Self {
            id,
            subscriptions: Vec::new(),
            registry,
            dispatcher,
        }
    }

    /// Handle a WebSocket connection
    pub async fn handle(
        id: String,
        ws: WebSocketStream<TcpStream>,
        output_agg: Arc<OutputAggregator>,
        registry: Arc<AgentRegistry>,
        dispatcher: Arc<CommandDispatcher>,
    ) {
        let (mut ws_tx, mut ws_rx) = ws.split();
        let (msg_tx, mut msg_rx) = mpsc::channel::<ServerMessage>(100);

        let mut conn = WsConnection::new(id.clone(), registry, dispatcher);

        info!("WebSocket client connected: {}", id);

        // Spawn task to forward output messages to client
        let output_agg_clone = output_agg.clone();
        let msg_tx_clone = msg_tx.clone();
        let id_clone = id.clone();
        let subscriptions_handle = tokio::spawn(async move {
            let mut subscription = output_agg_clone.subscribe(None, None);
            loop {
                match subscription.recv().await {
                    Some(msg) => {
                        // Will be filtered by the main loop based on subscriptions
                        let server_msg = output_to_server_message(&msg);
                        if msg_tx_clone.send(server_msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            debug!("Output subscription ended for {}", id_clone);
        });

        // Spawn task to send messages to WebSocket
        let id_clone2 = id.clone();
        let send_task = tokio::spawn(async move {
            while let Some(msg) = msg_rx.recv().await {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Failed to serialize message: {}", e);
                        continue;
                    }
                };
                if ws_tx.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            debug!("Send task ended for {}", id_clone2);
        });

        // Main receive loop
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(client_msg) => {
                            let response = conn.handle_message(client_msg).await;
                            if msg_tx.send(response).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let err = ServerMessage::Error {
                                message: format!("Invalid message: {}", e),
                            };
                            if msg_tx.send(err).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                Ok(Message::Ping(_data)) => {
                    // WebSocket-level ping, respond with pong
                    debug!("WS ping from {}", id);
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket client {} sent close", id);
                    break;
                }
                Err(e) => {
                    error!("WebSocket error from {}: {}", id, e);
                    break;
                }
                _ => {}
            }
        }

        // Cleanup
        subscriptions_handle.abort();
        send_task.abort();
        info!("WebSocket client disconnected: {}", id);
    }

    /// Handle a client message and return response
    async fn handle_message(&mut self, msg: ClientMessage) -> ServerMessage {
        match msg {
            ClientMessage::Subscribe { agent_id } => {
                if !self.subscriptions.contains(&agent_id) {
                    self.subscriptions.push(agent_id.clone());
                }
                info!("Client {} subscribed to {}", self.id, agent_id);
                ServerMessage::Subscribed { agent_id }
            }
            ClientMessage::Unsubscribe { agent_id } => {
                self.subscriptions.retain(|a| a != &agent_id);
                info!("Client {} unsubscribed from {}", self.id, agent_id);
                ServerMessage::Unsubscribed { agent_id }
            }
            ClientMessage::Ping { timestamp } => ServerMessage::Pong { timestamp },

            ClientMessage::ListAgents => {
                let agents: Vec<AgentInfoWs> = self
                    .registry
                    .list_agents()
                    .into_iter()
                    .map(|a| AgentInfoWs {
                        id: a.id,
                        hostname: a.hostname,
                        ip_address: a.ip_address,
                        status: format!("{:?}", a.status),
                        connected_at: a.connected_at,
                        last_heartbeat: a.last_heartbeat,
                    })
                    .collect();
                info!("Client {} requested agent list ({} agents)", self.id, agents.len());
                ServerMessage::AgentList { agents }
            }

            ClientMessage::SendInput { agent_id, command_id, data } => {
                info!("Client {} sending input to {}:{}", self.id, agent_id, command_id);
                match self.dispatcher.send_stdin(&command_id, data.into_bytes()).await {
                    Ok(_) => ServerMessage::InputSent { agent_id, command_id },
                    Err(e) => {
                        warn!("Failed to send input: {}", e);
                        ServerMessage::Error {
                            message: format!("Failed to send input: {}", e),
                        }
                    }
                }
            }

            ClientMessage::SendCommand { agent_id, command, args } => {
                info!("Client {} sending command to {}: {}", self.id, agent_id, command);
                use std::collections::HashMap;
                match self.dispatcher.dispatch(
                    &agent_id,
                    command.clone(),
                    args,
                    String::new(), // working_dir
                    HashMap::new(), // env
                    0, // timeout_secs (no timeout)
                ).await {
                    Ok((command_id, _rx)) => ServerMessage::CommandStarted {
                        agent_id,
                        command_id,
                        command,
                    },
                    Err(e) => {
                        warn!("Failed to dispatch command: {}", e);
                        ServerMessage::Error {
                            message: format!("Failed to send command: {}", e),
                        }
                    }
                }
            }
        }
    }

    /// Check if connection is subscribed to a given agent
    #[allow(dead_code)]
    pub fn is_subscribed_to(&self, agent_id: &str) -> bool {
        self.subscriptions.contains(&"*".to_string())
            || self.subscriptions.contains(&agent_id.to_string())
    }
}

/// Convert OutputMessage to ServerMessage
fn output_to_server_message(msg: &OutputMessage) -> ServerMessage {
    let stream = match msg.stream_type {
        StreamType::Stdout => "stdout",
        StreamType::Stderr => "stderr",
        StreamType::Log => "log",
    };

    ServerMessage::Output {
        agent_id: msg.agent_id.clone(),
        command_id: msg.command_id.clone(),
        stream: stream.to_string(),
        data: String::from_utf8_lossy(&msg.data).to_string(),
        ts: msg.timestamp,
    }
}
