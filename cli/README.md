# cli — `sandboxctl`

Operator/admin CLI for the agentic-sandbox management server. Resource-noun + verb taxonomy (`sandboxctl vm list`, `sandboxctl task get <id>`), kubeconfig-style contexts, structured JSON output via `--json`, exit codes that scripts can branch on.

See [`docs/cli-design.md`](../docs/cli-design.md) for the full command taxonomy and design rationale.

## Crate Layout

| Module               | Responsibility                                                                                                       |
|----------------------|----------------------------------------------------------------------------------------------------------------------|
| `src/main.rs`        | Clap definitions for top-level resource groups (`vm`, `agent`, `session`, `container`, `task`, `hitl`, `loadout`, `storage`, `event`, `health`, `ops`), context loading, dispatch to `cmd/` modules. |
| `src/config.rs`      | `ContextsFile` (kubeconfig-style): named contexts with server URL + auth, persisted to `~/.config/sandboxctl/contexts.toml`. |
| `src/client/`        | Transport-agnostic SDK. `http.rs` carries the typed REST client + uniform `ClientError`. `sse.rs` does server-sent events for log + event streams. `ws.rs` speaks the JoinSession/SessionInput protocol against the WebSocket port. `models.rs` mirrors server response shapes. |
| `src/cmd/`           | One module per noun: `vm`, `agent`, `container`, `task`, `session`, `hitl`, `loadout`, `storage`, `event`, `health`, `ops`. Each implements its verbs, owns its rendering, and returns the right exit code. |
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
| `container`    | Container lifecycle (Docker runtime — `list`, `get`, `create`, `start`, `stop`, `delete`).                         |
| `task`         | Task orchestrator — submit, list, get, cancel, artifacts.                                                          |
| `hitl`         | Human-in-the-loop queue — list pending prompts, resolve, deny.                                                     |
| `loadout`      | Loadout profiles — list, get, validate.                                                                            |
| `storage`      | Agentshare REST surface — browse global/, per-agent inbox.                                                         |
| `event`        | Server events buffered snapshot. Includes `event tail --filter <regex>` for client-side filtering.                 |
| `health`       | Diagnostic surface (healthz/readyz rollup).                                                                        |
| `ops`          | Long-running operations tracker (the `operation_id` surface for async POSTs).                                      |
| `audit`        | Local CLI audit log viewer.                                                                                        |
| `completions`  | Print shell completion script (bash/zsh/fish).                                                                     |

## Connection Model

- **REST + SSE** — via `client::http::HttpClient`. The main API surface (`:8122/api/v1/*` and the A2A v2 routes). SSE is used for event tails (`event tail`, `task watch`).
- **WebSocket** — via `client::ws`. The formal session-registry protocol (`JoinSession`, `LeaveSession`, `SessionInput`, `SessionResize`) backing `session attach`, `session tail`, `session record`, `session input`, `session resize`, and `agent shell`.

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

# Tail events with a filter
sandboxctl event tail --filter '"task\\..*"'

# Generate completion
sandboxctl completions bash > ~/.local/share/bash-completion/completions/sandboxctl
```

## See Also

- [`docs/cli-design.md`](../docs/cli-design.md) — full taxonomy and design rationale
- [`docs/API.md`](../docs/API.md) — REST + WebSocket reference the CLI is built on
- [`docs/ws-protocol.md`](../docs/ws-protocol.md) — session WebSocket protocol
- [`docs/v2-migration-guide.md`](../docs/v2-migration-guide.md) — how the CLI maps to v1 vs v2 surfaces
