# Fleet Workload Contract v1

Status: proposed compatibility contract
Issues: AIWG #1990; Agentic Sandbox #736
Schema identity: `urn:agentic-orchestration:fleet-workload:v1`

This contract is the neutral boundary between an orchestration management plane
and an execution substrate. AIWG/Cockpit is the first management-plane
implementation and Agentic Sandbox is the first execution-substrate
implementation. Neither product's internal mission or task model is normative.

## Documents

- `workload`: desired and observed state for one child execution.
- `inventory`: a revisioned snapshot of one or more workload records.
- `reconciliation`: before/after classifications after reconnect or restart.

## Workload kinds

`persistent-agent`, `daemon`, `scheduled-collector`, and `one-shot-command`
retain distinct semantics. Daemons require health. Scheduled collectors require
a schedule. Persistent agents may detach and retain identity. One-shots have
terminal exit classification, timeout, and cancellation semantics.

## Ownership

The orchestrator owns parent intent, policy, admission, placement, budgets,
aggregation, and operator decisions. The substrate owns runtime capability,
execution-local lifecycle, isolation, health, artifacts, and truthful observed
state. Parent fan-out is orchestrator-owned; explicitly delegated nested runtime
children still carry the same lineage.

Target, executor, and runtime identity are assigned before admission. Optional
`session_id`, `task_id`, and `command_id` fields bind the admitted workload to
the substrate resources created later. They are `null` while unassigned; once
non-null, an identity is stable for that workload and survives inventory and
reconciliation snapshots.

## Safety invariants

- Desired and observed state are separate.
- State revisions are monotonic within one workload identity.
- Dispatch idempotency keys are stable across retry and restart.
- Non-null session, task, and command identities are immutable within a child.
- Unknown reconciliation cannot be represented as terminal success.
- Unsupported, degraded, and policy-blocked controls are data, not log text.
- Credential material is not part of the schema. Only policy references cross
  the boundary.
- Orchestrator-specific metadata is optional and namespaced within
  `orchestrator_metadata`.

## Compatibility

AIWG `executor.v1` remains a supported singleton mission profile. Adapters map
its mission identity to one fleet workload child. Fleet-aware adapters
negotiate `agentic-orchestration/v1`; old adapters continue to operate without
claiming daemon, schedule, typed backpressure, or fleet reconciliation support.

Breaking changes require a new API version. Additive optional fields may be
introduced in a compatible revision after both repositories carry matching
fixtures and validation.

## Agentic Sandbox management projection

When the v2 executor surface is mounted, Agentic Sandbox exposes the neutral
contract under `/api/v2/fleet`:

| Method | Route | Semantics |
|---|---|---|
| `POST` | `/workloads` | Admit a pending revision-0 workload. Returns `202`; an identical durable idempotency replay returns `200`; a key/payload collision returns `422`. |
| `GET` | `/workloads` | Return a revisioned `inventory` document containing all durable workload records. |
| `GET` | `/workloads/{child_id}` | Return one durable workload record. |
| `POST` | `/workloads/{child_id}/observations` | Atomically advance a workload observation by exactly one revision and optionally bind newly assigned runtime identities. Stale writers or identity reassignment receive `409`. |
| `POST` | `/reconcile` | Classify expected children as `re-adopted`, `terminal`, `unknown`, `failed-or-aborted`, or `operator-review-required`. Returns `202` with a canonical `fleet.reconcile` operation, a pollable `Location`, and terminal classification evidence. |

Mutating routes require the normal management admin principal and accept an
optional `Idempotency-Key`. An exact replay returns the original response with
`Idempotency-Replayed: true`; key collisions and concurrent duplicates fail
closed with `409`. Records are
stored in the same SQLite database as the v2 executor task store, so admission,
idempotency replay, observations, inventory, and reconciliation survive a
management-server restart. The runtime rejects unknown top-level contract
fields and secret-bearing metadata. Credential and network *policy references*
remain allowed; credential values do not.

An observation request has this transport envelope:

```json
{
  "expected_revision": 1,
  "runtime_identity": {
    "session_id": "session-1",
    "task_id": "task-1",
    "command_id": "command-1"
  },
  "status": {
    "observed_state": "running",
    "revision": 2,
    "last_seen": "2026-08-02T15:00:00Z",
    "artifacts": []
  }
}
```

`runtime_identity` is optional and may contain any non-empty subset of the
three bindings. A null admission binding can become non-null once. Re-reporting
the same value is idempotent; changing an assigned value fails with
`fleet.runtime_identity_immutable` and does not advance the observation.

When `task_id` names a task in Agentic Sandbox's durable A2A TaskStore, workload
GET and inventory reconciliation project that task's newer state and artifacts
into the fleet record with a monotonic revision. This keeps observed runtime
truth substrate-owned: AIWG binds the task but does not claim its completion.
Unchanged task projections do not churn the inventory revision. A completed
one-shot becomes `succeeded` with a result reference; an exited daemon becomes
`operator-review-required` unless independent health evidence says it remains
healthy.

This projection does not make AIWG a runtime dependency. AIWG/Cockpit supplies
parent mission fan-out and aggregation; Agentic Sandbox implements a generalized
execution-substrate protocol that another orchestrator can call directly.

`scripts/test-fleet-workload-live.sh` is the generalized executable proof. It
uses a non-AIWG orchestrator id and a real ephemeral management binary to admit
a child, dispatch and bind an A2A task, restart the server, retry admission with
a new transport timestamp, recover inventory, and classify the child as
`re-adopted` without duplicating its task identity.

## Agentic Sandbox CLI projection

The admin CLI exposes the same neutral surface without introducing an AIWG
dependency:

```text
sandboxctl fleet dispatch --file workload.json
sandboxctl fleet inventory [--watch 5s]
sandboxctl fleet get <child-id>
sandboxctl fleet reconcile --before-revision <n> \
  --child-id <expected-child> [--child-id <expected-child> ...]
```

`dispatch` sends the contract document unchanged. `inventory` and `get` are
read-only; only `inventory` is watchable. `reconcile` requires the caller's
previous inventory revision and an explicit expected-child set, preserving the
orchestrator/substrate ownership boundary. Authentication remains in the normal
HTTP authorization header and is never written into the workload record.
