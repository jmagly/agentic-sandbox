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
mod pty;

use crate::client::http::{ClientError, HttpClient, EXIT_GENERIC};
use crate::config::{ContextEntry, ContextsFile};

pub mod proto {
    tonic::include_proto!("agentic.sandbox.v1");
}

/// Combined version string surfaced via `--version`. Includes both
/// the crate version and the build SHA captured by `build.rs`.
const FULL_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("SANDBOXCTL_BUILD_SHA"),
    ")"
);

#[derive(Parser)]
#[command(name = "sandboxctl")]
#[command(
    author,
    version = FULL_VERSION,
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

    /// Re-render `list` verbs every <DURATION> until interrupted.
    /// Format: `2s`, `500ms`, `1m`, etc. Ignored by non-list verbs.
    #[arg(long, global = true, value_name = "INTERVAL")]
    watch: Option<String>,

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
    /// Attach to an agent output stream, or to an executor PTY with two args.
    Attach {
        agent_or_instance_id: String,
        /// PTY session id. When present, attach uses `pty-ws.v1` by default.
        session_id: Option<String>,
        /// With two args, force the legacy formal-session protocol.
        #[arg(long = "legacy-pty")]
        legacy_pty: bool,
        /// With two args, request controller/write role.
        #[arg(long)]
        write: bool,
        /// With two args, replay from a specific pty-ws/v1 seq.
        #[arg(long = "replay-from", value_name = "SEQ")]
        replay_from: Option<u64>,
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
    /// Orchestrator-oriented TUI driver commands.
    Tui {
        #[command(subcommand)]
        action: TuiCommands,
    },
    /// Container lifecycle (Docker runtime — #173).
    Container {
        #[command(subcommand)]
        action: ContainerCommands,
    },
    /// Task orchestrator.
    Task {
        #[command(subcommand)]
        action: TaskCommands,
    },
    /// Human-in-the-loop queue.
    Hitl {
        #[command(subcommand)]
        action: HitlCommands,
    },
    /// Loadout profiles.
    Loadout {
        #[command(subcommand)]
        action: LoadoutCommands,
    },
    /// Agentshare REST surface.
    Storage {
        #[command(subcommand)]
        action: StorageCommands,
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
    /// Local CLI audit log viewer.
    /// Reads `$XDG_STATE_HOME/sandboxctl/audit.log` (default
    /// `~/.local/state/sandboxctl/audit.log`).
    AuditLog {
        #[command(subcommand)]
        action: AuditLogCommands,
    },

    /// A2A core operations against a specific executor instance.
    /// Backing routes: `/agents/{instance_id}/v1/{messages:send,tasks,...}`.
    Tasks {
        #[command(subcommand)]
        action: TasksCommands,
    },

    /// Fetch and verify a signed AgentCard from an executor instance.
    /// Backing route: `/agents/{instance_id}/.well-known/agent-card.json`.
    Agentcard {
        #[command(subcommand)]
        action: AgentcardCommands,
    },

    /// Print shell completion script. Pipe to your shell's
    /// completion directory:
    ///   `sandboxctl completions bash > ~/.local/share/bash-completion/completions/sandboxctl`
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
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
    Get { id: String },
    /// Graceful agent stop. Backing route: POST /api/v1/agents/{id}/stop.
    Stop { id: String },
    /// Rotate the per-agent shared secret. Backing route:
    /// POST /api/v1/agents/{id}/rotate-secret. Returns operation_id.
    RotateSecret {
        id: String,
        #[arg(long)]
        wait: bool,
    },
    /// Open an interactive shell on the agent (creates a session, then
    /// attaches as controller). Convenience for `session attach`.
    Shell {
        id: String,
        /// Override the shell command (default `/bin/bash`).
        #[arg(long)]
        cmd: Option<String>,
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
    Get {
        id: String,
        platform: String,
        name: String,
    },
    /// POST /api/v1/agents/{id}/manifests/{platform}/{name}.
    Push {
        id: String,
        platform: String,
        name: String,
        /// Path to the manifest file to push.
        #[arg(short, long)]
        file: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum ContainerCommands {
    /// List managed containers (label `agentic-sandbox=true`).
    /// Backing route: GET /api/v1/containers.
    List {
        /// Filter by status: `running` | `stopped` | `all` (default).
        #[arg(long)]
        state: Option<String>,
    },
    /// Inspect a single container.
    Get {
        name: String,
    },
    /// Spawn a new container. Backing route: POST /api/v1/containers.
    /// PTY exec inside the container is tracked separately (#174).
    Create {
        name: String,
        #[arg(long)]
        image: String,
        /// Env var as KEY=VALUE. Repeatable.
        #[arg(short = 'e', long = "env")]
        env: Vec<String>,
        /// Bind mount as host:container. Repeatable.
        #[arg(short = 'v', long = "mount")]
        mounts: Vec<String>,
        /// Network mode (bridge, host, custom).
        #[arg(long)]
        network: Option<String>,
        /// Override the image's default command. Pass after --.
        #[arg(trailing_var_arg = true)]
        cmd: Vec<String>,
    },
    Start {
        name: String,
    },
    Stop {
        name: String,
        /// Graceful-stop timeout before SIGKILL (default 10s).
        #[arg(long, default_value = "10")]
        timeout: u64,
    },
    /// Force-remove a container. Requires --yes or interactive confirm.
    Delete {
        name: String,
        /// Skip the destructive-verb confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
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
    /// Kill a session. Backing route: DELETE /api/v1/sessions/{id}?signal=...
    Kill {
        id: String,
        /// Signal to send: TERM (default), KILL, INT, HUP.
        #[arg(long, default_value = "TERM")]
        signal: String,
        /// Skip the destructive-verb confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Observer attach, line-buffered stdout (scriptable). WS protocol.
    Tail {
        id: String,
        /// Replay from a specific seq, or 0 for the full buffered window.
        #[arg(long = "replay-from", value_name = "SEQ")]
        replay_from: Option<u64>,
    },
    /// Record raw SessionFrame JSON Lines to a file (or `-` for stdout).
    Record {
        id: String,
        #[arg(short, long, value_name = "FILE")]
        output: std::path::PathBuf,
        #[arg(long = "replay-from", value_name = "SEQ")]
        replay_from: Option<u64>,
    },
    /// One-shot stdin push to a session (controller required).
    Input {
        id: String,
        /// Path to read input from; use `-` for stdin.
        #[arg(short, long, value_name = "FILE")]
        file: std::path::PathBuf,
    },
    /// One-shot PTY resize.
    Resize {
        id: String,
        #[arg(long)]
        cols: u16,
        #[arg(long)]
        rows: u16,
    },
    /// Full interactive PTY join. Default role observer; --write or
    /// --role controller takes write access. Detach with Ctrl-A d.
    Attach {
        id: String,
        /// Attach as a controller (write-capable). Multi-writer is
        /// allowed; existing controllers are listed via MembershipChanged.
        #[arg(long)]
        write: bool,
        #[arg(long = "replay-from", value_name = "SEQ")]
        replay_from: Option<u64>,
    },
}

#[derive(Subcommand)]
enum TuiCommands {
    /// Read the current structured screen snapshot.
    Snapshot { id: String },
    /// Attach to the structured orchestrator stream as observer.
    Observe {
        id: String,
        /// Number of orchestrator frames to print before exiting.
        #[arg(long, default_value_t = 1)]
        frames: usize,
        /// Overall wait timeout, e.g. 10s, 1m.
        #[arg(long, default_value = "10s")]
        timeout: String,
        /// Exit successfully on timeout after at least one frame, useful for idle attach probes.
        #[arg(long)]
        idle_ok: bool,
    },
    /// Send one controller write frame. Requires --yes-controller.
    Send {
        id: String,
        #[arg(long)]
        text: String,
        /// Append a newline after --text.
        #[arg(long)]
        enter: bool,
        /// Explicit policy opt-in for write-capable controller access.
        #[arg(long)]
        yes_controller: bool,
    },
    /// Search the durable transcript archive beyond the live screen window.
    Search {
        id: String,
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
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
    /// Submit a task manifest. Backing route: POST /api/v1/tasks.
    /// `.yaml`/`.yml` files are sent as `manifest_yaml`; `.json` parsed
    /// and sent as `manifest`.
    Submit {
        /// Path to the task manifest (YAML or JSON).
        #[arg(short, long)]
        file: std::path::PathBuf,
        /// Block until the task reaches a terminal state.
        #[arg(long)]
        wait: bool,
    },
    /// Cancel a task. Backing route: DELETE /api/v1/tasks/{id}.
    Cancel {
        id: String,
        /// Optional reason; surfaced in the task's metadata.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Tail task logs. Backing route: GET /api/v1/tasks/{id}/logs (SSE).
    Logs {
        id: String,
        /// Stream new entries as they arrive (otherwise prints the
        /// buffered snapshot and exits).
        #[arg(short, long)]
        follow: bool,
    },
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
    /// Tail events as they happen. Backing route:
    /// GET /api/v1/events?follow=true (SSE).
    Tail {
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long = "event-type")]
        event_type: Option<String>,
        /// Client-side regex applied to each event's JSON wire form.
        #[arg(long)]
        filter: Option<String>,
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
enum StorageCommands {
    /// Operations on the global RO share.
    Global {
        #[command(subcommand)]
        action: StorageGlobalCommands,
    },
    /// Operations on a per-agent inbox.
    Inbox {
        #[command(subcommand)]
        action: StorageInboxCommands,
    },
    /// Operations on a per-task outbox.
    Outbox {
        #[command(subcommand)]
        action: StorageOutboxCommands,
    },
}

#[derive(Subcommand)]
enum StorageGlobalCommands {
    /// GET /api/v1/storage/global?path=<p>.
    Ls {
        #[arg(long)]
        path: Option<String>,
    },
    /// POST /api/v1/storage/global?path=<p> (raw bytes).
    Push {
        #[arg(long)]
        path: String,
        #[arg(short, long, value_name = "FILE")]
        file: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum StorageInboxCommands {
    Ls {
        agent: String,
        #[arg(long)]
        path: Option<String>,
    },
    Push {
        agent: String,
        #[arg(long)]
        path: String,
        #[arg(short, long, value_name = "FILE")]
        file: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum StorageOutboxCommands {
    Ls {
        task: String,
        #[arg(long)]
        path: Option<String>,
    },
}

#[derive(Subcommand)]
enum TasksCommands {
    /// POST /agents/{instance_id}/v1/messages:send.
    /// Reads a Message JSON envelope from <message-file> (or `-` for stdin).
    /// Sets the required `A2A-Extensions: runtime/v1, idempotency/v1` header.
    Send {
        /// Target executor instance id (from `agent get <id>`).
        instance_id: String,
        /// Path to the message JSON file, or `-` to read from stdin.
        message_file: String,
    },
    /// GET /agents/{instance_id}/v1/tasks.
    List {
        instance_id: String,
        /// Filter by task state (e.g. `working`, `completed`).
        #[arg(long)]
        state: Option<String>,
        /// Continuation cursor from a previous page.
        #[arg(long)]
        cursor: Option<String>,
        /// Page size limit.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// GET /agents/{instance_id}/v1/tasks/{tid}.
    Get {
        instance_id: String,
        task_id: String,
    },
    /// GET /agents/{instance_id}/v1/tasks/{tid}/subscribe (SSE stream).
    /// Exits when the connection closes or a terminal state is observed.
    Subscribe {
        instance_id: String,
        task_id: String,
    },
    /// POST /agents/{instance_id}/v1/tasks/{tid}/cancel.
    Cancel {
        instance_id: String,
        task_id: String,
    },
}

#[derive(Subcommand)]
enum AgentcardCommands {
    /// Fetch and pretty-print the signed AgentCard.
    /// Backing route: `/agents/{instance_id}/.well-known/agent-card.json`.
    Get { instance_id: String },
    /// Fetch the AgentCard and verify its JWS Compact signature against
    /// the supplied JWKS (file path or http(s) URL).
    /// Algorithm: EdDSA (Ed25519) only. JCS canonicalization per RFC 8785.
    Verify {
        instance_id: String,
        /// Path to a local JWKS file or an http(s) URL.
        #[arg(long)]
        jwks: String,
    },
}

#[derive(Subcommand)]
enum AuditLogCommands {
    /// Show the most recent N records (default 50).
    Tail {
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
        /// Follow new records as they're written (like `tail -F`).
        #[arg(short, long)]
        follow: bool,
    },
    /// Filter records by regex against the raw JSON line.
    Grep { pattern: String },
    /// Print the full path to the audit log file.
    Path,
}

#[derive(Subcommand)]
enum HitlCommands {
    /// Reply to a pending HITL prompt. Backing route:
    /// POST /api/v1/hitl/{id}/respond.
    Respond {
        id: String,
        #[arg(long)]
        text: String,
    },
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
    /// Create a VM. Backing route: POST /api/v1/vms.
    /// Returns an operation; pass --wait to block until terminal.
    Create {
        name: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        loadout: Option<String>,
        #[arg(long)]
        vcpus: Option<u32>,
        #[arg(long = "memory", value_name = "MB")]
        memory_mb: Option<u32>,
        #[arg(long = "disk", value_name = "GB")]
        disk_gb: Option<u32>,
        #[arg(long)]
        agentshare: bool,
        #[arg(long, default_value_t = true)]
        start: bool,
        #[arg(long)]
        wait: bool,
    },
    /// List VMs. Backing route: GET /api/v1/vms.
    List {
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Inspect a VM. Backing route: GET /api/v1/vms/{name}.
    Get { name: String },
    /// Start a VM. Backing route: POST /api/v1/vms/{name}/start.
    Start { name: String },
    /// Stop a VM gracefully. Backing route: POST /api/v1/vms/{name}/stop.
    /// (Server uses a 15s default; --force/--timeout are accepted for
    /// CLI compatibility but not yet honored on this route — see followup.)
    Stop {
        name: String,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "15")]
        timeout: u64,
    },
    /// Restart a VM. Backing route: POST /api/v1/vms/{name}/restart.
    Restart {
        name: String,
        /// Hard restart (skip graceful shutdown).
        #[arg(long)]
        hard: bool,
        #[arg(long, default_value = "15")]
        timeout: u64,
        #[arg(long)]
        wait: bool,
    },
    /// Destroy a VM. Backing route: DELETE /api/v1/vms/{name}.
    Destroy {
        name: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        delete_disk: bool,
        /// Skip the destructive-verb confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Reprovision a VM in place. Backing route:
    /// POST /api/v1/agents/{id}/reprovision. Returns an operation_id.
    Reprovision {
        name: String,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        wait: bool,
    },
    /// Re-run agent deploy on a running VM. Backing route:
    /// POST /api/v1/vms/{name}/deploy-agent.
    DeployAgent {
        name: String,
        #[arg(long)]
        wait: bool,
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

    // `--watch INTERVAL`: re-render eligible list verbs every interval
    // until interrupted. Eligibility is checked here so non-list verbs
    // ignore the flag instead of looping a one-shot mutation forever.
    if let Some(interval) = cli.watch.as_deref() {
        if is_watchable(&cli.command) {
            let dur = match cmd::parse_duration(interval) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("{}: invalid --watch interval: {}", "Error".red().bold(), e);
                    std::process::exit(1);
                }
            };
            // Loop: clear screen, re-parse args (so the same flags apply),
            // dispatch, sleep. Audit each tick separately so the audit log
            // shows watch cadence.
            loop {
                eprint!("\x1b[2J\x1b[H");
                let cli_tick = Cli::parse();
                let verb = describe_verb(&cli_tick.command);
                let target = describe_target(&cli_tick.command);
                let span = audit::Span::new(&verb, &target, &context_name);
                let res = dispatch(cli_tick, &contexts).await;
                span.finish(&res);
                if let Err(e) = res {
                    eprintln!("{}: {:#}", "Error".red().bold(), e);
                }
                tokio::time::sleep(dur).await;
            }
        }
        // Non-list verb with --watch: fall through to the one-shot path.
    }

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
            ConfigCommands::SetContext {
                name,
                server,
                token,
                role,
            } => {
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
                                if e.token.is_empty() {
                                    "<none>"
                                } else {
                                    "<set>"
                                }
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
                            let active = if cf.current_context.as_deref() == Some(n.as_str()) {
                                "*"
                            } else {
                                ""
                            };
                            vec![
                                active.to_string(),
                                n.clone(),
                                e.server.clone(),
                                e.role.clone(),
                                if e.token.is_empty() {
                                    "no".into()
                                } else {
                                    "yes".into()
                                },
                            ]
                        })
                        .collect();
                    print!(
                        "{}",
                        output::table::render(&["", "NAME", "SERVER", "ROLE", "TOKEN"], &rows)
                    );
                }
                Ok(())
            }
        },

        // ── Existing commands (kept verbatim) ───────────────────────────
        Commands::Vm { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                VmCommands::Create {
                    name,
                    profile,
                    loadout,
                    vcpus,
                    memory_mb,
                    disk_gb,
                    agentshare,
                    start,
                    wait,
                } => {
                    cmd::vm::create(
                        &c,
                        &name,
                        profile.as_deref(),
                        loadout.as_deref(),
                        vcpus,
                        memory_mb,
                        disk_gb,
                        agentshare,
                        start,
                        wait,
                        json,
                    )
                    .await
                }
                VmCommands::List { state, prefix } => {
                    cmd::vm::list(&c, state.as_deref(), prefix.as_deref(), json).await
                }
                VmCommands::Get { name } => cmd::vm::get(&c, &name, json).await,
                VmCommands::Start { name } => cmd::vm::start(&c, &name, json).await,
                VmCommands::Stop {
                    name,
                    force: _,
                    timeout: _,
                } => cmd::vm::stop(&c, &name, json).await,
                VmCommands::Restart {
                    name,
                    hard,
                    timeout,
                    wait,
                } => cmd::vm::restart(&c, &name, hard, timeout, wait, json).await,
                VmCommands::Destroy {
                    name,
                    force,
                    delete_disk,
                    yes,
                } => {
                    cmd::confirm_destructive("destroy", &name, yes)?;
                    cmd::vm::destroy(&c, &name, force, delete_disk, json).await
                }
                VmCommands::Reprovision { name, yes, wait } => {
                    cmd::confirm_destructive("reprovision", &name, yes)?;
                    cmd::vm::reprovision(&c, &name, wait, json).await
                }
                VmCommands::DeployAgent { name, wait } => {
                    cmd::vm::deploy_agent(&c, &name, wait, json).await
                }
            }
        }
        Commands::Exec {
            agent_id,
            command,
            args,
            stream,
            timeout,
        } => {
            let server = resolve_server(&cli.server, contexts);
            commands::exec::run(&server, &agent_id, &command, args, stream, timeout).await
        }
        Commands::Attach {
            agent_or_instance_id,
            session_id,
            legacy_pty,
            write,
            replay_from,
            stdout,
            stderr,
        } => {
            if let Some(session_id) = session_id {
                let c = build_client(server_override.as_deref(), contexts)?;
                if legacy_pty {
                    cmd::session::attach(&c, &session_id, write, replay_from).await
                } else {
                    cmd::session::attach_pty_ws_v1(
                        &c,
                        &agent_or_instance_id,
                        &session_id,
                        write,
                        replay_from,
                    )
                    .await
                }
            } else {
                let server = resolve_server(&cli.server, contexts);
                commands::attach::run(&server, &agent_or_instance_id, stdout, stderr).await
            }
        }
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
                AgentCommands::Stop { id } => cmd::agent::stop(&c, &id, json).await,
                AgentCommands::RotateSecret { id, wait } => {
                    cmd::agent::rotate_secret(&c, &id, wait, json).await
                }
                AgentCommands::Shell { id, cmd: shell_cmd } => {
                    cmd::agent::shell(&c, &id, shell_cmd.as_deref()).await
                }
                AgentCommands::Manifests { action } => match action {
                    AgentManifestsCommands::List { id, platform } => {
                        cmd::agent::manifests_list(&c, &id, &platform, json).await
                    }
                    AgentManifestsCommands::Get { id, platform, name } => {
                        cmd::agent::manifests_get(&c, &id, &platform, &name, json).await
                    }
                    AgentManifestsCommands::Push {
                        id,
                        platform,
                        name,
                        file,
                    } => {
                        let content = std::fs::read_to_string(&file)
                            .map_err(|e| anyhow::anyhow!("reading {}: {}", file.display(), e))?;
                        cmd::agent::manifests_push(&c, &id, &platform, &name, &content, json).await
                    }
                },
            }
        }
        Commands::Container { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                ContainerCommands::List { state } => {
                    cmd::container::list(&c, state.as_deref(), json).await
                }
                ContainerCommands::Get { name } => cmd::container::get(&c, &name, json).await,
                ContainerCommands::Create {
                    name,
                    image,
                    env,
                    mounts,
                    network,
                    cmd: container_cmd,
                } => {
                    cmd::container::create(
                        &c,
                        &name,
                        &image,
                        &env,
                        &mounts,
                        network.as_deref(),
                        &container_cmd,
                        json,
                    )
                    .await
                }
                ContainerCommands::Start { name } => cmd::container::start(&c, &name, json).await,
                ContainerCommands::Stop { name, timeout } => {
                    cmd::container::stop(&c, &name, timeout, json).await
                }
                ContainerCommands::Delete { name, yes } => {
                    cmd::confirm_destructive("delete container", &name, yes)?;
                    cmd::container::delete(&c, &name, json).await
                }
            }
        }
        Commands::Session { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                SessionCommands::List { agent } => {
                    cmd::session::list(&c, agent.as_deref(), json).await
                }
                SessionCommands::Get { id } => cmd::session::get(&c, &id, json).await,
                SessionCommands::Kill { id, signal, yes } => {
                    cmd::confirm_destructive("kill session", &id, yes)?;
                    cmd::session::kill(&c, &id, &signal, json).await
                }
                SessionCommands::Tail { id, replay_from } => {
                    cmd::session::tail(&c, &id, replay_from).await
                }
                SessionCommands::Record {
                    id,
                    output,
                    replay_from,
                } => cmd::session::record(&c, &id, &output, replay_from).await,
                SessionCommands::Input { id, file } => cmd::session::input(&c, &id, &file).await,
                SessionCommands::Resize { id, cols, rows } => {
                    cmd::session::resize(&c, &id, cols, rows).await
                }
                SessionCommands::Attach {
                    id,
                    write,
                    replay_from,
                } => cmd::session::attach(&c, &id, write, replay_from).await,
            }
        }
        Commands::Tui { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                TuiCommands::Snapshot { id } => cmd::tui::snapshot(&c, &id, json).await,
                TuiCommands::Observe {
                    id,
                    frames,
                    timeout,
                    idle_ok,
                } => {
                    let d = cmd::parse_duration(&timeout)?;
                    cmd::tui::observe(&c, &id, frames, d, idle_ok, json).await
                }
                TuiCommands::Send {
                    id,
                    text,
                    enter,
                    yes_controller,
                } => cmd::tui::send(&c, &id, &text, enter, yes_controller, json).await,
                TuiCommands::Search { id, query, limit } => {
                    cmd::tui::search(&c, &id, &query, limit, json).await
                }
            }
        }
        Commands::Task { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                TaskCommands::List {
                    state,
                    limit,
                    offset,
                } => cmd::task::list(&c, state.as_deref(), limit, offset, json).await,
                TaskCommands::Get { id } => cmd::task::get(&c, &id, json).await,
                TaskCommands::Submit { file, wait } => {
                    cmd::task::submit(&c, &file, wait, json).await
                }
                TaskCommands::Cancel { id, reason } => {
                    cmd::task::cancel(&c, &id, reason.as_deref(), json).await
                }
                TaskCommands::Logs { id, follow } => cmd::task::logs(&c, &id, follow).await,
                TaskCommands::Artifacts { action } => match action {
                    TaskArtifactsCommands::List { id } => {
                        cmd::task::artifacts_list(&c, &id, json).await
                    }
                },
            }
        }
        Commands::Hitl { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                HitlCommands::Respond { id, text } => {
                    cmd::hitl::respond(&c, &id, &text, json).await
                }
            }
        }
        Commands::Event { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                EventCommands::List {
                    source,
                    since,
                    event_type,
                } => {
                    cmd::event::list(
                        &c,
                        source.as_deref(),
                        since.as_deref(),
                        event_type.as_deref(),
                        json,
                    )
                    .await
                }
                EventCommands::Tail {
                    source,
                    since,
                    event_type,
                    filter,
                } => {
                    cmd::event::tail(
                        &c,
                        source.as_deref(),
                        since.as_deref(),
                        event_type.as_deref(),
                        filter.as_deref(),
                    )
                    .await
                }
            }
        }
        Commands::Storage { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                StorageCommands::Global { action } => match action {
                    StorageGlobalCommands::Ls { path } => {
                        cmd::storage::global_ls(&c, path.as_deref(), json).await
                    }
                    StorageGlobalCommands::Push { path, file } => {
                        cmd::storage::global_push(&c, &path, &file, json).await
                    }
                },
                StorageCommands::Inbox { action } => match action {
                    StorageInboxCommands::Ls { agent, path } => {
                        cmd::storage::inbox_ls(&c, &agent, path.as_deref(), json).await
                    }
                    StorageInboxCommands::Push { agent, path, file } => {
                        cmd::storage::inbox_push(&c, &agent, &path, &file, json).await
                    }
                },
                StorageCommands::Outbox { action } => match action {
                    StorageOutboxCommands::Ls { task, path } => {
                        cmd::storage::outbox_ls(&c, &task, path.as_deref(), json).await
                    }
                },
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

        Commands::AuditLog { action } => match action {
            AuditLogCommands::Tail { lines, follow } => audit::tail(lines, follow).await,
            AuditLogCommands::Grep { pattern } => audit::grep(&pattern),
            AuditLogCommands::Path => audit::print_path(),
        },

        Commands::Tasks { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                TasksCommands::Send {
                    instance_id,
                    message_file,
                } => commands::tasks::send(&c, &instance_id, &message_file, json).await,
                TasksCommands::List {
                    instance_id,
                    state,
                    cursor,
                    limit,
                } => {
                    commands::tasks::list(
                        &c,
                        &instance_id,
                        state.as_deref(),
                        cursor.as_deref(),
                        limit,
                        json,
                    )
                    .await
                }
                TasksCommands::Get {
                    instance_id,
                    task_id,
                } => commands::tasks::get(&c, &instance_id, &task_id, json).await,
                TasksCommands::Subscribe {
                    instance_id,
                    task_id,
                } => commands::tasks::subscribe(&c, &instance_id, &task_id).await,
                TasksCommands::Cancel {
                    instance_id,
                    task_id,
                } => commands::tasks::cancel(&c, &instance_id, &task_id, json).await,
            }
        }

        Commands::Agentcard { action } => {
            let c = build_client(server_override.as_deref(), contexts)?;
            match action {
                AgentcardCommands::Get { instance_id } => {
                    commands::agentcard::get(&c, &instance_id).await
                }
                AgentcardCommands::Verify { instance_id, jwks } => {
                    commands::agentcard::verify(&c, &instance_id, &jwks).await
                }
            }
        }

        Commands::Completions { shell } => {
            use clap::CommandFactory;
            use clap_complete::generate;
            let mut cmd = Cli::command();
            let bin = "sandboxctl";
            generate(shell, &mut cmd, bin, &mut std::io::stdout());
            Ok(())
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

/// Is this command a `list`-class verb that benefits from `--watch`?
/// Streaming verbs (event tail, task logs --follow, session attach/tail/record)
/// already produce live output and shouldn't loop on top of that.
fn is_watchable(c: &Commands) -> bool {
    matches!(
        c,
        Commands::Vm {
            action: VmCommands::List { .. }
        } | Commands::Container {
            action: ContainerCommands::List { .. }
        } | Commands::Agent {
            action: AgentCommands::List { .. }
        } | Commands::Session {
            action: SessionCommands::List { .. }
        } | Commands::Task {
            action: TaskCommands::List { .. }
        } | Commands::Event {
            action: EventCommands::List { .. }
        } | Commands::Loadout {
            action: LoadoutCommands::List
        } | Commands::Storage {
            action: StorageCommands::Global {
                action: StorageGlobalCommands::Ls { .. }
            } | StorageCommands::Inbox {
                action: StorageInboxCommands::Ls { .. }
            } | StorageCommands::Outbox {
                action: StorageOutboxCommands::Ls { .. }
            }
        } | Commands::Health {
            action: HealthCommands::Status
        } | Commands::Config {
            action: ConfigCommands::Contexts
        }
    )
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
            VmCommands::List { .. } => "vm list".into(),
            VmCommands::Get { .. } => "vm get".into(),
            VmCommands::Start { .. } => "vm start".into(),
            VmCommands::Stop { .. } => "vm stop".into(),
            VmCommands::Restart { .. } => "vm restart".into(),
            VmCommands::Destroy { .. } => "vm destroy".into(),
            VmCommands::Reprovision { .. } => "vm reprovision".into(),
            VmCommands::DeployAgent { .. } => "vm deploy-agent".into(),
        },
        Commands::Exec { .. } => "exec".into(),
        Commands::Attach { session_id, .. } => {
            if session_id.is_some() {
                "attach pty".into()
            } else {
                "attach".into()
            }
        }
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
            AgentCommands::Stop { .. } => "agent stop".into(),
            AgentCommands::RotateSecret { .. } => "agent rotate-secret".into(),
            AgentCommands::Shell { .. } => "agent shell".into(),
            AgentCommands::Manifests { action } => match action {
                AgentManifestsCommands::List { .. } => "agent manifests list".into(),
                AgentManifestsCommands::Get { .. } => "agent manifests get".into(),
                AgentManifestsCommands::Push { .. } => "agent manifests push".into(),
            },
        },
        Commands::Container { action } => match action {
            ContainerCommands::List { .. } => "container list".into(),
            ContainerCommands::Get { .. } => "container get".into(),
            ContainerCommands::Create { .. } => "container create".into(),
            ContainerCommands::Start { .. } => "container start".into(),
            ContainerCommands::Stop { .. } => "container stop".into(),
            ContainerCommands::Delete { .. } => "container delete".into(),
        },
        Commands::Session { action } => match action {
            SessionCommands::List { .. } => "session list".into(),
            SessionCommands::Get { .. } => "session get".into(),
            SessionCommands::Kill { .. } => "session kill".into(),
            SessionCommands::Tail { .. } => "session tail".into(),
            SessionCommands::Record { .. } => "session record".into(),
            SessionCommands::Input { .. } => "session input".into(),
            SessionCommands::Resize { .. } => "session resize".into(),
            SessionCommands::Attach { .. } => "session attach".into(),
        },
        Commands::Tui { action } => match action {
            TuiCommands::Snapshot { .. } => "tui snapshot".into(),
            TuiCommands::Observe { .. } => "tui observe".into(),
            TuiCommands::Send { .. } => "tui send".into(),
            TuiCommands::Search { .. } => "tui search".into(),
        },
        Commands::Task { action } => match action {
            TaskCommands::List { .. } => "task list".into(),
            TaskCommands::Get { .. } => "task get".into(),
            TaskCommands::Submit { .. } => "task submit".into(),
            TaskCommands::Cancel { .. } => "task cancel".into(),
            TaskCommands::Logs { .. } => "task logs".into(),
            TaskCommands::Artifacts { .. } => "task artifacts list".into(),
        },
        Commands::Hitl { action } => match action {
            HitlCommands::Respond { .. } => "hitl respond".into(),
        },
        Commands::Loadout { action } => match action {
            LoadoutCommands::List => "loadout list".into(),
            LoadoutCommands::Get { .. } => "loadout get".into(),
            LoadoutCommands::Registry => "loadout registry".into(),
        },
        Commands::Storage { action } => match action {
            StorageCommands::Global { action } => match action {
                StorageGlobalCommands::Ls { .. } => "storage global ls".into(),
                StorageGlobalCommands::Push { .. } => "storage global push".into(),
            },
            StorageCommands::Inbox { action } => match action {
                StorageInboxCommands::Ls { .. } => "storage inbox ls".into(),
                StorageInboxCommands::Push { .. } => "storage inbox push".into(),
            },
            StorageCommands::Outbox { action } => match action {
                StorageOutboxCommands::Ls { .. } => "storage outbox ls".into(),
            },
        },
        Commands::Event { action } => match action {
            EventCommands::List { .. } => "event list".into(),
            EventCommands::Tail { .. } => "event tail".into(),
        },
        Commands::Health { action } => match action {
            HealthCommands::Status => "health status".into(),
            HealthCommands::Watchdog => "health watchdog".into(),
        },
        Commands::Ops { action } => match action {
            OpsCommands::Get { .. } => "ops get".into(),
            OpsCommands::Wait { .. } => "ops wait".into(),
        },
        Commands::AuditLog { action } => match action {
            AuditLogCommands::Tail { .. } => "audit-log tail".into(),
            AuditLogCommands::Grep { .. } => "audit-log grep".into(),
            AuditLogCommands::Path => "audit-log path".into(),
        },
        Commands::Tasks { action } => match action {
            TasksCommands::Send { .. } => "tasks send".into(),
            TasksCommands::List { .. } => "tasks list".into(),
            TasksCommands::Get { .. } => "tasks get".into(),
            TasksCommands::Subscribe { .. } => "tasks subscribe".into(),
            TasksCommands::Cancel { .. } => "tasks cancel".into(),
        },
        Commands::Agentcard { action } => match action {
            AgentcardCommands::Get { .. } => "agentcard get".into(),
            AgentcardCommands::Verify { .. } => "agentcard verify".into(),
        },
        Commands::Completions { .. } => "completions".into(),
    }
}

fn describe_target(c: &Commands) -> String {
    match c {
        Commands::Vm { action } => match action {
            VmCommands::Create { name, .. }
            | VmCommands::Get { name }
            | VmCommands::Start { name }
            | VmCommands::Stop { name, .. }
            | VmCommands::Restart { name, .. }
            | VmCommands::Destroy { name, .. }
            | VmCommands::Reprovision { name, .. }
            | VmCommands::DeployAgent { name, .. } => name.clone(),
            VmCommands::List { .. } => String::new(),
        },
        Commands::Exec { agent_id, .. } | Commands::Logs { agent_id, .. } => agent_id.clone(),
        Commands::Attach {
            agent_or_instance_id,
            session_id,
            ..
        } => match session_id {
            Some(session_id) => format!("{}/{}", agent_or_instance_id, session_id),
            None => agent_or_instance_id.clone(),
        },
        Commands::Config { action } => match action {
            ConfigCommands::SetContext { name, .. } | ConfigCommands::UseContext { name } => {
                name.clone()
            }
            ConfigCommands::Whoami | ConfigCommands::Contexts => String::new(),
        },
        Commands::Agent { action } => match action {
            AgentCommands::Get { id }
            | AgentCommands::Stop { id }
            | AgentCommands::RotateSecret { id, .. }
            | AgentCommands::Shell { id, .. } => id.clone(),
            AgentCommands::Manifests { action } => match action {
                AgentManifestsCommands::List { id, platform } => format!("{}/{}", id, platform),
                AgentManifestsCommands::Get { id, platform, name }
                | AgentManifestsCommands::Push {
                    id, platform, name, ..
                } => {
                    format!("{}/{}/{}", id, platform, name)
                }
            },
            _ => String::new(),
        },
        Commands::Container { action } => match action {
            ContainerCommands::Get { name }
            | ContainerCommands::Create { name, .. }
            | ContainerCommands::Start { name }
            | ContainerCommands::Stop { name, .. }
            | ContainerCommands::Delete { name, .. } => name.clone(),
            ContainerCommands::List { .. } => String::new(),
        },
        Commands::Tui { action } => match action {
            TuiCommands::Snapshot { id }
            | TuiCommands::Observe { id, .. }
            | TuiCommands::Send { id, .. }
            | TuiCommands::Search { id, .. } => id.clone(),
        },
        Commands::Session { action } => match action {
            SessionCommands::Get { id }
            | SessionCommands::Kill { id, .. }
            | SessionCommands::Tail { id, .. }
            | SessionCommands::Record { id, .. }
            | SessionCommands::Input { id, .. }
            | SessionCommands::Resize { id, .. }
            | SessionCommands::Attach { id, .. } => id.clone(),
            _ => String::new(),
        },
        Commands::Task { action } => match action {
            TaskCommands::Get { id }
            | TaskCommands::Cancel { id, .. }
            | TaskCommands::Logs { id, .. } => id.clone(),
            TaskCommands::Artifacts { action } => match action {
                TaskArtifactsCommands::List { id } => id.clone(),
            },
            TaskCommands::Submit { file, .. } => file.display().to_string(),
            _ => String::new(),
        },
        Commands::Storage { action } => match action {
            StorageCommands::Global { action } => match action {
                StorageGlobalCommands::Ls { path } => path.clone().unwrap_or_default(),
                StorageGlobalCommands::Push { path, .. } => path.clone(),
            },
            StorageCommands::Inbox { action } => match action {
                StorageInboxCommands::Ls { agent, path } => {
                    let p = path.clone().unwrap_or_default();
                    if p.is_empty() {
                        agent.clone()
                    } else {
                        format!("{}/{}", agent, p)
                    }
                }
                StorageInboxCommands::Push { agent, path, .. } => format!("{}/{}", agent, path),
            },
            StorageCommands::Outbox { action } => match action {
                StorageOutboxCommands::Ls { task, path } => {
                    let p = path.clone().unwrap_or_default();
                    if p.is_empty() {
                        task.clone()
                    } else {
                        format!("{}/{}", task, p)
                    }
                }
            },
        },
        Commands::Hitl { action } => match action {
            HitlCommands::Respond { id, .. } => id.clone(),
        },
        Commands::Loadout { action } => match action {
            LoadoutCommands::Get { name } => name.clone(),
            _ => String::new(),
        },
        Commands::Ops { action } => match action {
            OpsCommands::Get { id } | OpsCommands::Wait { id, .. } => id.clone(),
        },
        Commands::Tasks { action } => match action {
            TasksCommands::Send {
                instance_id,
                message_file,
            } => format!("{}/{}", instance_id, message_file),
            TasksCommands::List { instance_id, .. } => instance_id.clone(),
            TasksCommands::Get {
                instance_id,
                task_id,
            }
            | TasksCommands::Subscribe {
                instance_id,
                task_id,
            }
            | TasksCommands::Cancel {
                instance_id,
                task_id,
            } => format!("{}/{}", instance_id, task_id),
        },
        Commands::Agentcard { action } => match action {
            AgentcardCommands::Get { instance_id }
            | AgentcardCommands::Verify { instance_id, .. } => instance_id.clone(),
        },
        _ => String::new(),
    }
}
