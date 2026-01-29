//! Agentic Sandbox - Agent gRPC Client (Rust)
//!
//! Runs inside the agent VM, connects to management server on boot.
//! Establishes bidirectional stream for commands and output streaming.

use anyhow::{Context, Result};
use chrono::Local;
use clap::Parser;
use futures::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as TokioMutex;
use std::time::Duration;
use sysinfo::{Disks, System};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::Request;
use tracing::{debug, error, info, warn};
use nix::pty::openpty;
use nix::unistd;

// =============================================================================
// CLI Arguments
// =============================================================================

#[derive(Parser, Debug)]
#[command(name = "agent-client", about = "Agentic Sandbox Agent Client")]
struct Cli {
    /// Management server address (host:port)
    #[arg(long, short = 's')]
    server: Option<String>,

    /// Agent identifier
    #[arg(long, short = 'i')]
    agent_id: Option<String>,

    /// Agent authentication secret
    #[arg(long, short = 'S')]
    secret: Option<String>,

    /// Heartbeat interval in seconds
    #[arg(long, short = 'H', default_value = "30")]
    heartbeat: u64,

    /// Environment file path
    #[arg(long, default_value = "/etc/agentic-sandbox/agent.env")]
    env_file: String,
}

// =============================================================================
// Agentshare File Logger
// =============================================================================

struct AgentshareLogger {
    run_dir: PathBuf,
    stdout_file: Mutex<Option<File>>,
    stderr_file: Mutex<Option<File>>,
    commands_file: Mutex<Option<File>>,
    enabled: bool,
}

impl AgentshareLogger {
    const INBOX_PATH: &'static str = "/mnt/inbox";

    fn new(_agent_id: &str) -> Self {
        let run_id = format!("run-{}", Local::now().format("%Y%m%d-%H%M%S"));
        let run_dir = PathBuf::from(Self::INBOX_PATH).join("runs").join(&run_id);

        Self {
            run_dir,
            stdout_file: Mutex::new(None),
            stderr_file: Mutex::new(None),
            commands_file: Mutex::new(None),
            enabled: false,
        }
    }

    fn initialize(&mut self, agent_id: &str) -> bool {
        let inbox_path = Path::new(Self::INBOX_PATH);
        if !inbox_path.exists() {
            info!("Agentshare inbox not mounted - file logging disabled");
            return false;
        }

        // Create run directory
        if let Err(e) = fs::create_dir_all(&self.run_dir) {
            error!("Failed to create run directory: {}", e);
            return false;
        }

        // Create subdirectories
        let _ = fs::create_dir_all(self.run_dir.join("outputs"));
        let _ = fs::create_dir_all(self.run_dir.join("trace"));

        // Update current symlink
        let current_link = inbox_path.join("current");
        let _ = fs::remove_file(&current_link);
        if let Err(e) = std::os::unix::fs::symlink(&self.run_dir, &current_link) {
            warn!("Failed to create current symlink: {}", e);
        }

        // Open log files
        match self.open_log_files() {
            Ok(_) => {
                self.enabled = true;
                info!("Agentshare logging initialized: {:?}", self.run_dir);
                self.write_metadata(agent_id);
                true
            }
            Err(e) => {
                error!("Failed to open log files: {}", e);
                false
            }
        }
    }

    fn open_log_files(&self) -> Result<()> {
        *self.stdout_file.lock().unwrap() = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.run_dir.join("stdout.log"))?,
        );
        *self.stderr_file.lock().unwrap() = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.run_dir.join("stderr.log"))?,
        );
        *self.commands_file.lock().unwrap() = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.run_dir.join("commands.log"))?,
        );
        Ok(())
    }

    fn write_metadata(&self, agent_id: &str) {
        let metadata = json!({
            "run_id": self.run_dir.file_name().unwrap().to_string_lossy(),
            "agent_id": agent_id,
            "started_at": Local::now().to_rfc3339(),
            "hostname": hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_default(),
        });

        if let Ok(mut f) = File::create(self.run_dir.join("metadata.json")) {
            let _ = f.write_all(serde_json::to_string_pretty(&metadata).unwrap().as_bytes());
        }
    }

    fn write_stdout(&self, data: &[u8]) {
        if !self.enabled {
            return;
        }
        if let Ok(mut guard) = self.stdout_file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = f.write_all(data);
                let _ = f.flush();
            }
        }
    }

    fn write_stderr(&self, data: &[u8]) {
        if !self.enabled {
            return;
        }
        if let Ok(mut guard) = self.stderr_file.lock() {
            if let Some(ref mut f) = *guard {
                let _ = f.write_all(data);
                let _ = f.flush();
            }
        }
    }

    fn write_command(&self, command_id: &str, command: &str, args: &[String]) {
        if !self.enabled {
            return;
        }
        if let Ok(mut guard) = self.commands_file.lock() {
            if let Some(ref mut f) = *guard {
                let timestamp = Local::now().to_rfc3339();
                let _ = writeln!(f, "[{}] [{}] {} {}", timestamp, command_id, command, args.join(" "));
                let _ = f.flush();
            }
        }
    }

    fn write_command_result(&self, command_id: &str, exit_code: i32, duration_ms: i64) {
        if !self.enabled {
            return;
        }
        if let Ok(mut guard) = self.commands_file.lock() {
            if let Some(ref mut f) = *guard {
                let timestamp = Local::now().to_rfc3339();
                let _ = writeln!(f, "[{}] [{}] EXIT {} ({}ms)", timestamp, command_id, exit_code, duration_ms);
                let _ = f.flush();
            }
        }
    }

    fn write_metrics(&self) {
        if !self.enabled {
            return;
        }
        let mut sys = System::new_all();
        sys.refresh_all();
        let disks = Disks::new_with_refreshed_list();

        let metrics = json!({
            "timestamp": Local::now().to_rfc3339(),
            "cpu_percent": sys.global_cpu_usage(),
            "memory": {
                "used_bytes": sys.used_memory(),
                "total_bytes": sys.total_memory(),
            },
            "disk": {
                "used_bytes": disks.first().map(|d| d.total_space() - d.available_space()).unwrap_or(0),
                "total_bytes": disks.first().map(|d| d.total_space()).unwrap_or(0),
            },
        });

        if let Ok(mut f) = File::create(self.run_dir.join("metrics.json")) {
            let _ = f.write_all(serde_json::to_string_pretty(&metrics).unwrap().as_bytes());
        }
    }
}

// Generated proto types
pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}

use proto::agent_service_client::AgentServiceClient;
use proto::{
    AgentMessage, AgentRegistration, AgentStatus, CommandResult, Heartbeat, ManagementMessage,
    Metrics, OutputChunk, SystemInfo,
};

/// Stdin sender for a running command
type StdinSender = mpsc::Sender<StdinData>;

/// Data to send to stdin
struct StdinData {
    data: Vec<u8>,
    eof: bool,
}

/// Control message for PTY sessions
enum PtyControlMsg {
    Resize { cols: u16, rows: u16 },
    Signal { signum: i32 },
}

/// PTY control sender
type PtyControlSender = mpsc::Sender<PtyControlMsg>;

/// Running commands with their stdin channel and optional PTY control
struct RunningCommand {
    stdin_tx: StdinSender,
    pty_control_tx: Option<PtyControlSender>,
    pid: Option<nix::unistd::Pid>,
}

/// Running commands map
type RunningCommands = Arc<TokioMutex<HashMap<String, RunningCommand>>>;

// =============================================================================
// Configuration
// =============================================================================

#[derive(Debug, Clone)]
struct AgentConfig {
    agent_id: String,
    agent_secret: String,
    server_address: String,
    heartbeat_interval: Duration,
    reconnect_delay: Duration,
    max_reconnect_delay: Duration,
}

impl AgentConfig {
    fn from_cli(cli: &Cli) -> Result<Self> {
        // Load from env file first (lowest priority)
        let env_file = &cli.env_file;
        if Path::new(env_file).exists() {
            if let Ok(contents) = std::fs::read_to_string(env_file) {
                for line in contents.lines() {
                    let line = line.trim();
                    if !line.is_empty() && !line.starts_with('#') {
                        if let Some((key, value)) = line.split_once('=') {
                            env::set_var(key.trim(), value.trim());
                        }
                    }
                }
            }
        }

        // Build config: CLI args override env vars override defaults
        let default_id = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        Ok(Self {
            agent_id: cli.agent_id.clone()
                .or_else(|| env::var("AGENT_ID").ok())
                .unwrap_or(default_id),
            agent_secret: cli.secret.clone()
                .or_else(|| env::var("AGENT_SECRET").ok())
                .unwrap_or_default(),
            server_address: cli.server.clone()
                .or_else(|| env::var("MANAGEMENT_SERVER").ok())
                .unwrap_or_else(|| "host.internal:8120".to_string()),
            heartbeat_interval: Duration::from_secs(cli.heartbeat),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_delay: Duration::from_secs(60),
        })
    }
}

// =============================================================================
// System Information
// =============================================================================

fn get_system_info() -> SystemInfo {
    let mut sys = System::new_all();
    sys.refresh_all();

    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find(|l| l.starts_with("PRETTY_NAME="))
                .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "Linux".to_string());

    let disks = Disks::new_with_refreshed_list();
    let disk_bytes = disks.first().map(|d| d.total_space() as i64).unwrap_or(0);

    SystemInfo {
        os,
        kernel: System::kernel_version().unwrap_or_default(),
        cpu_cores: sys.cpus().len() as i32,
        memory_bytes: sys.total_memory() as i64,
        disk_bytes,
    }
}

fn get_primary_ip() -> String {
    // Try to get IP by connecting to external address
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect("8.8.8.8:80")?;
            socket.local_addr()
        })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "0.0.0.0".to_string())
}

// =============================================================================
// Command Executor
// =============================================================================

async fn execute_command(
    cmd: proto::CommandRequest,
    output_tx: mpsc::Sender<AgentMessage>,
    agentshare: Option<Arc<AgentshareLogger>>,
    running_commands: RunningCommands,
) {
    let command_id = cmd.command_id.clone();
    let start = std::time::Instant::now();

    info!("[{}] Executing: {} {:?}", command_id, cmd.command, cmd.args);

    // Log to agentshare
    if let Some(ref logger) = agentshare {
        logger.write_command(&command_id, &cmd.command, &cmd.args);
    }

    let mut full_cmd = vec![cmd.command.clone()];
    full_cmd.extend(cmd.args.clone());

    // Prepend sudo if run_as specified
    if !cmd.run_as.is_empty() && cmd.run_as != env::var("USER").unwrap_or_default() {
        full_cmd.insert(0, "-u".to_string());
        full_cmd.insert(1, cmd.run_as.clone());
        full_cmd.insert(0, "sudo".to_string());
    }

    let mut process = match Command::new(&full_cmd[0])
        .args(&full_cmd[1..])
        .current_dir(if cmd.working_dir.is_empty() { "." } else { &cmd.working_dir })
        .envs(cmd.env.iter())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(p) => p,
        Err(e) => {
            error!("[{}] Failed to spawn: {}", command_id, e);
            let result = CommandResult {
                command_id: command_id.clone(),
                exit_code: -1,
                error: e.to_string(),
                duration_ms: 0,
                success: false,
            };
            let _ = output_tx.send(AgentMessage {
                payload: Some(proto::agent_message::Payload::CommandResult(result)),
            }).await;
            return;
        }
    };

    // Set up stdin channel for interactive input
    let stdin = process.stdin.take();
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<StdinData>(100);

    // Store sender in running_commands
    {
        let mut running = running_commands.lock().await;
        running.insert(command_id.clone(), RunningCommand {
            stdin_tx,
            pty_control_tx: None,
            pid: None,
        });
    }

    // Spawn task to forward stdin data to process
    let cmd_id_stdin = command_id.clone();
    let stdin_task = tokio::spawn(async move {
        if let Some(mut stdin) = stdin {
            while let Some(stdin_data) = stdin_rx.recv().await {
                if let Err(e) = stdin.write_all(&stdin_data.data).await {
                    error!("[{}] Failed to write stdin: {}", cmd_id_stdin, e);
                    break;
                }
                if stdin_data.eof {
                    info!("[{}] Closing stdin (EOF)", cmd_id_stdin);
                    drop(stdin);
                    break;
                }
            }
        }
    });

    // Stream stdout
    let stdout = process.stdout.take();
    let stderr = process.stderr.take();
    let cmd_id_stdout = command_id.clone();
    let cmd_id_stderr = command_id.clone();
    let tx_stdout = output_tx.clone();
    let tx_stderr = output_tx.clone();
    let agentshare_stdout = agentshare.clone();
    let agentshare_stderr = agentshare.clone();

    let stdout_task = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let data = format!("{}\n", line).into_bytes();
                // Write to agentshare
                if let Some(ref logger) = agentshare_stdout {
                    logger.write_stdout(&data);
                }
                let chunk = OutputChunk {
                    stream_id: cmd_id_stdout.clone(),
                    data,
                    timestamp_ms: chrono_timestamp_ms(),
                    eof: false,
                };
                let _ = tx_stdout.send(AgentMessage {
                    payload: Some(proto::agent_message::Payload::Stdout(chunk)),
                }).await;
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let data = format!("{}\n", line).into_bytes();
                // Write to agentshare
                if let Some(ref logger) = agentshare_stderr {
                    logger.write_stderr(&data);
                }
                let chunk = OutputChunk {
                    stream_id: cmd_id_stderr.clone(),
                    data,
                    timestamp_ms: chrono_timestamp_ms(),
                    eof: false,
                };
                let _ = tx_stderr.send(AgentMessage {
                    payload: Some(proto::agent_message::Payload::Stderr(chunk)),
                }).await;
            }
        }
    });

    // Wait for completion with optional timeout
    let exit_status = if cmd.timeout_seconds > 0 {
        tokio::time::timeout(
            Duration::from_secs(cmd.timeout_seconds as u64),
            process.wait(),
        )
        .await
        .map_err(|_| {
            let _ = process.kill();
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Command timed out")
        })
        .and_then(|r| r)
    } else {
        process.wait().await
    };

    // Wait for output streams and stdin task to finish
    let _ = tokio::join!(stdout_task, stderr_task, stdin_task);

    let duration_ms = start.elapsed().as_millis() as i64;
    let (exit_code, error_msg, success) = match exit_status {
        Ok(status) => (
            status.code().unwrap_or(-1),
            String::new(),
            status.success(),
        ),
        Err(e) => (-1, e.to_string(), false),
    };

    info!("[{}] Completed: exit={}, duration={}ms", command_id, exit_code, duration_ms);

    // Remove stdin channel from running commands
    running_commands.lock().await.remove(&command_id);

    // Log to agentshare
    if let Some(ref logger) = agentshare {
        logger.write_command_result(&command_id, exit_code, duration_ms);
    }

    let result = CommandResult {
        command_id,
        exit_code,
        error: error_msg,
        duration_ms,
        success,
    };
    let _ = output_tx.send(AgentMessage {
        payload: Some(proto::agent_message::Payload::CommandResult(result)),
    }).await;
}

// =============================================================================
// PTY Command Executor
// =============================================================================

async fn execute_command_pty(
    cmd: proto::CommandRequest,
    output_tx: mpsc::Sender<AgentMessage>,
    agentshare: Option<Arc<AgentshareLogger>>,
    running_commands: RunningCommands,
) {
    let command_id = cmd.command_id.clone();
    let start = std::time::Instant::now();

    info!("[{}] Executing (PTY): {} {:?}", command_id, cmd.command, cmd.args);

    if let Some(ref logger) = agentshare {
        logger.write_command(&command_id, &cmd.command, &cmd.args);
    }

    // Determine terminal size
    let cols = if cmd.pty_cols > 0 { cmd.pty_cols as u16 } else { 80 };
    let rows = if cmd.pty_rows > 0 { cmd.pty_rows as u16 } else { 24 };
    let term_env = if cmd.pty_term.is_empty() { "xterm-256color".to_string() } else { cmd.pty_term.clone() };

    // Open PTY pair
    let pty_result = openpty(None, None);
    let pty = match pty_result {
        Ok(pty) => pty,
        Err(e) => {
            error!("[{}] Failed to open PTY: {}", command_id, e);
            let result = CommandResult {
                command_id: command_id.clone(),
                exit_code: -1,
                error: format!("Failed to open PTY: {}", e),
                duration_ms: 0,
                success: false,
            };
            let _ = output_tx.send(AgentMessage {
                payload: Some(proto::agent_message::Payload::CommandResult(result)),
            }).await;
            return;
        }
    };

    let master_fd = pty.master;
    let slave_fd = pty.slave;

    // Set initial window size
    let winsize = nix::pty::Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(master_fd.as_raw_fd(), libc::TIOCSWINSZ, &winsize);
    }

    // Build command
    let shell_cmd = if cmd.args.is_empty() {
        cmd.command.clone()
    } else {
        format!("{} {}", cmd.command, cmd.args.join(" "))
    };

    // Fork child process
    let child_pid = match unsafe { unistd::fork() } {
        Ok(unistd::ForkResult::Child) => {
            // Child process: set up PTY slave as controlling terminal
            drop(master_fd); // Close master in child

            // Create new session
            let _ = unistd::setsid();

            // Set slave as controlling terminal
            unsafe {
                libc::ioctl(slave_fd.as_raw_fd(), libc::TIOCSCTTY, 0);
            }

            // Redirect stdio to slave
            let slave_raw = slave_fd.as_raw_fd();
            let _ = unistd::dup2(slave_raw, 0); // stdin
            let _ = unistd::dup2(slave_raw, 1); // stdout
            let _ = unistd::dup2(slave_raw, 2); // stderr
            if slave_raw > 2 {
                drop(slave_fd);
            }

            // Set TERM
            std::env::set_var("TERM", &term_env);

            // Set env vars from command
            for (key, value) in &cmd.env {
                std::env::set_var(key, value);
            }

            // Change working directory
            if !cmd.working_dir.is_empty() {
                let _ = std::env::set_current_dir(&cmd.working_dir);
            }

            // Exec via shell
            let c_shell = std::ffi::CString::new("/bin/bash").unwrap();
            let c_arg0 = std::ffi::CString::new("-bash").unwrap();
            let c_cmd = std::ffi::CString::new(format!("-c")).unwrap();
            let c_script = std::ffi::CString::new(shell_cmd.as_str()).unwrap();

            if cmd.args.is_empty() && (cmd.command == "/bin/bash" || cmd.command == "bash" || cmd.command == "/bin/sh" || cmd.command == "sh") {
                // Interactive shell — exec directly as login shell
                let _ = unistd::execvp(&c_shell, &[&c_arg0]);
            } else {
                // Run command via bash -c
                let _ = unistd::execvp(&c_shell, &[&c_arg0, &c_cmd, &c_script]);
            }

            // If exec fails
            std::process::exit(127);
        }
        Ok(unistd::ForkResult::Parent { child }) => {
            // Parent: close slave side
            drop(slave_fd);
            child
        }
        Err(e) => {
            error!("[{}] Fork failed: {}", command_id, e);
            let result = CommandResult {
                command_id: command_id.clone(),
                exit_code: -1,
                error: format!("Fork failed: {}", e),
                duration_ms: 0,
                success: false,
            };
            let _ = output_tx.send(AgentMessage {
                payload: Some(proto::agent_message::Payload::CommandResult(result)),
            }).await;
            return;
        }
    };

    info!("[{}] Child PID: {}", command_id, child_pid);

    // Use blocking I/O with dedicated threads to avoid busy-loop.
    // tokio::fs::File uses the blocking threadpool (NOT epoll), so O_NONBLOCK
    // causes WouldBlock -> yield -> retry hot loops that starve the runtime.
    let master_raw = master_fd.as_raw_fd();

    // Dup master fd: read_fd for blocking reads, write_fd for writes
    let write_fd = unsafe { libc::dup(master_raw) };
    if write_fd < 0 {
        error!("[{}] Failed to dup master fd: {}", command_id, std::io::Error::last_os_error());
        let result = CommandResult {
            command_id: command_id.clone(),
            exit_code: -1,
            error: "Failed to dup PTY master fd".to_string(),
            duration_ms: 0,
            success: false,
        };
        let _ = output_tx.send(AgentMessage {
            payload: Some(proto::agent_message::Payload::CommandResult(result)),
        }).await;
        return;
    }

    // Prevent OwnedFd from closing master_raw on drop
    let read_fd = master_raw;
    std::mem::forget(master_fd);

    // Set up stdin and pty_control channels
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<StdinData>(256);
    let (pty_ctl_tx, mut pty_ctl_rx) = mpsc::channel::<PtyControlMsg>(32);

    // Register running command
    {
        let mut running = running_commands.lock().await;
        running.insert(command_id.clone(), RunningCommand {
            stdin_tx,
            pty_control_tx: Some(pty_ctl_tx),
            pid: Some(child_pid),
        });
    }

    // Task: blocking read on dedicated thread → stream output via mpsc
    let cmd_id_out = command_id.clone();
    let tx_out = output_tx.clone();
    let agentshare_out = agentshare.clone();
    let output_task = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                // EOF (0) or error (-1, typically EIO when child exits)
                break;
            }
            let data = buf[..n as usize].to_vec();
            if let Some(ref logger) = agentshare_out {
                logger.write_stdout(&data);
            }
            let chunk = OutputChunk {
                stream_id: cmd_id_out.clone(),
                data,
                timestamp_ms: chrono_timestamp_ms(),
                eof: false,
            };
            if tx_out.blocking_send(AgentMessage {
                payload: Some(proto::agent_message::Payload::Stdout(chunk)),
            }).is_err() {
                break;
            }
        }
    });

    // Task: stdin → write to master via libc::write (small writes, won't block)
    let cmd_id_in = command_id.clone();
    let stdin_task = tokio::spawn(async move {
        while let Some(stdin_data) = stdin_rx.recv().await {
            let data = stdin_data.data;
            let eof = stdin_data.eof;
            let result = unsafe {
                libc::write(write_fd, data.as_ptr() as *const libc::c_void, data.len())
            };
            if result < 0 {
                debug!("[{}] Master write error: {}", cmd_id_in, std::io::Error::last_os_error());
                break;
            }
            if eof {
                break;
            }
        }
    });

    // Task: PTY control (resize, signal)
    let cmd_id_ctl = command_id.clone();
    let ctl_task = tokio::spawn(async move {
        while let Some(msg) = pty_ctl_rx.recv().await {
            match msg {
                PtyControlMsg::Resize { cols, rows } => {
                    debug!("[{}] PTY resize: {}x{}", cmd_id_ctl, cols, rows);
                    let winsize = nix::pty::Winsize {
                        ws_row: rows,
                        ws_col: cols,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    };
                    unsafe {
                        libc::ioctl(master_raw, libc::TIOCSWINSZ, &winsize);
                    }
                    // Send SIGWINCH to child process group
                    let _ = nix::sys::signal::kill(child_pid, nix::sys::signal::Signal::SIGWINCH);
                }
                PtyControlMsg::Signal { signum } => {
                    debug!("[{}] Sending signal {} to child", cmd_id_ctl, signum);
                    if let Ok(sig) = nix::sys::signal::Signal::try_from(signum) {
                        let _ = nix::sys::signal::kill(child_pid, sig);
                    }
                }
            }
        }
    });

    // Wait for child process
    let exit_status = tokio::task::spawn_blocking(move || {
        use nix::sys::wait::{waitpid, WaitStatus};
        match waitpid(child_pid, None) {
            Ok(WaitStatus::Exited(_, code)) => code,
            Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
            _ => -1,
        }
    }).await.unwrap_or(-1);

    // Wait for blocking read to finish (returns EOF/EIO after child exits)
    let _ = tokio::time::timeout(Duration::from_secs(2), output_task).await;

    // Abort async tasks before closing fds
    stdin_task.abort();
    ctl_task.abort();

    // Close PTY master file descriptors
    unsafe {
        libc::close(read_fd);
        libc::close(write_fd);
    }

    let duration_ms = start.elapsed().as_millis() as i64;
    let success = exit_status == 0;

    info!("[{}] PTY completed: exit={}, duration={}ms", command_id, exit_status, duration_ms);

    // Remove from running commands
    running_commands.lock().await.remove(&command_id);

    if let Some(ref logger) = agentshare {
        logger.write_command_result(&command_id, exit_status, duration_ms);
    }

    // Send EOF marker
    let _ = output_tx.send(AgentMessage {
        payload: Some(proto::agent_message::Payload::Stdout(OutputChunk {
            stream_id: command_id.clone(),
            data: vec![],
            timestamp_ms: chrono_timestamp_ms(),
            eof: true,
        })),
    }).await;

    let result = CommandResult {
        command_id,
        exit_code: exit_status,
        error: String::new(),
        duration_ms,
        success,
    };
    let _ = output_tx.send(AgentMessage {
        payload: Some(proto::agent_message::Payload::CommandResult(result)),
    }).await;
}

fn chrono_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// =============================================================================
// Agent Client
// =============================================================================

struct AgentClient {
    config: AgentConfig,
    output_tx: mpsc::Sender<AgentMessage>,
    output_rx: Option<mpsc::Receiver<AgentMessage>>,
    agentshare: Option<Arc<AgentshareLogger>>,
    running_commands: RunningCommands,
}

impl AgentClient {
    fn new(config: AgentConfig) -> Self {
        let (tx, rx) = mpsc::channel(1000);

        // Initialize agentshare logger
        let mut logger = AgentshareLogger::new(&config.agent_id);
        let agentshare = if logger.initialize(&config.agent_id) {
            Some(Arc::new(logger))
        } else {
            None
        };

        Self {
            config,
            output_tx: tx,
            output_rx: Some(rx),
            agentshare,
            running_commands: Arc::new(TokioMutex::new(HashMap::new())),
        }
    }

    async fn connect(&self) -> Result<AgentServiceClient<Channel>> {
        info!("Connecting to {}...", self.config.server_address);

        let channel = Channel::from_shared(format!("http://{}", self.config.server_address))?
            .connect()
            .await
            .context("Failed to connect to management server")?;

        info!("Connected to management server");
        Ok(AgentServiceClient::new(channel))
    }

    fn create_registration(&self) -> AgentMessage {
        let reg = AgentRegistration {
            agent_id: self.config.agent_id.clone(),
            ip_address: get_primary_ip(),
            hostname: hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_default(),
            profile: env::var("AGENT_PROFILE").unwrap_or_else(|_| "basic".to_string()),
            labels: HashMap::new(),
            system: Some(get_system_info()),
        };
        AgentMessage {
            payload: Some(proto::agent_message::Payload::Registration(reg)),
        }
    }

    async fn run(&mut self) -> Result<()> {
        let mut reconnect_delay = self.config.reconnect_delay;

        // Spawn metrics writer if agentshare enabled
        if let Some(ref agentshare) = self.agentshare {
            let logger = agentshare.clone();
            tokio::spawn(async move {
                let mut interval = interval(Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    logger.write_metrics();
                }
            });
        }

        loop {
            match self.connect().await {
                Ok(mut client) => {
                    reconnect_delay = self.config.reconnect_delay;

                    if let Err(e) = self.stream_loop(&mut client).await {
                        error!("Stream error: {}", e);
                    }
                }
                Err(e) => {
                    error!("Connection failed: {}", e);
                }
            }

            info!("Retrying in {:?}...", reconnect_delay);
            sleep(reconnect_delay).await;
            reconnect_delay = std::cmp::min(
                reconnect_delay * 2,
                self.config.max_reconnect_delay,
            );
        }
    }

    async fn stream_loop(&mut self, client: &mut AgentServiceClient<Channel>) -> Result<()> {
        info!("Starting bidirectional stream...");

        let output_rx = self.output_rx.take().context("Output receiver already taken")?;
        let output_tx = self.output_tx.clone();
        let config = self.config.clone();

        // Create outbound message stream
        let (msg_tx, msg_rx) = mpsc::channel::<AgentMessage>(100);
        let heartbeat_interval = config.heartbeat_interval;

        // Spawn heartbeat + metrics sender
        let heartbeat_tx = msg_tx.clone();
        let agent_id = config.agent_id.clone();
        tokio::spawn(async move {
            let mut interval = interval(heartbeat_interval);
            loop {
                interval.tick().await;
                let mut sys = System::new_all();
                sys.refresh_all();
                let disks = Disks::new_with_refreshed_list();

                let cpu = sys.global_cpu_usage();
                let mem_used = sys.used_memory() as i64;
                let mem_total = sys.total_memory() as i64;
                let disk_used = disks.first().map(|d| (d.total_space() - d.available_space()) as i64).unwrap_or(0);
                let disk_total = disks.first().map(|d| d.total_space() as i64).unwrap_or(0);
                let load = System::load_average();
                let uptime = System::uptime() as i64;

                // Send heartbeat (liveness)
                let hb = Heartbeat {
                    agent_id: agent_id.clone(),
                    timestamp_ms: chrono_timestamp_ms(),
                    status: AgentStatus::Ready as i32,
                    cpu_percent: cpu,
                    memory_used_bytes: mem_used,
                    uptime_seconds: uptime,
                };
                if heartbeat_tx.send(AgentMessage {
                    payload: Some(proto::agent_message::Payload::Heartbeat(hb)),
                }).await.is_err() {
                    break;
                }

                // Send full metrics (dashboard display)
                let metrics = Metrics {
                    agent_id: agent_id.clone(),
                    timestamp_ms: chrono_timestamp_ms(),
                    cpu_percent: cpu,
                    memory_used_bytes: mem_used,
                    memory_total_bytes: mem_total,
                    disk_used_bytes: disk_used,
                    disk_total_bytes: disk_total,
                    load_avg: vec![load.one as f32, load.five as f32, load.fifteen as f32],
                    custom: std::collections::HashMap::new(),
                };
                if heartbeat_tx.send(AgentMessage {
                    payload: Some(proto::agent_message::Payload::Metrics(metrics)),
                }).await.is_err() {
                    break;
                }
            }
        });

        // Forward output queue to message stream
        let fwd_tx = msg_tx.clone();
        tokio::spawn(async move {
            let mut rx = output_rx;
            while let Some(msg) = rx.recv().await {
                if fwd_tx.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Send registration first
        msg_tx.send(self.create_registration()).await?;
        info!("Sent registration for {}", config.agent_id);

        // Create request with auth metadata
        let mut request = Request::new(ReceiverStream::new(msg_rx));
        request.metadata_mut().insert(
            "x-agent-id",
            MetadataValue::try_from(&config.agent_id)?,
        );
        request.metadata_mut().insert(
            "x-agent-secret",
            MetadataValue::try_from(&config.agent_secret)?,
        );

        // Open stream
        let mut response = client.connect(request).await?.into_inner();

        // Process inbound messages
        while let Some(msg) = response.next().await {
            match msg {
                Ok(msg) => self.handle_inbound(msg, output_tx.clone()).await,
                Err(e) => {
                    error!("Receive error: {}", e);
                    break;
                }
            }
        }

        // Put back the receiver for next connection
        let (tx, rx) = mpsc::channel(1000);
        self.output_tx = tx;
        self.output_rx = Some(rx);

        Ok(())
    }

    async fn handle_inbound(&self, msg: ManagementMessage, output_tx: mpsc::Sender<AgentMessage>) {
        use proto::management_message::Payload;

        match msg.payload {
            Some(Payload::RegistrationAck(ack)) => {
                if ack.accepted {
                    info!("Registration accepted: {}", ack.message);
                } else {
                    error!("Registration rejected: {}", ack.message);
                }
            }
            Some(Payload::Command(cmd)) => {
                info!("Received command: {} - {} (pty={})", cmd.command_id, cmd.command, cmd.allocate_pty);
                let agentshare = self.agentshare.clone();
                let running_commands = self.running_commands.clone();
                if cmd.allocate_pty {
                    tokio::spawn(execute_command_pty(cmd, output_tx, agentshare, running_commands));
                } else {
                    tokio::spawn(execute_command(cmd, output_tx, agentshare, running_commands));
                }
            }
            Some(Payload::Config(cfg)) => {
                info!("Config update received");
                for (key, value) in cfg.config {
                    env::set_var(&key, &value);
                }
            }
            Some(Payload::Shutdown(sig)) => {
                warn!("Shutdown signal received: {}", sig.reason);
                tokio::time::sleep(Duration::from_secs(sig.grace_period_seconds as u64)).await;
                std::process::exit(0);
            }
            Some(Payload::Ping(_)) => {
                // Could send heartbeat in response
            }
            Some(Payload::Stdin(stdin_chunk)) => {
                let command_id = stdin_chunk.command_id.clone();
                debug!("Received stdin for command {}: {} bytes", command_id, stdin_chunk.data.len());

                // Clone sender and drop lock before await to prevent stalling
                let stdin_tx = {
                    let running = self.running_commands.lock().await;
                    running.get(&command_id).map(|rc| rc.stdin_tx.clone())
                };
                if let Some(tx) = stdin_tx {
                    let stdin_data = StdinData {
                        data: stdin_chunk.data,
                        eof: stdin_chunk.eof,
                    };
                    if tx.send(stdin_data).await.is_err() {
                        warn!("Failed to send stdin to command {}: channel closed", command_id);
                    }
                } else {
                    warn!("Cannot write stdin: command {} not found", command_id);
                }
            }
            Some(Payload::PtyControl(ctl)) => {
                let command_id = ctl.command_id.clone();
                debug!("Received PTY control for command {}", command_id);

                // Clone sender and drop lock before await
                let pty_tx = {
                    let running = self.running_commands.lock().await;
                    running.get(&command_id).and_then(|rc| rc.pty_control_tx.clone())
                };
                if let Some(tx) = pty_tx {
                    let msg = match ctl.action {
                        Some(proto::pty_control::Action::Resize(r)) => {
                            PtyControlMsg::Resize { cols: r.cols as u16, rows: r.rows as u16 }
                        }
                        Some(proto::pty_control::Action::Signal(s)) => {
                            PtyControlMsg::Signal { signum: s.signal_number }
                        }
                        None => return,
                    };
                    if tx.send(msg).await.is_err() {
                        warn!("PTY control channel closed for {}", command_id);
                    }
                } else {
                    debug!("Command {} not found or not a PTY session", command_id);
                }
            }
            None => {}
        }
    }
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("agent_client=info".parse()?)
        )
        .init();

    let cli = Cli::parse();
    let config = AgentConfig::from_cli(&cli)?;

    if config.agent_id.is_empty() {
        anyhow::bail!("AGENT_ID required (use --agent-id or AGENT_ID env var)");
    }
    if config.agent_secret.is_empty() {
        warn!("AGENT_SECRET not set - authentication may fail (use --secret or AGENT_SECRET env var)");
    }

    info!("Starting agent: {}", config.agent_id);
    info!("Management server: {}", config.server_address);

    let mut client = AgentClient::new(config);
    client.run().await
}
