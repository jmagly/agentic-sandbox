# Sandbox Operator/Admin CLI — Design

Tracking issue: [#152](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/152)

This document captures the design for a first-class operator/admin CLI for
the sandbox. Today the system is driven through a mix of HTTP endpoints,
the web dashboard, ad-hoc shell scripts, and direct `virsh`/`ssh` calls.
The goal is one binary — `sandboxctl` — that covers the full surface,
including attaching to any live PTY session.

The plan is built from a complete audit of the current surface area
(below). It identifies the API gaps the CLI exposes; those gaps are
filed as their own issues so server-side work can ship in parallel.

---

## 1. Surface area inventory

A short reference. File:line citations are authoritative; this section
is deliberately terse.

### 1.1 HTTP REST (`:8122`)

Defined in `management/src/http/server.rs:145-254`. Grouped:

- **Health**: `/healthz`, `/healthz/http`, `/readyz`, `/healthz/deep`,
  legacy `/api/health`, `/api/v1/health[/ready|/live]`.
- **Agents**: `GET/POST/DELETE /api/v1/agents{,/{id}}`,
  `POST /api/v1/agents/{id}/{start,stop,destroy,reprovision}`.
- **VMs**: `GET/POST/DELETE /api/v1/vms{,/{name}}`,
  `POST /api/v1/vms/{name}/{start,stop,destroy,restart,deploy-agent}`.
- **Sessions (legacy, agent-scoped)**:
  `GET/POST/DELETE /api/v1/agents/{id}/sessions{,/{name}}`.
- **Sessions (formal, server-owned)**:
  `GET /api/v1/sessions`, `GET /api/v1/sessions/{id}/stream` (SSE
  with `?from=<seq>` replay), `GET /api/v1/sessions/{id}/screen`.
- **Tasks**: `POST/GET/DELETE /api/v1/tasks{,/{id}}`,
  `GET /api/v1/tasks/{id}/{logs (SSE),artifacts,artifacts/{name}}`.
- **HITL**: `POST /api/v1/agents/{id}/hitl`, `GET /api/v1/hitl`,
  `POST /api/v1/hitl/{id}/respond`.
- **Loadouts**: `GET /api/v1/loadouts{,/{name}}`,
  `GET /api/v1/loadout/registry`.
- **Events**: `GET/POST /api/v1/events`.
- **AIWG proxy**: `/api/v1/agents/{id}/manifests/{platform}{,/{name}}`,
  `/api/v1/agents/{id}/aiwg/exec`, `/api/v1/aiwg/{status,reconnect}`.
- **Operations**: `GET /api/v1/operations/{id}`.
- **Metrics**: `GET /metrics` (Prometheus).
- **Static UI**: `/*` fallback.

### 1.2 gRPC (`:8120`)

`AgentService` in `proto/agent.proto:7-23`, implemented in
`management/src/grpc.rs:27-209`.

- `Connect` — bidirectional stream. Agents send registration, heartbeat,
  output, command results, metrics, session reports. Server sends
  registration ack, command, config, shutdown, ping, stdin, PTY control,
  session query, session reconcile.
- `Exec` — server-streaming one-shot.
- **Auth**: gRPC metadata `x-agent-id` + `x-agent-secret` (sha256
  validated against `agent-hashes.json`, `management/src/auth.rs:11-103`).
- **Audience**: agent → server only. Operators do not use gRPC.

### 1.3 WebSocket (`:8121`)

`management/src/ws/connection.rs:22-243`.

- **Legacy agent-scoped messages**: `Subscribe`, `SendInput`, `SendCommand`,
  `StartShell`, `PtyResize`, `ListAgents`, `ListSessions`,
  `AttachSession`, `DetachSession`, `KillSession`, `CreateSession`.
- **Formal session-registry messages** (post-refactor): `JoinSession`,
  `LeaveSession`, `SessionInput`, `SessionResize`. Server sends
  `SessionJoined`, `SessionLeft`, `SessionFrame { seq, ts, payload }`
  where `SessionPayload` is one of `Output`, `Resize`, `RoleAssigned`,
  `MembershipChanged`, `Closed`, `Error`.
- **Orchestrator**: `GET /ws/sessions/{id}/orchestrate` streams
  `OrchestratorFrame` (parsed screen state, prompt detection).

### 1.4 Operator-facing scripts

Most are wrapped or partially wrapped by REST already:

| Script | Wrapped by | Notes |
|---|---|---|
| `images/qemu/provision-vm.sh` | `POST /api/v1/vms` | invoked by `provision-vm-agent.sh` |
| `scripts/reprovision-vm.sh` | `POST /api/v1/agents/{id}/reprovision` | returns operation_id |
| `scripts/deploy-agent.sh` | `POST /api/v1/vms/{name}/deploy-agent` | |
| `scripts/destroy-vm.sh` | `DELETE /api/v1/vms/{name}?delete_disk=true` | |
| `scripts/validate-vm.sh` | — | health probe; not in REST |
| `scripts/vm-event-bridge.py` | — | event producer; POSTs to `/api/v1/events` |
| `scripts/setup-agentshare.sh` | — | one-time host setup |
| `scripts/setup-disk-quotas.sh` | — | one-time host setup |

### 1.5 Storage (agentshare)

Filesystem-only today. Shape under `/agentshare`:

- `global/` — read-only shared resources, mounted RO in VMs
- `inbox/<agent-id>/` — per-agent RW input
- `outbox/<task-id>/` — per-task output

The only API surface is task-artifact download
(`GET /api/v1/tasks/{id}/artifacts{,/{name}}`).

### 1.6 Auth

| Surface | Auth today |
|---|---|
| gRPC | `x-agent-id` + sha256 secret (agents only) |
| HTTP REST | none |
| WebSocket | none |
| Unix socket | none (no socket exists) |

Operators currently rely on the management server being on a trusted
network. Any remote use of the CLI requires adding operator auth first.

### 1.7 Capability gaps the CLI exposes

These are the items the CLI cannot satisfy from existing surfaces.
Each is filed as its own issue:

1. Operator auth on HTTP/WS (Unix socket + bearer token).
2. `DELETE /api/v1/sessions/{id}` for the formal session model.
3. SSE on `/api/v1/events` (currently poll-only).
4. Agentshare REST endpoints (list/push for `global`, `inbox`, `outbox`).
5. `POST /api/v1/agents/{id}/rotate-secret` for secret rotation.

---

## 2. Command taxonomy

`kubectl`-style noun-then-verb. Resource groups are top-level commands;
verbs are consistent across groups (`list`, `get`, `create`, `delete`,
`start`, `stop`, `attach`, `tail`, `submit`, `cancel`).

```
sandboxctl
├── vm                       # libvirt domain lifecycle
│   ├── list                       [--state running|stopped|all] [--prefix p]
│   ├── get <name>                 [--json]
│   ├── create                     [--profile|--loadout F] [--wait]
│   ├── start <name>
│   ├── stop <name>                [--force] [--timeout 15s]
│   ├── restart <name>             [--graceful|--hard]
│   ├── destroy <name>             [--force] [--delete-disk]
│   ├── reprovision <name>         [--loadout F] [--wait]
│   └── deploy-agent <name>        [--debug]
│
├── agent                    # registered agent processes
│   ├── list                       [--state ready|busy|stale|all]
│   ├── get <id>                   [--json]
│   ├── shell <id>                 [--cmd ...]
│   ├── exec <id> -- <cmd> ...     [--timeout] [--pty]
│   ├── stop <id>
│   ├── rotate-secret <id>         # depends on API gap (#5 above)
│   └── manifests
│       ├── list <id> <platform>
│       ├── get  <id> <platform> <name>
│       └── push <id> <platform> <name> --file f.md
│
├── session                  # formal SessionRegistry (multi-client)
│   ├── list                       [--agent ID] [--json]
│   ├── get <session-id>
│   ├── attach <session-id>        [--role observer|controller] [--replay-from N|all]
│   ├── tail <session-id>
│   ├── record <session-id> -o f
│   ├── input <session-id> --file -
│   ├── resize <session-id> --cols C --rows R
│   ├── kill <session-id>          [--signal TERM|KILL]   # depends on API gap #2
│   └── orchestrate <session-id>
│
├── task
│   ├── submit -f manifest.yaml    [--wait] [--follow]
│   ├── list                       [--state ...] [--limit N]
│   ├── get <task-id>
│   ├── logs <task-id>             [--follow]
│   ├── cancel <task-id>           [--reason ...]
│   └── artifacts
│       ├── list <task-id>
│       └── get  <task-id> <name> -o file
│
├── hitl
│   ├── list
│   ├── get <id>
│   └── respond <id> --text ...
│
├── loadout
│   ├── list
│   ├── get <name>
│   └── registry
│
├── storage                  # depends on API gap #4
│   ├── global ls   <path>
│   ├── global push <src> <dst>
│   ├── inbox  ls   <agent-id>
│   ├── inbox  push <agent-id> <src>
│   └── outbox ls   <task-id>
│
├── event
│   ├── list                       [--source ...] [--since 1h]
│   └── tail                       [--source ...] [--filter regex]   # depends on API gap #3
│
├── health
│   ├── status                     # rolls up healthz, healthz/http, readyz, healthz/deep
│   ├── watchdog                   # consecutive failures, last probe
│   └── logs <component>           # mgmt | agent <id>
│
├── ops
│   ├── get <op-id>
│   └── wait <op-id>               [--timeout 5m]
│
├── audit                    # local CLI-side audit log
│   ├── tail
│   └── grep <pattern>
│
└── config
    ├── set-context <name> --server URL --token T
    ├── use-context <name>
    └── whoami
```

### Verb-consistency rules

- `list` → table by default, `--json` for machine output, `--watch`
  for tail-style streams (poll for HTTP, SSE for streaming surfaces).
- `get <id>` → single-resource inspection; key:value block by default.
- `start`/`stop`/`destroy` only on lifecycle resources
  (`vm`, `agent`, `session`).
- `attach` reserved for streams the user joins (PTY, orchestrator);
  `tail` is the non-interactive variant.
- `submit`/`cancel` only on `task` (semantically distinct from
  create/destroy).

---

## 3. Technical architecture

### 3.1 Crate layout

Extend the existing `cli/` crate; do not ship a second binary. Internal
split:

```
cli/
├── src/
│   ├── main.rs              # clap dispatch
│   ├── cmd/                 # one module per resource group
│   ├── client/              # transport-agnostic SDK
│   │   ├── http.rs          # reqwest, retry, auth headers
│   │   ├── ws.rs            # tokio-tungstenite, formal session protocol
│   │   ├── sse.rs           # eventsource-client wrapper
│   │   └── models.rs        # serde types shared with mgmt server
│   ├── pty/                 # local terminal handling for `attach`
│   ├── output/              # table | json | watch renderers
│   ├── audit/               # structured client-side audit log
│   └── config.rs            # contexts, tokens, server URLs
└── tests/
```

Canonical binary name: `sandboxctl`. Existing `agentic` name kept as
alias for compatibility.

### 3.2 Transport per verb class

| Verb class | Transport |
|---|---|
| list / get / one-shot mutations | HTTP REST |
| streaming logs (`task logs`, `event tail`) | SSE |
| PTY attach (`session attach`, `agent shell`) | WebSocket — formal `JoinSession` / `SessionInput` / `SessionResize` / `LeaveSession` |
| orchestrator screen | WebSocket `/ws/sessions/{id}/orchestrate` |
| agent admin (rotate-secret, push config) | HTTP (new endpoints) |

Rationale: `AgentService` gRPC is agent-facing. Adding operator endpoints
to gRPC would require a second service surface; layering admin verbs
onto the existing HTTP API is cheaper and consistent with how tasks,
HITL, and session management already work.

### 3.3 Auth model

Two layers, both opt-in via mgmt-server config:

1. **Local admin** — Unix domain socket on `/run/agentic-mgmt.sock`
   (opt-in via `AGENTIC_MGMT_UDS=/run/agentic-mgmt.sock`),
   peer-creds-authenticated (`SO_PEERCRED`). CLI auto-uses it if
   present and writable. No token needed. Socket is mode 0660 and
   `chgrp`'d to `agentic-admin` (override via `AGENTIC_MGMT_UDS_GROUP`)
   so members of that group can connect, others cannot. Token reload
   is SIGHUP-driven; reload count and active-token gauge surfaced via
   `/metrics`, and each reload emits an `operator.tokens_reloaded`
   event.
2. **Remote operator** — Bearer token over HTTP/WS. Tokens stored in
   `~/.config/agentic-sandbox/contexts.toml` (kubeconfig-style):

   ```toml
   current_context = "lab"
   [contexts.lab]
   server = "https://mgmt.lab:8122"
   token  = "..."
   role   = "admin"      # or "operator"
   ```

   Server validates against a token file (mode 0600). `admin` vs
   `operator` gates destructive verbs (`vm destroy`, `session kill`,
   `agent rotate-secret`).

WS auth is the same Bearer token in the `Authorization` header on the
upgrade request. Both layers are tracked under the operator-auth issue.

### 3.4 PTY attach UX

`session attach <id>` flow:

1. `GET /api/v1/sessions/{id}` to verify existence and pick replay seq.
2. WS upgrade. Send `JoinSession { session_id, role, replay_from }`.
3. On `SessionJoined`: switch local TTY to raw mode, emit
   `SessionResize` for current cols/rows, install a SIGWINCH handler
   that sends `SessionResize`, pipe stdin → `SessionInput`
   (controllers only).
4. Each inbound `SessionFrame::Output` writes to local stdout
   (base64-decoded, binary-safe).
5. `MembershipChanged` frames optionally render a status line
   (`--status-line on`).
6. Exit hotkey (default `Ctrl-A d`, configurable) sends `LeaveSession`
   and restores the TTY. `Ctrl-A k` (admin only) sends a kill once
   the API exists.
7. Default role is **observer**. `--role controller` or `--write` to
   attach as writer; CLI prints a one-line warning naming current
   controllers (from the `MembershipChanged` snapshot) so the operator
   knows they are joining a multi-writer session.

`session record` is the same path with no TTY switch: raw `SessionFrame`
JSON Lines to stdout/file for offline replay.

### 3.5 Output, streaming, scripting

- Default human renderer: aligned table for `list`, key:value block for
  `get`.
- `--json` everywhere; schema = mgmt-server REST DTOs, no CLI-side
  reshaping.
- `--watch` on list verbs: poll for HTTP-only resources, SSE-tail for
  streamable ones (`event tail`, `task logs`).
- All streaming verbs accept `--since duration` and `--from seq` where
  the surface supports it (sessions, events).
- Exit codes: `0` success, `1` generic, `2` not-found (404), `3`
  conflict (409), `4` auth (401/403), `5` timeout. Documented in
  `--help`.

### 3.6 Audit

Every verb writes one JSON-Lines record to
`~/.local/state/sandboxctl/audit.log` *before* dispatch (intent) and
*after* completion (outcome). Fields: `ts, actor, context, verb,
target, args_redacted, outcome, http_status, duration_ms`.

Server-side audit is out of scope for v1, but the CLI's local log
gives operators a record before the server has one.

### 3.7 Long-running operations

Verbs whose API returns `operation_id` (`vm create`, `vm reprovision`,
`task submit`) accept `--wait` and `--timeout`. Implementation: poll
`/api/v1/operations/{id}` with exponential backoff capped at 5s;
surface intermediate progress when the operation reports it.

---

## 4. Build order

Sized by scope (atomic deliverables), not duration. Each step is
independently shippable.

1. **Foundation** — config/contexts, `client::http`, output renderers,
   audit log writer. Replaces existing `cli/src/config.rs`.
2. **Read-only verbs** — `vm list/get`, `agent list/get`,
   `session list/get`, `task list/get`, `event list`,
   `loadout list/get`, `health status`, `ops get/wait`. No new server
   work.
3. **Lifecycle verbs** — `vm start/stop/restart/destroy/reprovision/
   deploy-agent/create`, `task submit/cancel`, `hitl respond`.
   All map to existing endpoints.
4. **Streaming** — `task logs --follow`, `session tail`, `agent shell`,
   `session attach` (observer-only first, then controller). Adds
   `client::ws` and `client::sse`. Validates the formal session
   protocol end-to-end.
5. **Server-side gaps** (parallel to 4):
   - `DELETE /api/v1/sessions/{id}` (formal-model kill)
   - SSE on `/api/v1/events`
   - Agentshare REST endpoints
   - Operator auth (Unix socket + bearer token)
   - `POST /api/v1/agents/{id}/rotate-secret`
6. **Admin verbs** — `session kill`, `agent rotate-secret`,
   `storage *`, `event tail` (depends on step 5).
7. **Polish** — `--watch` on all list verbs, audit grep, shell
   completions (`bash`/`zsh`/`fish`), packaging.

Step 4 is the highest-leverage user-visible milestone (the headline
"attach to any PTY" capability).

---

## 5. Acceptance criteria (from #152)

- `sandboxctl --help` lists all top-level groups: `vm`, `agent`,
  `session`, `task`, `hitl`, `loadout`, `storage`, `event`, `health`,
  `ops`, `audit`, `config`.
- An admin can attach to a PTY session that was started by another
  operator's tooling, observe live output, become a writer, and detach
  without disturbing the session or its other clients.
- Every operator-facing shell script under `scripts/` has a documented
  CLI equivalent (the scripts can stay as thin wrappers).
- Every command writes a structured audit log entry containing actor,
  command, target, and outcome.
- The CLI works when the dashboard UI is down, as long as the
  management server's gRPC/HTTP listeners are responsive.
