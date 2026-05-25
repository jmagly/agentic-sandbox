# cli — `sandboxctl`

Operator/admin CLI for the agentic-sandbox management server. Resource-noun + verb taxonomy (`sandboxctl vm list`, `sandboxctl task get <id>`), kubeconfig-style contexts, structured JSON output via `--json`, exit codes that scripts can branch on.

See [`docs/cli-design.md`](../docs/cli-design.md) for the full command taxonomy and design rationale.

## Crate Layout

| Module               | Responsibility                                                                                                       |
|----------------------|----------------------------------------------------------------------------------------------------------------------|
| `src/main.rs`        | Clap definitions for top-level resource groups (`vm`, `agent`, `session`, `tui`, `container`, `task`, `hitl`, `loadout`, `storage`, `event`, `health`, `ops`), context loading, dispatch to `cmd/` modules. |
| `src/config.rs`      | `ContextsFile` (kubeconfig-style): named contexts with server URL + auth, persisted to `~/.config/sandboxctl/contexts.toml`. |
| `src/client/`        | Transport-agnostic SDK. `http.rs` carries the typed REST client + uniform `ClientError`. `sse.rs` does server-sent events for log + event streams. `ws.rs` speaks the JoinSession/SessionInput protocol against the WebSocket port. `models.rs` mirrors server response shapes. |
| `src/cmd/`           | One module per noun: `vm`, `agent`, `container`, `task`, `session`, `tui`, `hitl`, `loadout`, `storage`, `event`, `health`, `ops`. Each implements its verbs, owns its rendering, and returns the right exit code. |
| `src/commands/`      | Legacy verb implementations (`agents`, `attach`, `exec`, `logs`, `server`) preserved for back-compat. New work goes in `cmd/`. |
| `src/output/`        | Renderers: `kv.rs` for key-value, `table.rs` for tabular human output. Suppressed by `--json`.                       |
| `src/pty/`           | Terminal raw-mode helper for `session attach` and `agent shell`. Wraps crossterm.                                    |
| `src/audit/`         | Local CLI audit log writer. Every verb appends an entry to `$XDG_STATE_HOME/sandboxctl/audit.log`.                   |

## Top-Level Subcommands

Derived from the `Commands` enum in `src/main.rs`:

| Verb           | Purpose                                                                                                            |
|----------------|--------------------------------------------------------------------------------------------------------------------|
| `config`       | Manage client contexts (kubeconfig-style).                                                                         |
| `vm`           | VM lifecycle (legacy; see also `agent`).                                                                           |
| `exec`         | One-shot command on an agent (legacy; see also `agent exec`).                                                      |
| `attach`       | Attach to agent output stream (legacy; see also `session attach`).                                                 |
| `logs`         | Tail agent logs from agentshare.                                                                                   |
| `server`       | Manage the management server daemon.                                                                               |
| `agents`       | List connected agents (legacy; see also `agent list`).                                                             |
| `agent`        | Agent inspection — read-only verbs, AIWG manifest push, shell.                                                     |
| `session`      | Live PTY sessions registry — list, attach, tail, record, input, resize.                                            |
| `tui`          | Orchestrator TUI driver — snapshots, observer frames, approved controller writes, transcript search.                |
| `container`    | Container lifecycle (Docker runtime — `list`, `get`, `create`, `start`, `stop`, `delete`).                         |
| `task`         | Task orchestrator — submit, list, get, cancel, artifacts.                                                          |
| `hitl`         | Human-in-the-loop queue — list pending prompts, resolve, deny.                                                     |
| `loadout`      | Loadout profiles — list, get, validate.                                                                            |
| `storage`      | Agentshare REST surface — browse global/, per-agent inbox.                                                         |
| `event`        | Server events buffered snapshot. Includes `event tail --filter <regex>` for client-side filtering.                 |
| `health`       | Diagnostic surface (healthz/readyz rollup).                                                                        |
| `ops`          | Long-running operations tracker (the `operation_id` surface for async POSTs).                                      |
| `audit`        | Local CLI audit log viewer.                                                                                        |
| `tasks`        | A2A core operations against a specific executor instance — `send`, `list`, `get`, `subscribe`, `cancel`.            |
| `agentcard`    | Fetch and verify a signed AgentCard from an executor instance (EdDSA JWS over JCS-canonicalized body).             |
| `completions`  | Print shell completion script (bash/zsh/fish).                                                                     |

## Connection Model

- **REST + SSE** — via `client::http::HttpClient`. The main API surface (`:8122/api/v1/*` and the A2A v2 routes). SSE is used for event tails (`event tail`, `task watch`).
- **WebSocket** — via `client::ws`. The formal session-registry protocol (`JoinSession`, `LeaveSession`, `SessionInput`, `SessionResize`) backing `session attach`, `session tail`, `session record`, `session input`, `session resize`, and `agent shell`. The `tui observe` and `tui send` commands use the HTTP orchestrator WebSocket path `/ws/sessions/{id}/orchestrate` on the management HTTP port.

The `--server` flag (or active context) supplies the HTTP URL; the WS sibling port is derived as `http_port - 1` (default mgmt-server convention: gRPC 8120, WS 8121, HTTP 8122). Override with `AGENTIC_WS_PORT`.

## Auth

Auth headers come from the active `ContextsFile` context (per-context bearer token). `--server` overrides URL only; auth still flows from the active context.

## Exit Codes

```
0   success
1   generic error
2   not found (404)
3   conflict (409)
4   auth required or denied (401/403)
5   timeout
```

These match the surface in `client::http::ClientError`. Scripts can branch on them.

## Build

```bash
cd cli
cargo build --release
# Binary: target/release/sandboxctl
```

`build.rs` captures the build SHA into `SANDBOXCTL_BUILD_SHA`, which is concatenated with the crate version for `--version`.

## Quick Tour

```bash
# Setup a context
sandboxctl config add-context dev --server http://localhost:8122 --token "$TOKEN"
sandboxctl config use-context dev

# Run a verb
sandboxctl vm list
sandboxctl agent list --json | jq '.[] | .id'
sandboxctl session attach <session-id>
sandboxctl tui snapshot <session-id> --json
sandboxctl tui observe <session-id> --frames 3 --json

# Tail events with a filter
sandboxctl event tail --filter '"task\\..*"'

# Generate completion
sandboxctl completions bash > ~/.local/share/bash-completion/completions/sandboxctl
```

## v2 Admin API + A2A (#251)

Admin verbs (`vm`, `agent`, `storage`, `ops`) try the v2 admin paths first
(`/api/v2/admin/...`) and fall back to v1 (`/api/v1/...`) when the server
returns 404, surfacing a one-line `Sunset:` warning to stderr. v1 paths
are scheduled for removal — pin executors to the latest server release to
silence the warnings.

### `tasks` (A2A core)

```bash
# Send a Message envelope, get a task_id back. Sets the required
# A2A-Extensions header (runtime/v1 + idempotency/v1) automatically.
sandboxctl tasks send <instance-id> ./message.json
sandboxctl tasks send <instance-id> -        # stdin

# List, inspect, cancel
sandboxctl tasks list <instance-id> --state working --limit 50
sandboxctl tasks get <instance-id> <task-id>
sandboxctl tasks cancel <instance-id> <task-id>

# Stream task updates via SSE; exits on terminal state.
sandboxctl tasks subscribe <instance-id> <task-id>
```

### `agentcard`

```bash
# Fetch the signed AgentCard.
sandboxctl agentcard get <instance-id>

# Verify the JWS Compact signature against a JWKS (local file or URL).
sandboxctl agentcard verify <instance-id> --jwks ./jwks.json
sandboxctl agentcard verify <instance-id> --jwks http://localhost:8122/agents/<instance-id>/.well-known/jwks.json
```

EdDSA (Ed25519) only; JCS canonicalization per RFC 8785.

### `tui` (orchestrator driver)

Use `tui` when an external orchestrator needs to read or drive the application as a user-facing terminal rather than attach a local human TTY. It is observer-first: read operations do not grant write authority, and write operations require `--yes-controller`.

```bash
# Read the current parsed screen. This is the cheapest bounded context read.
sandboxctl tui snapshot <session-id> --json

# Watch structured frames as observer. The first frame shows role and can_write=false.
sandboxctl tui observe <session-id> --frames 3 --json

# Search the bounded hot TUI window plus durable transcript spill.
sandboxctl tui search <session-id> "panic" --limit 20 --json

# Send one approved controller write frame. Observe first, then opt in.
sandboxctl tui send <session-id> --text "npm test" --enter --yes-controller
```

Backing routes: `GET /api/v1/sessions/{id}/screen`, `GET /api/v1/sessions/{id}/transcript`, and `GET /ws/sessions/{id}/orchestrate?role=observer|controller`. No extra in-guest agent process is required; the commands use the existing sandbox agent and PTY bridge.

See [`docs/tui-orchestration-support.md`](../docs/tui-orchestration-support.md) for support and evidence-capture workflows.

### PTY attach migration (deferred)

The new executor exposes a `pty-ws.v1`-protocol WebSocket at
`/agents/{instance_id}/sessions/{session_id}/attach` with a structured
`{op, seq, ts, payload}` envelope. The CLI's existing `session attach`
continues to use the legacy `ws://host:8121/` formal-session protocol;
full migration to `pty-ws.v1` is tracked as a follow-up. The
`--legacy-pty` flag is reserved for the transition window.

## See Also

- [`docs/cli-design.md`](../docs/cli-design.md) — full taxonomy and design rationale
- [`docs/API.md`](../docs/API.md) — REST + WebSocket reference the CLI is built on
- [`docs/ws-protocol.md`](../docs/ws-protocol.md) — session WebSocket protocol
- [`docs/tui-orchestration-support.md`](../docs/tui-orchestration-support.md) — support runbook for orchestrator TUI sessions
- [`docs/v2-migration-guide.md`](../docs/v2-migration-guide.md) — how the CLI maps to v1 vs v2 surfaces
