# Glossary

Terms used throughout the agentic-sandbox codebase, docs, ADRs, and contract specs. Cross-linkable by anchor (`#agentshare`, `#hitl`, …). When in doubt, read [`concepts.md`](concepts.md) first — it explains how these terms relate.

## agentshare

Bidirectional file-exchange surface between the host and agent VMs, mounted via virtiofs. `~/global` (RO inside the VM, host write) ships shared resources; `~/inbox/<agent-id>` (RW inside the VM) collects agent outputs. Lives at `/srv/agentshare/` on the host; mounted at `/mnt/global` and `/mnt/inbox/<agent-id>` inside the VM. See [agentshare.md](agentshare.md).

## loadout

Declarative YAML manifest layered on top of a base profile that says which tools, runtimes, AI providers, and AIWG frameworks a VM should install. Loadouts are composable — `provision-vm.sh agent-02 --loadout profiles/dual-review.yaml` installs everything Dual Review declares. See [LOADOUTS.md](LOADOUTS.md). The `runtime/v1` A2A extension carries the loadout name on every Task so consumers can route by loadout.

## mission / task

A mission is a unit of work dispatched from an external `aiwg serve` orchestrator. Internally, missions decompose into one or more A2A **tasks** with eight-state lifecycle (see [concepts.md](concepts.md)). The legacy v1 surface called these "missions"; the A2A v2 surface uses "task" throughout. The two terms refer to the same thing in different vocabularies. See [aiwg-executor.md](aiwg-executor.md) and [task-run-lifecycle.md](task-run-lifecycle.md).

## executor

A management-server instance that has registered itself as a worker against an external `aiwg serve` orchestrator (via `AIWG_SERVE_ENDPOINT`). The executor accepts mission dispatches on `POST /api/v1/sessions/:id/dispatch`, pushes events back over `/ws/executors/{id}`, and persists mission state across restarts. See [aiwg-executor.md](aiwg-executor.md).

## HITL

Human-in-the-loop. The A2A `input-required` task state pauses execution and prompts a human via the `hitl-prompt/v1` extension; the prompt sits in the HITL queue (`/api/v1/hitl/*` v1, `task` `input-required` state on v2) until resolved. CLI: `sandboxctl hitl list`. See [v2-migration-guide.md](v2-migration-guide.md).

## instance

A running agent. Today an instance is either a libvirt+QEMU VM (the primary path) or a Docker container (via `management/src/docker_runtime.rs`). The runtime abstraction in #119 makes "instance" the operator-facing noun; the implementation (`vm-qemu`, `container-docker`) is carried as metadata via the `runtime/v1` extension. See [platform-support.md](platform-support.md).

## runtime

The execution substrate hosting an instance. Values: `vm-qemu` (libvirt+QEMU), `container-docker` (Docker), with `vm-proxmox` and `container-containerd` on the roadmap (#119, #120). Declared in the AgentCard via the [`runtime/v1` A2A extension](contracts/extensions/runtime/v1/spec.md).

## session

A live PTY or output stream attached to a process running on an agent. Sessions are first-class objects with their own registry, formal WebSocket protocol (`JoinSession`, `SessionInput`, `SessionResize`), and CLI verbs (`session attach`, `session tail`, `session record`). Sessions can admit controllers and observers according to the advertised capability; the `pty-ws/v1` reference profile uses one controller plus observers. See [SESSION_ARCHITECTURE.md](SESSION_ARCHITECTURE.md) and [ws-protocol.md](ws-protocol.md).

## dispatch

The act of handing a mission/task to an executor. The dispatch endpoint is `POST /api/v1/sessions/:id/dispatch` on the executor; idempotency-keyed so re-dispatch on retry doesn't double-execute. See ADR-008 and [aiwg-executor.md](aiwg-executor.md).

## controller / observer

Roles on a PTY session. A **controller** can send input (stdin, signals, resize); an **observer** is read-only. The allowed number of simultaneous controllers is capability-specific; the `pty-ws/v1` reference profile admits one controller at a time, plus observers. See [SESSION_ARCHITECTURE.md](SESSION_ARCHITECTURE.md).

## AgentCard

The A2A discovery document served at `/agents/{instance_id}/.well-known/agent-card.json` (per-instance, per ADR-022). Declares the agent's capabilities, supported extensions, and authentication scheme. JCS-canonicalized and JWS-signed (Ed25519) so consumers can verify they're talking to the right agent without trusting transport alone. See [v2-migration-guide.md](v2-migration-guide.md) §AgentCard.

## custom binding

An A2A extension that defines a non-standard transport for a sub-protocol. The PTY-over-WebSocket binding (ADR-020) is the canonical example: A2A core says nothing about interactive streams, so we publish a custom binding that pins our `JoinSession`/`SessionInput` shape under an extension URI.

## extension

An A2A capability beyond core. Identified by URI (`https://agentic-sandbox.aiwg.io/extensions/<name>/v<n>`), declared in `AgentCard.capabilities.extensions[]`, activated by the `A2A-Extensions` HTTP header. The project ships `runtime/v1`, `hitl-prompt/v1`, and the PTY custom binding. URI permanence is governed by ADR-019.

## A2A

Agent-to-Agent protocol, the spec we align to for the v2 executor contract. We mirror the upstream spec (`jmagly/A2A`) and Rust SDK (`jmagly/a2a-rs`) into Gitea as a [fork-as-update-gate](#fork-as-update-gate) and pin against tagged Gitea revisions. See [contracts/upstream-sync.md](contracts/upstream-sync.md) and ADR-018 / ADR-021.

## JCS

JSON Canonicalization Scheme (RFC 8785). Deterministic JSON serialization — sorted keys, normalized numbers, exact UTF-8 — so two implementations produce byte-identical output for the same logical document. Required for signing AgentCards: the signer JCS-canonicalizes, then JWS-signs over the canonical bytes.

## JWS

JSON Web Signature (RFC 7515). The signature envelope used for AgentCard signing. Algorithm: Ed25519. Verification: parse the card, strip `signature`, JCS-canonicalize the remainder, verify the JWS against the issuer's public key.

## idempotency-key

A client-supplied identifier on a state-changing POST that lets the server safely de-dupe retries. The executor's dispatch endpoint requires one; the `IdempotencyCache` (persisted in the outbox table, ADR-008 / ADR-014) returns the original response for any repeated key. See ADR-008.

## fork-as-update-gate

Pattern for vendor-pinning upstream dependencies that we don't fully control. Both the A2A spec and `a2a-rs` SDK live as Gitea mirrors of their GitHub origins; Cargo manifests pin against Gitea-tagged revisions. Nothing from upstream enters our build until a deliberate sync + tag bump. See [contracts/upstream-sync.md](contracts/upstream-sync.md).

## outbox pattern

Durability mechanism for at-least-once event delivery. Inbound state changes write the new state + the pending outbound events to a single SQLite transaction (`outbox` table); a background worker drains the outbox and publishes. Survives crashes mid-publish. Implementation per ADR-014; the outbox also backs the [idempotency cache](#idempotency-key) (ADR-008).

## operator surface

The set of endpoints, CLI verbs, and dashboard panels that an operator uses to manage the fleet — distinct from the **A2A per-instance surface** that an external orchestrator uses to drive a single agent. ADR-022 calls this **Surface 1 (admin)** and explicitly says it is NOT A2A.

## per-instance routing

Routing pattern where every agent gets its own URL prefix (`/agents/{instance_id}/*`) and its own AgentCard. Lets a single management server host many distinct A2A endpoints, one per agent, while keeping the orchestration plane separate (per ADR-022). The A2A spec assumes one agent per origin; we satisfy that by namespacing.

## Surface 1 / 2 / 3

ADR-022's three-surface architecture:

- **Surface 1 — Admin** (`/api/v2/admin/*`): operator-facing, NOT A2A. Lifecycle, fleet inventory, secrets rotation.
- **Surface 2 — A2A per-instance** (`/agents/{instance_id}/*`): orchestrator-facing, full A2A. AgentCards, tasks, message/send, subscribe.
- **Surface 3 — Observability** (`/metrics`, `/api/v1/events`): read-only telemetry. Prometheus exposition + buffered event snapshot for the dashboard.

Each surface has distinct routing, auth, and schema. See [concepts.md](concepts.md) §Three Surfaces.

## input-required

One of the eight A2A task lifecycle states. The agent has paused awaiting human input (HITL). Resumes when the orchestrator sends a follow-up `message/send` with the supplied input, transitioning back to `working`. See [concepts.md](concepts.md) §Task Lifecycle.

## Sunset header

HTTP `Sunset:` header (RFC 8594) emitted on every response from a deprecated v1 endpoint, carrying the planned removal date. Paired with `Deprecated:` and a `Link:` header pointing to the v2 equivalent. Default removal date is configurable per deployment via `AIWG_V1_SUNSET_DATE`. See [v2-migration-guide.md](v2-migration-guide.md).
