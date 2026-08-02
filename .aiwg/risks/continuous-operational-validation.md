# Risk register: continuous operational validation

Date: 2026-08-01  
Status: Proposed

Likelihood and impact use Low (1), Medium (2), High (3); score is their product.

| ID | Risk | L | I | Score | Mitigation / acceptance evidence |
|---|---|---:|---:|---:|---|
| R-1 | Quiet traffic is mistaken for healthy complete coverage. | 2 | 3 | 6 | Required-source coverage is explicit; missing data is `insufficient_evidence`; bounded canary checks end-to-end liveness. |
| R-2 | Synthetic activity is reported as organic duration or load. | 2 | 3 | 6 | Immutable evidence-class field; separate rates and summaries; tests reject synthetic promotion. |
| R-3 | A local evidence producer rewrites history. | 2 | 3 | 6 | Atomic daily records, input digests, previous-record hash chain, optional external ADR-035 checkpoint, verification command. |
| R-4 | Canary cost or volume grows without bound. | 2 | 2 | 4 | Configuration caps rate and outstanding signals; default is low-rate; independent disable switch and budget report. |
| R-5 | A drill harms a live workload. | 2 | 3 | 6 | Named allowlisted profiles, dry-run/preflight, bounded targets and duration, abort thresholds, rollback, cleanup verification, default deny. |
| R-6 | Clock changes create false seven-day qualification. | 2 | 3 | 6 | Record wall and monotonic clocks; expose jumps; require 604,800 seconds of actual consecutive source coverage; clock anomaly tests. |
| R-7 | Cardinality or telemetry overhead destabilizes management. | 2 | 2 | 4 | Bounded labels, sampling interval floor, resource budgets, backpressure, and overhead benchmark. |
| R-8 | SQLite WAL growth hides or causes capacity failure. | 2 | 2 | 4 | Record database/WAL bytes and checkpoint state; include restart/checkpoint drill and alert thresholds. |
| R-9 | Compression breaks legacy rows or creates decompression abuse. | 2 | 3 | 6 | Versioned magic header, legacy text reads, maximum decoded size, corruption errors, migration and adversarial tests. |
| R-10 | Collected operational metadata contains sensitive content. | 2 | 3 | 6 | ADR-035 metadata-only allowlist, no prompt/transcript/environment bodies, recursive prohibited-key checks, sanitized artifacts. |
| R-11 | A seven-day report overclaims unsupported sources such as Apple Endpoint Security. | 2 | 3 | 6 | Capability matrix names supported/unsupported collectors; unsupported is insufficient, and Apple qualification remains externally gated. |
| R-12 | Normal-operation evidence is not reproducible enough for capacity comparison. | 2 | 2 | 4 | Bind build/config/runtime/input identities; preserve #661 isolated workload for controlled comparison; do not replace it. |

## Exit criteria

- All score-6 risks have automated negative-path tests and documented operator
  recovery.
- The first actual rolling seven-day report is manually reviewed before it can
  gate a release.
- The checked storage benchmark meets the unchanged 1,024-byte/event target.

