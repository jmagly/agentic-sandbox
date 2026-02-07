//! Library module for agent-client to expose types for testing

pub use std::time::Duration;
pub use tokio::sync::mpsc;

/// Stdin sender for a running command
pub type StdinSender = mpsc::Sender<StdinData>;

/// Data to send to stdin
pub struct StdinData {
    pub data: Vec<u8>,
    pub eof: bool,
}

/// Control message for PTY sessions
pub enum PtyControlMsg {
    Resize { cols: u16, rows: u16 },
    Signal { signum: i32 },
}

/// PTY control sender
pub type PtyControlSender = mpsc::Sender<PtyControlMsg>;

/// Running commands with their stdin channel and optional PTY control
#[allow(dead_code)] // pid reserved for future signal handling
pub struct RunningCommand {
    pub stdin_tx: StdinSender,
    pub pty_control_tx: Option<PtyControlSender>,
    pub pid: Option<nix::unistd::Pid>,
}

/// Running commands map
pub type RunningCommands = std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, RunningCommand>>>;

/// Agent configuration
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub agent_id: String,
    pub agent_secret: String,
    pub server_address: String,
    pub heartbeat_interval: Duration,
    pub reconnect_delay: Duration,
    pub max_reconnect_delay: Duration,
}

/// Agent client (minimal public interface for testing)
pub struct AgentClient {
    pub running_commands: RunningCommands,
}

impl AgentClient {
    pub fn new(config: AgentConfig) -> Self {
        use std::collections::HashMap;
        Self {
            running_commands: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Clean up all running PTY sessions and clear the running commands map
    pub async fn cleanup_sessions(&self) {
        use nix::sys::signal;
        use tokio::process::Command;
        use tracing::{debug, info, warn};

        info!("Cleaning up running PTY sessions on disconnect");

        let mut running = self.running_commands.lock().await;
        let session_count = running.len();

        if session_count > 0 {
            info!("Killing {} running session(s)", session_count);

            // Send SIGTERM to all tracked PIDs
            for (cmd_id, cmd) in running.iter() {
                if let Some(pid) = cmd.pid {
                    debug!("[{}] Sending SIGTERM to PID {}", cmd_id, pid);
                    let _ = signal::kill(pid, signal::Signal::SIGTERM);
                }
            }

            // Clear the running commands map
            running.clear();
        }

        drop(running); // Release lock before running killall

        // Safety net: kill any remaining tmux sessions
        let killall_result = Command::new("pkill")
            .args(&["-TERM", "tmux"])
            .output()
            .await;

        match killall_result {
            Ok(output) => {
                if output.status.success() {
                    debug!("Successfully killed remaining tmux sessions");
                }
            }
            Err(e) => {
                warn!("Failed to run pkill for cleanup: {}", e);
            }
        }

        info!("Session cleanup complete");
    }
}
