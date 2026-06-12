# Core Concepts

This is the recommended first read for anyone new to agentic-sandbox. It defines the four ideas that the rest of the documentation assumes, in the order you'll trip over them: the **naming model**, the **task lifecycle**, the **three surfaces**, and the **fork-as-update-gate** pattern for upstream alignment.

If you only want term-level definitions, see the [glossary](glossary.md).

---

## 1. Naming: Sessions vs Tasks vs Runs vs Missions vs Agents vs Instances

These terms get conflated. They are not the same thing. Reading them as synonyms will lead you astray.

| Term         | What it is                                                                                                                          | Lifetime                          | Identifier            |
|--------------|--------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------|-----------------------|
| **Instance** | A hosting substrate — a running VM or container. Created by `provision-vm.sh` or the container runtime.                              | Hours to weeks                    | `agent-01`, container ID |
| **Agent**    | The `agent-client` process *inside* an instance. There is exactly one agent per instance; the agent owns the gRPC connection back to management. | Bound to the instance              | `AGENT_ID` (matches instance name) |
| **Session**  | A live PTY (or output stream) attached to a process running on an agent. An agent may have many sessions concurrently. Has a controller + observers. | Until the underlying process exits or the controller disconnects | `session_id` (UUID) |
| **Task**     | A unit of A2A-protocol work dispatched to an instance. Eight-state lifecycle (see below). Carried over the `runtime/v1` extension so consumers know which instance handled it. | Until terminal state (completed / failed / canceled / rejected) | `task_id` (UUID) |
| **Run**     | A specific execution attempt of a task. Tasks may have multiple runs (retry, resume after pause). Surfaces in the v1 task API; the v2 API uses `Task` + state transitions instead. | One physical execution               | `run_id` |
| **Mission**  | Legacy v1 term for the work envelope dispatched by `aiwg serve`. Under v2/A2A this is just a Task; the executor contract still uses "mission" in older code paths. | Same as task                       | `mission_id` (legacy) |

The mapping that matters:

```
External orchestrator (aiwg serve)
  └── dispatches Mission/Task to
      └── Executor (one management server instance)
          └── routes to one of many
              └── Instance (VM or container)
                  └── hosting exactly one
                      └── Agent (agent-client process)
                          └── running one or more
                              └── Sessions (PTYs attached to processes)
```

When you see "agent" in the dashboard or `sandboxctl agent list`, it's effectively the instance — the two have a 1:1 relationship in practice. When you see "agent" in an A2A AgentCard URL (`/agents/{instance_id}/...`), it's the per-instance A2A endpoint, also 1:1 with the instance.

---

## 2. A2A Task Lifecycle

The v2 executor contract uses A2A's task state machine verbatim. Eight states; not all transitions are reachable from every state.

| State              | Meaning                                                                                                | Operator term       |
|--------------------|--------------------------------------------------------------------------------------------------------|---------------------|
| `submitted`        | Task accepted, queued, not yet started.                                                                | Queued              |
| `working`          | Agent is actively executing.                                                                            | Running             |
| `input-required`   | Paused awaiting human input (HITL).                                                                     | Awaiting human      |
| `completed`        | Terminal success.                                                                                       | Done                |
| `failed`           | Terminal failure (agent error, infrastructure error, unrecoverable exception).                          | Failed              |
| `canceled`         | Terminal: client called `cancel`.                                                                       | Canceled            |
| `rejected`         | Terminal: server refused at submit time (auth, schema, capacity). The task never ran.                   | Rejected            |
| `auth-required`    | Paused awaiting auth completion (rare; OAuth re-prompt mid-task).                                       | Auth needed         |

Typical paths:

- Happy: `submitted → working → completed`
- HITL: `submitted → working → input-required → working → completed`
- Cancel mid-run: `submitted → working → canceled`
- Bad submit: `submitted → rejected` (never enters `working`)

The state is queryable on `GET /agents/{instance_id}/tasks/{task_id}` (Surface 2). Operators see the rolled-up view on Surface 1 (`/api/v2/admin/missions`).

The legacy v1 task API (gRPC + `/api/v1/tasks/*`) has its own simpler state machine; the v2 migration is documented in [`v2-migration-guide.md`](v2-migration-guide.md).

---

## 3. Three Surfaces (ADR-022)

[ADR-022](https://github.com/jmagly/agentic-sandbox/blob/main/.aiwg/architecture/adr/ADR-022-three-surface-architecture.md) splits the executor into three surfaces with distinct routing, auth, and schemas. **Pick the right one when you wire a client.**

### Surface 1 — Admin (NOT A2A)

- **Routes**: `/api/v2/admin/*`
- **Vocabulary**: Mission, instance, secret, fleet, loadout, runtime backend
- **Auth**: Operator credentials (bearer token from the CLI context, dashboard session)
- **Consumers**: The dashboard, `sandboxctl`, internal automation
- **Why not A2A?** A2A is an agent-to-agent contract. Provisioning a VM is not an agent talking to another agent — it's an operator talking to the platform. Putting fleet management under A2A would force orchestration concerns into a protocol that doesn't want them.

### Surface 2 — A2A Per-Instance

- **Routes**: `/agents/{instance_id}/*` (one URL prefix per instance)
- **Vocabulary**: Pure A2A — AgentCard, Task, Message, Artifact, push notifications
- **Auth**: Per-agent auth declared in the AgentCard (the per-instance route is its own A2A origin)
- **Consumers**: External orchestrators (`aiwg serve`), other A2A-speaking agents
- **AgentCard URL**: `/agents/{instance_id}/.well-known/agent-card.json` — one card per instance, signed (JCS + JWS / Ed25519)

### Surface 3 — Observability

- **Routes**: `/metrics` (Prometheus), `/api/v1/events` (buffered event snapshot)
- **Vocabulary**: Metric names, event types
- **Auth**: Read-only, may be unauthenticated on a private network; gated by reverse proxy in production
- **Consumers**: Prometheus, Grafana, the dashboard's events panel, `sandboxctl event tail`

These don't overlap. An A2A client never reaches the admin surface; an operator never goes through the per-instance A2A endpoint for fleet ops; a Prometheus scraper never touches the task API.

---

## 4. Fork-as-Update-Gate (Upstream Sync)

We depend on two upstreams we don't control:

| Upstream (GitHub) | Mirror (Gitea)     | Role                                             |
|-------------------|--------------------|--------------------------------------------------|
| `jmagly/A2A`      | `roctinam/A2A`     | The A2A protocol specification                   |
| `jmagly/a2a-rs`   | `roctinam/a2a-rs`  | Rust SDK; Cargo wire dependency (ADR-021)        |

**The rule**: nothing from upstream enters our build until we deliberately bump. Cargo pins against a tagged Gitea revision of `roctinam/a2a-rs`. Spec changes are reviewed in the Gitea mirror's history before any contract artifact lands in this repo.

**Cadence**:

- **Monthly**: review upstream commits, decide whether to bump.
- **On-demand**: any upstream CVE or correctness fix touching our usage path is pulled within one business day.

**Sync procedure**: each local clone has two remotes — `origin` (GitHub upstream, read-only) and `gitea` (the mirror, we push). `git fetch origin && git push gitea --mirror`. The full procedure is in [`contracts/upstream-sync.md`](contracts/upstream-sync.md).

This pattern is referenced as **fork-as-update-gate** throughout the docs. It exists because we want the option to align tightly with A2A while controlling the timing of every breaking change ourselves.

---

## See Also

- [welcome.md](welcome.md) — entry point and quick links
- [glossary.md](glossary.md) — term-level definitions
- [ARCHITECTURE.md](ARCHITECTURE.md) — component-level view
- [platform-support.md](platform-support.md) — supported runtimes
- [v2-migration-guide.md](v2-migration-guide.md) — moving from v1 to v2/A2A
- [aiwg-executor.md](aiwg-executor.md) — executor contract details
- [contracts/upstream-sync.md](contracts/upstream-sync.md) — full sync procedure
- [.aiwg/architecture/adr/ADR-022-three-surface-architecture.md](https://github.com/jmagly/agentic-sandbox/blob/main/.aiwg/architecture/adr/ADR-022-three-surface-architecture.md) — ADR-022 in full
