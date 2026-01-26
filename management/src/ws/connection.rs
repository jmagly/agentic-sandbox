//! WebSocket connection handler

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tracing::{debug, error, info};

use crate::output::{OutputAggregator, OutputMessage, StreamType};

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
}

/// Represents a WebSocket client connection
pub struct WsConnection {
    id: String,
    /// Subscribed agents (empty = none, ["*"] = all)
    subscriptions: Vec<String>,
}

impl WsConnection {
    pub fn new(id: String) -> Self {
        Self {
            id,
            subscriptions: Vec::new(),
        }
    }

    /// Handle a WebSocket connection
    pub async fn handle(
        id: String,
        ws: WebSocketStream<TcpStream>,
        output_agg: Arc<OutputAggregator>,
    ) {
        let (mut ws_tx, mut ws_rx) = ws.split();
        let (msg_tx, mut msg_rx) = mpsc::channel::<ServerMessage>(100);

        let mut conn = WsConnection::new(id.clone());

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
                            let response = conn.handle_message(client_msg);
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
    fn handle_message(&mut self, msg: ClientMessage) -> ServerMessage {
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
