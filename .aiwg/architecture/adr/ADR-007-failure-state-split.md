# ADR-007: Split Terminal Failure States — `failed` vs `errored`

## Status

**Reframed under ADR-018 (A2A as base protocol).** A2A's `TaskState` enum is fixed; per A2A extension limitations, extensions cannot add new enum values. Our `failed` vs `errored` distinction therefore lives in `metadata.fail_kind: "application" | "infrastructure"` on the `TaskStatus.message.metadata` rather than as new enum values. The decision criteria in §"Implementation Notes" below remain valid; only the on-the-wire encoding changes. See `.aiwg/working/issue-planner/a2a-gap-matrix.md` row 1.

Original disposition: Proposed

## Date

2026-05-09

## Context

v1 mission state machine (`management/src/aiwg_serve.rs:354`) has terminal states `Completed | Failed | Aborted`. The orchestrator (AIWG) cannot tell from a `mission.failed` event whether the failure was:

- **Deterministic / application-level**: the agent ran successfully but produced a non-success outcome (compilation error in the agent's task, refused tool use, timeout from agent's own logic). Retrying the same dispatch will produce the same result. Should NOT retry.
- **Infrastructure / transient**: the executor crashed, the VM was OOM-killed, libvirt timed out, network glitched. Retrying may succeed. SHOULD retry.

Argo Workflows splits this exactly (`Failed` vs `Error`). AWS Step Functions splits `Failed` from `TimedOut`. The collapse in v1 forces the orchestrator to either always retry (wasteful, may DOS the executor) or never retry (loses recoverable missions).

## Decision

Split terminal failure into two states in v2:

- **`failed`** — deterministic, do-not-retry. The dispatch shape was sound but the work itself completed unsuccessfully. Examples: agent produced an error result, HITL response was "abort", timeout from agent's own deadline.
- **`errored`** — infrastructure / transient, retryable at orchestrator's discretion. Examples: executor process crashed, VM unresponsive, libvirt RPC timeout, OOM, disk full.

Both are terminal (no further events for that mission). Both have the same envelope. The distinction lives in the `state` field of `mission.failed` / `mission.errored` events.

The decision is the executor's: only the executor knows whether a failure was application or infrastructure. v2 spec gives normative guidance:

| Cause | State |
|---|---|
| Agent process exited non-zero with output | `failed` |
| Agent reached its own timeout | `failed` |
| HITL response was abort/deny | `failed` |
| Executor process crashed mid-mission | `errored` |
| VM/container OOM killed | `errored` |
| libvirt timeout > 30s | `errored` |
| Disk full during artifact collection | `errored` |
| Internal error (Rust panic, lock poisoning, etc.) | `errored` |

If the cause is genuinely ambiguous (rare), executors SHOULD prefer `errored` — orchestrators retry-budgeted retries are safer than missing recoverable failures.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Split `failed` vs `errored` (chosen)** | Standard pattern (Argo); orchestrator retry decision is unambiguous | Small additional spec surface |
| B. Add `retryable: bool` to `mission.failed` payload | Single state, single event | Boolean is less expressive than state; harder to extend later |
| C. Three-way split (Argo + Step Functions: `failed`, `errored`, `timed_out`) | Most expressive | Overkill; timeout origin (agent vs infra) maps cleanly into the binary split |
| D. Keep v1's collapsed `failed` | No change | Forces orchestrator to guess; the problem we're solving |

## Consequences

### Positive

- AIWG can implement retry-with-backoff on `errored` only, never on `failed` — cleaner orchestrator logic.
- Operator dashboards can distinguish "agent had a bad day" from "infra had a bad day" — different remediation paths.
- Aligns with Argo / Step Functions conventions; integrators familiar with those products have correct intuition.

### Negative

- Adds one event type and one state to the spec (minor).
- Existing v1 consumers must update — handled via deprecation window: v1 endpoints continue to emit `mission.failed` for both, v2 endpoints emit the split.
- Executor implementations must classify failures correctly; ambiguity risk addressed by "prefer `errored`" guidance.

### Neutral

- v1 `mission.failed` is preserved on legacy `/api/v1/...` paths during deprecation.

## Implementation Notes

- `MissionState::Failed` becomes `Failed { kind: FailKind }` where `FailKind = Application | Infrastructure`.
- Persisted form in `missions.json` adds `fail_kind` field; absent in v1 records (treat as `Application` for retroactive classification, since v1 retry was never automatic).
- Conformance harness verifies executor correctly classifies a deliberately-induced infrastructure failure (kill the executor mid-mission → expect `mission.errored`).

## Related

- Synthesis C6
- Best-practices research §2 (Argo, Step Functions)
- Vision §3.1 (sandbox owns contract, AIWG owns orchestration logic — orchestrator decides whether to retry)
