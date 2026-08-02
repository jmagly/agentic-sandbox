# ADR-035: Separate activity metadata from restricted content

Status: Accepted  
Date: 2026-08-01

## Context

Operational activity is useful for reliability and incident response, but raw
prompts, terminal input, environment values, files, network payloads, and
application headers can contain credentials or personal data. A single store and
authorization path would make routine timeline access equivalent to content
access. Workloads also cannot be trusted to anchor evidence about themselves.

## Decision

- The default activity store accepts `sensitivity=metadata` only and rejects
  prohibited field names recursively.
- Restricted content, when explicitly enabled for a case, is encrypted into a
  separate envelope and authorized by an actor-bound, exact-scope grant with an
  automatic expiry of no more than seven days.
- Default retention is 30 days for standard metadata, 90 days for security
  metadata, seven days for restricted content, and one day for ephemeral data.
- Expiring forensic holds can delay deletion only for their exact tenant and
  instance scope.
- Query, restricted read, export, hold, policy change, verification, redaction
  failure, deletion, and cryptographic erasure create governance audit records,
  including denied attempts.
- Bounded batches use HMAC-SHA256 authenticated Merkle roots. Checkpoints contain
  the batch/root binding and must be persisted by an external append-only or
  non-overwrite service outside workload control.

## Consequences

Metadata timelines stay broadly useful without implicitly authorizing content.
Case-scoped content acquisition has operational overhead and requires a separate
encryption-key lifecycle. HMAC manifests prove integrity to parties holding the
key; they do not provide public non-repudiation. External checkpoint durability
and non-overwrite guarantees depend on the configured anchor backend and must be
reported honestly.
