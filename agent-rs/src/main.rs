//! Agentic Sandbox - Agent gRPC Client (Rust)
//!
//! Runs inside the agent VM, connects to management server on boot.
//! Establishes bidirectional stream for commands and output streaming.

use anyhow::{Context, Result};
use chrono::Local;
use clap::{Parser, ValueEnum};
use futures::StreamExt;
use hyper::rt::{Read as HyperRead, ReadBufCursor, Write as HyperWrite};
use hyper_util::rt::TokioIo;
use nix::pty::openpty;
use nix::unistd;
use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};
use sysinfo::{Disks, System};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::Notify;
use tokio::time::{interval, sleep};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::{MetadataMap, MetadataValue};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tonic::Request;
use tower::service_fn;
use tracing::{debug, error, info, warn};

#[derive(Debug)]
struct TonicVsockIo {
    inner: TokioIo<tokio_vsock::VsockStream>,
}

impl TonicVsockIo {
    fn new(stream: tokio_vsock::VsockStream) -> Self {
        Self {
            inner: TokioIo::new(stream),
        }
    }
}

impl HyperRead for TonicVsockIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl HyperWrite for TonicVsockIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

impl AsyncRead for TonicVsockIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_read(cx, buf)
    }
}

impl AsyncWrite for TonicVsockIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        tokio::io::AsyncWrite::is_write_vectored(self.inner.inner())
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_write_vectored(cx, bufs)
    }
}

// Internal modules
mod claude;
mod credentials;
mod health;
mod metrics;

// =============================================================================
// CLI Arguments
// =============================================================================

#[derive(Parser, Debug, Clone)]
#[command(name = "agent-client", about = "Agentic Sandbox Agent Client")]
struct Cli {
    /// Management server address (host:port)
    #[arg(long, short = 's')]
    server: Option<String>,

    /// Agent identifier
    #[arg(long, short = 'i')]
    agent_id: Option<String>,

    /// Management gRPC Unix socket path.
    #[arg(long)]
    uds_path: Option<String>,

    /// Management server vsock CID.
    #[arg(long)]
    vsock_cid: Option<u32>,

    /// Management server vsock port.
    #[arg(long)]
    vsock_port: Option<u32>,

    /// CA bundle path for management gRPC mTLS.
    #[arg(long)]
    tls_ca: Option<String>,

    /// Client certificate path for management gRPC mTLS.
    #[arg(long)]
    tls_cert: Option<String>,

    /// Client private key path for management gRPC mTLS.
    #[arg(long)]
    tls_key: Option<String>,

    /// Server name used to verify the management gRPC mTLS certificate.
    #[arg(long)]
    tls_server_name: Option<String>,

    /// Management gRPC transport mode: tcp, uds, vsock, tls, or auto.
    #[arg(long, value_enum)]
    transport: Option<TransportMode>,

    /// Heartbeat interval in seconds
    #[arg(long, short = 'H', default_value = "5")]
    heartbeat: u64,

    /// Environment file path
    #[arg(long, default_value = "/etc/agentic-sandbox/agent.env")]
    env_file: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TransportMode {
    /// Always connect over TCP/h2c.
    Tcp,
    /// Always connect over the configured Unix socket.
    Uds,
    /// Always connect over the configured vsock CID/port.
    Vsock,
    /// Always connect over TCP with mTLS.
    Tls,
    /// Use UDS when a socket path is configured, otherwise TCP.
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedTransport {
    Tcp,
    Uds,
    Vsock,
    Tls,
}

impl TransportMode {
    fn from_env_value(raw: &str) -> Result<Self> {
        TransportMode::from_str(raw, true).map_err(|_| {
            anyhow::anyhow!(
                "invalid AGENT_TRANSPORT `{raw}`; expected tcp, uds, vsock, tls, or auto"
            )
        })
    }

    fn resolve(
        self,
        uds_path: Option<&str>,
        vsock: Option<(u32, u32)>,
        tls_configured: bool,
    ) -> Result<ResolvedTransport> {
        match self {
            TransportMode::Tcp => Ok(ResolvedTransport::Tcp),
            TransportMode::Uds => {
                if uds_path.is_some() {
                    Ok(ResolvedTransport::Uds)
                } else {
                    anyhow::bail!("transport mode `uds` requires --uds-path or AGENT_GRPC_UDS_PATH")
                }
            }
            TransportMode::Vsock => {
                if vsock.is_some() {
                    Ok(ResolvedTransport::Vsock)
                } else {
                    anyhow::bail!(
                        "transport mode `vsock` requires --vsock-cid/--vsock-port or AGENT_GRPC_VSOCK_CID/AGENT_GRPC_VSOCK_PORT"
                    )
                }
            }
            TransportMode::Tls => {
                if tls_configured {
                    Ok(ResolvedTransport::Tls)
                } else {
                    anyhow::bail!(
                        "transport mode `tls` requires --tls-ca/--tls-cert/--tls-key or AGENT_GRPC_TLS_CA/AGENT_GRPC_TLS_CERT/AGENT_GRPC_TLS_KEY"
                    )
                }
            }
            TransportMode::Auto => {
                if uds_path.is_some() {
                    Ok(ResolvedTransport::Uds)
                } else if vsock.is_some() {
                    Ok(ResolvedTransport::Vsock)
                } else if tls_configured {
                    Ok(ResolvedTransport::Tls)
                } else {
                    Ok(ResolvedTransport::Tcp)
                }
            }
        }
    }
}

// =============================================================================
// Agentshare File Logger
// =============================================================================

struct AgentshareLogger {
    run_dir: PathBuf,
    stdout_file: Mutex<Option<File>>,
    stderr_file: Mutex<Option<File>>,
    commands_file: Mutex<Option<File>>,
    last_cgroup_cpu: Mutex<Option<(Instant, u64)>>,
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
            last_cgroup_cpu: Mutex::new(None),
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
                let _ = writeln!(
                    f,
                    "[{}] [{}] {} {}",
                    timestamp,
                    command_id,
                    command,
                    args.join(" ")
                );
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
                let _ = writeln!(
                    f,
                    "[{}] [{}] EXIT {} ({}ms)",
                    timestamp, command_id, exit_code, duration_ms
                );
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
        let cgroup_memory_used = read_u64_file("/sys/fs/cgroup/memory.current");
        let cgroup_memory_total = read_limit_file("/sys/fs/cgroup/memory.max");
        let cgroup_cpu_percent = read_cgroup_cpu_usage_usec().and_then(|usage_usec| {
            let now = Instant::now();
            let mut previous = self.last_cgroup_cpu.lock().ok()?;
            let percent = previous.map(|(then, prior)| {
                let elapsed_usec = now.duration_since(then).as_micros() as f64;
                if elapsed_usec == 0.0 {
                    0.0
                } else {
                    ((usage_usec.saturating_sub(prior)) as f64 / elapsed_usec * 100.0) as f32
                }
            });
            *previous = Some((now, usage_usec));
            percent
        });
        let (disk_used, disk_total) = filesystem_usage(&self.run_dir).unwrap_or((0, 0));
        let metrics = json!({
            "timestamp": Local::now().to_rfc3339(),
            "scope": if cgroup_memory_used.is_some() { "cgroup_v2" } else { "host_fallback" },
            "cpu_percent": cgroup_cpu_percent.unwrap_or_else(|| sys.global_cpu_usage()),
            "memory": {
                "used_bytes": cgroup_memory_used.unwrap_or_else(|| sys.used_memory()),
                "total_bytes": cgroup_memory_total.unwrap_or_else(|| sys.total_memory()),
            },
            "disk": {
                "used_bytes": disk_used,
                "total_bytes": disk_total,
            },
        });

        if let Ok(mut f) = File::create(self.run_dir.join("metrics.json")) {
            let _ = f.write_all(serde_json::to_string_pretty(&metrics).unwrap().as_bytes());
        }
    }
}

fn read_u64_file(path: &str) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_limit_file(path: &str) -> Option<u64> {
    let value = fs::read_to_string(path).ok()?;
    let value = value.trim();
    if value == "max" {
        None
    } else {
        value.parse().ok()
    }
}

fn read_cgroup_cpu_usage_usec() -> Option<u64> {
    fs::read_to_string("/sys/fs/cgroup/cpu.stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("usage_usec ")?.parse().ok())
}

fn filesystem_usage(path: &Path) -> Option<(u64, u64)> {
    let stats = nix::sys::statvfs::statvfs(path).ok()?;
    let block_size = stats.fragment_size() as u64;
    let total = (stats.blocks() as u64).saturating_mul(block_size);
    let available = (stats.blocks_available() as u64).saturating_mul(block_size);
    Some((total.saturating_sub(available), total))
}

// Generated proto types
pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}

use proto::agent_service_client::AgentServiceClient;
use proto::{
    ActiveSession, AgentMessage, AgentRegistration, AgentStatus, CommandResult, Heartbeat,
    ManagementMessage, Metrics, OutputChunk, SessionReconcileAck, SessionReport, SystemInfo,
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
#[allow(dead_code)] // pid reserved for future signal handling
struct RunningCommand {
    stdin_tx: StdinSender,
    pty_control_tx: Option<PtyControlSender>,
    pid: Option<nix::unistd::Pid>,
    // Session metadata for reconciliation
    session_name: Option<String>,
    session_id: Option<String>,
    command: String,
    started_at: std::time::Instant,
    is_pty: bool,
}

/// Running commands map
type RunningCommands = Arc<TokioMutex<HashMap<String, RunningCommand>>>;

fn adopt_tmux_sessions_enabled() -> bool {
    !matches!(
        env::var("AGENTIC_ADOPT_TMUX_SESSIONS")
            .unwrap_or_else(|_| "true".to_string())
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn tmux_command_id(session_name: &str) -> String {
    format!("tmux:{session_name}")
}

fn tmux_session_name_from_command_id(command_id: &str) -> Option<&str> {
    command_id
        .strip_prefix("tmux:")
        .filter(|name| !name.is_empty())
}

async fn discover_tmux_sessions() -> Vec<ActiveSession> {
    if !adopt_tmux_sessions_enabled() {
        return Vec::new();
    }

    let output = match Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}\t#{session_created}"])
        .output()
        .await
    {
        Ok(output) => output,
        Err(e) => {
            debug!("tmux session discovery unavailable: {}", e);
            return Vec::new();
        }
    };

    if !output.status.success() {
        debug!(
            "tmux session discovery returned status {:?}",
            output.status.code()
        );
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '\t');
            let name = parts.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let created_ms = parts
                .next()
                .and_then(|raw| raw.trim().parse::<i64>().ok())
                .map(|seconds| seconds.saturating_mul(1000))
                .unwrap_or_default();

            Some(ActiveSession {
                command_id: tmux_command_id(name),
                session_name: name.to_string(),
                session_type: proto::SessionType::Interactive as i32,
                command: format!("tmux attach-session -t {name}"),
                started_at_ms: created_ms,
                pid: 0,
                is_pty: true,
                session_id: String::new(),
            })
        })
        .collect()
}

async fn kill_tmux_session(session_name: &str) -> bool {
    match Command::new("tmux")
        .args(["kill-session", "-t", session_name])
        .output()
        .await
    {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            warn!(
                "tmux kill-session failed for {} with status {:?}",
                session_name,
                output.status.code()
            );
            false
        }
        Err(e) => {
            warn!(
                "failed to run tmux kill-session for {}: {}",
                session_name, e
            );
            false
        }
    }
}

async fn stdin_sender_for_command(
    running_commands: &RunningCommands,
    command_id: &str,
    wait_for_registration: Duration,
) -> Option<StdinSender> {
    let started = std::time::Instant::now();

    loop {
        let stdin_tx = {
            let running = running_commands.lock().await;
            running.get(command_id).map(|rc| rc.stdin_tx.clone())
        };
        if stdin_tx.is_some() {
            return stdin_tx;
        }

        if started.elapsed() >= wait_for_registration {
            return None;
        }

        sleep(Duration::from_millis(10)).await;
    }
}

// =============================================================================
// Configuration
// =============================================================================

#[derive(Debug, Clone)]
struct AgentConfig {
    agent_id: String,
    server_address: String,
    uds_path: Option<String>,
    vsock_cid: Option<u32>,
    vsock_port: Option<u32>,
    tls_ca: Option<String>,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    tls_server_name: Option<String>,
    transport_mode: TransportMode,
    heartbeat_interval: Duration,
    reconnect_delay: Duration,
    max_reconnect_delay: Duration,
    /// Canonical instance UUIDv7 assigned by the management server's admin
    /// pipeline at provision time (#252). Read from `AGENT_INSTANCE_ID` or
    /// `AIWG_INSTANCE_ID` env vars. Empty when the agent was started
    /// outside the v2 provisioning flow — the management server then
    /// generates a UUIDv7 on its side to preserve back-compat.
    instance_id: String,
}

impl AgentConfig {
    fn from_cli(cli: &Cli) -> Result<Self> {
        // Load from env file first (lowest priority)
        load_env_file(&cli.env_file);

        // Build config: CLI args override env vars override defaults
        let default_id = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        // #252: Read canonical instance UUIDv7 from env. QEMU agents get
        // it via cloud-init writing AGENT_INSTANCE_ID into agent.env;
        // docker agents receive it via `-e AIWG_INSTANCE_ID=...`. Falls
        // back to empty when started outside the v2 flow.
        let instance_id = env::var("AGENT_INSTANCE_ID")
            .ok()
            .or_else(|| env::var("AIWG_INSTANCE_ID").ok())
            .unwrap_or_default();
        if instance_id.is_empty() {
            warn!(
                "no provisioned instance_id (AGENT_INSTANCE_ID / AIWG_INSTANCE_ID env unset); server will generate one"
            );
        }

        let uds_path = cli
            .uds_path
            .clone()
            .or_else(|| env::var("AGENT_GRPC_UDS_PATH").ok())
            .filter(|path| !path.trim().is_empty());
        let vsock_cid = match cli.vsock_cid {
            Some(cid) => Some(cid),
            None => env_u32_optional("AGENT_GRPC_VSOCK_CID")?,
        };
        let vsock_port = match cli.vsock_port {
            Some(port) => Some(port),
            None => env_u32_optional("AGENT_GRPC_VSOCK_PORT")?,
        };
        let tls_ca = cli
            .tls_ca
            .clone()
            .or_else(|| env_string_optional("AGENT_GRPC_TLS_CA"));
        let tls_cert = cli
            .tls_cert
            .clone()
            .or_else(|| env_string_optional("AGENT_GRPC_TLS_CERT"));
        let tls_key = cli
            .tls_key
            .clone()
            .or_else(|| env_string_optional("AGENT_GRPC_TLS_KEY"));
        let tls_server_name = cli
            .tls_server_name
            .clone()
            .or_else(|| env_string_optional("AGENT_GRPC_TLS_SERVER_NAME"));
        let tls_configured = tls_configured(&tls_ca, &tls_cert, &tls_key)?;
        let transport_mode = match cli.transport {
            Some(mode) => mode,
            None if tls_configured
                && env_string_optional("AGENT_RESTORE_ENROLLED").as_deref() == Some("true") =>
            {
                TransportMode::Tls
            }
            None => env::var("AGENT_TRANSPORT")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(|value| TransportMode::from_env_value(&value))
                .transpose()?
                .unwrap_or(TransportMode::Auto),
        };
        transport_mode.resolve(
            uds_path.as_deref(),
            vsock_pair(vsock_cid, vsock_port)?,
            tls_configured,
        )?;

        Ok(Self {
            agent_id: cli
                .agent_id
                .clone()
                .or_else(|| env::var("AGENT_ID").ok())
                .unwrap_or(default_id),
            server_address: cli
                .server
                .clone()
                .or_else(|| env_string_optional("AGENT_RESTORE_GRPC_SERVER"))
                .or_else(|| env::var("MANAGEMENT_SERVER").ok())
                .unwrap_or_else(|| "host.internal:8120".to_string()),
            uds_path,
            vsock_cid,
            vsock_port,
            tls_ca,
            tls_cert,
            tls_key,
            tls_server_name,
            transport_mode,
            heartbeat_interval: Duration::from_secs(cli.heartbeat),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_delay: Duration::from_secs(60),
            instance_id,
        })
    }

    fn resolved_transport(&self) -> Result<ResolvedTransport> {
        self.transport_mode.resolve(
            self.uds_path.as_deref(),
            vsock_pair(self.vsock_cid, self.vsock_port)?,
            tls_configured(&self.tls_ca, &self.tls_cert, &self.tls_key)?,
        )
    }

    fn vsock_addr(&self) -> Result<tokio_vsock::VsockAddr> {
        let (cid, port) = vsock_pair(self.vsock_cid, self.vsock_port)?
            .context("vsock transport selected without CID/port")?;
        Ok(tokio_vsock::VsockAddr::new(cid, port))
    }

    fn client_tls_config(&self) -> Result<ClientTlsConfig> {
        let ca_path = self
            .tls_ca
            .as_ref()
            .context("TLS transport selected without a CA bundle path")?;
        let cert_path = self
            .tls_cert
            .as_ref()
            .context("TLS transport selected without a client certificate path")?;
        let key_path = self
            .tls_key
            .as_ref()
            .context("TLS transport selected without a client private key path")?;

        let ca = fs::read(ca_path).with_context(|| format!("failed to read TLS CA {ca_path}"))?;
        let cert = fs::read(cert_path)
            .with_context(|| format!("failed to read TLS client certificate {cert_path}"))?;
        let key = fs::read(key_path)
            .with_context(|| format!("failed to read TLS client private key {key_path}"))?;
        let server_name = self.tls_server_name()?;

        Ok(ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(ca))
            .identity(Identity::from_pem(cert, key))
            .domain_name(server_name))
    }

    fn tls_server_name(&self) -> Result<String> {
        if let Some(name) = self
            .tls_server_name
            .as_ref()
            .filter(|name| !name.trim().is_empty())
        {
            return Ok(name.trim().to_string());
        }

        server_host(&self.server_address)
            .map(str::to_string)
            .context("could not infer TLS server name from MANAGEMENT_SERVER")
    }
}

fn load_env_file(env_file: &str) {
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
}

fn restore_bootstrap_env_file() -> String {
    env_string_optional(RESTORE_BOOTSTRAP_ENV_FILE_ENV)
        .unwrap_or_else(|| DEFAULT_RESTORE_BOOTSTRAP_ENV_FILE.to_string())
}

fn restore_bootstrap_env_file_if_usable() -> Option<String> {
    let path = restore_bootstrap_env_file();
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        warn!(
            path = %path,
            mode = format!("{mode:o}"),
            "ignoring restore bootstrap env file with group/world permissions"
        );
        return None;
    }
    Some(path)
}

fn load_restore_bootstrap_env_file() -> Option<String> {
    let path = restore_bootstrap_env_file_if_usable()?;
    load_env_file(&path);
    Some(path)
}

fn env_string_optional(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u32_optional(name: &str) -> Result<Option<u32>> {
    let Some(value) = env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    value
        .parse()
        .map(Some)
        .map_err(|e| anyhow::anyhow!("invalid {name} value `{value}`: {e}"))
}

fn vsock_pair(cid: Option<u32>, port: Option<u32>) -> Result<Option<(u32, u32)>> {
    match (cid, port) {
        (Some(cid), Some(port)) => Ok(Some((cid, port))),
        (None, None) => Ok(None),
        (Some(_), None) => anyhow::bail!("vsock CID configured without vsock port"),
        (None, Some(_)) => anyhow::bail!("vsock port configured without vsock CID"),
    }
}

fn tls_configured(
    ca: &Option<String>,
    cert: &Option<String>,
    key: &Option<String>,
) -> Result<bool> {
    match (ca.is_some(), cert.is_some(), key.is_some()) {
        (false, false, false) => Ok(false),
        (true, true, true) => Ok(true),
        _ => anyhow::bail!(
            "TLS transport requires AGENT_GRPC_TLS_CA, AGENT_GRPC_TLS_CERT, and AGENT_GRPC_TLS_KEY together"
        ),
    }
}

fn server_host(address: &str) -> Option<&str> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    if let Some(rest) = address.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host);
    }
    address
        .split_once(':')
        .map(|(host, _)| host)
        .or(Some(address))
}

// =============================================================================
// Bootstrap Enrollment
// =============================================================================

const DEFAULT_BOOTSTRAP_TLS_DIR: &str = "/etc/agentic-sandbox/grpc-mtls";

/// Control-channel keepalive knobs (#633). Mirrors the server listeners
/// (10s interval / 20s timeout in management/src/main.rs) so both ends
/// probe the flow; see `apply_channel_keepalive` for rationale.
const GRPC_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const GRPC_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// OS-level TCP keepalive for the TCP/mTLS transports — a second layer of
/// conntrack refresh that survives even an HTTP/2-quiet connection.
const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_RESTORE_BOOTSTRAP_ENV_FILE: &str = "/mnt/inbox/restore-bootstrap.env";
const RESTORE_BOOTSTRAP_ENV_FILE_ENV: &str = "AGENT_RESTORE_BOOTSTRAP_ENV_FILE";

#[derive(Debug, Serialize)]
struct BootstrapConsumeRequest {
    token: String,
    spiffe_id: String,
    csr_pem: String,
}

#[derive(Debug, Deserialize)]
struct BootstrapConsumeResponse {
    spiffe_id: String,
    certificate_pem: String,
    ca_pem: String,
}

#[derive(Debug, Clone)]
struct BootstrapTlsPaths {
    ca: PathBuf,
    cert: PathBuf,
    key: PathBuf,
}

impl BootstrapTlsPaths {
    fn from_env() -> Self {
        let dir = env_string_optional("AGENT_BOOTSTRAP_TLS_DIR")
            .unwrap_or_else(|| DEFAULT_BOOTSTRAP_TLS_DIR.to_string());
        let dir = PathBuf::from(dir);
        Self {
            ca: env_string_optional("AGENT_GRPC_TLS_CA")
                .map(PathBuf::from)
                .unwrap_or_else(|| dir.join("ca.pem")),
            cert: env_string_optional("AGENT_GRPC_TLS_CERT")
                .map(PathBuf::from)
                .unwrap_or_else(|| dir.join("agent.pem")),
            key: env_string_optional("AGENT_GRPC_TLS_KEY")
                .map(PathBuf::from)
                .unwrap_or_else(|| dir.join("agent-key.pem")),
        }
    }

    fn complete(&self) -> bool {
        self.ca.is_file() && self.cert.is_file() && self.key.is_file()
    }
}

async fn maybe_bootstrap_enroll(cli: &Cli) -> Result<()> {
    load_env_file(&cli.env_file);
    let main_env_had_bootstrap_token = env_string_optional("AGENT_BOOTSTRAP_TOKEN").is_some();
    let restore_env_file = load_restore_bootstrap_env_file();
    let restore_enrollment_requested =
        restore_env_file.is_some() && env_string_optional("AGENT_BOOTSTRAP_TOKEN").is_some();

    if tls_configured(
        &env_string_optional("AGENT_GRPC_TLS_CA"),
        &env_string_optional("AGENT_GRPC_TLS_CERT"),
        &env_string_optional("AGENT_GRPC_TLS_KEY"),
    )? {
        if restore_enrollment_requested {
            anyhow::bail!(
                "restore enrollment refused cloned TLS configuration; clean-base snapshots must contain no prior identity"
            );
        }
        if main_env_had_bootstrap_token {
            scrub_bootstrap_env_file(&cli.env_file)?;
        }
        if let Some(path) = restore_env_file.as_deref() {
            remove_restore_bootstrap_env_file(path)?;
            clear_bootstrap_token_env();
        }
        return Ok(());
    }

    let paths = BootstrapTlsPaths::from_env();
    if paths.complete() {
        if restore_enrollment_requested {
            anyhow::bail!(
                "restore enrollment refused cloned TLS files in {}; clean-base snapshots must contain no prior identity",
                paths.key.parent().unwrap_or_else(|| Path::new(".")).display()
            );
        }
        configure_bootstrap_tls_env(&paths);
        if main_env_had_bootstrap_token {
            scrub_bootstrap_env_file(&cli.env_file)?;
        }
        if let Some(path) = restore_env_file.as_deref() {
            remove_restore_bootstrap_env_file(path)?;
            clear_bootstrap_token_env();
        }
        return Ok(());
    }

    let Some(token) = env_string_optional("AGENT_BOOTSTRAP_TOKEN") else {
        return Ok(());
    };
    let spiffe_id = env_string_optional("AGENT_BOOTSTRAP_SPIFFE_ID")
        .context("AGENT_BOOTSTRAP_TOKEN requires AGENT_BOOTSTRAP_SPIFFE_ID")?;
    let endpoint = bootstrap_enrollment_url(
        env_string_optional("AGENT_BOOTSTRAP_ENROLLMENT_URL"),
        cli.server
            .clone()
            .or_else(|| env_string_optional("MANAGEMENT_SERVER"))
            .unwrap_or_else(|| "host.internal:8120".to_string()),
    )?;

    let key = KeyPair::generate().context("failed to generate bootstrap mTLS key")?;
    let csr_pem = csr_pem_for_spiffe(&spiffe_id, &key)?;
    let response = consume_bootstrap_enrollment(&endpoint, token, &spiffe_id, &csr_pem).await?;

    if response.spiffe_id != spiffe_id {
        anyhow::bail!("bootstrap enrollment returned mismatched SPIFFE id");
    }

    validate_bootstrap_certificate_response(&response, &spiffe_id, &key)?;
    write_bootstrap_tls_files(&paths, &response, &key)?;
    write_restore_runtime_env(&paths, &spiffe_id)?;
    if main_env_had_bootstrap_token {
        scrub_bootstrap_env_file(&cli.env_file)?;
    }
    if let Some(path) = restore_env_file.as_deref() {
        write_restore_enrollment_ack(path, &paths, &spiffe_id)?;
        remove_restore_bootstrap_env_file(path)?;
    }
    clear_bootstrap_token_env();
    configure_bootstrap_tls_env(&paths);
    info!(
        spiffe_id = %spiffe_id,
        ca = %paths.ca.display(),
        cert = %paths.cert.display(),
        key = %paths.key.display(),
        "bootstrap enrollment materialized mTLS credentials"
    );

    Ok(())
}

fn csr_pem_for_spiffe(spiffe_id: &str, key: &KeyPair) -> Result<String> {
    let mut params = CertificateParams::new(Vec::<String>::new())
        .context("failed to initialize bootstrap CSR params")?;
    params.distinguished_name = DistinguishedName::new();
    params
        .subject_alt_names
        .push(SanType::URI(spiffe_id.try_into()?));

    Ok(params
        .serialize_request(key)
        .context("failed to serialize bootstrap CSR")?
        .pem()
        .context("failed to encode bootstrap CSR as PEM")?)
}

async fn consume_bootstrap_enrollment(
    endpoint: &str,
    token: String,
    spiffe_id: &str,
    csr_pem: &str,
) -> Result<BootstrapConsumeResponse> {
    #[cfg(not(test))]
    if !endpoint.starts_with("https://") {
        anyhow::bail!("bootstrap enrollment endpoint must use HTTPS");
    }
    let mut client = reqwest::Client::builder();
    if let Some(ca_path) = env_string_optional("AGENT_BOOTSTRAP_CA") {
        let ca_pem = fs::read(&ca_path)
            .with_context(|| format!("failed to read bootstrap HTTPS CA {ca_path}"))?;
        let ca = reqwest::Certificate::from_pem(&ca_pem)
            .context("bootstrap HTTPS CA is not valid PEM")?;
        client = client.add_root_certificate(ca);
    } else {
        #[cfg(not(test))]
        anyhow::bail!("AGENT_BOOTSTRAP_CA is required for bootstrap HTTPS trust pinning");
    }
    let response = client
        .build()
        .context("failed to build bootstrap HTTPS client")?
        .post(endpoint)
        .json(&BootstrapConsumeRequest {
            token,
            spiffe_id: spiffe_id.to_string(),
            csr_pem: csr_pem.to_string(),
        })
        .send()
        .await
        .context("bootstrap enrollment request failed")?;
    let status = response.status();
    if !status.is_success() {
        let problem = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "bootstrap enrollment rejected CSR: HTTP {} {}",
            status.as_u16(),
            redact_bootstrap_token_text(&problem)
        );
    }

    response
        .json()
        .await
        .context("bootstrap enrollment response was not valid JSON")
}

fn write_bootstrap_tls_files(
    paths: &BootstrapTlsPaths,
    response: &BootstrapConsumeResponse,
    key: &KeyPair,
) -> Result<()> {
    let dir = paths
        .key
        .parent()
        .context("bootstrap TLS key path has no parent directory")?;
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to chmod {}", dir.display()))?;

    write_private_file(&paths.key, &key.serialize_pem())?;
    write_private_file(&paths.cert, &response.certificate_pem)?;
    write_private_file(&paths.ca, &response.ca_pem)?;
    Ok(())
}

fn validate_bootstrap_certificate_response(
    response: &BootstrapConsumeResponse,
    expected_spiffe_id: &str,
    key: &KeyPair,
) -> Result<()> {
    use x509_parser::extensions::GeneralName;

    let mut leaf_reader = std::io::BufReader::new(response.certificate_pem.as_bytes());
    let leaf_der = rustls_pemfile::certs(&mut leaf_reader)
        .next()
        .transpose()?
        .context("bootstrap response contained no leaf certificate")?;
    let (_, leaf) = x509_parser::parse_x509_certificate(leaf_der.as_ref())
        .map_err(|error| anyhow::anyhow!("bootstrap leaf certificate is malformed: {error}"))?;

    let san = leaf
        .subject_alternative_name()
        .map_err(|error| anyhow::anyhow!("bootstrap leaf SAN is malformed: {error}"))?
        .context("bootstrap leaf certificate has no SAN")?;
    match san.value.general_names.as_slice() {
        [GeneralName::URI(uri)] if *uri == expected_spiffe_id => {}
        _ => anyhow::bail!(
            "bootstrap leaf certificate must contain exactly the requested SPIFFE URI-SAN"
        ),
    }
    if leaf.public_key().raw != key.public_key_der() {
        anyhow::bail!("bootstrap leaf certificate is bound to a different private key");
    }
    if !leaf.validity().is_valid() {
        anyhow::bail!("bootstrap leaf certificate is not currently valid");
    }

    let mut ca_reader = std::io::BufReader::new(response.ca_pem.as_bytes());
    let ca_der = rustls_pemfile::certs(&mut ca_reader)
        .next()
        .transpose()?
        .context("bootstrap response contained no CA certificate")?;
    let (_, ca) = x509_parser::parse_x509_certificate(ca_der.as_ref())
        .map_err(|error| anyhow::anyhow!("bootstrap CA certificate is malformed: {error}"))?;
    if !ca.is_ca() {
        anyhow::bail!("bootstrap response trust anchor is not a CA certificate");
    }
    if !ca.validity().is_valid() {
        anyhow::bail!("bootstrap CA certificate is not currently valid");
    }
    if let Some(ca_path) = env_string_optional("AGENT_BOOTSTRAP_CA") {
        let pinned = fs::read(ca_path).context("failed to read pinned bootstrap CA")?;
        let mut pinned_reader = std::io::BufReader::new(pinned.as_slice());
        let pinned_der = rustls_pemfile::certs(&mut pinned_reader)
            .next()
            .transpose()?
            .context("pinned bootstrap CA file contains no certificate")?;
        if pinned_der.as_ref() != ca_der.as_ref() {
            anyhow::bail!("bootstrap response CA does not match the host-delivered trust anchor");
        }
    } else {
        #[cfg(not(test))]
        anyhow::bail!("AGENT_BOOTSTRAP_CA is required to validate the enrollment response");
    }
    ca.verify_signature(Some(ca.public_key()))
        .map_err(|error| anyhow::anyhow!("bootstrap CA is not self-signed: {error}"))?;
    leaf.verify_signature(Some(ca.public_key()))
        .map_err(|error| anyhow::anyhow!("bootstrap leaf certificate chain is invalid: {error}"))?;
    Ok(())
}

fn write_restore_runtime_env(paths: &BootstrapTlsPaths, spiffe_id: &str) -> Result<()> {
    let runtime_env = paths
        .key
        .parent()
        .context("bootstrap TLS key path has no parent directory")?
        .join("agent.env");
    let instance_id = spiffe_id
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .context("bootstrap SPIFFE id has no instance component")?;
    let mut contents = format!(
        "AGENT_TRANSPORT=auto\nAGENT_ID={instance_id}\nAGENT_INSTANCE_ID={instance_id}\nAGENT_RESTORE_ENROLLED=true\nAGENT_GRPC_TLS_CA={}\nAGENT_GRPC_TLS_CERT={}\nAGENT_GRPC_TLS_KEY={}\n",
        paths.ca.display(),
        paths.cert.display(),
        paths.key.display(),
    );
    if let Some(server) = env_string_optional("AGENT_RESTORE_GRPC_SERVER") {
        contents.push_str(&format!("AGENT_RESTORE_GRPC_SERVER={server}\n"));
    }
    write_private_file_atomic(&runtime_env, &contents).with_context(|| {
        format!(
            "failed to persist restore runtime identity in {}",
            runtime_env.display()
        )
    })
}

fn write_restore_enrollment_ack(
    restore_env_file: &str,
    paths: &BootstrapTlsPaths,
    spiffe_id: &str,
) -> Result<()> {
    use sha2::{Digest, Sha256};

    let parent = Path::new(restore_env_file)
        .parent()
        .context("restore bootstrap path has no parent directory")?;
    let cert = fs::read(&paths.cert).with_context(|| {
        format!(
            "failed to read enrolled certificate {}",
            paths.cert.display()
        )
    })?;
    let ack = serde_json::json!({
        "schema": 1,
        "spiffe_id": spiffe_id,
        "certificate_sha256": hex::encode(Sha256::digest(cert)),
        "tls_materialized": true,
    });
    write_private_file_atomic(
        &parent.join("restore-enrollment-ready.json"),
        &serde_json::to_string(&ack)?,
    )
}

fn write_private_file(path: &Path, contents: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to chmod {}", path.display()))?;
    Ok(())
}

fn write_private_file_atomic(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("TLS material path has no parent directory")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tls"),
        uuid::Uuid::new_v4()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;
    let result = (|| -> Result<()> {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn certificate_renewal_delay(cert_path: &Path) -> Result<Duration> {
    use sha2::{Digest, Sha256};

    let pem = fs::read(cert_path)
        .with_context(|| format!("failed to read TLS certificate {}", cert_path.display()))?;
    let mut reader = std::io::BufReader::new(pem.as_slice());
    let cert = rustls_pemfile::certs(&mut reader)
        .next()
        .transpose()?
        .context("TLS certificate file contains no certificate")?;
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
        .map_err(|error| anyhow::anyhow!("failed to parse TLS certificate: {error}"))?;
    let not_before = parsed.validity().not_before.timestamp();
    let not_after = parsed.validity().not_after.timestamp();
    if not_after <= not_before {
        anyhow::bail!("TLS certificate validity interval is invalid");
    }
    let lifetime = not_after - not_before;
    let jitter_window = lifetime / 10;
    let digest = Sha256::digest(&pem);
    let sample = u64::from_be_bytes(digest[..8].try_into().expect("fixed digest slice"));
    let jitter = if jitter_window == 0 {
        0
    } else {
        i64::try_from(sample % (jitter_window as u64 + 1)).unwrap_or(0)
    };
    let renew_at = not_before
        .saturating_add(lifetime / 2)
        .saturating_add(jitter)
        .min(not_after - 1);
    let now = chrono::Utc::now().timestamp();
    Ok(Duration::from_secs(
        renew_at.saturating_sub(now).max(0) as u64
    ))
}

fn certificate_spiffe_id(cert_path: &Path) -> Result<String> {
    use x509_parser::extensions::GeneralName;

    let pem = fs::read(cert_path)
        .with_context(|| format!("failed to read TLS certificate {}", cert_path.display()))?;
    let mut reader = std::io::BufReader::new(pem.as_slice());
    let cert = rustls_pemfile::certs(&mut reader)
        .next()
        .transpose()?
        .context("TLS certificate file contains no certificate")?;
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
        .map_err(|error| anyhow::anyhow!("failed to parse TLS certificate: {error}"))?;
    let san = parsed
        .subject_alternative_name()
        .map_err(|error| anyhow::anyhow!("failed to parse TLS certificate SAN: {error}"))?
        .context("TLS certificate has no SAN")?;
    match san.value.general_names.as_slice() {
        [GeneralName::URI(uri)] if uri.starts_with("spiffe://") => Ok((*uri).to_string()),
        _ => anyhow::bail!("TLS certificate must contain exactly one SPIFFE URI-SAN"),
    }
}

async fn renew_agent_certificate(
    client: &mut AgentServiceClient<Channel>,
    config: &AgentConfig,
) -> Result<()> {
    let cert_path = Path::new(
        config
            .tls_cert
            .as_deref()
            .context("TLS certificate path is unavailable")?,
    );
    let key_path = Path::new(
        config
            .tls_key
            .as_deref()
            .context("TLS private key path is unavailable")?,
    );
    let ca_path = Path::new(
        config
            .tls_ca
            .as_deref()
            .context("TLS CA path is unavailable")?,
    );
    let key_pem = fs::read_to_string(key_path)
        .with_context(|| format!("failed to read TLS private key {}", key_path.display()))?;
    let key = KeyPair::from_pem(&key_pem).context("failed to parse TLS renewal key")?;
    let spiffe_id = certificate_spiffe_id(cert_path)?;
    let csr_pem = csr_pem_for_spiffe(&spiffe_id, &key)?;
    let mut request = Request::new(proto::RenewCertificateRequest { csr_pem });
    populate_agent_metadata(request.metadata_mut(), config, ResolvedTransport::Tls)?;
    let renewed = client.renew_certificate(request).await?.into_inner();
    let returned_delay = {
        let temporary = cert_path.with_extension("renewed-validation.pem");
        write_private_file_atomic(&temporary, &renewed.certificate_pem)?;
        if certificate_spiffe_id(&temporary)? != spiffe_id {
            let _ = fs::remove_file(&temporary);
            anyhow::bail!("renewal returned a certificate for a different SPIFFE identity");
        }
        let renewed_pem = fs::read(&temporary)?;
        let mut reader = std::io::BufReader::new(renewed_pem.as_slice());
        let renewed_der = rustls_pemfile::certs(&mut reader)
            .next()
            .transpose()?
            .context("renewal returned no certificate")?;
        let (_, renewed_cert) = x509_parser::parse_x509_certificate(renewed_der.as_ref())
            .map_err(|error| anyhow::anyhow!("failed to parse renewed certificate: {error}"))?;
        if renewed_cert.public_key().raw != key.public_key_der() {
            let _ = fs::remove_file(&temporary);
            anyhow::bail!("renewal returned a certificate for a different private key");
        }
        let result = certificate_renewal_delay(&temporary);
        let _ = fs::remove_file(&temporary);
        result?
    };
    if returned_delay.is_zero() || renewed.not_after_unix <= chrono::Utc::now().timestamp() {
        anyhow::bail!("renewal returned an expired or immediately expiring certificate");
    }
    write_private_file_atomic(cert_path, &renewed.certificate_pem)?;
    if !renewed.ca_pem.trim().is_empty() {
        write_private_file_atomic(ca_path, &renewed.ca_pem)?;
    }
    Ok(())
}

fn configure_bootstrap_tls_env(paths: &BootstrapTlsPaths) {
    env::set_var("AGENT_GRPC_TLS_CA", paths.ca.to_string_lossy().as_ref());
    env::set_var("AGENT_GRPC_TLS_CERT", paths.cert.to_string_lossy().as_ref());
    env::set_var("AGENT_GRPC_TLS_KEY", paths.key.to_string_lossy().as_ref());
    env::set_var("AGENT_RESTORE_ENROLLED", "true");
    if env_string_optional("AGENT_TRANSPORT").is_none() {
        env::set_var("AGENT_TRANSPORT", "auto");
    }
}

fn scrub_bootstrap_env_file(env_file: &str) -> Result<()> {
    let path = Path::new(env_file);
    if !path.exists() {
        return Ok(());
    }
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let scrubbed = contents
        .lines()
        .filter(|line| {
            !line.starts_with("AGENT_BOOTSTRAP_TOKEN=")
                && !line.starts_with("AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut scrubbed = if scrubbed.is_empty() {
        String::new()
    } else {
        format!("{scrubbed}\n")
    };
    if contents.ends_with('\n') && scrubbed.is_empty() {
        scrubbed.push('\n');
    }

    write_private_file(path, &scrubbed)
}

fn remove_restore_bootstrap_env_file(env_file: &str) -> Result<()> {
    let path = Path::new(env_file);
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))
}

fn clear_bootstrap_token_env() {
    env::remove_var("AGENT_BOOTSTRAP_TOKEN");
    env::remove_var("AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS");
}

fn bootstrap_enrollment_url(
    configured: Option<String>,
    _management_server: String,
) -> Result<String> {
    if let Some(url) = configured.filter(|url| !url.trim().is_empty()) {
        return Ok(url);
    }

    anyhow::bail!(
        "AGENT_BOOTSTRAP_ENROLLMENT_URL is required with AGENT_BOOTSTRAP_TOKEN; \
         MANAGEMENT_SERVER is only the gRPC endpoint"
    )
}

fn redact_bootstrap_token_text(text: &str) -> String {
    let Some(token) = env_string_optional("AGENT_BOOTSTRAP_TOKEN") else {
        return text.to_string();
    };
    text.replace(&token, "[REDACTED_BOOTSTRAP_TOKEN]")
}

fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(": ")
}

fn populate_agent_metadata(
    metadata: &mut MetadataMap,
    config: &AgentConfig,
    _transport: ResolvedTransport,
) -> Result<()> {
    metadata.insert("x-agent-id", MetadataValue::try_from(&config.agent_id)?);
    if !config.instance_id.is_empty() {
        metadata.insert(
            "x-agent-instance-id",
            MetadataValue::try_from(&config.instance_id)?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod transport_mode_tests {
    use super::*;

    const BOOTSTRAP_TEST_ENV_KEYS: &[&str] = &[
        RESTORE_BOOTSTRAP_ENV_FILE_ENV,
        "AGENT_BOOTSTRAP_TOKEN",
        "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS",
        "AGENT_BOOTSTRAP_SPIFFE_ID",
        "AGENT_BOOTSTRAP_TLS_DIR",
        "AGENT_BOOTSTRAP_ENROLLMENT_URL",
        "AGENT_GRPC_TLS_CA",
        "AGENT_GRPC_TLS_CERT",
        "AGENT_GRPC_TLS_KEY",
        "AGENT_RESTORE_ENROLLED",
        "AGENT_RESTORE_GRPC_SERVER",
        "AGENT_TRANSPORT",
    ];
    static BOOTSTRAP_ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct BootstrapEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl Drop for BootstrapEnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
    }

    fn lock_bootstrap_env() -> BootstrapEnvGuard {
        let lock = BOOTSTRAP_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = BOOTSTRAP_TEST_ENV_KEYS
            .iter()
            .map(|key| (*key, env::var_os(key)))
            .collect();
        BootstrapEnvGuard { _lock: lock, saved }
    }

    fn metadata_test_config(mode: TransportMode) -> AgentConfig {
        AgentConfig {
            agent_id: "agent-01".to_string(),
            server_address: "host.internal:8120".to_string(),
            uds_path: Some("/run/agentic/grpc.sock".to_string()),
            vsock_cid: Some(3),
            vsock_port: Some(1024),
            tls_ca: Some("ca.pem".to_string()),
            tls_cert: Some("agent.pem".to_string()),
            tls_key: Some("agent-key.pem".to_string()),
            tls_server_name: Some("host.internal".to_string()),
            transport_mode: mode,
            heartbeat_interval: Duration::from_secs(5),
            reconnect_delay: Duration::from_secs(5),
            max_reconnect_delay: Duration::from_secs(60),
            instance_id: "018fb9f1-3291-7a73-b261-c7de8a2af4d1".to_string(),
        }
    }

    fn test_cli() -> Cli {
        Cli {
            server: None,
            agent_id: None,
            uds_path: None,
            vsock_cid: None,
            vsock_port: None,
            tls_ca: None,
            tls_cert: None,
            tls_key: None,
            tls_server_name: None,
            transport: None,
            heartbeat: 5,
            env_file: "/nonexistent/agent.env".to_string(),
        }
    }

    fn test_agent_message(command_id: &str) -> AgentMessage {
        AgentMessage {
            payload: Some(proto::agent_message::Payload::CommandResult(
                CommandResult {
                    command_id: command_id.to_string(),
                    exit_code: 0,
                    error: String::new(),
                    duration_ms: 0,
                    success: true,
                },
            )),
        }
    }

    fn active_session(command_id: &str, session_name: &str) -> ActiveSession {
        ActiveSession {
            command_id: command_id.to_string(),
            session_name: session_name.to_string(),
            session_type: proto::SessionType::Interactive as i32,
            command: String::new(),
            started_at_ms: 0,
            pid: 0,
            is_pty: true,
            session_id: String::new(),
        }
    }

    // #637: on the normal reconnect path the forwarder hands the receiver back,
    // so recover_output_channel must NOT swap output_tx — the sender clones that
    // running sessions captured at spawn have to keep delivering after recovery.
    #[tokio::test]
    async fn output_channel_recovery_preserves_session_senders() {
        let mut client = AgentClient::new(test_cli(), metadata_test_config(TransportMode::Auto));

        // stream_loop takes the receiver; a running session captures a sender clone.
        let rx0 = client.output_rx.take().expect("receiver present");
        let session_tx = client.output_tx.clone();

        // Forwarder exits after teardown and hands the SAME receiver back.
        let (ret_tx, ret_rx) = oneshot::channel();
        ret_tx.send(rx0).expect("hand receiver back");
        client.output_rx_return = Some(ret_rx);

        client.recover_output_channel().await;

        // The pre-existing session's captured sender still reaches the receiver.
        session_tx
            .send(test_agent_message("c1"))
            .await
            .expect("session sender still connected to the live channel");
        let got = client
            .output_rx
            .as_mut()
            .expect("receiver reinstalled")
            .recv()
            .await;
        assert!(
            got.is_some(),
            "output from a pre-existing session must survive recovery (no orphaned sender)"
        );
    }

    // Deadlock-guard fallback: only when the forwarder never returns the receiver
    // (panic/wedge) does recover_output_channel recreate the pair. It must still
    // leave the agent with a usable channel (#637).
    #[tokio::test]
    async fn output_channel_recovery_last_resort_recreates_a_working_channel() {
        let mut client = AgentClient::new(test_cli(), metadata_test_config(TransportMode::Auto));
        let old_rx = client.output_rx.take().expect("receiver present");

        // Forwarder drops the return sender without sending (panic) -> Err arm,
        // resolves immediately (no 30s wait).
        let (ret_tx, ret_rx) = oneshot::channel::<mpsc::Receiver<AgentMessage>>();
        drop(ret_tx);
        client.output_rx_return = Some(ret_rx);
        drop(old_rx);

        client.recover_output_channel().await;

        let tx = client.output_tx.clone();
        tx.send(test_agent_message("c2"))
            .await
            .expect("recreated channel usable");
        assert!(
            client
                .output_rx
                .as_mut()
                .expect("receiver present")
                .recv()
                .await
                .is_some(),
            "last-resort fallback must install a working channel"
        );
    }

    // #634 report dedup / tmux alias identity migration: a tmux-backed tracked
    // session must not be double-reported under both its command_id and its
    // discovered `tmux:<name>` alias.
    #[test]
    fn merge_discovered_sessions_dedups_by_command_id_and_session_name() {
        let tracked = vec![
            active_session("cmd-1", "workbench"),
            active_session("tmux:build", "build"),
        ];
        let discovered = vec![
            active_session("tmux:workbench", "workbench"), // dup NAME of cmd-1 -> drop
            active_session("tmux:build", "build"),         // dup ID -> drop
            active_session("tmux:scratch", "scratch"),     // genuinely new -> keep
        ];

        let merged = merge_discovered_sessions(tracked, discovered);

        let ids: HashSet<String> = merged.iter().map(|s| s.command_id.clone()).collect();
        let names: HashSet<String> = merged
            .iter()
            .map(|s| s.session_name.clone())
            .filter(|n| !n.is_empty())
            .collect();
        assert_eq!(merged.len(), 3, "two duplicates dropped, one new kept");
        assert_eq!(ids.len(), merged.len(), "no duplicate command_ids");
        assert_eq!(names.len(), merged.len(), "no duplicate session identities");
        assert!(ids.contains("tmux:scratch"), "new discovered session kept");
        assert!(
            !ids.contains("tmux:workbench"),
            "name-collision alias must be dropped"
        );
    }

    // #633: client keepalive probes must mirror the management listeners so both
    // ends refresh conntrack; TCP keepalive is the second, HTTP/2-quiet-proof layer.
    #[test]
    fn keepalive_constants_mirror_server_listeners() {
        assert_eq!(GRPC_KEEPALIVE_INTERVAL, Duration::from_secs(10));
        assert_eq!(GRPC_KEEPALIVE_TIMEOUT, Duration::from_secs(20));
        assert_eq!(TCP_KEEPALIVE_INTERVAL, Duration::from_secs(30));
    }

    #[test]
    fn tmux_adoption_command_ids_round_trip_session_names() {
        let command_id = tmux_command_id("workbench");
        assert_eq!(command_id, "tmux:workbench");
        assert_eq!(
            tmux_session_name_from_command_id(&command_id),
            Some("workbench")
        );
        assert_eq!(tmux_session_name_from_command_id("regular-command"), None);
        assert_eq!(tmux_session_name_from_command_id("tmux:"), None);
    }

    #[test]
    fn transport_mode_defaults_to_auto_tcp_fallback() {
        assert_eq!(
            TransportMode::Auto.resolve(None, None, false).unwrap(),
            ResolvedTransport::Tcp
        );
    }

    #[test]
    fn transport_mode_auto_uses_uds_only_when_path_exists() {
        assert_eq!(
            TransportMode::Auto.resolve(None, None, false).unwrap(),
            ResolvedTransport::Tcp
        );
        assert_eq!(
            TransportMode::Auto
                .resolve(Some("/run/agentic/grpc.sock"), Some((3, 1024)), true)
                .unwrap(),
            ResolvedTransport::Uds
        );
    }

    #[test]
    fn transport_mode_uds_requires_path() {
        let err = TransportMode::Uds.resolve(None, None, false).unwrap_err();

        assert!(
            err.to_string().contains("requires --uds-path"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn transport_mode_vsock_requires_cid_and_port() {
        let err = TransportMode::Vsock.resolve(None, None, false).unwrap_err();

        assert!(
            err.to_string().contains("requires --vsock-cid"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn transport_mode_auto_uses_vsock_when_configured_without_uds() {
        assert_eq!(
            TransportMode::Auto
                .resolve(None, Some((3, 1024)), true)
                .unwrap(),
            ResolvedTransport::Vsock
        );
    }

    #[test]
    fn transport_mode_tls_requires_full_tls_config() {
        let err = TransportMode::Tls.resolve(None, None, false).unwrap_err();

        assert!(
            err.to_string().contains("requires --tls-ca"),
            "unexpected error: {err:#}"
        );
        assert_eq!(
            TransportMode::Tls.resolve(None, None, true).unwrap(),
            ResolvedTransport::Tls
        );
    }

    #[test]
    fn transport_mode_auto_uses_tls_before_tcp() {
        assert_eq!(
            TransportMode::Auto.resolve(None, None, true).unwrap(),
            ResolvedTransport::Tls
        );
    }

    #[test]
    fn tls_config_requires_ca_cert_and_key_together() {
        let err =
            tls_configured(&Some("ca.pem".into()), &None, &Some("key.pem".into())).unwrap_err();

        assert!(
            err.to_string()
                .contains("TLS transport requires AGENT_GRPC_TLS_CA"),
            "unexpected error: {err:#}"
        );
        assert!(!tls_configured(&None, &None, &None).unwrap());
        assert!(tls_configured(
            &Some("ca.pem".into()),
            &Some("client.pem".into()),
            &Some("client-key.pem".into())
        )
        .unwrap());
    }

    #[test]
    fn server_host_extracts_tls_name_from_address() {
        assert_eq!(server_host("host.internal:8124"), Some("host.internal"));
        assert_eq!(server_host("[::1]:8124"), Some("::1"));
        assert_eq!(server_host("management.local"), Some("management.local"));
        assert_eq!(server_host(""), None);
    }

    #[test]
    fn vsock_pair_requires_both_cid_and_port() {
        assert_eq!(vsock_pair(Some(3), Some(1024)).unwrap(), Some((3, 1024)));
        assert!(vsock_pair(Some(3), None)
            .unwrap_err()
            .to_string()
            .contains("without vsock port"));
        assert!(vsock_pair(None, Some(1024))
            .unwrap_err()
            .to_string()
            .contains("without vsock CID"));
    }

    #[test]
    fn transport_mode_parses_env_case_insensitively() {
        assert_eq!(
            TransportMode::from_env_value("AUTO").unwrap(),
            TransportMode::Auto
        );
        assert_eq!(
            TransportMode::from_env_value("vsock").unwrap(),
            TransportMode::Vsock
        );
        assert_eq!(
            TransportMode::from_env_value("TLS").unwrap(),
            TransportMode::Tls
        );
    }

    #[test]
    fn tcp_metadata_omits_retired_legacy_secret() {
        let config = metadata_test_config(TransportMode::Tcp);
        let mut metadata = MetadataMap::new();

        populate_agent_metadata(&mut metadata, &config, ResolvedTransport::Tcp).unwrap();

        assert_eq!(metadata.get("x-agent-id").unwrap(), "agent-01");
        assert_eq!(
            metadata.get("x-agent-instance-id").unwrap(),
            "018fb9f1-3291-7a73-b261-c7de8a2af4d1"
        );
        assert!(
            !metadata.contains_key("x-agent-secret"),
            "legacy bearer metadata was retired in #412"
        );
    }

    #[test]
    fn transport_identity_metadata_omits_legacy_secret_on_secure_transports() {
        let config = metadata_test_config(TransportMode::Auto);

        for transport in [
            ResolvedTransport::Uds,
            ResolvedTransport::Vsock,
            ResolvedTransport::Tls,
        ] {
            let mut metadata = MetadataMap::new();

            populate_agent_metadata(&mut metadata, &config, transport).unwrap();

            assert_eq!(metadata.get("x-agent-id").unwrap(), "agent-01");
            assert_eq!(
                metadata.get("x-agent-instance-id").unwrap(),
                "018fb9f1-3291-7a73-b261-c7de8a2af4d1"
            );
            assert!(
                !metadata.contains_key("x-agent-secret"),
                "{transport:?} must not carry the legacy bearer"
            );
        }
    }

    #[test]
    fn bootstrap_enrollment_url_requires_explicit_endpoint() {
        let err = bootstrap_enrollment_url(None, "host.internal:8120".to_string()).unwrap_err();
        assert!(
            err.to_string()
                .contains("AGENT_BOOTSTRAP_ENROLLMENT_URL is required"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn bootstrap_enrollment_url_accepts_explicit_endpoint() {
        assert_eq!(
            bootstrap_enrollment_url(
                Some("http://mgmt.example/bootstrap".to_string()),
                "host.internal:8120".to_string()
            )
            .unwrap(),
            "http://mgmt.example/bootstrap"
        );
    }

    #[test]
    fn bootstrap_tls_paths_detect_complete_material() {
        let dir = tempfile::tempdir().unwrap();
        let paths = BootstrapTlsPaths {
            ca: dir.path().join("ca.pem"),
            cert: dir.path().join("agent.pem"),
            key: dir.path().join("agent-key.pem"),
        };

        assert!(!paths.complete());
        fs::write(&paths.ca, "ca").unwrap();
        fs::write(&paths.cert, "cert").unwrap();
        fs::write(&paths.key, "key").unwrap();
        assert!(paths.complete());
    }

    #[test]
    fn renewal_schedule_is_stable_and_between_half_and_sixty_percent() {
        let dir = tempfile::tempdir().unwrap();
        let key = KeyPair::generate().unwrap();
        let spiffe_id = "spiffe://sandbox.example/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params.subject_alt_names = vec![SanType::URI(spiffe_id.try_into().unwrap())];
        params.not_before = time::OffsetDateTime::now_utc() - time::Duration::minutes(1);
        params.not_after = params.not_before + time::Duration::hours(1);
        let cert = params.self_signed(&key).unwrap();
        let path = dir.path().join("agent.pem");
        fs::write(&path, cert.pem()).unwrap();

        let first = certificate_renewal_delay(&path).unwrap();
        let second = certificate_renewal_delay(&path).unwrap();
        assert_eq!(first, second);
        assert!(first <= Duration::from_secs(36 * 60));
        assert_eq!(certificate_spiffe_id(&path).unwrap(), spiffe_id);
    }

    #[test]
    fn atomic_tls_write_replaces_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.pem");
        write_private_file_atomic(&path, "first").unwrap();
        write_private_file_atomic(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn write_bootstrap_tls_files_uses_private_modes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = BootstrapTlsPaths {
            ca: dir.path().join("nested/ca.pem"),
            cert: dir.path().join("nested/agent.pem"),
            key: dir.path().join("nested/agent-key.pem"),
        };
        let key = KeyPair::generate().unwrap();
        let response = BootstrapConsumeResponse {
            spiffe_id: "spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1"
                .to_string(),
            certificate_pem: "cert-pem".to_string(),
            ca_pem: "ca-pem".to_string(),
        };

        write_bootstrap_tls_files(&paths, &response, &key).unwrap();

        let parent_mode = fs::metadata(paths.key.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let key_mode = fs::metadata(&paths.key).unwrap().permissions().mode() & 0o777;
        let cert_mode = fs::metadata(&paths.cert).unwrap().permissions().mode() & 0o777;
        let ca_mode = fs::metadata(&paths.ca).unwrap().permissions().mode() & 0o777;
        assert_eq!(parent_mode, 0o700);
        assert_eq!(key_mode, 0o600);
        assert_eq!(cert_mode, 0o600);
        assert_eq!(ca_mode, 0o600);
    }

    fn signed_bootstrap_response(key: &KeyPair, spiffe_id: &str) -> BootstrapConsumeResponse {
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        let ca_key = KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let csr = csr_pem_for_spiffe(spiffe_id, key).unwrap();
        let leaf = rcgen::CertificateSigningRequestParams::from_pem(&csr)
            .unwrap()
            .signed_by(&ca_cert, &ca_key)
            .unwrap();
        BootstrapConsumeResponse {
            spiffe_id: spiffe_id.to_string(),
            certificate_pem: leaf.pem(),
            ca_pem: ca_cert.pem(),
        }
    }

    #[test]
    fn bootstrap_response_requires_expected_san_key_and_chain() {
        let expected = "spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let key = KeyPair::generate().unwrap();
        let valid = signed_bootstrap_response(&key, expected);
        validate_bootstrap_certificate_response(&valid, expected, &key).unwrap();

        let wrong_key = KeyPair::generate().unwrap();
        assert!(
            validate_bootstrap_certificate_response(&valid, expected, &wrong_key)
                .unwrap_err()
                .to_string()
                .contains("different private key")
        );

        let wrong_san = signed_bootstrap_response(
            &key,
            "spiffe://sandbox.agentic.local/agent/018fb9f2-94a1-7c2d-b0c4-01fd58bb5ec1",
        );
        assert!(
            validate_bootstrap_certificate_response(&wrong_san, expected, &key)
                .unwrap_err()
                .to_string()
                .contains("requested SPIFFE")
        );

        let mut rogue_chain = valid;
        rogue_chain.ca_pem = signed_bootstrap_response(&key, expected).ca_pem;
        assert!(
            validate_bootstrap_certificate_response(&rogue_chain, expected, &key)
                .unwrap_err()
                .to_string()
                .contains("chain is invalid")
        );
    }

    #[test]
    fn bootstrap_problem_text_redacts_token() {
        let _env = lock_bootstrap_env();
        env::set_var("AGENT_BOOTSTRAP_TOKEN", "synthetic-token");
        assert_eq!(
            redact_bootstrap_token_text("token synthetic-token failed"),
            "token [REDACTED_BOOTSTRAP_TOKEN] failed"
        );
        env::remove_var("AGENT_BOOTSTRAP_TOKEN");
    }

    #[test]
    fn format_error_chain_preserves_underlying_transport_cause() {
        let err = anyhow::anyhow!("received fatal alert: BadCertificate")
            .context("transport error")
            .context("Failed to connect to management server over mTLS");

        assert_eq!(
            format_error_chain(&err),
            "Failed to connect to management server over mTLS: transport error: received fatal alert: BadCertificate"
        );
    }

    #[test]
    fn scrub_bootstrap_env_file_removes_one_time_token_only() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join("agent.env");
        fs::write(
            &env_file,
            [
                "AGENT_ID=agent-01",
                "AGENT_BOOTSTRAP_TOKEN=synthetic-token",
                "AGENT_BOOTSTRAP_SPIFFE_ID=spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1",
                "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=1900000000000",
                "MANAGEMENT_SERVER=host.internal:8120",
            ]
            .join("\n"),
        )
        .unwrap();

        scrub_bootstrap_env_file(env_file.to_str().unwrap()).unwrap();
        let scrubbed = fs::read_to_string(&env_file).unwrap();

        assert!(scrubbed.contains("AGENT_ID=agent-01"));
        assert!(scrubbed.contains("AGENT_BOOTSTRAP_SPIFFE_ID="));
        assert!(scrubbed.contains("MANAGEMENT_SERVER=host.internal:8120"));
        assert!(!scrubbed.contains("AGENT_BOOTSTRAP_TOKEN=synthetic-token"));
        assert!(!scrubbed.contains("AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS"));
        assert_eq!(
            fs::metadata(&env_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn restore_bootstrap_env_file_loads_private_drop() {
        let _env = lock_bootstrap_env();
        let dir = tempfile::tempdir().unwrap();
        let restore_env = dir.path().join("restore-bootstrap.env");
        fs::write(
            &restore_env,
            [
                "AGENT_BOOTSTRAP_TOKEN=restore-token",
                "AGENT_BOOTSTRAP_SPIFFE_ID=spiffe://sandbox.agentic.local/agent/restored",
                "AGENT_BOOTSTRAP_ENROLLMENT_URL=https://host.internal:8124/api/v1/bootstrap-enrollment/consume",
            ]
            .join("\n"),
        )
        .unwrap();
        fs::set_permissions(&restore_env, fs::Permissions::from_mode(0o600)).unwrap();

        env::set_var(
            RESTORE_BOOTSTRAP_ENV_FILE_ENV,
            restore_env.to_string_lossy().as_ref(),
        );
        env::remove_var("AGENT_BOOTSTRAP_TOKEN");
        env::remove_var("AGENT_BOOTSTRAP_SPIFFE_ID");

        assert_eq!(
            load_restore_bootstrap_env_file().as_deref(),
            Some(restore_env.to_str().unwrap())
        );
        assert_eq!(
            env_string_optional("AGENT_BOOTSTRAP_TOKEN").as_deref(),
            Some("restore-token")
        );
        assert_eq!(
            env_string_optional("AGENT_BOOTSTRAP_SPIFFE_ID").as_deref(),
            Some("spiffe://sandbox.agentic.local/agent/restored")
        );

        env::remove_var(RESTORE_BOOTSTRAP_ENV_FILE_ENV);
        env::remove_var("AGENT_BOOTSTRAP_TOKEN");
        env::remove_var("AGENT_BOOTSTRAP_SPIFFE_ID");
        env::remove_var("AGENT_BOOTSTRAP_ENROLLMENT_URL");
    }

    #[test]
    fn restore_bootstrap_env_file_rejects_loose_permissions() {
        let _env = lock_bootstrap_env();
        let dir = tempfile::tempdir().unwrap();
        let restore_env = dir.path().join("restore-bootstrap.env");
        fs::write(&restore_env, "AGENT_BOOTSTRAP_TOKEN=restore-token\n").unwrap();
        fs::set_permissions(&restore_env, fs::Permissions::from_mode(0o644)).unwrap();

        env::set_var(
            RESTORE_BOOTSTRAP_ENV_FILE_ENV,
            restore_env.to_string_lossy().as_ref(),
        );
        env::remove_var("AGENT_BOOTSTRAP_TOKEN");

        assert!(load_restore_bootstrap_env_file().is_none());
        assert!(env_string_optional("AGENT_BOOTSTRAP_TOKEN").is_none());

        env::remove_var(RESTORE_BOOTSTRAP_ENV_FILE_ENV);
    }

    #[tokio::test]
    async fn restore_bootstrap_drop_consumes_token_and_materializes_mtls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let _env = lock_bootstrap_env();
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join("agent.env");
        let restore_env = dir.path().join("restore-bootstrap.env");
        let tls_dir = dir.path().join("bootstrap-tls");
        let spiffe_id = "spiffe://sandbox.agentic.local/agent/restored-child";
        let token = "restore-token-one-time";

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/api/v1/bootstrap-enrollment/consume",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn({
            let spiffe_id = spiffe_id.to_string();
            async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                assert!(request.contains("\"token\":\"restore-token-one-time\""));
                assert!(request.contains(
                    "\"spiffe_id\":\"spiffe://sandbox.agentic.local/agent/restored-child\""
                ));
                assert!(request.contains("\"csr_pem\":"));

                let request_body = request.split("\r\n\r\n").nth(1).unwrap();
                let payload: serde_json::Value = serde_json::from_str(request_body).unwrap();
                let csr_pem = payload["csr_pem"].as_str().unwrap();
                let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
                ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
                ca_params.key_usages = vec![
                    rcgen::KeyUsagePurpose::KeyCertSign,
                    rcgen::KeyUsagePurpose::CrlSign,
                ];
                let ca_key = KeyPair::generate().unwrap();
                let ca_cert = ca_params.self_signed(&ca_key).unwrap();
                let mut csr = rcgen::CertificateSigningRequestParams::from_pem(csr_pem).unwrap();
                csr.params.is_ca = rcgen::IsCa::ExplicitNoCa;
                csr.params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
                let leaf = csr.signed_by(&ca_cert, &ca_key).unwrap();

                let body = serde_json::json!({
                    "spiffe_id": spiffe_id,
                    "certificate_pem": leaf.pem(),
                    "ca_pem": ca_cert.pem()
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        fs::write(&env_file, "AGENT_ID=restored-child\n").unwrap();
        // Baked clean-base configuration is root-owned. A restore token lives
        // only in the isolated inbox drop, so enrollment must not attempt to
        // rewrite this unrelated file as the unprivileged agent user.
        fs::set_permissions(&env_file, fs::Permissions::from_mode(0o000)).unwrap();
        fs::write(
            &restore_env,
            [
                "AGENT_TRANSPORT=auto",
                &format!("AGENT_BOOTSTRAP_TOKEN={token}"),
                &format!("AGENT_BOOTSTRAP_SPIFFE_ID={spiffe_id}"),
                &format!("AGENT_BOOTSTRAP_TLS_DIR={}", tls_dir.display()),
                &format!("AGENT_BOOTSTRAP_ENROLLMENT_URL={endpoint}"),
            ]
            .join("\n"),
        )
        .unwrap();
        fs::set_permissions(&restore_env, fs::Permissions::from_mode(0o600)).unwrap();

        for key in [
            RESTORE_BOOTSTRAP_ENV_FILE_ENV,
            "AGENT_BOOTSTRAP_TOKEN",
            "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS",
            "AGENT_BOOTSTRAP_SPIFFE_ID",
            "AGENT_BOOTSTRAP_TLS_DIR",
            "AGENT_BOOTSTRAP_ENROLLMENT_URL",
            "AGENT_GRPC_TLS_CA",
            "AGENT_GRPC_TLS_CERT",
            "AGENT_GRPC_TLS_KEY",
        ] {
            env::remove_var(key);
        }
        env::set_var(
            RESTORE_BOOTSTRAP_ENV_FILE_ENV,
            restore_env.to_string_lossy().as_ref(),
        );

        let mut cli = test_cli();
        cli.env_file = env_file.to_string_lossy().to_string();
        maybe_bootstrap_enroll(&cli).await.unwrap();
        server.await.unwrap();

        let ca = tls_dir.join("ca.pem");
        let cert = tls_dir.join("agent.pem");
        let key = tls_dir.join("agent-key.pem");
        assert!(fs::read_to_string(&ca)
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
        assert!(fs::read_to_string(&cert)
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
        assert!(fs::read_to_string(&key).unwrap().contains("PRIVATE KEY"));
        for path in [&ca, &cert, &key] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let runtime_env = tls_dir.join("agent.env");
        assert!(fs::read_to_string(&runtime_env)
            .unwrap()
            .contains("AGENT_RESTORE_ENROLLED=true"));
        assert_eq!(
            fs::metadata(&runtime_env).unwrap().permissions().mode() & 0o777,
            0o600
        );

        assert!(!restore_env.exists());
        assert_eq!(
            fs::metadata(&env_file).unwrap().permissions().mode() & 0o777,
            0o000
        );
        assert!(env_string_optional("AGENT_BOOTSTRAP_TOKEN").is_none());
        assert_eq!(
            env_string_optional("AGENT_GRPC_TLS_CA").as_deref(),
            Some(ca.to_string_lossy().as_ref())
        );
        assert_eq!(
            env_string_optional("AGENT_GRPC_TLS_CERT").as_deref(),
            Some(cert.to_string_lossy().as_ref())
        );
        assert_eq!(
            env_string_optional("AGENT_GRPC_TLS_KEY").as_deref(),
            Some(key.to_string_lossy().as_ref())
        );

        for key in [
            RESTORE_BOOTSTRAP_ENV_FILE_ENV,
            "AGENT_BOOTSTRAP_TOKEN",
            "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS",
            "AGENT_BOOTSTRAP_SPIFFE_ID",
            "AGENT_BOOTSTRAP_TLS_DIR",
            "AGENT_BOOTSTRAP_ENROLLMENT_URL",
            "AGENT_GRPC_TLS_CA",
            "AGENT_GRPC_TLS_CERT",
            "AGENT_GRPC_TLS_KEY",
        ] {
            env::remove_var(key);
        }
    }

    #[tokio::test]
    async fn restore_bootstrap_refuses_cloned_tls_identity_without_scrubbing_token() {
        let _env = lock_bootstrap_env();
        let dir = tempfile::tempdir().unwrap();
        let restore_env = dir.path().join("restore-bootstrap.env");
        let tls_dir = dir.path().join("bootstrap-tls");
        fs::create_dir_all(&tls_dir).unwrap();
        for name in ["ca.pem", "agent.pem", "agent-key.pem"] {
            fs::write(tls_dir.join(name), "cloned-identity").unwrap();
        }
        fs::write(
            &restore_env,
            [
                "AGENT_BOOTSTRAP_TOKEN=must-not-be-scrubbed",
                "AGENT_BOOTSTRAP_SPIFFE_ID=spiffe://sandbox.agentic.local/agent/new-child",
                &format!("AGENT_BOOTSTRAP_TLS_DIR={}", tls_dir.display()),
                "AGENT_BOOTSTRAP_ENROLLMENT_URL=https://host.internal:8124/api/v1/bootstrap-enrollment/consume",
            ]
            .join("\n"),
        )
        .unwrap();
        fs::set_permissions(&restore_env, fs::Permissions::from_mode(0o600)).unwrap();
        for key in BOOTSTRAP_TEST_ENV_KEYS {
            env::remove_var(key);
        }
        env::set_var(
            RESTORE_BOOTSTRAP_ENV_FILE_ENV,
            restore_env.to_string_lossy().as_ref(),
        );
        let mut cli = test_cli();
        cli.env_file = dir.path().join("missing.env").display().to_string();

        let error = maybe_bootstrap_enroll(&cli).await.unwrap_err();
        assert!(error.to_string().contains("refused cloned TLS files"));
        assert!(restore_env.exists());
        assert!(fs::read_to_string(&restore_env)
            .unwrap()
            .contains("AGENT_BOOTSTRAP_TOKEN=must-not-be-scrubbed"));
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
                .map(|l| {
                    l.trim_start_matches("PRETTY_NAME=")
                        .trim_matches('"')
                        .to_string()
                })
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

/// Resolve the directory an interactive/managed session should start in.
///
/// #615: an unset (empty/whitespace) or "~" working_dir resolves to the agent's
/// $HOME (e.g. /root in a container, /home/agent in a VM), NOT the agent process
/// cwd — which is `/` in Docker and the bridge cwd on host, so a managed tmux
/// attach previously landed in the wrong directory. A non-empty path is honored
/// verbatim so an explicitly requested working_dir is respected.
fn resolve_working_dir(working_dir: &str) -> String {
    match working_dir.trim() {
        "" | "~" => env::var("HOME").unwrap_or_else(|_| "/home/agent".to_string()),
        dir => dir.to_string(),
    }
}

#[cfg(test)]
mod working_dir_tests {
    use super::resolve_working_dir;

    #[test]
    fn honors_explicit_path() {
        assert_eq!(resolve_working_dir("/tmp/work"), "/tmp/work");
        assert_eq!(resolve_working_dir("/root"), "/root");
        assert_eq!(
            resolve_working_dir("/home/agent/workspace"),
            "/home/agent/workspace"
        );
    }

    #[test]
    fn defaults_unset_to_home() {
        // #615: empty / whitespace-only / "~" must land in the agent's $HOME,
        // not the agent process cwd. Compare against the same env the fn reads
        // (no set_var — safe under parallel test execution).
        let expected = std::env::var("HOME").unwrap_or_else(|_| "/home/agent".to_string());
        assert_eq!(resolve_working_dir(""), expected);
        assert_eq!(resolve_working_dir("   "), expected);
        assert_eq!(resolve_working_dir("~"), expected);
    }
}

/// Read AIWG framework info from ~/.loadout-manifest.json if present.
/// Returns an empty Vec if the file doesn't exist or cannot be parsed.
fn read_loadout_manifest_frameworks() -> Vec<proto::AiwgFramework> {
    let home = env::var("HOME").unwrap_or_else(|_| "/home/agent".to_string());
    let manifest_path = PathBuf::from(&home).join(".loadout-manifest.json");

    let content = match fs::read_to_string(&manifest_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let manifest: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            warn!(path = %manifest_path.display(), error = %e, "Failed to parse loadout-manifest.json");
            return Vec::new();
        }
    };

    let frameworks = match manifest.get("aiwg_frameworks").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    frameworks
        .iter()
        .filter_map(|fw| {
            let name = fw.get("name")?.as_str()?.to_string();
            let providers = fw
                .get("providers")
                .and_then(|p| p.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            Some(proto::AiwgFramework { name, providers })
        })
        .collect()
}

// =============================================================================
// Command Executor
// =============================================================================

fn command_correlation(cmd: &proto::CommandRequest) -> (String, String) {
    let task_id = cmd
        .env
        .get("AIWG_A2A_TASK_ID")
        .cloned()
        .unwrap_or_else(|| cmd.command_id.clone());
    let mission_id = cmd
        .env
        .get("AIWG_MISSION_ID")
        .cloned()
        .unwrap_or_else(|| task_id.clone());
    (mission_id, task_id)
}

async fn execute_command(
    cmd: proto::CommandRequest,
    output_tx: mpsc::Sender<AgentMessage>,
    agentshare: Option<Arc<AgentshareLogger>>,
    running_commands: RunningCommands,
) {
    let command_id = cmd.command_id.clone();
    let start = std::time::Instant::now();
    let (mission_id, task_id) = command_correlation(&cmd);

    info!("[{}] Executing: {} {:?}", command_id, cmd.command, cmd.args);
    info!(
        command_id = %command_id,
        mission_id = %mission_id,
        task_id = %task_id,
        command = %cmd.command,
        "Executing command"
    );

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

    // #615: default an empty working_dir to the agent's $HOME rather than "."
    // (the agent process cwd — `/` in a container, the bridge cwd on host).
    let spawn_dir = resolve_working_dir(&cmd.working_dir);
    let mut process = match Command::new(&full_cmd[0])
        .args(&full_cmd[1..])
        .current_dir(&spawn_dir)
        .envs(cmd.env.iter())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(p) => p,
        Err(e) => {
            error!(
                command_id = %command_id,
                mission_id = %mission_id,
                task_id = %task_id,
                error = %e,
                "Failed to spawn command"
            );
            error!("[{}] Failed to spawn: {}", command_id, e);
            let result = CommandResult {
                command_id: command_id.clone(),
                exit_code: -1,
                error: e.to_string(),
                duration_ms: 0,
                success: false,
            };
            let _ = output_tx
                .send(AgentMessage {
                    payload: Some(proto::agent_message::Payload::CommandResult(result)),
                })
                .await;
            return;
        }
    };

    // Set up stdin channel for interactive input
    let stdin = process.stdin.take();
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<StdinData>(100);

    // Store sender in running_commands
    {
        let mut running = running_commands.lock().await;
        running.insert(
            command_id.clone(),
            RunningCommand {
                stdin_tx,
                pty_control_tx: None,
                pid: None,
                session_name: (!cmd.session_name.is_empty()).then(|| cmd.session_name.clone()),
                session_id: (!cmd.session_id.is_empty()).then(|| cmd.session_id.clone()),
                command: cmd.command.clone(),
                started_at: std::time::Instant::now(),
                is_pty: false,
            },
        );
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
                let _ = tx_stdout
                    .send(AgentMessage {
                        payload: Some(proto::agent_message::Payload::Stdout(chunk)),
                    })
                    .await;
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
                let _ = tx_stderr
                    .send(AgentMessage {
                        payload: Some(proto::agent_message::Payload::Stderr(chunk)),
                    })
                    .await;
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
            drop(process.kill()); // Fire-and-forget kill on timeout
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Command timed out")
        })
        .and_then(|r| r)
    } else {
        process.wait().await
    };

    // Wait for output streams to drain (they finish naturally when the
    // child closes stdout/stderr). The stdin task can't finish on its
    // own — its sender (`stdin_tx`) is held inside `running_commands`
    // and is dropped only after this function returns, so joining it
    // here deadlocks any command that doesn't receive a stdin EOF
    // marker (#271). Abort it instead; dropping the captured `stdin`
    // pipe closes the write end to the (already exited) child.
    let _ = tokio::join!(stdout_task, stderr_task);
    stdin_task.abort();

    let duration_ms = start.elapsed().as_millis() as i64;
    let (exit_code, error_msg, success) = match exit_status {
        Ok(status) => (status.code().unwrap_or(-1), String::new(), status.success()),
        Err(e) => (-1, e.to_string(), false),
    };

    info!(
        "[{}] Completed: exit={}, duration={}ms",
        command_id, exit_code, duration_ms
    );
    info!(
        command_id = %command_id,
        mission_id = %mission_id,
        task_id = %task_id,
        exit_code = exit_code,
        duration_ms = duration_ms,
        "Command completed"
    );

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
    let _ = output_tx
        .send(AgentMessage {
            payload: Some(proto::agent_message::Payload::CommandResult(result)),
        })
        .await;
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
    let (mission_id, task_id) = command_correlation(&cmd);

    info!(
        "[{}] Executing (PTY): {} {:?}",
        command_id, cmd.command, cmd.args
    );
    info!(
        command_id = %command_id,
        mission_id = %mission_id,
        task_id = %task_id,
        command = %cmd.command,
        "Executing PTY command"
    );

    if let Some(ref logger) = agentshare {
        logger.write_command(&command_id, &cmd.command, &cmd.args);
    }

    // Determine terminal size
    let cols = if cmd.pty_cols > 0 {
        cmd.pty_cols as u16
    } else {
        80
    };
    let rows = if cmd.pty_rows > 0 {
        cmd.pty_rows as u16
    } else {
        24
    };
    let term_env = if cmd.pty_term.is_empty() {
        "xterm-256color".to_string()
    } else {
        cmd.pty_term.clone()
    };

    // Open PTY pair
    let pty_result = openpty(None, None);
    let pty = match pty_result {
        Ok(pty) => pty,
        Err(e) => {
            error!(
                command_id = %command_id,
                mission_id = %mission_id,
                task_id = %task_id,
                error = %e,
                "Failed to open PTY"
            );
            error!("[{}] Failed to open PTY: {}", command_id, e);
            let result = CommandResult {
                command_id: command_id.clone(),
                exit_code: -1,
                error: format!("Failed to open PTY: {}", e),
                duration_ms: 0,
                success: false,
            };
            let _ = output_tx
                .send(AgentMessage {
                    payload: Some(proto::agent_message::Payload::CommandResult(result)),
                })
                .await;
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
                #[cfg(target_os = "linux")]
                libc::ioctl(slave_fd.as_raw_fd(), libc::TIOCSCTTY, 0);

                #[cfg(not(target_os = "linux"))]
                libc::ioctl(slave_fd.as_raw_fd(), libc::TIOCSCTTY as libc::c_ulong, 0);
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

            // Change working directory. #615: honor the requested working_dir;
            // when it is unset (or "~"), default to the agent's $HOME instead of
            // inheriting the agent process cwd — which is `/` in Docker and the
            // bridge cwd on host, so a managed tmux attach landed in the wrong
            // directory. VM only appeared correct because its agent process
            // already runs in /home/agent.
            let _ = std::env::set_current_dir(resolve_working_dir(&cmd.working_dir));

            // Exec via shell
            let c_shell = std::ffi::CString::new("/bin/bash").unwrap();
            let c_arg0 = std::ffi::CString::new("-bash").unwrap();
            let c_cmd = std::ffi::CString::new("-c".to_string()).unwrap();
            let c_script = std::ffi::CString::new(shell_cmd.as_str()).unwrap();

            if cmd.args.is_empty()
                && (cmd.command == "/bin/bash"
                    || cmd.command == "bash"
                    || cmd.command == "/bin/sh"
                    || cmd.command == "sh")
            {
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
            error!(
                command_id = %command_id,
                mission_id = %mission_id,
                task_id = %task_id,
                error = %e,
                "Failed to fork PTY command"
            );
            error!("[{}] Fork failed: {}", command_id, e);
            let result = CommandResult {
                command_id: command_id.clone(),
                exit_code: -1,
                error: format!("Fork failed: {}", e),
                duration_ms: 0,
                success: false,
            };
            let _ = output_tx
                .send(AgentMessage {
                    payload: Some(proto::agent_message::Payload::CommandResult(result)),
                })
                .await;
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
        error!(
            "[{}] Failed to dup master fd: {}",
            command_id,
            std::io::Error::last_os_error()
        );
        let result = CommandResult {
            command_id: command_id.clone(),
            exit_code: -1,
            error: "Failed to dup PTY master fd".to_string(),
            duration_ms: 0,
            success: false,
        };
        let _ = output_tx
            .send(AgentMessage {
                payload: Some(proto::agent_message::Payload::CommandResult(result)),
            })
            .await;
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
        running.insert(
            command_id.clone(),
            RunningCommand {
                stdin_tx,
                pty_control_tx: Some(pty_ctl_tx),
                pid: Some(child_pid),
                session_name: (!cmd.session_name.is_empty()).then(|| cmd.session_name.clone()),
                session_id: (!cmd.session_id.is_empty()).then(|| cmd.session_id.clone()),
                command: cmd.command.clone(),
                started_at: std::time::Instant::now(),
                is_pty: true,
            },
        );
    }

    // Task: blocking read on dedicated thread → stream output via mpsc
    let cmd_id_out = command_id.clone();
    let tx_out = output_tx.clone();
    let agentshare_out = agentshare.clone();
    let output_task = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            let n =
                unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
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
            if tx_out
                .blocking_send(AgentMessage {
                    payload: Some(proto::agent_message::Payload::Stdout(chunk)),
                })
                .is_err()
            {
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
            let result =
                unsafe { libc::write(write_fd, data.as_ptr() as *const libc::c_void, data.len()) };
            if result < 0 {
                debug!(
                    "[{}] Master write error: {}",
                    cmd_id_in,
                    std::io::Error::last_os_error()
                );
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
    })
    .await
    .unwrap_or(-1);

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

    info!(
        "[{}] PTY completed: exit={}, duration={}ms",
        command_id, exit_status, duration_ms
    );
    info!(
        command_id = %command_id,
        mission_id = %mission_id,
        task_id = %task_id,
        exit_code = exit_status,
        duration_ms = duration_ms,
        "PTY command completed"
    );

    // Remove from running commands
    running_commands.lock().await.remove(&command_id);

    if let Some(ref logger) = agentshare {
        logger.write_command_result(&command_id, exit_status, duration_ms);
    }

    // Send EOF marker
    let _ = output_tx
        .send(AgentMessage {
            payload: Some(proto::agent_message::Payload::Stdout(OutputChunk {
                stream_id: command_id.clone(),
                data: vec![],
                timestamp_ms: chrono_timestamp_ms(),
                eof: true,
            })),
        })
        .await;

    let result = CommandResult {
        command_id,
        exit_code: exit_status,
        error: String::new(),
        duration_ms,
        success,
    };
    let _ = output_tx
        .send(AgentMessage {
            payload: Some(proto::agent_message::Payload::CommandResult(result)),
        })
        .await;
}
// =============================================================================
// Claude Task Executor
// =============================================================================

/// Execute a Claude Code task using the ClaudeRunner
///
/// This is invoked when the orchestrator sends a `__claude_task__` command.
/// The command format is:
/// - command: "__claude_task__"
/// - args[0]: JSON-encoded ClaudeTaskConfig
///
/// Output is streamed back via gRPC OutputChunk messages.
async fn execute_claude_task(
    cmd: proto::CommandRequest,
    output_tx: mpsc::Sender<AgentMessage>,
    running_commands: RunningCommands,
) {
    let command_id = cmd.command_id.clone();
    let start = std::time::Instant::now();
    let (mission_id, fallback_task_id) = command_correlation(&cmd);

    // Parse task config from first argument
    let mut config: claude::ClaudeTaskConfig =
        match cmd.args.first().and_then(|s| serde_json::from_str(s).ok()) {
            Some(c) => c,
            None => {
                error!("[{}] Invalid Claude task config", command_id);
                let result = CommandResult {
                    command_id: command_id.clone(),
                    exit_code: -1,
                    error: "Invalid Claude task config: expected JSON in first argument"
                        .to_string(),
                    duration_ms: 0,
                    success: false,
                };
                let _ = output_tx
                    .send(AgentMessage {
                        payload: Some(proto::agent_message::Payload::CommandResult(result)),
                    })
                    .await;
                return;
            }
        };

    // Set task_id from command_id if not already set
    if config.task_id.is_empty() {
        config.task_id = fallback_task_id;
    }
    let task_id = config.task_id.clone();

    // Use default working directory if not specified
    if config.working_dir.is_empty() {
        config.working_dir = "/home/agent/workspace".to_string();
    }

    info!(
        "[{}] Executing Claude task: {}",
        command_id,
        config.prompt.chars().take(80).collect::<String>()
    );
    info!(
        command_id = %command_id,
        mission_id = %mission_id,
        task_id = %task_id,
        working_dir = %config.working_dir,
        "Executing Claude task"
    );

    // Create ClaudeRunner
    let runner = claude::ClaudeRunner::new(config);

    // Create output channel for ClaudeRunner
    let (claude_tx, mut claude_rx) = mpsc::channel::<claude::OutputChunk>(256);

    // Set up stdin placeholder (Claude reads from prompt, not stdin)
    let (stdin_tx, _stdin_rx) = mpsc::channel::<StdinData>(1);

    // Store in running commands (for potential cancellation)
    {
        let mut running = running_commands.lock().await;
        running.insert(
            command_id.clone(),
            RunningCommand {
                stdin_tx,
                pty_control_tx: None,
                pid: None,
                session_name: Some(if cmd.session_name.is_empty() {
                    "claude".to_string()
                } else {
                    cmd.session_name.clone()
                }),
                session_id: (!cmd.session_id.is_empty()).then(|| cmd.session_id.clone()),
                command: "__claude_task__".to_string(),
                started_at: std::time::Instant::now(),
                is_pty: false,
            },
        );
    }

    // Spawn task to forward ClaudeRunner output to gRPC stream
    let cmd_id_fwd = command_id.clone();
    let tx_fwd = output_tx.clone();
    let forward_task = tokio::spawn(async move {
        while let Some(chunk) = claude_rx.recv().await {
            let proto_chunk = OutputChunk {
                stream_id: cmd_id_fwd.clone(),
                data: chunk.data.into_bytes(),
                timestamp_ms: chunk.timestamp,
                eof: false,
            };

            let payload = if chunk.stream == "stdout" {
                proto::agent_message::Payload::Stdout(proto_chunk)
            } else {
                proto::agent_message::Payload::Stderr(proto_chunk)
            };

            if tx_fwd
                .send(AgentMessage {
                    payload: Some(payload),
                })
                .await
                .is_err()
            {
                warn!("[{}] Output receiver dropped", cmd_id_fwd);
                break;
            }
        }
    });

    // Run Claude Code
    let exit_result = runner.run(claude_tx).await;

    // Wait for output forwarding to complete
    let _ = forward_task.await;

    let duration_ms = start.elapsed().as_millis() as i64;

    let (exit_code, error_msg, success) = match exit_result {
        Ok(code) => (code, String::new(), code == 0),
        Err(e) => {
            error!(
                command_id = %command_id,
                mission_id = %mission_id,
                task_id = %task_id,
                error = %e,
                "Claude execution failed"
            );
            error!("[{}] Claude execution failed: {}", command_id, e);
            (-1, e.to_string(), false)
        }
    };

    info!(
        "[{}] Claude task completed: exit={}, duration={}ms",
        command_id, exit_code, duration_ms
    );
    info!(
        command_id = %command_id,
        mission_id = %mission_id,
        task_id = %task_id,
        exit_code = exit_code,
        duration_ms = duration_ms,
        "Claude task completed"
    );

    // Remove from running commands
    running_commands.lock().await.remove(&command_id);

    // Send EOF marker
    let _ = output_tx
        .send(AgentMessage {
            payload: Some(proto::agent_message::Payload::Stdout(OutputChunk {
                stream_id: command_id.clone(),
                data: vec![],
                timestamp_ms: chrono_timestamp_ms(),
                eof: true,
            })),
        })
        .await;

    // Send command result
    let result = CommandResult {
        command_id,
        exit_code,
        error: error_msg,
        duration_ms,
        success,
    };
    let _ = output_tx
        .send(AgentMessage {
            payload: Some(proto::agent_message::Payload::CommandResult(result)),
        })
        .await;
}

fn chrono_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Read setup progress from /var/run/agentic-setup-progress.json.
///
/// Native-host instances do not run the guest image setup pipeline, so their
/// supervisor sets `AGENT_SETUP_COMPLETE=1` after preparing the process state.
/// This explicit signal is equivalent to the guest completion marker; absent
/// the signal, the existing guest/VM behavior is unchanged.
/// Returns (setup_status, progress_json, agent_status)
fn read_setup_progress() -> (String, String, AgentStatus) {
    let complete_override = env::var("AGENT_SETUP_COMPLETE")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"));
    read_setup_progress_from(
        complete_override,
        std::path::Path::new("/var/run/agentic-setup-complete"),
        std::path::Path::new("/var/run/agentic-setup-progress.json"),
    )
}

fn read_setup_progress_from(
    complete_override: bool,
    complete_path: &std::path::Path,
    progress_path: &std::path::Path,
) -> (String, String, AgentStatus) {
    let complete = complete_override || complete_path.exists();

    if complete {
        // Setup done — check if there were errors
        if let Ok(json) = std::fs::read_to_string(progress_path) {
            let has_failed = json.contains("\"failed\"");
            let status = if has_failed {
                "ready-with-errors"
            } else {
                "ready"
            };
            return (status.to_string(), json, AgentStatus::Ready);
        }
        return ("ready".to_string(), String::new(), AgentStatus::Ready);
    }

    // Setup still running or hasn't started
    if let Ok(json) = std::fs::read_to_string(progress_path) {
        // Extract current step from JSON
        let status = if let Some(start) = json.find("\"current_step\":\"") {
            let rest = &json[start + 16..];
            if let Some(end) = rest.find('"') {
                format!("installing:{}", &rest[..end])
            } else {
                "provisioning".to_string()
            }
        } else {
            "provisioning".to_string()
        };
        return (status, json, AgentStatus::Provisioning);
    }

    // No progress file yet — very early boot
    (
        "provisioning".to_string(),
        String::new(),
        AgentStatus::Provisioning,
    )
}

#[cfg(test)]
mod setup_progress_tests {
    use super::*;

    #[test]
    fn explicit_completion_marks_native_host_ready_without_guest_files() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let (setup_status, progress, status) = read_setup_progress_from(
            true,
            &tmp.path().join("setup-complete"),
            &tmp.path().join("setup-progress.json"),
        );

        assert_eq!(setup_status, "ready");
        assert!(progress.is_empty());
        assert_eq!(status, AgentStatus::Ready);
    }

    #[test]
    fn missing_completion_signal_preserves_guest_provisioning_state() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let (setup_status, progress, status) = read_setup_progress_from(
            false,
            &tmp.path().join("setup-complete"),
            &tmp.path().join("setup-progress.json"),
        );

        assert_eq!(setup_status, "provisioning");
        assert!(progress.is_empty());
        assert_eq!(status, AgentStatus::Provisioning);
    }
}

// =============================================================================
// Agent Client
// =============================================================================

/// Merge tmux-discovered sessions into the tracked set for a `SessionReport`,
/// dropping any discovered session that duplicates a tracked one. Tracked
/// sessions survive reconnects (#634), so a tmux-backed tracked session would
/// otherwise be double-reported — once under its original `command_id` and once
/// as the discovered `tmux:<name>` alias, migrating its server-side identity. A
/// discovered session is skipped when either its `command_id` OR its (non-empty)
/// `session_name` matches a tracked session. Extracted from `build_session_report`
/// so the dedup is unit-testable without a live tmux (#637).
fn merge_discovered_sessions(
    tracked: Vec<ActiveSession>,
    discovered: Vec<ActiveSession>,
) -> Vec<ActiveSession> {
    let known_ids: HashSet<String> = tracked
        .iter()
        .map(|session| session.command_id.clone())
        .collect();
    let known_names: HashSet<String> = tracked
        .iter()
        .map(|session| session.session_name.clone())
        .filter(|name| !name.is_empty())
        .collect();
    let mut sessions = tracked;
    sessions.extend(discovered.into_iter().filter(|session| {
        !known_ids.contains(&session.command_id) && !known_names.contains(&session.session_name)
    }));
    sessions
}

struct AgentClient {
    cli: Cli,
    config: AgentConfig,
    output_tx: mpsc::Sender<AgentMessage>,
    output_rx: Option<mpsc::Receiver<AgentMessage>>,
    agentshare: Option<Arc<AgentshareLogger>>,
    running_commands: RunningCommands,
    health: Arc<health::HealthMonitor>,
    watchdog: Option<Arc<health::SystemdWatchdog>>,
    /// Notified to force the control stream to tear down, re-dial, and
    /// re-register WITHOUT exiting the process (#625 operator-initiated
    /// reconnect, delivered via SIGHUP). `notify_one` semantics: a signal
    /// raised while not awaiting is stored as a permit, so it is never lost.
    reconnect_signal: Arc<Notify>,
    /// Hands the persistent output receiver back from the finished
    /// connection's forwarding task so session output survives reconnects
    /// (#634). Populated by `stream_loop`, consumed by
    /// `recover_output_channel` after the connection is torn down.
    output_rx_return: Option<oneshot::Receiver<mpsc::Receiver<AgentMessage>>>,
}

impl AgentClient {
    fn new(cli: Cli, config: AgentConfig) -> Self {
        let (tx, rx) = mpsc::channel(1000);

        // Initialize agentshare logger
        let mut logger = AgentshareLogger::new(&config.agent_id);
        let agentshare = if logger.initialize(&config.agent_id) {
            Some(Arc::new(logger))
        } else {
            None
        };

        let agent_id = config.agent_id.clone();

        Self {
            cli,
            config,
            output_tx: tx,
            output_rx: Some(rx),
            agentshare,
            running_commands: Arc::new(TokioMutex::new(HashMap::new())),
            health: Arc::new(health::HealthMonitor::new(agent_id)),
            watchdog: None, // Initialized in run()
            reconnect_signal: Arc::new(Notify::new()),
            output_rx_return: None,
        }
    }

    async fn refresh_bootstrap_and_config(&mut self) -> Result<()> {
        maybe_bootstrap_enroll(&self.cli).await?;
        self.config = AgentConfig::from_cli(&self.cli)?;
        Ok(())
    }

    async fn connect(&self) -> Result<AgentServiceClient<Channel>> {
        match self.config.resolved_transport()? {
            ResolvedTransport::Uds => {
                let uds_path = self
                    .config
                    .uds_path
                    .as_ref()
                    .context("UDS transport selected without a socket path")?;
                info!("Connecting to management UDS {}...", uds_path);
                let uds_path = Arc::new(PathBuf::from(uds_path));
                let channel =
                    Self::apply_channel_keepalive(Endpoint::from_static("http://[::]:50051"))
                        .connect_with_connector(service_fn(move |_| {
                            let uds_path = uds_path.clone();
                            async move { UnixStream::connect(&*uds_path).await.map(TokioIo::new) }
                        }))
                        .await
                        .context("Failed to connect to management UDS")?;

                info!("Connected to management server over UDS");
                return Ok(AgentServiceClient::new(channel));
            }
            ResolvedTransport::Vsock => {
                let addr = self.config.vsock_addr()?;
                info!(
                    cid = addr.cid(),
                    port = addr.port(),
                    "Connecting to management vsock..."
                );
                let channel =
                    Self::apply_channel_keepalive(Endpoint::from_static("http://agentic-vsock"))
                        .connect_with_connector(service_fn(move |_| async move {
                            tokio_vsock::VsockStream::connect(addr)
                                .await
                                .map(TonicVsockIo::new)
                        }))
                        .await
                        .context("Failed to connect to management vsock")?;

                info!("Connected to management server over vsock");
                return Ok(AgentServiceClient::new(channel));
            }
            ResolvedTransport::Tls => {
                info!(
                    server = %self.config.server_address,
                    "Connecting to management server over mTLS..."
                );
                let tls_config = self.config.client_tls_config()?;
                let channel = Self::apply_channel_keepalive(Channel::from_shared(format!(
                    "https://{}",
                    self.config.server_address
                ))?)
                .tcp_keepalive(Some(TCP_KEEPALIVE_INTERVAL))
                .tls_config(tls_config)?
                .connect()
                .await
                .context("Failed to connect to management server over mTLS")?;

                info!("Connected to management server over mTLS");
                return Ok(AgentServiceClient::new(channel));
            }
            ResolvedTransport::Tcp => {}
        }

        info!("Connecting to {}...", self.config.server_address);

        let channel = Self::apply_channel_keepalive(Channel::from_shared(format!(
            "http://{}",
            self.config.server_address
        ))?)
        .tcp_keepalive(Some(TCP_KEEPALIVE_INTERVAL))
        .connect()
        .await
        .context("Failed to connect to management server")?;

        info!("Connected to management server");
        Ok(AgentServiceClient::new(channel))
    }

    /// Client-side keepalive for the control channel (#633). The server
    /// already PINGs every 10s, but a stateful middlebox on the VM path
    /// (libvirt NAT conntrack, guest ufw state table, DHCP renewal events)
    /// can tear down the guest→host flow in a way only the client can
    /// detect. Client-originated HTTP/2 PINGs both refresh that state from
    /// the guest side and convert a silently dead flow into a transport
    /// error within ~30s, which trips the existing reconnect/backoff loop
    /// instead of leaving a zombie connection until the next write.
    fn apply_channel_keepalive(endpoint: Endpoint) -> Endpoint {
        endpoint
            .http2_keep_alive_interval(GRPC_KEEPALIVE_INTERVAL)
            .keep_alive_timeout(GRPC_KEEPALIVE_TIMEOUT)
            .keep_alive_while_idle(true)
    }

    fn create_registration(&self) -> AgentMessage {
        let aiwg_frameworks = read_loadout_manifest_frameworks();
        let reg = AgentRegistration {
            agent_id: self.config.agent_id.clone(),
            ip_address: get_primary_ip(),
            hostname: hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_default(),
            profile: env::var("AGENT_PROFILE").unwrap_or_default(),
            loadout: env::var("AGENT_LOADOUT").unwrap_or_default(),
            labels: HashMap::new(),
            system: Some(get_system_info()),
            aiwg_frameworks,
            // #252: echo back the provisioned instance_id (empty when
            // agent runs outside v2 flow; server generates one).
            instance_id: self.config.instance_id.clone(),
        };
        AgentMessage {
            payload: Some(proto::agent_message::Payload::Registration(reg)),
        }
    }

    /// Build session report from running_commands for reconciliation
    async fn build_session_report(&self) -> SessionReport {
        let running = self.running_commands.lock().await;

        let sessions: Vec<ActiveSession> = running
            .iter()
            .map(|(cmd_id, cmd)| ActiveSession {
                command_id: cmd_id.clone(),
                session_name: cmd.session_name.clone().unwrap_or_default(),
                session_type: if cmd.is_pty {
                    proto::SessionType::Interactive as i32
                } else {
                    proto::SessionType::Headless as i32
                },
                command: cmd.command.clone(),
                started_at_ms: cmd.started_at.elapsed().as_millis() as i64,
                pid: cmd.pid.map(|p| p.as_raw()).unwrap_or(0),
                is_pty: cmd.is_pty,
                session_id: cmd.session_id.clone().unwrap_or_default(),
            })
            .collect();
        drop(running);

        let sessions = merge_discovered_sessions(sessions, discover_tmux_sessions().await);

        SessionReport {
            agent_id: self.config.agent_id.clone(),
            sessions,
            timestamp_ms: chrono_timestamp_ms(),
        }
    }

    /// Kill sessions as instructed by server during reconciliation
    async fn kill_sessions(
        &self,
        session_ids: &[String],
        grace_seconds: i32,
    ) -> (Vec<String>, Vec<String>) {
        let mut killed = Vec::new();
        let mut failed = Vec::new();

        // First pass: send SIGTERM
        {
            let running = self.running_commands.lock().await;
            for cmd_id in session_ids {
                if tmux_session_name_from_command_id(cmd_id).is_some() {
                    continue;
                }
                if let Some(cmd) = running.get(cmd_id) {
                    if let Some(pid) = cmd.pid {
                        info!(
                            "[{}] Sending SIGTERM to PID {} for reconciliation",
                            cmd_id, pid
                        );
                        if nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).is_ok() {
                            // Will track success after grace period
                        } else {
                            warn!("[{}] Failed to send SIGTERM to PID {}", cmd_id, pid);
                        }
                    }
                }
            }
        }

        // Wait for grace period
        if grace_seconds > 0 {
            tokio::time::sleep(Duration::from_secs(grace_seconds as u64)).await;
        }

        // Second pass: check what's still running and SIGKILL
        {
            let mut running = self.running_commands.lock().await;
            for cmd_id in session_ids {
                if let Some(session_name) = tmux_session_name_from_command_id(cmd_id) {
                    drop(running);
                    if kill_tmux_session(session_name).await {
                        killed.push(cmd_id.clone());
                    } else {
                        failed.push(cmd_id.clone());
                    }
                    running = self.running_commands.lock().await;
                    continue;
                }
                if let Some(cmd) = running.remove(cmd_id) {
                    if let Some(pid) = cmd.pid {
                        // Check if process still exists
                        match nix::sys::signal::kill(pid, None) {
                            Ok(_) => {
                                // Process still alive, SIGKILL it
                                info!("[{}] Process {} still alive, sending SIGKILL", cmd_id, pid);
                                let _ =
                                    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
                            }
                            Err(nix::errno::Errno::ESRCH) => {
                                // Process already dead, good
                            }
                            Err(_) => {
                                // Other error, process probably dead
                            }
                        }
                    }
                    killed.push(cmd_id.clone());
                } else {
                    // Session not found in our tracking
                    debug!("[{}] Session not found for reconciliation kill", cmd_id);
                    killed.push(cmd_id.clone()); // Treat as killed since it's not running
                }
            }
        }

        (killed, failed)
    }

    /// Transport loss is NOT a reason to kill workloads (#634). Tracked
    /// sessions (and their child processes) are preserved across the
    /// reconnect: the next registration re-reports them in `SessionReport`,
    /// the server imports them via `import_reported_sessions`, and
    /// server-side `reconcile_sessions` decides what, if anything, to kill.
    /// The old behavior — SIGTERM every tracked pid on every stream loss —
    /// permanently destroyed all non-tmux sessions on any network blip.
    async fn log_preserved_sessions(&self) {
        let preserved = self.running_commands.lock().await.len();
        if preserved > 0 {
            info!(
                "Preserving {} running session(s) across reconnect; server reconcile decides their fate",
                preserved
            );
        }
    }

    /// Recover the persistent output receiver from the finished connection's
    /// forwarding task (#634). Must be called after the gRPC client for the
    /// dead connection is dropped — that tears down the transport, which
    /// unblocks the forwarder and makes it hand the receiver back. Falls
    /// back to a fresh channel if the forwarder doesn't return it promptly;
    /// workloads stay alive either way, only their in-flight output is lost.
    async fn recover_output_channel(&mut self) {
        let Some(rx_return) = self.output_rx_return.take() else {
            return;
        };
        // The forwarder hands `rx` back on EVERY select exit path (the
        // `rx_return_tx.send(rx)` after its loop), and the transport is
        // already torn down here (run() dropped the client), so
        // `fwd_tx.closed()` / `fwd_stop` fire and it exits within
        // milliseconds. WAIT for that hand-back instead of swapping the
        // channel on a short timeout: swapping replaces `self.output_tx`,
        // orphaning the sender clones every already-running session captured
        // at spawn — their future output would then vanish silently while
        // they appear healthy and re-adopted (#637). `output_tx` must stay
        // stable for the channel to be "persistent across reconnects" (#634).
        //
        // The 30s timeout is ONLY a deadlock guard against a wedged/panicked
        // forwarder that never returns `rx` (unreachable in normal operation).
        // If it ever fires we have no receiver for the existing `tx` and must
        // recreate the pair as a last resort — which genuinely DEGRADES every
        // pre-existing session (all their future output is dropped), so we say
        // so at ERROR instead of understating it as lost buffered output.
        match tokio::time::timeout(Duration::from_secs(30), rx_return).await {
            Ok(Ok(rx)) => {
                self.output_rx = Some(rx);
            }
            other => {
                let reason = match &other {
                    Err(_) => "did not return the receiver within 30s (forwarder task wedged)",
                    Ok(Err(_)) => {
                        "dropped the return channel without sending (forwarder task panicked)"
                    }
                    Ok(Ok(_)) => unreachable!("Ok(Ok(_)) handled above"),
                };
                error!(
                    "Output forwarder {reason}; recreating the output channel as a last \
                     resort. Every session started before this point holds a clone of the \
                     OLD sender and will silently drop all future output — those sessions \
                     are now DEGRADED even though they remain alive (#637)."
                );
                let (tx, rx) = mpsc::channel(1000);
                self.output_tx = tx;
                self.output_rx = Some(rx);
            }
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

            // Initialize systemd watchdog
            let watchdog = Arc::new(health::SystemdWatchdog::new(self.health.clone()));
            self.watchdog = Some(watchdog.clone());

            // Start watchdog ping loop
            tokio::spawn(async move {
                watchdog.run_ping_loop().await;
            });

            // Notify systemd that we're ready
            if let Some(ref wd) = self.watchdog {
                if let Err(e) = wd.notify_ready() {
                    warn!("Failed to notify systemd READY: {}", e);
                }
            }
        }

        // #625: operator-initiated reconnect. SIGHUP tears down the control
        // stream and forces a fresh registration (re-adopting existing tmux
        // sessions per #613) WITHOUT exiting the process — so a soft-locked
        // container (server unregistered it, but the client + tmux are alive)
        // can rejoin the control plane without losing running work. Delivered
        // via `docker exec <ctr> agent-reconnect`, which sends SIGHUP to pid 1.
        {
            let reconnect_signal = self.reconnect_signal.clone();
            tokio::spawn(async move {
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
                    Ok(mut sighup) => {
                        while sighup.recv().await.is_some() {
                            info!(
                                "SIGHUP received — forcing control-stream reconnect + re-register"
                            );
                            reconnect_signal.notify_one();
                        }
                    }
                    Err(e) => warn!("SIGHUP reconnect handler unavailable: {}", e),
                }
            });
        }

        loop {
            // The output channel is persistent across reconnects (#634):
            // created once in new(), handed back by the forwarding task when
            // a connection dies (recover_output_channel). Session output
            // produced while disconnected buffers in the channel and flushes
            // after reconnect instead of being dropped.
            if let Err(e) = self.refresh_bootstrap_and_config().await {
                error!(
                    "Runtime bootstrap/config refresh failed: {}",
                    format_error_chain(&e)
                );
                self.health.record_failure();
                self.log_preserved_sessions().await;
                info!("Retrying in {:?}...", reconnect_delay);
                sleep(reconnect_delay).await;
                reconnect_delay =
                    std::cmp::min(reconnect_delay * 2, self.config.max_reconnect_delay);
                continue;
            }
            match self.connect().await {
                Ok(mut client) => {
                    reconnect_delay = self.config.reconnect_delay;
                    self.health.record_success();

                    if let Err(e) = self.stream_loop(&mut client).await {
                        error!("Stream error: {}", format_error_chain(&e));
                    }

                    // Tear down the transport so the output forwarder exits
                    // and hands the receiver back, then preserve — never
                    // kill — running sessions across the reconnect (#634).
                    drop(client);
                    self.recover_output_channel().await;
                    self.log_preserved_sessions().await;
                }
                Err(e) => {
                    error!("Connection failed: {}", format_error_chain(&e));
                    self.health.record_failure();

                    // Never connected — output channel untouched; sessions
                    // from a previous connection stay alive (#634).
                    self.log_preserved_sessions().await;
                }
            }

            info!("Retrying in {:?}...", reconnect_delay);
            sleep(reconnect_delay).await;
            reconnect_delay = std::cmp::min(reconnect_delay * 2, self.config.max_reconnect_delay);
        }
    }

    async fn stream_loop(&mut self, client: &mut AgentServiceClient<Channel>) -> Result<()> {
        info!("Starting bidirectional stream...");

        let output_rx = self
            .output_rx
            .take()
            .context("Output receiver already taken")?;
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
            // Reuse System and Disks objects - creating new ones is expensive
            // as it reads all /proc entries including every process
            let mut sys = System::new();
            let mut disks = Disks::new_with_refreshed_list();
            loop {
                interval.tick().await;
                // Only refresh what we need - not all processes
                sys.refresh_cpu_usage();
                sys.refresh_memory();
                disks.refresh();

                let cpu = sys.global_cpu_usage();
                let mem_used = sys.used_memory() as i64;
                let mem_total = sys.total_memory() as i64;
                let disk_used = disks
                    .first()
                    .map(|d| (d.total_space() - d.available_space()) as i64)
                    .unwrap_or(0);
                let disk_total = disks.first().map(|d| d.total_space() as i64).unwrap_or(0);
                let load = System::load_average();
                let uptime = System::uptime() as i64;

                // Read setup progress for heartbeat
                let (setup_status, setup_json, agent_status) = read_setup_progress();

                // Send heartbeat (liveness)
                let hb = Heartbeat {
                    agent_id: agent_id.clone(),
                    timestamp_ms: chrono_timestamp_ms(),
                    status: agent_status as i32,
                    cpu_percent: cpu,
                    memory_used_bytes: mem_used,
                    uptime_seconds: uptime,
                    setup_status,
                    setup_progress_json: setup_json,
                };
                if heartbeat_tx
                    .send(AgentMessage {
                        payload: Some(proto::agent_message::Payload::Heartbeat(hb)),
                    })
                    .await
                    .is_err()
                {
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
                if heartbeat_tx
                    .send(AgentMessage {
                        payload: Some(proto::agent_message::Payload::Metrics(metrics)),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Forward the persistent output queue to this connection's message
        // stream. When the connection dies (or stream_loop returns and drops
        // fwd_stop_tx), the task exits and hands the receiver back through
        // the oneshot so surviving sessions keep streaming into the next
        // connection instead of writing into a dropped channel (#634).
        let fwd_tx = msg_tx.clone();
        let (rx_return_tx, rx_return_rx) = oneshot::channel();
        let (fwd_stop_tx, mut fwd_stop_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let mut rx = output_rx;
            loop {
                tokio::select! {
                    msg = rx.recv() => match msg {
                        Some(msg) => {
                            if fwd_tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    },
                    _ = fwd_tx.closed() => break,
                    _ = &mut fwd_stop_rx => break,
                }
            }
            let _ = rx_return_tx.send(rx);
        });
        self.output_rx_return = Some(rx_return_rx);

        // Send registration first
        msg_tx.send(self.create_registration()).await?;
        info!("Sent registration for {}", config.agent_id);

        // Create request with auth metadata
        let mut request = Request::new(ReceiverStream::new(msg_rx));
        populate_agent_metadata(
            request.metadata_mut(),
            &config,
            self.config.resolved_transport()?,
        )?;

        // Renew the authenticated client leaf on the existing channel. The
        // long-lived Connect stream stays established; replacement material
        // is picked up by the next reconnect.
        let (renewal_stop_tx, mut renewal_stop_rx) = oneshot::channel::<()>();
        let renewal_task = if self.config.resolved_transport()? == ResolvedTransport::Tls {
            let mut renewal_client = client.clone();
            let renewal_config = config.clone();
            Some(tokio::spawn(async move {
                loop {
                    let delay = renewal_config
                        .tls_cert
                        .as_deref()
                        .map(Path::new)
                        .map(certificate_renewal_delay)
                        .transpose()
                        .unwrap_or_else(|error| {
                            warn!("Unable to schedule certificate renewal: {}", error);
                            None
                        })
                        .unwrap_or_else(|| Duration::from_secs(60));
                    tokio::select! {
                        _ = sleep(delay) => {}
                        _ = &mut renewal_stop_rx => break,
                    }

                    match renew_agent_certificate(&mut renewal_client, &renewal_config).await {
                        Ok(()) => info!("Renewed agent mTLS certificate"),
                        Err(error) => {
                            warn!(
                                "Agent mTLS certificate renewal failed: {}",
                                format_error_chain(&error)
                            );
                            tokio::select! {
                                _ = sleep(Duration::from_secs(60)) => {}
                                _ = &mut renewal_stop_rx => break,
                            }
                        }
                    }
                }
            }))
        } else {
            None
        };

        // Open stream
        let mut response = client.connect(request).await?.into_inner();

        // Process inbound messages until the stream ends, errors, or an
        // operator-initiated reconnect (SIGHUP → #625) is requested. Breaking
        // returns from stream_loop, and run()'s loop re-dials + re-registers.
        let reconnect_signal = self.reconnect_signal.clone();
        loop {
            tokio::select! {
                inbound = response.next() => {
                    match inbound {
                        Some(Ok(msg)) => self.handle_inbound(msg, output_tx.clone()).await,
                        Some(Err(e)) => {
                            error!("Receive error: {}", e);
                            break;
                        }
                        None => break,
                    }
                }
                _ = reconnect_signal.notified() => {
                    info!("Reconnect requested — closing control stream to re-register");
                    break;
                }
            }
        }

        // Dropping fwd_stop_tx here (function return) stops the output
        // forwarder; run() recovers the receiver via recover_output_channel
        // after the transport is torn down.
        drop(fwd_stop_tx);
        drop(renewal_stop_tx);
        if let Some(task) = renewal_task {
            let _ = task.await;
        }

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
                info!(
                    "Received command: {} - {} (pty={})",
                    cmd.command_id, cmd.command, cmd.allocate_pty
                );
                let agentshare = self.agentshare.clone();
                let running_commands = self.running_commands.clone();

                // Check for special Claude task command
                if cmd.command == "__claude_task__" {
                    info!("Routing to Claude task executor");
                    tokio::spawn(execute_claude_task(cmd, output_tx, running_commands));
                } else if cmd.allocate_pty {
                    tokio::spawn(execute_command_pty(
                        cmd,
                        output_tx,
                        agentshare,
                        running_commands,
                    ));
                } else {
                    tokio::spawn(execute_command(
                        cmd,
                        output_tx,
                        agentshare,
                        running_commands,
                    ));
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
                debug!(
                    "Received stdin for command {}: {} bytes",
                    command_id,
                    stdin_chunk.data.len()
                );

                // Command dispatch is handled on a spawned task. A stdin frame can
                // arrive just after the command request but before that task has
                // registered its stdin channel, so wait briefly before declaring
                // the command missing.
                let stdin_tx = stdin_sender_for_command(
                    &self.running_commands,
                    &command_id,
                    Duration::from_secs(2),
                )
                .await;
                if let Some(tx) = stdin_tx {
                    let stdin_data = StdinData {
                        data: stdin_chunk.data,
                        eof: stdin_chunk.eof,
                    };
                    if tx.send(stdin_data).await.is_err() {
                        warn!(
                            "Failed to send stdin to command {}: channel closed",
                            command_id
                        );
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
                    running
                        .get(&command_id)
                        .and_then(|rc| rc.pty_control_tx.clone())
                };
                if let Some(tx) = pty_tx {
                    let msg = match ctl.action {
                        Some(proto::pty_control::Action::Resize(r)) => PtyControlMsg::Resize {
                            cols: r.cols as u16,
                            rows: r.rows as u16,
                        },
                        Some(proto::pty_control::Action::Signal(s)) => PtyControlMsg::Signal {
                            signum: s.signal_number,
                        },
                        None => return,
                    };
                    if tx.send(msg).await.is_err() {
                        warn!("PTY control channel closed for {}", command_id);
                    }
                } else {
                    debug!("Command {} not found or not a PTY session", command_id);
                }
            }
            Some(Payload::SessionQuery(query)) => {
                info!("Received session query (report_all={})", query.report_all);

                let report = self.build_session_report().await;
                info!(
                    "Reporting {} active session(s) for reconciliation",
                    report.sessions.len()
                );

                let _ = output_tx
                    .send(AgentMessage {
                        payload: Some(proto::agent_message::Payload::SessionReport(report)),
                    })
                    .await;
            }
            Some(Payload::SessionReconcile(reconcile)) => {
                info!(
                    "Received session reconcile: keep={}, kill={}, kill_unrecognized={}",
                    reconcile.keep_session_ids.len(),
                    reconcile.kill_session_ids.len(),
                    reconcile.kill_unrecognized
                );

                // Determine which sessions to kill
                let to_kill = if reconcile.kill_unrecognized {
                    // Kill everything not in keep list
                    self.build_session_report()
                        .await
                        .sessions
                        .into_iter()
                        .map(|session| session.command_id)
                        .filter(|id| !reconcile.keep_session_ids.contains(id))
                        .collect::<Vec<_>>()
                } else {
                    reconcile.kill_session_ids.clone()
                };

                // Kill the sessions
                let (killed, failed) = self
                    .kill_sessions(&to_kill, reconcile.grace_period_seconds)
                    .await;

                // Build kept list from what remains
                let kept: Vec<String> = self
                    .build_session_report()
                    .await
                    .sessions
                    .into_iter()
                    .map(|session| session.command_id)
                    .collect();

                // Send acknowledgment
                let ack = SessionReconcileAck {
                    agent_id: self.config.agent_id.clone(),
                    killed_session_ids: killed,
                    kept_session_ids: kept,
                    failed_to_kill: failed,
                    timestamp_ms: chrono_timestamp_ms(),
                };

                info!(
                    "Session reconciliation complete: killed={}, kept={}, failed={}",
                    ack.killed_session_ids.len(),
                    ack.kept_session_ids.len(),
                    ack.failed_to_kill.len()
                );

                let _ = output_tx
                    .send(AgentMessage {
                        payload: Some(proto::agent_message::Payload::SessionReconcileAck(ack)),
                    })
                    .await;
            }
            None => {}
        }
    }
}

// =============================================================================
// Telemetry Configuration
// =============================================================================

/// Log format selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFormat {
    Pretty,
    Json,
    Compact,
}

impl std::str::FromStr for LogFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pretty" => Ok(LogFormat::Pretty),
            "json" => Ok(LogFormat::Json),
            "compact" => Ok(LogFormat::Compact),
            _ => Err(format!("unknown log format: {}", s)),
        }
    }
}

/// Initialize logging based on LOG_FORMAT environment variable
fn init_logging() -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, EnvFilter};

    let log_format: LogFormat = env::var("LOG_FORMAT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LogFormat::Pretty);

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info").add_directive("agent_client=info".parse().unwrap())
    });

    match log_format {
        LogFormat::Json => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().json())
                .init();
        }
        LogFormat::Compact => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().compact())
                .init();
        }
        LogFormat::Pretty => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer())
                .init();
        }
    }

    Ok(())
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Record start time for uptime metrics
    metrics::record_start_time();

    // Initialize logging with format support
    init_logging()?;

    let cli = Cli::parse();
    maybe_bootstrap_enroll(&cli).await?;
    let config = AgentConfig::from_cli(&cli)?;

    if config.agent_id.is_empty() {
        anyhow::bail!("AGENT_ID required (use --agent-id or AGENT_ID env var)");
    }
    if config.resolved_transport()? == ResolvedTransport::Tcp {
        warn!(
            "TCP transport has no authentication metadata path; use UDS, vsock, or mTLS transport identity"
        );
    }
    if let Some(credential_contract) = credentials::initialize_from_env()? {
        info!(
            credential_refs = credential_contract.credential_refs().len(),
            policy_path = %credential_contract.policy_path().display(),
            runtime_dir = %credential_contract.runtime_dir().display(),
            "initialized workload credential reference contract"
        );
    }

    // Check if this is a restart (for health tracking)
    let restart_marker = std::path::Path::new("/tmp/agent-client-restart.marker");
    let is_restart = restart_marker.exists();
    if is_restart {
        info!("Detected restart (marker file exists)");
    }
    let _ = std::fs::write(restart_marker, "1"); // Create marker for next restart

    info!("Starting agent: {}", config.agent_id);
    info!("Management server: {}", config.server_address);

    let mut client = AgentClient::new(cli, config);
    if is_restart {
        client.health.record_restart();
    }
    client.run().await
}
