//! agentic-sandbox CLI - Unified command-line interface

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;

mod commands;
mod config;

pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}

#[derive(Parser)]
#[command(name = "agentic-sandbox")]
#[command(author, version, about = "Agentic Sandbox CLI - VM management for AI agents")]
struct Cli {
    /// Management server address
    #[arg(short, long, env = "AGENTIC_SERVER", default_value = "http://localhost:8120")]
    server: String,

    /// Output format
    #[arg(short, long, default_value = "text")]
    format: OutputFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// VM lifecycle management
    Vm {
        #[command(subcommand)]
        action: VmCommands,
    },
    /// Execute commands on agents
    Exec {
        /// Agent ID to execute on
        agent_id: String,
        /// Command to execute
        command: String,
        /// Command arguments
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        /// Stream output in real-time
        #[arg(short, long)]
        stream: bool,
        /// Timeout in seconds
        #[arg(short, long, default_value = "300")]
        timeout: u32,
    },
    /// Attach to agent output streams
    Attach {
        /// Agent ID to attach to
        agent_id: String,
        /// Show only stdout
        #[arg(long)]
        stdout: bool,
        /// Show only stderr
        #[arg(long)]
        stderr: bool,
    },
    /// View agent logs (from agentshare)
    Logs {
        /// Agent ID
        agent_id: String,
        /// Follow logs in real-time
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show
        #[arg(short, long, default_value = "100")]
        lines: usize,
    },
    /// Management server commands
    Server {
        #[command(subcommand)]
        action: ServerCommands,
    },
    /// List connected agents
    Agents {
        /// Show detailed output
        #[arg(short, long)]
        verbose: bool,
    },
}

#[derive(Subcommand)]
enum VmCommands {
    /// Create a new agent VM
    Create {
        /// VM name
        name: String,
        /// Profile to use
        #[arg(short, long, default_value = "basic")]
        profile: String,
        /// Enable agentshare mount
        #[arg(long)]
        agentshare: bool,
    },
    /// List all VMs
    List,
    /// Show VM status
    Status {
        /// VM name
        name: String,
    },
    /// Start a VM
    Start {
        /// VM name
        name: String,
    },
    /// Stop a VM
    Stop {
        /// VM name
        name: String,
        /// Force stop (don't wait for graceful shutdown)
        #[arg(short, long)]
        force: bool,
    },
    /// Destroy a VM
    Destroy {
        /// VM name
        name: String,
        /// Don't prompt for confirmation
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ServerCommands {
    /// Start management server
    Start {
        /// Run in foreground
        #[arg(short, long)]
        foreground: bool,
    },
    /// Show server status
    Status,
    /// Stop management server
    Stop,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Vm { action } => match action {
            VmCommands::Create { name, profile, agentshare } => {
                commands::vm::create(&name, &profile, agentshare).await
            }
            VmCommands::List => commands::vm::list().await,
            VmCommands::Status { name } => commands::vm::status(&name).await,
            VmCommands::Start { name } => commands::vm::start(&name).await,
            VmCommands::Stop { name, force } => commands::vm::stop(&name, force).await,
            VmCommands::Destroy { name, yes } => commands::vm::destroy(&name, yes).await,
        },
        Commands::Exec {
            agent_id,
            command,
            args,
            stream,
            timeout,
        } => commands::exec::run(&cli.server, &agent_id, &command, args, stream, timeout).await,
        Commands::Attach {
            agent_id,
            stdout,
            stderr,
        } => commands::attach::run(&cli.server, &agent_id, stdout, stderr).await,
        Commands::Logs {
            agent_id,
            follow,
            lines,
        } => commands::logs::show(&agent_id, follow, lines).await,
        Commands::Server { action } => match action {
            ServerCommands::Start { foreground } => commands::server::start(foreground).await,
            ServerCommands::Status => commands::server::status().await,
            ServerCommands::Stop => commands::server::stop().await,
        },
        Commands::Agents { verbose } => commands::agents::list(&cli.server, verbose).await,
    };

    if let Err(e) = result {
        eprintln!("{}: {}", "Error".red().bold(), e);
        std::process::exit(1);
    }

    Ok(())
}
