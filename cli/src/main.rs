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
mod cmd;
mod commands;
mod config;
mod output;

use crate::client::http::{ClientError, HttpClient, EXIT_GENERIC};
use crate::config::{ContextEntry, ContextsFile};

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

    // ── New noun-first taxonomy ─────────────────────────────────────────
    /// Agent inspection (read-only verbs from #154).
    Agent {
        #[command(subcommand)]
        action: AgentCommands,
    },
    /// Live PTY sessions registry.
    Session {
        #[command(subcommand)]
        action: SessionCommands,
    },
    /// Task orchestrator.
    Task {
        #[command(subcommand)]
        action: TaskCommands,
    },
    /// (#155) Human-in-the-loop queue.
    Hitl {
        #[command(subcommand)]
        action: StubAction,
    },
    /// Loadout profiles.
    Loadout {
        #[command(subcommand)]
        action: LoadoutCommands,
    },
    /// (#162) Agentshare REST surface.
    Storage {
        #[command(subcommand)]
        action: StubAction,
    },
    /// Server events buffered snapshot.
    Event {
        #[command(subcommand)]
        action: EventCommands,
    },
    /// Diagnostic surface (healthz/readyz rollup).
    Health {
        #[command(subcommand)]
        action: HealthCommands,
    },
    /// Long-running operations tracker.
    Ops {
        #[command(subcommand)]
        action: OpsCommands,
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
enum AgentCommands {
    /// List agents. Backing route: GET /api/v1/agents.
    List {
        /// Filter by status (ready | busy | stale | all).
        #[arg(long)]
        state: Option<String>,
    },
    /// Inspect a single agent. Backing route: GET /api/v1/agents/{id}.
    Get {
        id: String,
    },
    /// AIWG-proxy manifests on the agent.
    Manifests {
        #[command(subcommand)]
        action: AgentManifestsCommands,
    },
}

#[derive(Subcommand)]
enum AgentManifestsCommands {
    /// GET /api/v1/agents/{id}/manifests/{platform}.
    List { id: String, platform: String },
    /// GET /api/v1/agents/{id}/manifests/{platform}/{name}.
    Get { id: String, platform: String, name: String },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List active sessions. Backing route: GET /api/v1/sessions.
    List {
        /// Filter by owning agent id.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Inspect a session. (Filtered from list; no per-id GET yet.)
    Get { id: String },
}

#[derive(Subcommand)]
enum TaskCommands {
    /// List tasks. Backing route: GET /api/v1/tasks.
    List {
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        offset: Option<usize>,
    },
    /// Inspect a task. Backing route: GET /api/v1/tasks/{id}.
    Get { id: String },
    /// Task artifacts.
    Artifacts {
        #[command(subcommand)]
        action: TaskArtifactsCommands,
    },
}

#[derive(Subcommand)]
enum TaskArtifactsCommands {
    /// GET /api/v1/tasks/{id}/artifacts.
    List { id: String },
}

#[derive(Subcommand)]
enum EventCommands {
    /// Buffered event snapshot. Backing route: GET /api/v1/events.
    List {
        /// Filter by event source (agent_id / vm_name).
        #[arg(long)]
        source: Option<String>,
        /// Only events newer than this (RFC3339 or duration like `1h`).
        #[arg(long)]
        since: Option<String>,
        /// Filter by event type wire name (e.g. `agent.connected`).
        #[arg(long = "event-type")]
        event_type: Option<String>,
    },
}

#[derive(Subcommand)]
enum LoadoutCommands {
    /// GET /api/v1/loadouts.
    List,
    /// GET /api/v1/loadouts/{name}.
    Get { name: String },
    /// GET /api/v1/loadout/registry.
    Registry,
}

#[derive(Subcommand)]
enum HealthCommands {
    /// Roll up /healthz, /healthz/http, /readyz, /healthz/deep.
    /// Non-zero exit when any probe is failing.
    Status,
    /// HTTP-only probe (the watchdog target). Server-side counters
    /// are not yet exposed via REST.
    Watchdog,
}

#[derive(Subcommand)]
enum OpsCommands {
    /// GET /api/v1/operations/{id}.
    Get { id: String },
    /// Poll until terminal or timeout.
    Wait {
        id: String,
        /// Duration: `30s`, `5m`, `2h`, `1d`. Default 5m.
        #[arg(long, default_value = "5m")]
        timeout: String,
    },
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
    // Hoist global flags out of `cli` before destructuring `cli.command`
    // so subsequent arms can borrow these without partial-move errors.
    let json = cli.json;
    let server_override = cli.server.clone();
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
                if json {
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
                if json {
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

        // ── #154: read-only verbs over the existing REST surface ───────
        Commands::Agent { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                AgentCommands::List { state } => cmd::agent::list(&c, state.as_deref(), json).await,
                AgentCommands::Get { id } => cmd::agent::get(&c, &id, json).await,
                AgentCommands::Manifests { action } => match action {
                    AgentManifestsCommands::List { id, platform } => {
                        cmd::agent::manifests_list(&c, &id, &platform, json).await
                    }
                    AgentManifestsCommands::Get { id, platform, name } => {
                        cmd::agent::manifests_get(&c, &id, &platform, &name, json).await
                    }
                },
            }
        }
        Commands::Session { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                SessionCommands::List { agent } => cmd::session::list(&c, agent.as_deref(), json).await,
                SessionCommands::Get { id } => cmd::session::get(&c, &id, json).await,
            }
        }
        Commands::Task { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                TaskCommands::List { state, limit, offset } => {
                    cmd::task::list(&c, state.as_deref(), limit, offset, json).await
                }
                TaskCommands::Get { id } => cmd::task::get(&c, &id, json).await,
                TaskCommands::Artifacts { action } => match action {
                    TaskArtifactsCommands::List { id } => {
                        cmd::task::artifacts_list(&c, &id, json).await
                    }
                },
            }
        }
        Commands::Event { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                EventCommands::List { source, since, event_type } => {
                    cmd::event::list(
                        &c,
                        source.as_deref(),
                        since.as_deref(),
                        event_type.as_deref(),
                        json,
                    )
                    .await
                }
            }
        }
        Commands::Loadout { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                LoadoutCommands::List => cmd::loadout::list(&c, json).await,
                LoadoutCommands::Get { name } => cmd::loadout::get(&c, &name, json).await,
                LoadoutCommands::Registry => cmd::loadout::registry(&c, json).await,
            }
        }
        Commands::Health { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                HealthCommands::Status => cmd::health::status(&c, json).await,
                HealthCommands::Watchdog => cmd::health::watchdog(&c, json).await,
            }
        }
        Commands::Ops { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                OpsCommands::Get { id } => cmd::ops::get(&c, &id, json).await,
                OpsCommands::Wait { id, timeout } => {
                    let d = cmd::parse_duration(&timeout)?;
                    cmd::ops::wait(&c, &id, d, json).await
                }
            }
        }

        // ── Still stubbed (their issues haven't shipped yet) ───────────
        Commands::Hitl { .. } | Commands::Storage { .. } | Commands::AuditLog { .. } => {
            Err(anyhow::anyhow!(
                "this resource group is not yet implemented — see issues #155 (hitl), \
                 #162 (storage), #163 (audit-log); `sandboxctl --help` lists the planned taxonomy"
            ))
        }
    }
}

/// Build the HTTP client from `--server` override plus the active context.
/// `--server` short-circuits the active context's server URL but still
/// uses its token if any. With no override and no active context, falls
/// back to localhost (matches earlier CLI behaviour).
fn build_client(server_override: Option<&str>, contexts: &ContextsFile) -> Result<HttpClient> {
    let active = contexts.active().map(|(_, e)| e.clone());
    let entry = match (server_override, active) {
        (Some(s), Some(mut e)) => {
            e.server = s.into();
            e
        }
        (Some(s), None) => ContextEntry {
            server: s.into(),
            token: String::new(),
            role: "operator".into(),
        },
        (None, Some(e)) => e,
        (None, None) => ContextEntry {
            server: "http://localhost:8122".into(),
            token: String::new(),
            role: "operator".into(),
        },
    };
    Ok(HttpClient::new(&entry)?)
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
        Commands::Agent { action } => match action {
            AgentCommands::List { .. } => "agent list".into(),
            AgentCommands::Get { .. } => "agent get".into(),
            AgentCommands::Manifests { action } => match action {
                AgentManifestsCommands::List { .. } => "agent manifests list".into(),
                AgentManifestsCommands::Get { .. } => "agent manifests get".into(),
            },
        },
        Commands::Session { action } => match action {
            SessionCommands::List { .. } => "session list".into(),
            SessionCommands::Get { .. } => "session get".into(),
        },
        Commands::Task { action } => match action {
            TaskCommands::List { .. } => "task list".into(),
            TaskCommands::Get { .. } => "task get".into(),
            TaskCommands::Artifacts { .. } => "task artifacts list".into(),
        },
        Commands::Hitl { .. } => "hitl <stub>".into(),
        Commands::Loadout { action } => match action {
            LoadoutCommands::List => "loadout list".into(),
            LoadoutCommands::Get { .. } => "loadout get".into(),
            LoadoutCommands::Registry => "loadout registry".into(),
        },
        Commands::Storage { .. } => "storage <stub>".into(),
        Commands::Event { .. } => "event list".into(),
        Commands::Health { action } => match action {
            HealthCommands::Status => "health status".into(),
            HealthCommands::Watchdog => "health watchdog".into(),
        },
        Commands::Ops { action } => match action {
            OpsCommands::Get { .. } => "ops get".into(),
            OpsCommands::Wait { .. } => "ops wait".into(),
        },
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
        Commands::Agent { action } => match action {
            AgentCommands::Get { id } => id.clone(),
            AgentCommands::Manifests { action } => match action {
                AgentManifestsCommands::List { id, platform } => format!("{}/{}", id, platform),
                AgentManifestsCommands::Get { id, platform, name } => {
                    format!("{}/{}/{}", id, platform, name)
                }
            },
            _ => String::new(),
        },
        Commands::Session { action } => match action {
            SessionCommands::Get { id } => id.clone(),
            _ => String::new(),
        },
        Commands::Task { action } => match action {
            TaskCommands::Get { id } => id.clone(),
            TaskCommands::Artifacts { action } => match action {
                TaskArtifactsCommands::List { id } => id.clone(),
            },
            _ => String::new(),
        },
        Commands::Loadout { action } => match action {
            LoadoutCommands::Get { name } => name.clone(),
            _ => String::new(),
        },
        Commands::Ops { action } => match action {
            OpsCommands::Get { id } | OpsCommands::Wait { id, .. } => id.clone(),
        },
        _ => String::new(),
    }
}
