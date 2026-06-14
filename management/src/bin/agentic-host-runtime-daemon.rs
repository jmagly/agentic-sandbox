use std::path::PathBuf;
use std::sync::Arc;

use agentic_management::host_runtime::{
    serve_host_runtime_daemon, HostRuntimeDaemonServerConfig, LocalHostRuntimeSupervisor,
    LocalHostSupervisorConfig,
};
use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "agentic-host-runtime-daemon",
    about = "Run the bare-host runtime supervisor daemon"
)]
struct Args {
    /// Unix socket path served by the daemon.
    #[arg(long, env = "AGENTIC_HOST_RUNTIME_DAEMON_SOCKET")]
    socket: Option<PathBuf>,

    /// Root directory for per-instance host runtime state.
    #[arg(long, env = "AGENTIC_HOST_RUNTIME_ROOT")]
    root_dir: Option<PathBuf>,

    /// agent-client binary used for host-backed instances.
    #[arg(long, env = "AGENTIC_HOST_AGENT_CLIENT")]
    agent_client: Option<PathBuf>,

    /// Management gRPC endpoint passed to host-backed agents.
    #[arg(long, env = "AGENTIC_HOST_GRPC_SERVER")]
    management_server: Option<String>,

    /// Supervisor ID reported in provision and lifecycle responses.
    #[arg(long, env = "AGENTIC_HOST_SUPERVISOR_ID")]
    supervisor_id: Option<String>,

    /// Unix socket permission mode, written in octal such as 660.
    #[arg(long, default_value = "660", value_parser = parse_octal_mode)]
    socket_mode: u32,

    /// Enable debug logging.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    init_logging(args.verbose)?;
    let socket_path = args
        .socket
        .unwrap_or_else(|| PathBuf::from("/run/agentic-sandbox/host-runtime.sock"));
    let root_dir = args
        .root_dir
        .unwrap_or_else(|| PathBuf::from("/var/lib/agentic-sandbox/host-runtime"));
    let agent_binary = args
        .agent_client
        .unwrap_or_else(|| PathBuf::from("agent-client"));
    let management_server = args
        .management_server
        .unwrap_or_else(|| "127.0.0.1:50051".to_string());
    let supervisor_id = args
        .supervisor_id
        .unwrap_or_else(|| "host-supervisor-daemon".to_string());

    let supervisor = Arc::new(LocalHostRuntimeSupervisor::new(LocalHostSupervisorConfig {
        root_dir,
        agent_binary,
        management_server,
        supervisor_id,
    }));
    let config = HostRuntimeDaemonServerConfig {
        socket_path: socket_path.clone(),
        socket_mode: args.socket_mode,
    };

    let runtime = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
    runtime.block_on(async move {
        info!(socket = %socket_path.display(), "starting host runtime daemon");
        serve_host_runtime_daemon(config, supervisor, async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::warn!(%error, "failed to wait for shutdown signal");
            }
        })
        .await
        .map_err(anyhow::Error::from)
    })
}

fn init_logging(verbose: bool) -> Result<()> {
    let default_filter = if verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)),
        )
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize logging: {error}"))
}

fn parse_octal_mode(value: &str) -> std::result::Result<u32, String> {
    u32::from_str_radix(value, 8).map_err(|error| format!("invalid octal mode `{value}`: {error}"))
}
