//! Output Aggregator - collects and broadcasts command output

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::broadcast;

const DEFAULT_SLOW_SUBSCRIBER_LAG_LIMIT: u64 = 3;

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
    /// Stable subscriber id for metrics and diagnostics.
    pub id: String,
    /// Receives all output matching filter
    pub receiver: broadcast::Receiver<OutputMessage>,
    /// Filter by agent (None = all agents)
    pub agent_filter: Option<String>,
    /// Filter by stream type (None = all types)
    pub stream_filter: Option<StreamType>,
    metrics: Arc<OutputAggregatorMetrics>,
    consecutive_lag_events: u64,
    slow_subscriber_lag_limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRecvError {
    SlowSubscriber { subscriber_id: String, dropped: u64 },
}

#[allow(dead_code)]
impl OutputSubscription {
    /// Receive next matching message
    pub async fn recv(&mut self) -> Option<OutputMessage> {
        self.recv_with_policy().await.ok().flatten()
    }

    /// Receive next matching message, returning an explicit error when a
    /// subscriber repeatedly falls behind the bounded broadcast ring.
    pub async fn recv_with_policy(&mut self) -> Result<Option<OutputMessage>, OutputRecvError> {
        loop {
            match self.receiver.recv().await {
                Ok(msg) => {
                    self.consecutive_lag_events = 0;
                    self.metrics.record_broadcast_lag(&self.id, msg.timestamp);

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
                    return Ok(Some(msg));
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(None),
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    self.consecutive_lag_events = self.consecutive_lag_events.saturating_add(1);
                    self.metrics.record_lagged_messages(dropped);

                    if self.consecutive_lag_events >= self.slow_subscriber_lag_limit {
                        self.metrics.record_slow_subscriber_kicked();
                        return Err(OutputRecvError::SlowSubscriber {
                            subscriber_id: self.id.clone(),
                            dropped,
                        });
                    }
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
    metrics: Arc<OutputAggregatorMetrics>,
    next_subscriber_id: AtomicU64,
    slow_subscriber_lag_limit: u64,
}

impl OutputAggregator {
    pub fn new(channel_capacity: usize) -> Self {
        Self::new_with_slow_subscriber_lag_limit(
            channel_capacity,
            DEFAULT_SLOW_SUBSCRIBER_LAG_LIMIT,
        )
    }

    pub fn new_with_slow_subscriber_lag_limit(
        channel_capacity: usize,
        slow_subscriber_lag_limit: u64,
    ) -> Self {
        let (sender, _) = broadcast::channel(channel_capacity);
        Self {
            sender,
            buffers: RwLock::new(HashMap::new()),
            max_buffer_size: 1000,
            metrics: Arc::new(OutputAggregatorMetrics::default()),
            next_subscriber_id: AtomicU64::new(1),
            slow_subscriber_lag_limit: slow_subscriber_lag_limit.max(1),
        }
    }

    /// Push output to aggregator
    pub fn push(
        &self,
        agent_id: String,
        command_id: String,
        stream_type: StreamType,
        data: Vec<u8>,
    ) {
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
            let buffer = buffers.entry(command_id).or_default();
            if buffer.len() < self.max_buffer_size {
                buffer.push(msg.clone());
            }
        }

        // Broadcast (ignore if no receivers)
        let _ = self.sender.send(msg);
    }

    /// Subscribe to output stream
    #[allow(dead_code)]
    pub fn subscribe(
        &self,
        agent_filter: Option<String>,
        stream_filter: Option<StreamType>,
    ) -> OutputSubscription {
        let id = self
            .next_subscriber_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string();
        OutputSubscription {
            id,
            receiver: self.sender.subscribe(),
            agent_filter,
            stream_filter,
            metrics: self.metrics.clone(),
            consecutive_lag_events: 0,
            slow_subscriber_lag_limit: self.slow_subscriber_lag_limit,
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

    pub fn metrics_snapshot(&self) -> OutputAggregatorMetricsSnapshot {
        self.metrics.snapshot()
    }
}

impl Default for OutputAggregator {
    fn default() -> Self {
        Self::new(10000)
    }
}

#[derive(Default)]
struct OutputAggregatorMetrics {
    slow_subscriber_kicked_total: AtomicU64,
    lagged_messages_dropped_total: AtomicU64,
    broadcast_lag_millis: RwLock<HashMap<String, u64>>,
}

impl OutputAggregatorMetrics {
    fn record_broadcast_lag(&self, subscriber_id: &str, timestamp_ms: i64) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let lag_ms = now_ms.saturating_sub(timestamp_ms).max(0) as u64;
        self.broadcast_lag_millis
            .write()
            .insert(subscriber_id.to_string(), lag_ms);
    }

    fn record_lagged_messages(&self, dropped: u64) {
        self.lagged_messages_dropped_total
            .fetch_add(dropped, Ordering::Relaxed);
    }

    fn record_slow_subscriber_kicked(&self) {
        self.slow_subscriber_kicked_total
            .fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> OutputAggregatorMetricsSnapshot {
        let broadcast_lag_seconds = self
            .broadcast_lag_millis
            .read()
            .iter()
            .map(|(subscriber_id, lag_ms)| (subscriber_id.clone(), *lag_ms as f64 / 1000.0))
            .collect();

        OutputAggregatorMetricsSnapshot {
            slow_subscriber_kicked_total: self.slow_subscriber_kicked_total.load(Ordering::Relaxed),
            lagged_messages_dropped_total: self
                .lagged_messages_dropped_total
                .load(Ordering::Relaxed),
            broadcast_lag_seconds,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputAggregatorMetricsSnapshot {
    pub slow_subscriber_kicked_total: u64,
    pub lagged_messages_dropped_total: u64,
    pub broadcast_lag_seconds: Vec<(String, f64)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slow_subscriber_is_kicked_and_counted_after_broadcast_lag() {
        let aggregator = OutputAggregator::new_with_slow_subscriber_lag_limit(2, 1);
        let mut subscription = aggregator.subscribe(None, None);

        for i in 0..8 {
            aggregator.push(
                "agent-1".to_string(),
                format!("cmd-{i}"),
                StreamType::Stdout,
                vec![b'x'],
            );
        }

        let err = subscription.recv_with_policy().await.unwrap_err();
        assert!(matches!(
            err,
            OutputRecvError::SlowSubscriber {
                subscriber_id: _,
                dropped: 6
            }
        ));

        let metrics = aggregator.metrics_snapshot();
        assert_eq!(metrics.slow_subscriber_kicked_total, 1);
        assert_eq!(metrics.lagged_messages_dropped_total, 6);
    }

    #[tokio::test]
    async fn delivered_messages_update_per_subscriber_broadcast_lag() {
        let aggregator = OutputAggregator::new(8);
        let mut subscription = aggregator.subscribe(None, None);
        let subscriber_id = subscription.id.clone();

        aggregator.push(
            "agent-1".to_string(),
            "cmd-1".to_string(),
            StreamType::Stdout,
            b"hello".to_vec(),
        );

        let msg = subscription.recv_with_policy().await.unwrap().unwrap();
        assert_eq!(msg.command_id, "cmd-1");

        let metrics = aggregator.metrics_snapshot();
        assert!(metrics
            .broadcast_lag_seconds
            .iter()
            .any(|(id, lag)| id == &subscriber_id && *lag >= 0.0));
    }
}
