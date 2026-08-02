# UC: Accumulate continuous operational validation evidence

Date: 2026-08-01  
Status: Proposed  
Parents: #661, #715

## Primary actor

An operator running persistent Agentic Sandbox agents under normal workloads.

## Goal

Accumulate trustworthy rolling reliability and performance evidence from normal
operation without paying for an artificial continuously busy agent, while using
small canaries and bounded drills to cover quiet periods and failure paths.

## Preconditions

- The activity API and loss-aware store from ADR-034 are enabled.
- The operator selects explicit tenants, hosts, instances, collectors, and
  observation sources.
- Evidence output is written to an operator-controlled directory.
- Any drill target and action are explicitly allowlisted.

## Main flow

1. The passive collector samples configured activity, coverage, loss, latency,
   resource, storage, and export signals without submitting tasks or changing
   agent lifecycle state.
2. It records build, configuration, runtime, collector, host, tenant, and input
   identities with the sample.
3. It classifies observations as `organic`, `canary`, or `drill`.
4. It appends an atomic daily record and links it to the preceding record by
   digest.
5. The evaluator computes rolling 1-hour, 24-hour, and 7-day results.
6. Each objective is reported as `pass`, `fail`, or `insufficient_evidence`,
   with coverage and gaps shown explicitly.
7. After seven actual consecutive days with complete required coverage, the
   operator can use the signed or hash-bound report as qualification evidence
   for #661 and #715.

## Alternative flows

### Quiet organic traffic

A low-rate canary emits a known metadata-only activity. It proves the selected
path is live but remains classified as canary evidence and does not inflate the
organic traffic rate.

### Collector or source is unavailable

The evaluator records the interruption and returns `insufficient_evidence` for
affected objectives. It never substitutes zero or success.

### Fault path needs validation

The operator runs a short allowlisted drill. Preflight shows targets, actions,
maximum duration, abort thresholds, rollback, and cleanup. Drill results are
classified separately from organic evidence.

### Evaluator is tested in CI

Tests may use accelerated clocks and generated fixtures to validate window and
failure semantics. Such output is marked synthetic and cannot satisfy the
seven-day wall-clock requirement.

## Postconditions

- Daily evidence is append-only in logical order and hash-linked.
- Reports distinguish source classes and identify unsupported or missing data.
- The normal observation path has not created agent tasks or altered lifecycle
  state.
- A drill leaves its target in the documented restored state or reports cleanup
  failure.

## Acceptance criteria

- AC-1: Passive collection creates no task, PTY, restart, or agent lifecycle
  mutation.
- AC-2: Records identify build/config/runtime/collector scope and input digests.
- AC-3: `organic`, `canary`, and `drill` are reported separately.
- AC-4: Missing required data yields `insufficient_evidence`, never zero/pass.
- AC-5: Rolling reports cover 1 hour, 24 hours, and 7 days and expose coverage,
  interruptions, gaps/loss, latency, resource use, storage, export, and cost.
- AC-6: Only seven actual consecutive days can satisfy duration qualification.
- AC-7: Canary rate is bounded and can be disabled independently.
- AC-8: Drills enforce allowlists, maximum duration, dry-run/preflight, abort
  thresholds, rollback, and cleanup verification.
- AC-9: The checked release-mode storage benchmark is no more than 1,024 bytes
  per activity event without dropping required ADR-034 fields.

## Non-goals

- Replacing the isolated capacity profile already tracked by #661.
- Treating canary or drill traffic as representative user demand.
- Claiming macOS Endpoint Security coverage without Apple's external approvals
  and a qualified host.
- Storing raw prompts, terminal bodies, environment values, or credentials in
  the routine evidence ledger.

