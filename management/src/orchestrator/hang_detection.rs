//! Hang detection for task orchestration
#![allow(dead_code)] // Fields reserved for future hang detection strategies
//!
//! Monitors task progress to detect hangs using multiple strategies:
//! - Output silence: No output for X minutes
//! - CPU idle: Process using minimal CPU for extended period
//! - Process stuck: No activity indicators despite process running
//!
//! Provides configurable callbacks and auto-recovery options.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, instrument, warn};

use super::monitor::TaskOutputEvent;

/// Detection strategy for identifying hangs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionStrategy {
    /// Detect when no output received for threshold duration
    OutputSilence,
    /// Detect when CPU usage drops below threshold for duration
    CpuIdle,
    /// Detect when process appears stuck (no progress indicators)
    ProcessStuck,
}

/// Recovery action when hang is detected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    /// Terminate task and clean up VM
    Terminate,
    /// Restart task from last checkpoint
    Restart,
    /// Preserve VM for manual debugging
    PreserveForDebug,
    /// Notify only, no automatic action
    NotifyOnly,
}

/// Configuration for hang detection
#[derive(Debug, Clone)]
pub struct HangDetectionConfig {
    /// Timeout for output silence detection (e.g., 5 minutes)
    pub output_silence_threshold: Duration,
    /// Timeout for CPU idle detection
    pub cpu_idle_threshold: Duration,
    /// Timeout for process stuck detection
    pub process_stuck_threshold: Duration,
    /// CPU usage percentage below which is considered idle (0-100)
    pub cpu_idle_percentage: f32,
    /// Recovery action when hang detected
    pub recovery_action: RecoveryAction,
    /// Enabled detection strategies
    pub enabled_strategies: Vec<DetectionStrategy>,
}

impl Default for HangDetectionConfig {
    fn default() -> Self {
        Self {
            output_silence_threshold: Duration::minutes(10),
            cpu_idle_threshold: Duration::minutes(15),
            process_stuck_threshold: Duration::minutes(20),
            cpu_idle_percentage: 5.0,
            recovery_action: RecoveryAction::NotifyOnly,
            enabled_strategies: vec![DetectionStrategy::OutputSilence],
        }
    }
}

impl HangDetectionConfig {
    /// Create a new configuration with custom thresholds
    pub fn new(
        output_silence_threshold: Duration,
        cpu_idle_threshold: Duration,
        process_stuck_threshold: Duration,
    ) -> Self {
        Self {
            output_silence_threshold,
            cpu_idle_threshold,
            process_stuck_threshold,
            ..Default::default()
        }
    }

    /// Enable output silence detection
    pub fn with_output_silence(mut self, threshold: Duration) -> Self {
        self.output_silence_threshold = threshold;
        if !self.enabled_strategies.contains(&DetectionStrategy::OutputSilence) {
            self.enabled_strategies.push(DetectionStrategy::OutputSilence);
        }
        self
    }

    /// Enable CPU idle detection
    pub fn with_cpu_idle(mut self, threshold: Duration, idle_percentage: f32) -> Self {
        self.cpu_idle_threshold = threshold;
        self.cpu_idle_percentage = idle_percentage;
        if !self.enabled_strategies.contains(&DetectionStrategy::CpuIdle) {
            self.enabled_strategies.push(DetectionStrategy::CpuIdle);
        }
        self
    }

    /// Enable process stuck detection
    pub fn with_process_stuck(mut self, threshold: Duration) -> Self {
        self.process_stuck_threshold = threshold;
        if !self.enabled_strategies.contains(&DetectionStrategy::ProcessStuck) {
            self.enabled_strategies.push(DetectionStrategy::ProcessStuck);
        }
        self
    }

    /// Set recovery action
    pub fn with_recovery_action(mut self, action: RecoveryAction) -> Self {
        self.recovery_action = action;
        self
    }
}

/// Event emitted when a hang is detected
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HangEvent {
    pub task_id: String,
    pub detected_at: DateTime<Utc>,
    pub strategy: DetectionStrategy,
    pub details: String,
    pub recovery_action: RecoveryAction,
}

/// Callback function type for hang detection
pub type HangCallback = Arc<dyn Fn(HangEvent) + Send + Sync>;

/// Task monitoring state for hang detection
#[derive(Debug)]
struct TaskMonitorState {
    task_id: String,
    last_output_at: Option<DateTime<Utc>>,
    last_cpu_check_at: Option<DateTime<Utc>>,
    last_progress_at: Option<DateTime<Utc>>,
    cpu_usage: f32,
    output_bytes: u64,
    tool_calls: u32,
}

impl TaskMonitorState {
    fn new(task_id: String) -> Self {
        Self {
            task_id,
            last_output_at: None,
            last_cpu_check_at: None,
            last_progress_at: None,
            cpu_usage: 0.0,
            output_bytes: 0,
            tool_calls: 0,
        }
    }

    fn update_output(&mut self, bytes: u64) {
        let now = Utc::now();
        self.last_output_at = Some(now);
        self.last_progress_at = Some(now);
        self.output_bytes += bytes;
    }

    fn update_cpu(&mut self, usage: f32) {
        self.last_cpu_check_at = Some(Utc::now());
        self.cpu_usage = usage;
    }

    fn update_progress(&mut self) {
        self.last_progress_at = Some(Utc::now());
    }
}

/// Hang detector for monitoring task progress
pub struct HangDetector {
    config: HangDetectionConfig,
    /// Monitored tasks by ID
    monitored: Arc<RwLock<HashMap<String, TaskMonitorState>>>,
    /// Hang event callback
    callback: Option<HangCallback>,
    /// Stop signal
    stop_tx: Option<mpsc::Sender<()>>,
}

impl HangDetector {
    /// Create a new hang detector with default configuration
    pub fn new() -> Self {
        Self {
            config: HangDetectionConfig::default(),
            monitored: Arc::new(RwLock::new(HashMap::new())),
            callback: None,
            stop_tx: None,
        }
    }

    /// Create a new hang detector with custom configuration
    pub fn with_config(config: HangDetectionConfig) -> Self {
        Self {
            config,
            monitored: Arc::new(RwLock::new(HashMap::new())),
            callback: None,
            stop_tx: None,
        }
    }

    /// Set the hang detection callback
    pub fn with_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(HangEvent) + Send + Sync + 'static,
    {
        self.callback = Some(Arc::new(callback));
        self
    }

    /// Start monitoring a task
    #[instrument(skip(self))]
    pub async fn start_monitoring(&mut self, task_id: &str) {
        let mut monitored = self.monitored.write().await;
        if monitored.contains_key(task_id) {
            debug!("Task {} already being monitored for hangs", task_id);
            return;
        }

        let state = TaskMonitorState::new(task_id.to_string());
        monitored.insert(task_id.to_string(), state);
        info!("Started hang detection for task {}", task_id);
    }

    /// Stop monitoring a task
    #[instrument(skip(self))]
    pub async fn stop_monitoring(&mut self, task_id: &str) {
        let mut monitored = self.monitored.write().await;
        if monitored.remove(task_id).is_some() {
            info!("Stopped hang detection for task {}", task_id);
        }
    }

    /// Update task output activity
    pub async fn record_output(&self, task_id: &str, bytes: u64) {
        let mut monitored = self.monitored.write().await;
        if let Some(state) = monitored.get_mut(task_id) {
            state.update_output(bytes);
            debug!("Recorded {} bytes of output for task {}", bytes, task_id);
        }
    }

    /// Update CPU usage for a task
    pub async fn record_cpu_usage(&self, task_id: &str, usage: f32) {
        let mut monitored = self.monitored.write().await;
        if let Some(state) = monitored.get_mut(task_id) {
            state.update_cpu(usage);
            debug!("Recorded CPU usage {}% for task {}", usage, task_id);
        }
    }

    /// Update task progress (e.g., tool calls)
    pub async fn record_progress(&self, task_id: &str) {
        let mut monitored = self.monitored.write().await;
        if let Some(state) = monitored.get_mut(task_id) {
            state.update_progress();
            debug!("Recorded progress activity for task {}", task_id);
        }
    }

    /// Check all monitored tasks for hangs
    #[instrument(skip(self))]
    pub async fn check_for_hangs(&self) -> Vec<HangEvent> {
        let monitored = self.monitored.read().await;
        let now = Utc::now();
        let mut events = Vec::new();

        for (task_id, state) in monitored.iter() {
            // Check output silence
            if self.config.enabled_strategies.contains(&DetectionStrategy::OutputSilence) {
                if let Some(hang_event) = self.check_output_silence(state, now) {
                    info!("Detected output silence hang for task {}", task_id);
                    events.push(hang_event);
                }
            }

            // Check CPU idle
            if self.config.enabled_strategies.contains(&DetectionStrategy::CpuIdle) {
                if let Some(hang_event) = self.check_cpu_idle(state, now) {
                    info!("Detected CPU idle hang for task {}", task_id);
                    events.push(hang_event);
                }
            }

            // Check process stuck
            if self.config.enabled_strategies.contains(&DetectionStrategy::ProcessStuck) {
                if let Some(hang_event) = self.check_process_stuck(state, now) {
                    info!("Detected process stuck hang for task {}", task_id);
                    events.push(hang_event);
                }
            }
        }

        // Invoke callback for each hang event
        if let Some(ref callback) = self.callback {
            for event in &events {
                callback(event.clone());
            }
        }

        events
    }

    /// Check for output silence hang
    fn check_output_silence(&self, state: &TaskMonitorState, now: DateTime<Utc>) -> Option<HangEvent> {
        if let Some(last_output) = state.last_output_at {
            let silence_duration = now.signed_duration_since(last_output);
            if silence_duration > self.config.output_silence_threshold {
                return Some(HangEvent {
                    task_id: state.task_id.clone(),
                    detected_at: now,
                    strategy: DetectionStrategy::OutputSilence,
                    details: format!(
                        "No output for {} minutes (threshold: {} minutes)",
                        silence_duration.num_minutes(),
                        self.config.output_silence_threshold.num_minutes()
                    ),
                    recovery_action: self.config.recovery_action,
                });
            }
        }
        None
    }

    /// Check for CPU idle hang
    fn check_cpu_idle(&self, state: &TaskMonitorState, now: DateTime<Utc>) -> Option<HangEvent> {
        if let Some(last_check) = state.last_cpu_check_at {
            let idle_duration = now.signed_duration_since(last_check);
            if idle_duration > self.config.cpu_idle_threshold
                && state.cpu_usage < self.config.cpu_idle_percentage
            {
                return Some(HangEvent {
                    task_id: state.task_id.clone(),
                    detected_at: now,
                    strategy: DetectionStrategy::CpuIdle,
                    details: format!(
                        "CPU idle at {:.1}% for {} minutes (threshold: {}% for {} minutes)",
                        state.cpu_usage,
                        idle_duration.num_minutes(),
                        self.config.cpu_idle_percentage,
                        self.config.cpu_idle_threshold.num_minutes()
                    ),
                    recovery_action: self.config.recovery_action,
                });
            }
        }
        None
    }

    /// Check for process stuck hang
    fn check_process_stuck(&self, state: &TaskMonitorState, now: DateTime<Utc>) -> Option<HangEvent> {
        if let Some(last_progress) = state.last_progress_at {
            let stuck_duration = now.signed_duration_since(last_progress);
            if stuck_duration > self.config.process_stuck_threshold {
                return Some(HangEvent {
                    task_id: state.task_id.clone(),
                    detected_at: now,
                    strategy: DetectionStrategy::ProcessStuck,
                    details: format!(
                        "No progress indicators for {} minutes (threshold: {} minutes)",
                        stuck_duration.num_minutes(),
                        self.config.process_stuck_threshold.num_minutes()
                    ),
                    recovery_action: self.config.recovery_action,
                });
            }
        }
        None
    }

    /// Start background monitoring loop
    #[instrument(skip(self))]
    pub async fn start_background_monitoring(&mut self, check_interval: std::time::Duration) {
        let (stop_tx, mut stop_rx) = mpsc::channel(1);
        self.stop_tx = Some(stop_tx);

        let monitored = self.monitored.clone();
        let config = self.config.clone();
        let callback = self.callback.clone();

        tokio::spawn(async move {
            let detector = HangDetector {
                config,
                monitored,
                callback,
                stop_tx: None,
            };

            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        debug!("Stopping background hang detection");
                        break;
                    }
                    _ = tokio::time::sleep(check_interval) => {
                        let events = detector.check_for_hangs().await;
                        if !events.is_empty() {
                            warn!("Detected {} hang(s)", events.len());
                        }
                    }
                }
            }
        });

        info!("Started background hang monitoring with interval {:?}", check_interval);
    }

    /// Stop background monitoring
    pub async fn stop_background_monitoring(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(()).await;
            info!("Stopped background hang monitoring");
        }
    }

    /// Get number of monitored tasks
    pub async fn monitored_count(&self) -> usize {
        self.monitored.read().await.len()
    }

    /// Check if a task is being monitored
    pub async fn is_monitoring(&self, task_id: &str) -> bool {
        self.monitored.read().await.contains_key(task_id)
    }

    /// Process task output events for automatic tracking
    pub async fn process_output_event(&self, event: &TaskOutputEvent) {
        match event {
            TaskOutputEvent::Stdout(task_id, data) => {
                self.record_output(task_id, data.len() as u64).await;
            }
            TaskOutputEvent::Stderr(task_id, data) => {
                self.record_output(task_id, data.len() as u64).await;
            }
            TaskOutputEvent::Event(task_id, _) => {
                self.record_progress(task_id).await;
            }
            TaskOutputEvent::Completed(_task_id, _) => {
                // Task completed, will be stopped by orchestrator
            }
            TaskOutputEvent::Error(_task_id, _) => {
                // Task errored, will be stopped by orchestrator
            }
        }
    }
}

impl Default for HangDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_detection_strategy_serialization() {
        let strategy = DetectionStrategy::OutputSilence;
        let json = serde_json::to_string(&strategy).unwrap();
        assert_eq!(json, "\"output_silence\"");

        let deserialized: DetectionStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, DetectionStrategy::OutputSilence);
    }

    #[test]
    fn test_recovery_action_serialization() {
        let action = RecoveryAction::Terminate;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"terminate\"");

        let deserialized: RecoveryAction = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, RecoveryAction::Terminate);
    }

    #[test]
    fn test_hang_detection_config_default() {
        let config = HangDetectionConfig::default();
        assert_eq!(config.output_silence_threshold, Duration::minutes(10));
        assert_eq!(config.cpu_idle_threshold, Duration::minutes(15));
        assert_eq!(config.process_stuck_threshold, Duration::minutes(20));
        assert_eq!(config.cpu_idle_percentage, 5.0);
        assert_eq!(config.recovery_action, RecoveryAction::NotifyOnly);
        assert_eq!(config.enabled_strategies, vec![DetectionStrategy::OutputSilence]);
    }

    #[test]
    fn test_hang_detection_config_custom() {
        let config = HangDetectionConfig::new(
            Duration::minutes(5),
            Duration::minutes(10),
            Duration::minutes(15),
        );
        assert_eq!(config.output_silence_threshold, Duration::minutes(5));
        assert_eq!(config.cpu_idle_threshold, Duration::minutes(10));
        assert_eq!(config.process_stuck_threshold, Duration::minutes(15));
    }

    #[test]
    fn test_config_builder_pattern() {
        let config = HangDetectionConfig::default()
            .with_output_silence(Duration::minutes(3))
            .with_cpu_idle(Duration::minutes(7), 10.0)
            .with_process_stuck(Duration::minutes(12))
            .with_recovery_action(RecoveryAction::Terminate);

        assert_eq!(config.output_silence_threshold, Duration::minutes(3));
        assert_eq!(config.cpu_idle_threshold, Duration::minutes(7));
        assert_eq!(config.process_stuck_threshold, Duration::minutes(12));
        assert_eq!(config.cpu_idle_percentage, 10.0);
        assert_eq!(config.recovery_action, RecoveryAction::Terminate);
        assert!(config.enabled_strategies.contains(&DetectionStrategy::OutputSilence));
        assert!(config.enabled_strategies.contains(&DetectionStrategy::CpuIdle));
        assert!(config.enabled_strategies.contains(&DetectionStrategy::ProcessStuck));
    }

    #[tokio::test]
    async fn test_hang_detector_new() {
        let detector = HangDetector::new();
        assert_eq!(detector.monitored_count().await, 0);
        assert!(detector.callback.is_none());
    }

    #[tokio::test]
    async fn test_start_stop_monitoring() {
        let mut detector = HangDetector::new();

        detector.start_monitoring("task-1").await;
        assert_eq!(detector.monitored_count().await, 1);
        assert!(detector.is_monitoring("task-1").await);

        detector.start_monitoring("task-2").await;
        assert_eq!(detector.monitored_count().await, 2);

        detector.stop_monitoring("task-1").await;
        assert_eq!(detector.monitored_count().await, 1);
        assert!(!detector.is_monitoring("task-1").await);
        assert!(detector.is_monitoring("task-2").await);
    }

    #[tokio::test]
    async fn test_record_output() {
        let mut detector = HangDetector::new();
        detector.start_monitoring("task-1").await;

        detector.record_output("task-1", 1024).await;

        let monitored = detector.monitored.read().await;
        let state = monitored.get("task-1").unwrap();
        assert_eq!(state.output_bytes, 1024);
        assert!(state.last_output_at.is_some());
        assert!(state.last_progress_at.is_some());
    }

    #[tokio::test]
    async fn test_record_cpu_usage() {
        let mut detector = HangDetector::new();
        detector.start_monitoring("task-1").await;

        detector.record_cpu_usage("task-1", 45.5).await;

        let monitored = detector.monitored.read().await;
        let state = monitored.get("task-1").unwrap();
        assert_eq!(state.cpu_usage, 45.5);
        assert!(state.last_cpu_check_at.is_some());
    }

    #[tokio::test]
    async fn test_record_progress() {
        let mut detector = HangDetector::new();
        detector.start_monitoring("task-1").await;

        detector.record_progress("task-1").await;

        let monitored = detector.monitored.read().await;
        let state = monitored.get("task-1").unwrap();
        assert!(state.last_progress_at.is_some());
    }

    #[tokio::test]
    async fn test_output_silence_detection() {
        let config = HangDetectionConfig::default()
            .with_output_silence(Duration::seconds(1));
        let mut detector = HangDetector::with_config(config);

        detector.start_monitoring("task-1").await;
        detector.record_output("task-1", 100).await;

        // No hang immediately
        let hangs = detector.check_for_hangs().await;
        assert_eq!(hangs.len(), 0);

        // Wait for threshold
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let hangs = detector.check_for_hangs().await;
        assert_eq!(hangs.len(), 1);
        assert_eq!(hangs[0].strategy, DetectionStrategy::OutputSilence);
        assert_eq!(hangs[0].task_id, "task-1");
    }

    #[tokio::test]
    async fn test_cpu_idle_detection() {
        let config = HangDetectionConfig::default()
            .with_cpu_idle(Duration::seconds(1), 10.0);
        let mut detector = HangDetector::with_config(config);

        detector.start_monitoring("task-1").await;
        detector.record_cpu_usage("task-1", 5.0).await;

        // No hang immediately
        let hangs = detector.check_for_hangs().await;
        assert_eq!(hangs.len(), 0);

        // Wait for threshold
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let hangs = detector.check_for_hangs().await;
        assert_eq!(hangs.len(), 1);
        assert_eq!(hangs[0].strategy, DetectionStrategy::CpuIdle);
        assert_eq!(hangs[0].task_id, "task-1");
    }

    #[tokio::test]
    async fn test_process_stuck_detection() {
        let config = HangDetectionConfig::default()
            .with_process_stuck(Duration::seconds(1));
        let mut detector = HangDetector::with_config(config);

        detector.start_monitoring("task-1").await;
        detector.record_progress("task-1").await;

        // No hang immediately
        let hangs = detector.check_for_hangs().await;
        assert_eq!(hangs.len(), 0);

        // Wait for threshold
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let hangs = detector.check_for_hangs().await;
        assert_eq!(hangs.len(), 1);
        assert_eq!(hangs[0].strategy, DetectionStrategy::ProcessStuck);
        assert_eq!(hangs[0].task_id, "task-1");
    }

    #[tokio::test]
    async fn test_callback_invocation() {
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let config = HangDetectionConfig::default()
            .with_output_silence(Duration::seconds(1));
        let mut detector = HangDetector::with_config(config)
            .with_callback(move |event| {
                callback_count_clone.fetch_add(1, Ordering::SeqCst);
            });

        detector.start_monitoring("task-1").await;
        detector.record_output("task-1", 100).await;

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        detector.check_for_hangs().await;
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_multiple_strategies() {
        let config = HangDetectionConfig::default()
            .with_output_silence(Duration::seconds(1))
            .with_cpu_idle(Duration::seconds(1), 10.0);
        let mut detector = HangDetector::with_config(config);

        detector.start_monitoring("task-1").await;
        detector.record_output("task-1", 100).await;
        detector.record_cpu_usage("task-1", 5.0).await;

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let hangs = detector.check_for_hangs().await;
        assert_eq!(hangs.len(), 2); // Both strategies should detect
    }

    #[tokio::test]
    async fn test_process_output_event() {
        let mut detector = HangDetector::new();
        detector.start_monitoring("task-1").await;

        let event = TaskOutputEvent::Stdout("task-1".to_string(), vec![1, 2, 3, 4, 5]);
        detector.process_output_event(&event).await;

        let monitored = detector.monitored.read().await;
        let state = monitored.get("task-1").unwrap();
        assert_eq!(state.output_bytes, 5);
    }

    #[tokio::test]
    async fn test_hang_event_serialization() {
        let event = HangEvent {
            task_id: "task-123".to_string(),
            detected_at: Utc::now(),
            strategy: DetectionStrategy::OutputSilence,
            details: "No output for 10 minutes".to_string(),
            recovery_action: RecoveryAction::Terminate,
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: HangEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.task_id, "task-123");
        assert_eq!(deserialized.strategy, DetectionStrategy::OutputSilence);
        assert_eq!(deserialized.recovery_action, RecoveryAction::Terminate);
    }

    #[tokio::test]
    async fn test_background_monitoring_start_stop() {
        let mut detector = HangDetector::new();

        detector.start_background_monitoring(std::time::Duration::from_millis(100)).await;
        assert!(detector.stop_tx.is_some());

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        detector.stop_background_monitoring().await;
        assert!(detector.stop_tx.is_none());
    }
}
