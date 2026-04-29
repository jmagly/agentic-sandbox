//! sandboxctl — operator/admin CLI for agentic-sandbox.
//!
//! See `docs/cli-design.md` for the taxonomy and architecture. The
//! existing `vm`, `exec`, `attach`, `logs`, `server`, `agents`
//! subcommands are kept working as-is for back-compat; new top-level
//! resource groups are stubbed and filled in by issues #154+.

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;

mod audit;
mod client;
mod commands;
mod config;
mod output;

use crate::client::http::{ClientError, HttpClient, EXIT_GENERIC};
use crate::config::ContextsFile;

pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}

#[derive(Parser)]
#[command(name = "sandboxctl")]
#[command(
    author,
    version,
    about = "sandboxctl — operator/admin CLI for agentic-sandbox",
    long_about = "Operator/admin CLI for the agentic-sandbox management server.\n\
                  See `docs/cli-design.md` for the full command taxonomy.\n\n\
                  Exit codes:\n  \
                    0  success\n  \
                    1  generic error\n  \
                    2  not found (404)\n  \
                    3  conflict (409)\n  \
                    4  auth required or denied (401/403)\n  \
                    5  timeout"
)]
struct Cli {
    /// Override management server URL (otherwise from active context or env).
    #[arg(long, env = "AGENTIC_SERVER", global = true)]
    server: Option<String>,

    /// Output as JSON instead of the verb's default human renderer.
    #[arg(long, global = true)]
    json: bool,

    /// Override active context (otherwise from contexts.toml).
    #[arg(long, env = "SANDBOXCTL_CONTEXT", global = true)]
    context: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    // ── Implemented in #153 ─────────────────────────────────────────────
    /// Manage client contexts (kubeconfig-style).
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },

    // ── Implemented in earlier work, kept verbatim ──────────────────────
    /// VM lifecycle management (legacy; see also `agent`).
    Vm {
        #[command(subcommand)]
        action: VmCommands,
    },
    /// Execute a one-shot command on an agent (legacy; see also `agent exec`).
    Exec {
        agent_id: String,
        command: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        #[arg(short, long)]
        stream: bool,
        #[arg(short, long, default_value = "300")]
        timeout: u32,
    },
    /// Attach to agent output stream (legacy; see also `session attach`).
    Attach {
        agent_id: String,
        #[arg(long)]
        stdout: bool,
        #[arg(long)]
        stderr: bool,
    },
    /// Tail agent logs from agentshare.
    Logs {
        agent_id: String,
        #[arg(short, long)]
        follow: bool,
        #[arg(short, long, default_value = "100")]
        lines: usize,
    },
    /// Manage the management server daemon.
    Server {
        #[command(subcommand)]
        action: ServerCommands,
    },
    /// List connected agents (legacy; see also `agent list`).
    Agents {
        #[arg(short, long)]
        verbose: bool,
    },

    // ── Stubs for the taxonomy in docs/cli-design.md ────────────────────
    // Each group will be implemented by its own issue (#154–#163).
    /// (#154) Agent inspection and admin verbs.
    Agent {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#154 / #156) Live PTY sessions registry.
    Session {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#155) Task orchestrator.
    Task {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#155) Human-in-the-loop queue.
    Hitl {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#154) Loadout profiles.
    Loadout {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#162) Agentshare REST surface.
    Storage {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#162) Server events stream.
    Event {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#154) Diagnostic surface (healthz/readyz rollup).
    Health {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#154) Long-running operations tracker.
    Ops {
        #[command(subcommand)]
        action: StubAction,
    },
    /// (#163) Local CLI audit log viewer.
    AuditLog {
        #[command(subcommand)]
        action: StubAction,
    },
}

#[derive(Subcommand, Clone)]
enum StubAction {
    /// Placeholder. Resource group not yet implemented.
    #[command(external_subcommand)]
    Any(Vec<String>),
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Create or update a context.
    SetContext {
        /// Context name (e.g. "lab", "prod").
        name: String,
        /// Management server URL (e.g. http://localhost:8122).
        #[arg(long)]
        server: String,
        /// Bearer token; empty for unauth or Unix-socket use.
        #[arg(long, default_value = "")]
        token: String,
        /// Operator-declared role: admin | operator. Server enforces actual role.
        #[arg(long, default_value = "operator")]
        role: String,
    },
    /// Switch the active context.
    UseContext {
        /// Context name.
        name: String,
    },
    /// Show the active context, server URL, and resolved role.
    Whoami,
    /// List all configured contexts.
    Contexts,
}

#[derive(Subcommand)]
enum VmCommands {
    Create {
        name: String,
        #[arg(short, long, default_value = "basic")]
        profile: String,
        #[arg(long)]
        agentshare: bool,
    },
    List,
    Status { name: String },
    Start { name: String },
    Stop {
        name: String,
        #[arg(short, long)]
        force: bool,
    },
    Destroy {
        name: String,
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ServerCommands {
    Start {
        #[arg(short, long)]
        foreground: bool,
    },
    Status,
    Stop,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Resolve active context once. CLI flags > env > contexts file.
    let contexts = ContextsFile::load().unwrap_or_default();
    let context_name = cli
        .context
        .clone()
        .or_else(|| contexts.current_context.clone())
        .unwrap_or_else(|| "<none>".to_string());

    let verb = describe_verb(&cli.command);
    let target = describe_target(&cli.command);
    let span = audit::Span::new(&verb, &target, &context_name);

    let result = dispatch(cli, &contexts).await;
    span.finish(&result);

    if let Err(e) = result {
        eprintln!("{}: {:#}", "Error".red().bold(), e);
        // If the underlying error chain has a ClientError, exit with its code.
        let code = e
            .chain()
            .find_map(|c| c.downcast_ref::<ClientError>())
            .map(|c| c.exit_code())
            .unwrap_or(EXIT_GENERIC);
        std::process::exit(code);
    }
}

async fn dispatch(cli: Cli, contexts: &ContextsFile) -> Result<()> {
    match cli.command {
        // ── #153 ────────────────────────────────────────────────────────
        Commands::Config { action } => match action {
            ConfigCommands::SetContext { name, server, token, role } => {
                let mut cf = ContextsFile::load().unwrap_or_default();
                cf.set_context(&name, server, token, role);
                if cf.current_context.is_none() {
                    // First context becomes active by default.
                    cf.use_context(&name)?;
                }
                let path = cf.save()?;
                println!("Wrote {}", path.display());
                Ok(())
            }
            ConfigCommands::UseContext { name } => {
                let mut cf = ContextsFile::load().unwrap_or_default();
                cf.use_context(&name)?;
                let path = cf.save()?;
                println!("Active context: {} ({})", name, path.display());
                Ok(())
            }
            ConfigCommands::Whoami => {
                let cf = ContextsFile::load().unwrap_or_default();
                if cli.json {
                    let payload = serde_json::json!({
                        "context": cf.current_context,
                        "server": cf.active().map(|(_, e)| e.server.clone()),
                        "role":   cf.active().map(|(_, e)| e.role.clone()),
                        "token_present": cf.active().map(|(_, e)| !e.token.is_empty()).unwrap_or(false),
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    match cf.active() {
                        Some((name, e)) => {
                            println!("context: {}", name);
                            println!("server:  {}", e.server);
                            println!("role:    {}", e.role);
                            println!(
                                "token:   {}",
                                if e.token.is_empty() { "<none>" } else { "<set>" }
                            );
                        }
                        None => {
                            println!(
                                "no active context — run `sandboxctl config set-context <name> --server <url>`"
                            );
                        }
                    }
                }
                Ok(())
            }
            ConfigCommands::Contexts => {
                let cf = ContextsFile::load().unwrap_or_default();
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&cf)?);
                } else {
                    let rows: Vec<Vec<String>> = cf
                        .contexts
                        .iter()
                        .map(|(n, e)| {
                            let active =
                                if cf.current_context.as_deref() == Some(n.as_str()) { "*" } else { "" };
                            vec![
                                active.to_string(),
                                n.clone(),
                                e.server.clone(),
                                e.role.clone(),
                                if e.token.is_empty() { "no".into() } else { "yes".into() },
                            ]
                        })
                        .collect();
                    print!(
                        "{}",
                        output::table::render(
                            &["", "NAME", "SERVER", "ROLE", "TOKEN"],
                            &rows
                        )
                    );
                }
                Ok(())
            }
        },

        // ── Existing commands (kept verbatim) ───────────────────────────
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
        Commands::Exec { agent_id, command, args, stream, timeout } => {
            let server = resolve_server(&cli.server, contexts);
            commands::exec::run(&server, &agent_id, &command, args, stream, timeout).await
        }
        Commands::Attach { agent_id, stdout, stderr } => {
            let server = resolve_server(&cli.server, contexts);
            commands::attach::run(&server, &agent_id, stdout, stderr).await
        }
        Commands::Logs { agent_id, follow, lines } => {
            commands::logs::show(&agent_id, follow, lines).await
        }
        Commands::Server { action } => match action {
            ServerCommands::Start { foreground } => commands::server::start(foreground).await,
            ServerCommands::Status => commands::server::status().await,
            ServerCommands::Stop => commands::server::stop().await,
        },
        Commands::Agents { verbose } => {
            let server = resolve_server(&cli.server, contexts);
            commands::agents::list(&server, verbose).await
        }

        // ── Stubs for the new taxonomy ──────────────────────────────────
        Commands::Agent { .. }
        | Commands::Session { .. }
        | Commands::Task { .. }
        | Commands::Hitl { .. }
        | Commands::Loadout { .. }
        | Commands::Storage { .. }
        | Commands::Event { .. }
        | Commands::Health { .. }
        | Commands::Ops { .. }
        | Commands::AuditLog { .. } => {
            // Validate the active context exists so operators discover
            // missing config now rather than later. Then bail with a
            // pointer to the issue that implements this group.
            if HttpClient::new(
                contexts
                    .active()
                    .map(|(_, e)| e)
                    .ok_or_else(|| anyhow::anyhow!(
                        "no active context — run `sandboxctl config set-context <name> --server <url>`"
                    ))?,
            )
            .is_err()
            {
                // fall through; this is informational
            }
            Err(anyhow::anyhow!(
                "this resource group is not yet implemented — see issues #154–#163; \
                 `sandboxctl --help` lists the planned taxonomy"
            ))
        }
    }
}

fn resolve_server(flag: &Option<String>, contexts: &ContextsFile) -> String {
    if let Some(s) = flag {
        return s.clone();
    }
    if let Some((_, e)) = contexts.active() {
        return e.server.clone();
    }
    "http://localhost:8120".to_string()
}

fn describe_verb(c: &Commands) -> String {
    match c {
        Commands::Config { action } => match action {
            ConfigCommands::SetContext { .. } => "config set-context".into(),
            ConfigCommands::UseContext { .. } => "config use-context".into(),
            ConfigCommands::Whoami => "config whoami".into(),
            ConfigCommands::Contexts => "config contexts".into(),
        },
        Commands::Vm { action } => match action {
            VmCommands::Create { .. } => "vm create".into(),
            VmCommands::List => "vm list".into(),
            VmCommands::Status { .. } => "vm status".into(),
            VmCommands::Start { .. } => "vm start".into(),
            VmCommands::Stop { .. } => "vm stop".into(),
            VmCommands::Destroy { .. } => "vm destroy".into(),
        },
        Commands::Exec { .. } => "exec".into(),
        Commands::Attach { .. } => "attach".into(),
        Commands::Logs { .. } => "logs".into(),
        Commands::Server { action } => match action {
            ServerCommands::Start { .. } => "server start".into(),
            ServerCommands::Status => "server status".into(),
            ServerCommands::Stop => "server stop".into(),
        },
        Commands::Agents { .. } => "agents".into(),
        Commands::Agent { .. } => "agent <stub>".into(),
        Commands::Session { .. } => "session <stub>".into(),
        Commands::Task { .. } => "task <stub>".into(),
        Commands::Hitl { .. } => "hitl <stub>".into(),
        Commands::Loadout { .. } => "loadout <stub>".into(),
        Commands::Storage { .. } => "storage <stub>".into(),
        Commands::Event { .. } => "event <stub>".into(),
        Commands::Health { .. } => "health <stub>".into(),
        Commands::Ops { .. } => "ops <stub>".into(),
        Commands::AuditLog { .. } => "audit-log <stub>".into(),
    }
}

fn describe_target(c: &Commands) -> String {
    match c {
        Commands::Vm { action } => match action {
            VmCommands::Create { name, .. }
            | VmCommands::Status { name }
            | VmCommands::Start { name }
            | VmCommands::Stop { name, .. }
            | VmCommands::Destroy { name, .. } => name.clone(),
            VmCommands::List => String::new(),
        },
        Commands::Exec { agent_id, .. }
        | Commands::Attach { agent_id, .. }
        | Commands::Logs { agent_id, .. } => agent_id.clone(),
        Commands::Config { action } => match action {
            ConfigCommands::SetContext { name, .. } | ConfigCommands::UseContext { name } => {
                name.clone()
            }
            ConfigCommands::Whoami | ConfigCommands::Contexts => String::new(),
        },
        _ => String::new(),
    }
}
