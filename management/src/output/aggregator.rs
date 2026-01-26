//! Output Aggregator - collects and broadcasts command output

use std::collections::HashMap;

use parking_lot::RwLock;
use tokio::sync::broadcast;

/// Output message for broadcast
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OutputMessage {
    pub agent_id: String,
    pub command_id: String,
    pub stream_type: StreamType,
    pub data: Vec<u8>,
    pub timestamp: i64,
}

/// Type of output stream
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Stdout,
    Stderr,
    Log,
}

impl std::fmt::Display for StreamType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamType::Stdout => write!(f, "stdout"),
            StreamType::Stderr => write!(f, "stderr"),
            StreamType::Log => write!(f, "log"),
        }
    }
}

/// Subscription to output streams
#[allow(dead_code)]
pub struct OutputSubscription {
    /// Receives all output matching filter
    pub receiver: broadcast::Receiver<OutputMessage>,
    /// Filter by agent (None = all agents)
    pub agent_filter: Option<String>,
    /// Filter by stream type (None = all types)
    pub stream_filter: Option<StreamType>,
}

#[allow(dead_code)]
impl OutputSubscription {
    /// Receive next matching message
    pub async fn recv(&mut self) -> Option<OutputMessage> {
        loop {
            match self.receiver.recv().await {
                Ok(msg) => {
                    // Apply filters
                    if let Some(ref agent) = self.agent_filter {
                        if &msg.agent_id != agent {
                            continue;
                        }
                    }
                    if let Some(stream) = self.stream_filter {
                        if msg.stream_type != stream {
                            continue;
                        }
                    }
                    return Some(msg);
                }
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Skip lagged messages
                    continue;
                }
            }
        }
    }
}

/// Aggregates output from all agents and provides broadcast subscriptions
pub struct OutputAggregator {
    /// Broadcast channel for output
    sender: broadcast::Sender<OutputMessage>,
    /// Per-command output buffers (for late subscribers)
    buffers: RwLock<HashMap<String, Vec<OutputMessage>>>,
    /// Max buffer size per command
    max_buffer_size: usize,
}

impl OutputAggregator {
    pub fn new(channel_capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(channel_capacity);
        Self {
            sender,
            buffers: RwLock::new(HashMap::new()),
            max_buffer_size: 1000,
        }
    }

    /// Push output to aggregator
    pub fn push(&self, agent_id: String, command_id: String, stream_type: StreamType, data: Vec<u8>) {
        let msg = OutputMessage {
            agent_id,
            command_id: command_id.clone(),
            stream_type,
            data,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        // Buffer the message
        {
            let mut buffers = self.buffers.write();
            let buffer = buffers.entry(command_id).or_insert_with(Vec::new);
            if buffer.len() < self.max_buffer_size {
                buffer.push(msg.clone());
            }
        }

        // Broadcast (ignore if no receivers)
        let _ = self.sender.send(msg);
    }

    /// Subscribe to output stream
    #[allow(dead_code)]
    pub fn subscribe(&self, agent_filter: Option<String>, stream_filter: Option<StreamType>) -> OutputSubscription {
        OutputSubscription {
            receiver: self.sender.subscribe(),
            agent_filter,
            stream_filter,
        }
    }

    /// Get buffered output for a command
    #[allow(dead_code)]
    pub fn get_buffered(&self, command_id: &str) -> Vec<OutputMessage> {
        self.buffers
            .read()
            .get(command_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Clear buffer for a completed command
    #[allow(dead_code)]
    pub fn clear_buffer(&self, command_id: &str) {
        self.buffers.write().remove(command_id);
    }

    /// Get count of active subscribers
    #[allow(dead_code)]
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for OutputAggregator {
    fn default() -> Self {
        Self::new(10000)
    }
}
