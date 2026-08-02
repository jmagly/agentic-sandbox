# Security screening: continuous operational validation

Date: 2026-08-01  
Classification: Internal  
Status: Proposed

## Scope

This screen covers the passive sampler, evidence ledger/evaluator, low-rate
known-signal canary, bounded fault-drill scheduler, and activity-store footprint
change. It inherits the metadata/content boundary and integrity rules from
ADR-034 and ADR-035.

## Data handling

Routine evidence may contain identifiers, timestamps, counters, digests,
versions, objective results, and bounded error categories. It must not contain
raw prompts, terminal input/output, command bodies, environment values, file or
network payloads, authorization material, or restricted-content fields.

Artifacts use owner-only defaults and the repository's sanitizer before
publication. Tenant/host/instance/agent identifiers are internal operational
metadata and must be pseudonymized when a report leaves the authorized
environment.

## Trust boundaries

- A workload-reported signal is self-reported and cannot independently prove
  host or collector health.
- The management activity store is authoritative for commit/coverage state but
  cannot prove that a compromised upstream collector emitted every event.
- Host/process resource counters and external monitoring are independent
  supporting sources.
- A local hash chain detects modification only when a trusted copy of a prior
  digest exists. Independent verification requires the external checkpoint or
  signing boundary described by ADR-035.

## Drill controls

- Default deny: only named profiles and exact target classes are accepted.
- Dry-run/preflight is mandatory and emits the resolved target, action,
  hypothesis, maximum duration, blast radius, abort thresholds, rollback, and
  cleanup checks.
- Arbitrary commands, wildcard fleet scope, unbounded duration, and profiles
  without rollback are rejected.
- Initial execution targets disposable or explicitly designated validation
  environments. Promotion to a live environment requires a separate operator
  policy decision.
- Every start, abort, rollback, cleanup, and failure is recorded in the activity
  and governance audit planes.

## Storage-change controls

- New rows use a versioned representation; old text rows remain readable.
- Decompression has a strict decoded-size limit before schema parsing.
- Malformed, truncated, or unknown versions fail closed with a typed error.
- Compressed content is still subjected to the same ADR-034 schema and
  ADR-035 prohibited-field rules before write.
- No background rewrite of existing rows occurs during initial rollout.

## Threat disposition

| Threat | Disposition |
|---|---|
| Evidence forgery or deletion | Hash-linked records, input digests, verification, optional external checkpoint; residual local-admin risk is explicit. |
| Cross-tenant observation | Existing authenticated scope plus exact configured tenant allowlist; negative isolation tests. |
| Sensitive-content leakage | Metadata allowlist and recursive prohibited-field checks; sanitizer and artifact scan. |
| Canary amplification | Rate, outstanding, payload, and tenant bounds; independent disable switch. |
| Drill misuse | Named profiles, exact target allowlists, dry-run, duration/blast-radius caps, abort and rollback. |
| Decompression denial of service | Version/magic validation, compressed and decoded bounds, streaming limit, corruption tests. |
| False qualification | Three-state evaluator, actual elapsed-time gate, class separation, capability matrix, operator review. |

## Security acceptance

- Negative tests cover cross-tenant scope, prohibited fields, record mutation,
  canary amplification, unsafe drills, cleanup failure, and decompression bounds.
- Routine evidence is sanitized and contains no restricted content.
- The first live seven-day report is independently verified and manually
  reviewed before it becomes release-gating.
- Unsupported collectors remain visibly unqualified; this work does not bypass
  external platform approval requirements.

