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

## Safety invariants

- Desired and observed state are separate.
- State revisions are monotonic within one workload identity.
- Dispatch idempotency keys are stable across retry and restart.
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
| `POST` | `/workloads/{child_id}/observations` | Atomically advance a workload observation by exactly one revision. Stale writers receive `409`. |
| `POST` | `/reconcile` | Classify expected children as `re-adopted`, `terminal`, `unknown`, `failed-or-aborted`, or `operator-review-required`. |

Mutating routes require the normal management admin principal. Records are
stored in the same SQLite database as the v2 executor task store, so admission,
idempotency replay, observations, inventory, and reconciliation survive a
management-server restart. The runtime rejects unknown top-level contract
fields and secret-bearing metadata. Credential and network *policy references*
remain allowed; credential values do not.

This projection does not make AIWG a runtime dependency. AIWG/Cockpit supplies
parent mission fan-out and aggregation; Agentic Sandbox implements a generalized
execution-substrate protocol that another orchestrator can call directly.

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
