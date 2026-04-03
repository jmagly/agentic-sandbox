//! Task output monitoring
//!
//! Monitors task progress files and broadcasts updates.

use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::File;
#[allow(unused_imports)] // Used for file reading
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, info, warn};

/// Output event from task monitoring
#[derive(Debug, Clone)]
pub enum TaskOutputEvent {
    Stdout(String, Vec<u8>),          // task_id, data
    Stderr(String, Vec<u8>),          // task_id, data
    Event(String, serde_json::Value), // task_id, structured event
    Completed(String, i32),           // task_id, exit_code
    Error(String, String),            // task_id, error message
}

/// Monitors task output files for real-time streaming
pub struct TaskMonitor {
    tasks_root: PathBuf,
    /// Active monitors per task
    monitors: RwLock<HashMap<String, MonitorHandle>>,
    /// Broadcast sender for output events (subscribers connect here)
    event_tx: broadcast::Sender<TaskOutputEvent>,
}

struct MonitorHandle {
    stop_tx: mpsc::Sender<()>,
}

impl TaskMonitor {
    pub fn new(tasks_root: String) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            tasks_root: PathBuf::from(tasks_root),
            monitors: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    /// Subscribe to task output events
    pub fn subscribe(&self) -> broadcast::Receiver<TaskOutputEvent> {
        self.event_tx.subscribe()
    }

    /// Start monitoring a task's output files
    pub async fn start_monitoring(&self, task_id: &str) -> bool {
        let mut monitors = self.monitors.write().await;

        if monitors.contains_key(task_id) {
            debug!("Task {} already being monitored", task_id);
            return false;
        }

        let (stop_tx, stop_rx) = mpsc::channel(1);
        let task_id = task_id.to_string();
        let progress_dir = self
            .tasks_root
            .join(&task_id)
            .join("outbox")
            .join("progress");
        let event_tx = self.event_tx.clone();

        // Spawn monitor task
        tokio::spawn(Self::monitor_task(
            task_id.clone(),
            progress_dir,
            event_tx,
            stop_rx,
        ));

        monitors.insert(task_id.clone(), MonitorHandle { stop_tx });
        info!("Started monitoring task {}", task_id);
        true
    }

    /// Stop monitoring a task
    pub async fn stop_monitoring(&self, task_id: &str) {
        let mut monitors = self.monitors.write().await;

        if let Some(handle) = monitors.remove(task_id) {
            let _ = handle.stop_tx.send(()).await;
            info!("Stopped monitoring task {}", task_id);
        }
    }

    /// Internal monitor task that tails output files
    async fn monitor_task(
        task_id: String,
        progress_dir: PathBuf,
        event_tx: broadcast::Sender<TaskOutputEvent>,
        mut stop_rx: mpsc::Receiver<()>,
    ) {
        let stdout_path = progress_dir.join("stdout.log");
        let stderr_path = progress_dir.join("stderr.log");
        let events_path = progress_dir.join("events.jsonl");

        // Open files (wait for them to exist)
        let stdout_file = Self::wait_for_file(&stdout_path).await;
        let stderr_file = Self::wait_for_file(&stderr_path).await;
        let events_file = Self::wait_for_file(&events_path).await;

        // Track positions
        let mut stdout_pos = 0u64;
        let mut stderr_pos = 0u64;
        let mut events_pos = 0u64;

        loop {
            tokio::select! {
                _ = stop_rx.recv() => {
                    debug!("Monitor stopping for task {}", task_id);
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                    // Check stdout
                    if let Some(_file) = stdout_file {
                        if let Ok((data, new_pos)) = Self::read_new_data(&stdout_path, stdout_pos).await {
                            if !data.is_empty() {
                                let _ = event_tx.send(TaskOutputEvent::Stdout(task_id.clone(), data));
                            }
                            stdout_pos = new_pos;
                        }
                    }

                    // Check stderr
                    if let Some(_file) = stderr_file {
                        if let Ok((data, new_pos)) = Self::read_new_data(&stderr_path, stderr_pos).await {
                            if !data.is_empty() {
                                let _ = event_tx.send(TaskOutputEvent::Stderr(task_id.clone(), data));
                            }
                            stderr_pos = new_pos;
                        }
                    }

                    // Check events
                    if let Some(_file) = events_file {
                        if let Ok((data, new_pos)) = Self::read_new_data(&events_path, events_pos).await {
                            if !data.is_empty() {
                                // Parse JSONL
                                let text = String::from_utf8_lossy(&data);
                                for line in text.lines() {
                                    if let Ok(event) = serde_json::from_str(line) {
                                        let _ = event_tx.send(TaskOutputEvent::Event(task_id.clone(), event));
                                    }
                                }
                            }
                            events_pos = new_pos;
                        }
                    }
                }
            }
        }
    }

    /// Wait for a file to exist (up to 30s)
    async fn wait_for_file(path: &PathBuf) -> Option<()> {
        for _ in 0..300 {
            if path.exists() {
                return Some(());
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        warn!("File {:?} did not appear", path);
        None
    }

    /// Read new data from file since last position
    async fn read_new_data(path: &PathBuf, from_pos: u64) -> std::io::Result<(Vec<u8>, u64)> {
        let metadata = tokio::fs::metadata(path).await?;
        let file_len = metadata.len();

        if file_len <= from_pos {
            return Ok((Vec::new(), from_pos));
        }

        let mut file = File::open(path).await?;
        file.seek(SeekFrom::Start(from_pos)).await?;

        let to_read = (file_len - from_pos) as usize;
        let mut buffer = vec![0u8; to_read];
        tokio::io::AsyncReadExt::read_exact(&mut file, &mut buffer).await?;

        Ok((buffer, file_len))
    }

    /// Check if a task is being monitored
    pub async fn is_monitoring(&self, task_id: &str) -> bool {
        self.monitors.read().await.contains_key(task_id)
    }

    /// Get number of active monitors
    pub async fn active_count(&self) -> usize {
        self.monitors.read().await.len()
    }
}
