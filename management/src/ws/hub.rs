//! WebSocket Hub - manages WebSocket server and connections

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tracing::{error, info};
use uuid::Uuid;

use crate::dispatch::CommandDispatcher;
use crate::output::OutputAggregator;
use crate::registry::AgentRegistry;
use crate::ws::connection::WsConnection;

/// WebSocket server hub
pub struct WebSocketHub {
    listen_addr: SocketAddr,
    output_agg: Arc<OutputAggregator>,
    registry: Arc<AgentRegistry>,
    dispatcher: Arc<CommandDispatcher>,
}

impl WebSocketHub {
    pub fn new(
        listen_addr: SocketAddr,
        output_agg: Arc<OutputAggregator>,
        registry: Arc<AgentRegistry>,
        dispatcher: Arc<CommandDispatcher>,
    ) -> Self {
        Self {
            listen_addr,
            output_agg,
            registry,
            dispatcher,
        }
    }

    /// Start the WebSocket server
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = TcpListener::bind(self.listen_addr).await?;
        info!("WebSocket server listening on ws://{}", self.listen_addr);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    let output_agg = self.output_agg.clone();
                    let registry = self.registry.clone();
                    let dispatcher = self.dispatcher.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, addr, output_agg, registry, dispatcher).await {
                            error!("WebSocket connection error from {}: {}", addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }
}

/// Handle incoming TCP connection and upgrade to WebSocket
async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    output_agg: Arc<OutputAggregator>,
    registry: Arc<AgentRegistry>,
    dispatcher: Arc<CommandDispatcher>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("New connection from {}", addr);

    let ws = accept_async(stream).await?;
    let id = format!("ws-{}", Uuid::new_v4().to_string()[..8].to_string());

    WsConnection::handle(id, ws, output_agg, registry, dispatcher).await;

    Ok(())
}
