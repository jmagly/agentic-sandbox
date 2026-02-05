//! WebSocket connection handler
#![allow(dead_code)] // Some methods reserved for future UI integration

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tracing::{debug, error, info, warn};

use crate::dispatch::CommandDispatcher;
use crate::http::events::emit_pty_created;
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
    /// Start an interactive shell (PTY) on an agent
    StartShell {
        agent_id: String,
        #[serde(default = "default_cols")]
        cols: u32,
        #[serde(default = "default_rows")]
        rows: u32,
    },
    /// Resize PTY terminal
    PtyResize {
        agent_id: String,
        command_id: String,
        cols: u32,
        rows: u32,
    },
    /// Request list of connected agents
    ListAgents,
    /// List all sessions for an agent
    ListSessions { agent_id: String },
    /// Attach to existing session
    AttachSession {
        agent_id: String,
        session_name: String,
        #[serde(default = "default_cols")]
        cols: u32,
        #[serde(default = "default_rows")]
        rows: u32,
    },
    /// Detach from session (session continues running)
    DetachSession {
        agent_id: String,
        session_name: String,
    },
    /// Kill a session
    KillSession {
        agent_id: String,
        session_name: String,
        #[serde(default)]
        signal: Option<i32>,
    },
    /// Create a new session with specific type
    CreateSession {
        agent_id: String,
        session_name: String,
        session_type: String,  // "interactive", "headless", "background"
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        working_dir: Option<String>,  // defaults to ~ if not specified
        #[serde(default = "default_cols")]
        cols: u32,
        #[serde(default = "default_rows")]
        rows: u32,
    },
}

fn default_cols() -> u32 { 80 }
fn default_rows() -> u32 { 24 }

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
    /// Interactive shell started
    ShellStarted {
        agent_id: String,
        command_id: String,
    },
    /// Metrics update from agent
    MetricsUpdate {
        agent_id: String,
        cpu_percent: f32,
        memory_used_bytes: u64,
        memory_total_bytes: u64,
        disk_used_bytes: u64,
        disk_total_bytes: u64,
        load_avg: Vec<f32>,
        uptime_seconds: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        cpu_cores: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        os: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        kernel: Option<String>,
    },
    /// List of sessions for an agent
    SessionList {
        agent_id: String,
        sessions: Vec<SessionInfoWs>,
    },
    /// Session attached
    SessionAttached {
        agent_id: String,
        session_name: String,
        command_id: String,
    },
    /// Session detached
    SessionDetached {
        agent_id: String,
        session_name: String,
    },
    /// Session killed
    SessionKilled {
        agent_id: String,
        session_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },
    /// Session created
    SessionCreated {
        agent_id: String,
        session_name: String,
        session_type: String,
        command_id: String,
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

/// Session info for WebSocket responses
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfoWs {
    pub session_name: String,
    pub command_id: String,
    pub session_type: String,
    pub command: String,
    pub running: bool,
}

/// Represents a WebSocket client connection
pub struct WsConnection {
    id: String,
    /// Subscribed agents (empty = none, ["*"] = all)
    /// Shared with the output forwarding task for filtering
    subscriptions: Arc<RwLock<Vec<String>>>,
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
            subscriptions: Arc::new(RwLock::new(Vec::new())),
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

        // Spawn task to forward output messages to client (filtered by subscriptions)
        let output_agg_clone = output_agg.clone();
        let msg_tx_clone = msg_tx.clone();
        let id_clone = id.clone();
        let subs_clone = conn.subscriptions.clone();
        let registry_clone = conn.registry.clone();
        #[allow(clippy::while_let_loop)] // Match provides clearer intent for async channel
        let subscriptions_handle = tokio::spawn(async move {
            let mut subscription = output_agg_clone.subscribe(None, None);
            loop {
                match subscription.recv().await {
                    Some(msg) => {
                        // Check if client is subscribed to this agent's output
                        let subs = subs_clone.read().await;
                        let subscribed = subs.contains(&"*".to_string())
                            || subs.contains(&msg.agent_id);
                        drop(subs);

                        if !subscribed {
                            continue;
                        }

                        // Check if this is a metrics update (special command_id)
                        let server_msg = if msg.command_id == "__metrics__" {
                            // Parse metrics from the tagged data
                            let data_str = String::from_utf8_lossy(&msg.data);
                            if let Some(json_str) = data_str
                                .strip_prefix("\x1b[metrics]")
                                .and_then(|s| s.strip_suffix("\x1b[/metrics]"))
                            {
                                if let Ok(m) = serde_json::from_str::<serde_json::Value>(json_str) {
                                    // Get system info from registry
                                    let agent = registry_clone.get(&msg.agent_id);
                                    let sys = agent.as_ref().and_then(|a| a.system_info.as_ref());
                                    let metrics = agent.as_ref().and_then(|a| a.metrics.as_ref());
                                    ServerMessage::MetricsUpdate {
                                        agent_id: msg.agent_id.clone(),
                                        cpu_percent: m["cpu_percent"].as_f64().unwrap_or(0.0) as f32,
                                        memory_used_bytes: m["memory_used_bytes"].as_u64().unwrap_or(0),
                                        memory_total_bytes: m["memory_total_bytes"].as_u64().unwrap_or(0),
                                        disk_used_bytes: m["disk_used_bytes"].as_u64().unwrap_or(0),
                                        disk_total_bytes: m["disk_total_bytes"].as_u64().unwrap_or(0),
                                        load_avg: m["load_avg"].as_array()
                                            .map(|a| a.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect())
                                            .unwrap_or_default(),
                                        uptime_seconds: metrics.map_or(0, |m| m.uptime_seconds),
                                        cpu_cores: sys.map(|s| s.cpu_cores),
                                        os: sys.map(|s| s.os.clone()),
                                        kernel: sys.map(|s| s.kernel.clone()),
                                    }
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        } else {
                            output_to_server_message(&msg)
                        };

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
                if ws_tx.send(Message::Text(json)).await.is_err() {
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
                let mut subs = self.subscriptions.write().await;
                if !subs.contains(&agent_id) {
                    subs.push(agent_id.clone());
                }
                info!("Client {} subscribed to {} (active: {:?})", self.id, agent_id, *subs);
                ServerMessage::Subscribed { agent_id }
            }
            ClientMessage::Unsubscribe { agent_id } => {
                let mut subs = self.subscriptions.write().await;
                subs.retain(|a| a != &agent_id);
                info!("Client {} unsubscribed from {} (active: {:?})", self.id, agent_id, *subs);
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

            ClientMessage::StartShell { agent_id, cols, rows } => {
                info!("Client {} starting shell on {} ({}x{})", self.id, agent_id, cols, rows);
                match self.dispatcher.dispatch_shell(&agent_id, None, cols, rows).await {
                    Ok((command_id, _rx)) => {
                        // Emit PTY created event
                        emit_pty_created(&agent_id, &command_id).await;
                        ServerMessage::ShellStarted {
                            agent_id,
                            command_id,
                        }
                    }
                    Err(e) => {
                        warn!("Failed to start shell: {}", e);
                        ServerMessage::Error {
                            message: format!("Failed to start shell: {}", e),
                        }
                    }
                }
            }

            ClientMessage::PtyResize { agent_id: _, command_id, cols, rows } => {
                debug!("Client {} resizing PTY {} to {}x{}", self.id, command_id, cols, rows);
                match self.dispatcher.send_pty_resize(&command_id, cols, rows).await {
                    Ok(_) => ServerMessage::Pong { timestamp: 0 }, // lightweight ack
                    Err(e) => {
                        warn!("Failed to resize PTY: {}", e);
                        ServerMessage::Error {
                            message: format!("Failed to resize: {}", e),
                        }
                    }
                }
            }

            ClientMessage::ListSessions { agent_id } => {
                info!("Client {} listing sessions for {}", self.id, agent_id);
                let sessions: Vec<SessionInfoWs> = self.dispatcher
                    .get_active_sessions(&agent_id)
                    .into_iter()
                    .map(|s| SessionInfoWs {
                        session_name: s.session_name,
                        command_id: s.command_id,
                        session_type: format!("{:?}", s.session_type).to_lowercase(),
                        command: s.command,
                        running: true, // If in active_sessions, it's running
                    })
                    .collect();
                ServerMessage::SessionList { agent_id, sessions }
            }

            ClientMessage::AttachSession { agent_id, session_name, cols, rows } => {
                info!("Client {} attaching to session {}:{}", self.id, agent_id, session_name);
                // Find the session and get its command_id
                let sessions = self.dispatcher.get_active_sessions(&agent_id);
                if let Some(session) = sessions.iter().find(|s| s.session_name == session_name) {
                    let command_id = session.command_id.clone();
                    // Send resize to the session
                    let _ = self.dispatcher.send_pty_resize(&command_id, cols, rows).await;
                    ServerMessage::SessionAttached {
                        agent_id,
                        session_name,
                        command_id,
                    }
                } else {
                    ServerMessage::Error {
                        message: format!("Session '{}' not found on agent '{}'", session_name, agent_id),
                    }
                }
            }

            ClientMessage::DetachSession { agent_id, session_name } => {
                info!("Client {} detaching from session {}:{}", self.id, agent_id, session_name);
                ServerMessage::SessionDetached { agent_id, session_name }
            }

            ClientMessage::KillSession { agent_id, session_name, signal: _ } => {
                info!("Client {} killing session {}:{}", self.id, agent_id, session_name);
                match self.dispatcher.kill_session(&agent_id, &session_name).await {
                    Ok(_) => ServerMessage::SessionKilled {
                        agent_id,
                        session_name,
                        exit_code: None,
                    },
                    Err(e) => ServerMessage::Error {
                        message: format!("Failed to kill session: {}", e),
                    }
                }
            }

            ClientMessage::CreateSession { agent_id, session_name, session_type, command, args, working_dir, cols, rows } => {
                info!("Client {} creating session {}:{} ({}) in {:?}", self.id, agent_id, session_name, session_type, working_dir);
                use crate::dispatch::SessionType;
                let st = match session_type.as_str() {
                    "interactive" => SessionType::Interactive,
                    "headless" => SessionType::Headless,
                    "background" => SessionType::Background,
                    _ => {
                        return ServerMessage::Error {
                            message: format!("Invalid session type: {}", session_type),
                        }
                    }
                };
                match self.dispatcher.create_session(&agent_id, session_name.clone(), st, command, args, working_dir, cols, rows).await {
                    Ok((command_id, _rx)) => ServerMessage::SessionCreated {
                        agent_id,
                        session_name,
                        session_type,
                        command_id,
                    },
                    Err(e) => ServerMessage::Error {
                        message: format!("Failed to create session: {}", e),
                    }
                }
            }
        }
    }

    /// Check if connection is subscribed to a given agent
    pub async fn is_subscribed_to(&self, agent_id: &str) -> bool {
        let subs = self.subscriptions.read().await;
        subs.contains(&"*".to_string())
            || subs.contains(&agent_id.to_string())
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
