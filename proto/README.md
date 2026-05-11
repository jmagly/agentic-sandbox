# proto — gRPC Wire Schemas

Single source of truth for the gRPC contract between the management server, the agent client, and the `sandboxctl` CLI.

## Files

| File              | Purpose                                                                                                |
|-------------------|--------------------------------------------------------------------------------------------------------|
| `agent.proto`     | The full wire surface. One file by design — task lifecycle, agent protocol, PTY control, session reconciliation, and metrics all share message types so co-locating them is cheaper than splitting. |

If we ever split, the cut line is the in-file section banner comments (`Agent → Management Messages`, `Management → Agent Messages`, `Exec RPC`, `Task Orchestration Messages`, `Session Reconciliation Messages`). Don't split prematurely.

## Package

```protobuf
syntax = "proto3";
package agentic.sandbox.v1;
option go_package = "github.com/roctinam/agentic-sandbox/pkg/proto/agentpb";
```

The `v1` in the package is the gRPC schema version. It moves only on breaking changes. The HTTP REST surface has its own versioning under `/api/v1/*` and `/api/v2/*` (see [`../docs/v2-migration-guide.md`](../docs/v2-migration-guide.md)) — the two version namespaces are independent.

## Service

### `AgentService`

The primary bidirectional channel between management and agent.

| RPC          | Direction                                                | Purpose                                                                                                          |
|--------------|----------------------------------------------------------|------------------------------------------------------------------------------------------------------------------|
| `Connect`    | bidirectional stream — `AgentMessage` ↔ `ManagementMessage` | Long-lived per-agent connection. Carries registration, heartbeats, command dispatch, PTY control, output chunks, metrics, session reconciliation. The agent opens it on boot; the management server treats disconnect as agent-down. |
| `Exec`       | unary request, server stream — `ExecRequest` → `ExecOutput` | One-shot command for cases where opening a full session is overkill. Used by the dashboard for quick "run a command on this agent" UX. |

Flow (from the proto docstring):

```
1. Agent boots, establishes gRPC connection to management server
2. Agent calls Connect() with bidirectional stream
3. Management sends commands via stream
4. Agent sends outputs (stdout, stderr, logs, status) via stream
```

## Message Vocabulary (Selected)

The full set is in `agent.proto`. The high-traffic types:

**Agent → Management** (variants of `AgentMessage.payload`):
- `AgentRegistration` — initial handshake (`agent_id`, `ip_address`, `hostname`, `profile`, labels, `SystemInfo`, AIWG frameworks)
- `Heartbeat` — periodic liveness + status
- `OutputChunk` — stdout/stderr/log
- `CommandResult` — completion of a `CommandRequest`
- `Metrics` — sysinfo snapshot
- `SessionReport`, `SessionReconcileAck` — session reconciliation

**Management → Agent** (variants of `ManagementMessage.payload`):
- `RegistrationAck` — accept/reject registration
- `CommandRequest` — execute a command
- `PtyControl` (wrapping `PtyResize`, `PtySignal`, `StdinChunk`) — PTY plumbing
- `ConfigUpdate`, `ShutdownSignal`, `Ping` — control plane
- `SessionReconcile` — request session inventory

**Task orchestration** (used by the v1 task API; v2 uses A2A on REST):
- `TaskDefinition`, `TaskStatus`, `TaskProgress`, `TaskOutput`, `TaskArtifact`
- `SubmitTaskRequest`/`Response`, `ListTasksRequest`/`Response`, `GetTaskRequest`/`Response`, `CancelTaskRequest`/`Response`, `StreamTaskOutputRequest`

## Code Generation

`tonic-build` compiles `agent.proto` at build time. Each consuming crate has its own `build.rs` that picks the right client/server combination:

| Crate         | `build.rs` posture                                                                            |
|---------------|------------------------------------------------------------------------------------------------|
| `agent-rs`    | `.build_server(false).build_client(true).build_transport(false)` — client only, no transport layer (the channel is constructed manually). |
| `management`  | `.build_server(true).build_client(false).build_transport(false)` — server side only. |
| `cli`         | `.build_server(false).build_client(true).build_transport(false)` — client (used by a small number of CLI verbs that talk gRPC; most go through REST). |

The generated types land in each crate via `tonic::include_proto!("agentic.sandbox.v1")`. By convention they're re-exported under a `pub mod proto` in `lib.rs` or `main.rs`.

The `.build_transport(false)` setting in all three configs is intentional: tonic's auto-generated `connect<D>()` collides with the RPC method literally named `Connect()`, so the project constructs `Channel` manually.

## Convention: Adding a New Message

1. Edit `agent.proto`. Keep numeric tags stable; never reuse a removed tag.
2. Run `cargo check` in `agent-rs`, `management`, and `cli` to confirm all three regenerate.
3. If a new field is added to an existing message, mark it optional and pin a default. Wire compatibility is non-negotiable for agents in the field — old binaries must still parse new messages and vice versa within a `v1` lifetime.
4. Bump the package name to `v2` only on a true wire break, and only after a deprecation window. See [`../docs/v2-migration-guide.md`](../docs/v2-migration-guide.md) for the operational pattern (Sunset header, dual-serving period, Link header to v2 equivalent).

## See Also

- [`../docs/grpc-architecture.md`](../docs/grpc-architecture.md) — design rationale and end-to-end flow
- [`../docs/task-orchestration-api.md`](../docs/task-orchestration-api.md) — task RPC surface
- [`../docs/SESSION_RECONCILIATION.md`](../docs/SESSION_RECONCILIATION.md) — reconciliation protocol
- [`../docs/contracts/`](../docs/contracts/) — REST + A2A v2 contracts (separate from gRPC)
