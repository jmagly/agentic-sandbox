//! Crash loop detection and auto-remediation
//!
//! Monitors VM crash patterns and triggers automatic rebuilds when
//! a crash loop is detected (repeated crashes during boot).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::libvirt_events::{VmEvent, VmEventType};

/// Crash loop detection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashLoopConfig {
    /// Maximum restarts in window before declaring crash loop
    pub max_restarts: u32,
    /// Rolling time window for counting restarts (minutes)
    pub window_minutes: u32,
    /// Minimum uptime to count as "healthy" boot (seconds)
    pub min_uptime_seconds: u32,
    /// Continuous healthy time to reset restart counter (minutes)
    pub healthy_reset_minutes: u32,
    /// Enable auto-remediation (rebuild on crash loop)
    pub remediation_enabled: bool,
    /// Maximum rebuild attempts before giving up
    pub max_rebuild_attempts: u32,
    /// Cooldown between rebuilds (minutes)
    pub rebuild_cooldown_minutes: u32,
    /// Path to provision-vm.sh script
    pub provision_script: PathBuf,
    /// Data directory for crash history
    pub data_dir: PathBuf,
}

impl Default for CrashLoopConfig {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            window_minutes: 10,
            min_uptime_seconds: 60,
            healthy_reset_minutes: 5,
            remediation_enabled: true,
            max_rebuild_attempts: 3,
            rebuild_cooldown_minutes: 30,
            provision_script: PathBuf::from(
                "/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh",
            ),
            data_dir: PathBuf::from("/var/lib/agentic-sandbox/vms"),
        }
    }
}

/// VM crash state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VmState {
    /// VM is running and healthy
    Healthy,
    /// VM is recovering from a crash
    Recovering,
    /// VM is starting up
    Starting,
    /// VM is in a crash loop
    CrashLoop,
    /// VM is being rebuilt
    Rebuilding,
    /// VM has failed after max rebuilds
    Failed,
}

impl std::fmt::Display for VmState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmState::Healthy => write!(f, "healthy"),
            VmState::Recovering => write!(f, "recovering"),
            VmState::Starting => write!(f, "starting"),
            VmState::CrashLoop => write!(f, "crash_loop"),
            VmState::Rebuilding => write!(f, "rebuilding"),
            VmState::Failed => write!(f, "failed"),
        }
    }
}

/// Single crash event record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashEvent {
    pub timestamp: DateTime<Utc>,
    pub uptime_seconds: Option<i64>,
    pub reason: String,
}

/// Crash history for a single VM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmCrashHistory {
    pub vm_name: String,
    pub state: VmState,
    pub restart_events: Vec<CrashEvent>,
    pub rebuild_count: u32,
    pub last_healthy_boot: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub last_start_time: Option<Instant>,
    pub last_rebuild: Option<DateTime<Utc>>,
}

impl VmCrashHistory {
    fn new(vm_name: String) -> Self {
        Self {
            vm_name,
            state: VmState::Healthy,
            restart_events: Vec::new(),
            rebuild_count: 0,
            last_healthy_boot: None,
            last_start_time: None,
            last_rebuild: None,
        }
    }

    /// Count restarts within the time window
    fn restarts_in_window(&self, window_minutes: u32) -> u32 {
        let cutoff = Utc::now() - chrono::Duration::minutes(window_minutes as i64);
        self.restart_events
            .iter()
            .filter(|e| e.timestamp > cutoff)
            .count() as u32
    }

    /// Check if VM has been healthy long enough to reset counters
    fn should_reset(&self, healthy_reset_minutes: u32) -> bool {
        if let Some(last_healthy) = self.last_healthy_boot {
            let threshold = Utc::now() - chrono::Duration::minutes(healthy_reset_minutes as i64);
            last_healthy < threshold && self.state == VmState::Healthy
        } else {
            false
        }
    }

    /// Prune old events outside the window
    fn prune_old_events(&mut self, window_minutes: u32) {
        let cutoff = Utc::now() - chrono::Duration::minutes(window_minutes as i64 * 2);
        self.restart_events.retain(|e| e.timestamp > cutoff);
    }
}

/// Crash loop detector
pub struct CrashLoopDetector {
    config: CrashLoopConfig,
    histories: Arc<RwLock<HashMap<String, VmCrashHistory>>>,
    notification_tx: Option<mpsc::Sender<CrashLoopNotification>>,
}

/// Notification for crash loop events
#[derive(Debug, Clone, Serialize)]
pub struct CrashLoopNotification {
    pub vm_name: String,
    pub event_type: String,
    pub state: VmState,
    pub restart_count: u32,
    pub rebuild_count: u32,
    pub timestamp: DateTime<Utc>,
    pub message: String,
}

impl CrashLoopDetector {
    /// Create a new crash loop detector
    pub fn new(config: CrashLoopConfig) -> Self {
        Self {
            config,
            histories: Arc::new(RwLock::new(HashMap::new())),
            notification_tx: None,
        }
    }

    /// Set notification channel for crash loop events
    pub fn with_notifications(mut self, tx: mpsc::Sender<CrashLoopNotification>) -> Self {
        self.notification_tx = Some(tx);
        self
    }

    /// Process a VM lifecycle event
    pub async fn process_event(&self, event: &VmEvent) {
        let vm_name = &event.vm_name;

        match &event.event_type {
            VmEventType::Started => {
                self.handle_started(vm_name).await;
            }
            VmEventType::Crashed => {
                self.handle_crash(vm_name, event.uptime_seconds, "crashed")
                    .await;
            }
            VmEventType::Stopped => {
                // Check if this is a crash-stop
                if let Some(ref reason) = event.reason {
                    if reason == "crashed" {
                        self.handle_crash(vm_name, event.uptime_seconds, reason)
                            .await;
                    }
                }
            }
            _ => {}
        }
    }

    /// Handle VM started event
    async fn handle_started(&self, vm_name: &str) {
        let mut histories = self.histories.write();
        let history = histories
            .entry(vm_name.to_string())
            .or_insert_with(|| VmCrashHistory::new(vm_name.to_string()));

        history.state = VmState::Starting;
        history.last_start_time = Some(Instant::now());

        info!(vm = %vm_name, state = %history.state, "VM started");
    }

    /// Handle VM crash event
    async fn handle_crash(&self, vm_name: &str, uptime: Option<i64>, reason: &str) {
        let should_remediate;
        let notification;

        {
            let mut histories = self.histories.write();
            let history = histories
                .entry(vm_name.to_string())
                .or_insert_with(|| VmCrashHistory::new(vm_name.to_string()));

            // Record the crash
            history.restart_events.push(CrashEvent {
                timestamp: Utc::now(),
                uptime_seconds: uptime,
                reason: reason.to_string(),
            });
            history.state = VmState::Recovering;

            // Check for crash loop
            let restart_count = history.restarts_in_window(self.config.window_minutes);
            let is_crash_loop = restart_count >= self.config.max_restarts;

            // Check if uptime was too short (indicates boot crash)
            let is_boot_crash = uptime
                .map(|u| u < self.config.min_uptime_seconds as i64)
                .unwrap_or(true);

            warn!(
                vm = %vm_name,
                restart_count = restart_count,
                uptime = ?uptime,
                is_crash_loop = is_crash_loop,
                is_boot_crash = is_boot_crash,
                "VM crashed"
            );

            if is_crash_loop && is_boot_crash {
                history.state = VmState::CrashLoop;

                // Check if we can still try to rebuild
                if history.rebuild_count >= self.config.max_rebuild_attempts {
                    history.state = VmState::Failed;
                    should_remediate = false;

                    notification = Some(CrashLoopNotification {
                        vm_name: vm_name.to_string(),
                        event_type: "crash_loop_failed".to_string(),
                        state: history.state.clone(),
                        restart_count,
                        rebuild_count: history.rebuild_count,
                        timestamp: Utc::now(),
                        message: format!(
                            "VM {} has failed after {} rebuild attempts. Manual intervention required.",
                            vm_name, history.rebuild_count
                        ),
                    });

                    error!(
                        vm = %vm_name,
                        rebuild_attempts = history.rebuild_count,
                        "VM failed after max rebuild attempts"
                    );
                } else {
                    should_remediate = self.config.remediation_enabled;

                    notification = Some(CrashLoopNotification {
                        vm_name: vm_name.to_string(),
                        event_type: "crash_loop_detected".to_string(),
                        state: history.state.clone(),
                        restart_count,
                        rebuild_count: history.rebuild_count,
                        timestamp: Utc::now(),
                        message: format!(
                            "VM {} is in a crash loop ({} restarts in {} min). Triggering rebuild.",
                            vm_name, restart_count, self.config.window_minutes
                        ),
                    });

                    warn!(
                        vm = %vm_name,
                        restart_count = restart_count,
                        "Crash loop detected"
                    );
                }
            } else {
                should_remediate = false;
                notification = None;
            }

            // Prune old events
            history.prune_old_events(self.config.window_minutes);
        }

        // Send notification
        if let Some(notif) = notification {
            if let Some(ref tx) = self.notification_tx {
                let _ = tx.send(notif).await;
            }
        }

        // Trigger remediation outside the lock
        if should_remediate {
            self.trigger_rebuild(vm_name).await;
        }
    }

    /// Mark a VM as healthy (uptime exceeded threshold)
    pub fn mark_healthy(&self, vm_name: &str) {
        let mut histories = self.histories.write();
        if let Some(history) = histories.get_mut(vm_name) {
            if history.state == VmState::Starting || history.state == VmState::Recovering {
                history.state = VmState::Healthy;
                history.last_healthy_boot = Some(Utc::now());
                info!(vm = %vm_name, "VM marked as healthy");
            }

            // Reset counters if healthy long enough
            if history.should_reset(self.config.healthy_reset_minutes) {
                history.restart_events.clear();
                history.rebuild_count = 0;
                info!(vm = %vm_name, "VM crash counters reset after sustained healthy operation");
            }
        }
    }

    /// Trigger VM rebuild
    async fn trigger_rebuild(&self, vm_name: &str) {
        info!(vm = %vm_name, "Triggering VM rebuild");

        // Update state
        {
            let mut histories = self.histories.write();
            if let Some(history) = histories.get_mut(vm_name) {
                history.state = VmState::Rebuilding;
                history.rebuild_count += 1;
                history.last_rebuild = Some(Utc::now());
            }
        }

        // Step 1: Destroy the VM
        let destroy_result = Command::new("virsh")
            .args(["destroy", vm_name])
            .output()
            .await;

        if let Err(e) = destroy_result {
            error!(vm = %vm_name, error = %e, "Failed to destroy VM");
            return;
        }

        // Step 2: Undefine the VM
        let undefine_result = Command::new("virsh")
            .args(["undefine", vm_name, "--remove-all-storage"])
            .output()
            .await;

        if let Err(e) = undefine_result {
            error!(vm = %vm_name, error = %e, "Failed to undefine VM");
            return;
        }

        info!(vm = %vm_name, "VM destroyed and undefined");

        // Step 3: Rebuild with provision script
        let provision_result = Command::new(&self.config.provision_script)
            .args([
                vm_name,
                "--profile",
                "agentic-dev",
                "--agentshare",
                "--start",
            ])
            .output()
            .await;

        match provision_result {
            Ok(output) => {
                if output.status.success() {
                    info!(vm = %vm_name, "VM rebuilt successfully");

                    // Send rebuild notification
                    if let Some(ref tx) = self.notification_tx {
                        let rebuild_count = self
                            .histories
                            .read()
                            .get(vm_name)
                            .map(|h| h.rebuild_count)
                            .unwrap_or(0);
                        let _ = tx
                            .send(CrashLoopNotification {
                                vm_name: vm_name.to_string(),
                                event_type: "vm_rebuilt".to_string(),
                                state: VmState::Starting,
                                restart_count: 0,
                                rebuild_count,
                                timestamp: Utc::now(),
                                message: format!("VM {} has been rebuilt from base image", vm_name),
                            })
                            .await;
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    error!(vm = %vm_name, stderr = %stderr, "VM rebuild failed");

                    // Mark as failed
                    let mut histories = self.histories.write();
                    if let Some(history) = histories.get_mut(vm_name) {
                        history.state = VmState::Failed;
                    }
                }
            }
            Err(e) => {
                error!(vm = %vm_name, error = %e, "Failed to run provision script");

                let mut histories = self.histories.write();
                if let Some(history) = histories.get_mut(vm_name) {
                    history.state = VmState::Failed;
                }
            }
        }
    }

    /// Get crash history for a VM
    pub fn get_history(&self, vm_name: &str) -> Option<VmCrashHistory> {
        self.histories.read().get(vm_name).cloned()
    }

    /// Get all crash histories
    pub fn get_all_histories(&self) -> HashMap<String, VmCrashHistory> {
        self.histories.read().clone()
    }

    /// Get current state for a VM
    pub fn get_state(&self, vm_name: &str) -> Option<VmState> {
        self.histories.read().get(vm_name).map(|h| h.state.clone())
    }

    /// Persist crash histories to disk
    pub async fn persist(&self) -> anyhow::Result<()> {
        let histories = self.histories.read().clone();

        for (vm_name, history) in histories {
            let vm_dir = self.config.data_dir.join(&vm_name);
            tokio::fs::create_dir_all(&vm_dir).await?;

            let history_path = vm_dir.join("crash-history.json");
            let json = serde_json::to_string_pretty(&history)?;
            tokio::fs::write(&history_path, json).await?;
        }

        Ok(())
    }

    /// Load crash histories from disk
    pub async fn load(&self) -> anyhow::Result<()> {
        if !self.config.data_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&self.config.data_dir).await?;
        let mut loaded_histories = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let history_path = entry.path().join("crash-history.json");
                if history_path.exists() {
                    let json = tokio::fs::read_to_string(&history_path).await?;
                    if let Ok(history) = serde_json::from_str::<VmCrashHistory>(&json) {
                        loaded_histories.push(history);
                    }
                }
            }
        }

        // Update map outside of async context
        let mut histories = self.histories.write();
        for history in loaded_histories {
            histories.insert(history.vm_name.clone(), history);
        }

        Ok(())
    }
}

/// Spawn the crash loop detector as a background task
pub fn spawn_crash_loop_detector(
    config: CrashLoopConfig,
    mut event_rx: mpsc::Receiver<VmEvent>,
) -> (
    Arc<CrashLoopDetector>,
    mpsc::Receiver<CrashLoopNotification>,
    tokio::task::JoinHandle<()>,
) {
    let (notification_tx, notification_rx) = mpsc::channel(256);
    let detector = Arc::new(CrashLoopDetector::new(config).with_notifications(notification_tx));
    let detector_clone = detector.clone();

    let handle = tokio::spawn(async move {
        // Load persisted histories
        if let Err(e) = detector_clone.load().await {
            warn!(error = %e, "Failed to load crash histories");
        }

        while let Some(event) = event_rx.recv().await {
            detector_clone.process_event(&event).await;
        }
    });

    (detector, notification_rx, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_restarts_in_window() {
        let mut history = VmCrashHistory::new("test-vm".to_string());

        // Add some crash events
        for i in 0..5 {
            history.restart_events.push(CrashEvent {
                timestamp: Utc::now() - chrono::Duration::minutes(i),
                uptime_seconds: Some(10),
                reason: "crashed".to_string(),
            });
        }

        assert_eq!(history.restarts_in_window(10), 5);
        assert_eq!(history.restarts_in_window(3), 3);
    }

    #[test]
    fn test_prune_old_events() {
        let mut history = VmCrashHistory::new("test-vm".to_string());

        // Add old event
        history.restart_events.push(CrashEvent {
            timestamp: Utc::now() - chrono::Duration::hours(1),
            uptime_seconds: Some(10),
            reason: "crashed".to_string(),
        });

        // Add recent event
        history.restart_events.push(CrashEvent {
            timestamp: Utc::now(),
            uptime_seconds: Some(10),
            reason: "crashed".to_string(),
        });

        history.prune_old_events(10);

        assert_eq!(history.restart_events.len(), 1);
    }

    #[test]
    fn test_state_display() {
        assert_eq!(VmState::Healthy.to_string(), "healthy");
        assert_eq!(VmState::CrashLoop.to_string(), "crash_loop");
        assert_eq!(VmState::Failed.to_string(), "failed");
    }
}
