# Activity reliability and seven-day validation

Issue #715 has three complementary evidence layers:

1. Deterministic integration and chaos tests run on every repository test pass.
2. A passive rolling ledger samples normal persistent-agent operation and emits
   one-hour, 24-hour, and seven-day `pass`, `fail`, or
   `insufficient_evidence` results. It never creates agent work and only actual
   consecutive organic wall-clock samples count toward duration. See
   `operational-evidence.md`.
3. A resumable real-time campaign measures the complete ingest, durable store,
   coverage query, and signed-export path. Its default duration is exactly
   604,800 wall-clock seconds. Short contract runs are labeled and cannot count
   as seven-day evidence.

## Run the campaign

```bash
scripts/run-activity-reliability-campaign.sh \
  --output /var/lib/agentic-sandbox/evidence/activity-soak-7d.json \
  --state-dir /var/lib/agentic-sandbox/evidence/activity-soak-7d.state
```

Defaults implement #707's 100 events/second steady target and an initial
1,000 events/second, 60-second burst. The state directory contains SQLite
WAL state and an atomic checkpoint. Restarting the same command resumes at the
next collector sequence and remaining wall time; it neither replays the burst
nor relabels elapsed time. Run it on each supported runtime/collector tier and
retain the reports with host/build identity and platform source limitations.
The wrapper uses an optimized release build by default; `--debug-build` exists
only for fast contract testing and its measurements are not budget evidence.

The report records CPU time, maximum resident memory, main database and WAL
bytes before and after the final truncate-checkpoint, ingest/query p50, p95,
and p99, accepted security-event count, durable sequence, gaps, durable losses,
and signed-export count. A complete seven-day result must also attach
host-level I/O and collector-specific source-delivery latency metrics.

## Stored-envelope compatibility

New rows retain the indexed tenant, host, instance, agent, collector, sequence,
and event-time columns, while `event_json` is a versioned compressed blob. Its
header is `ASEV`, one version byte (`1`), and a four-byte big-endian decoded
length followed by a zlib stream. The decoded JSON is capped at 1 MiB. A read
requires the declared length, a complete checksum-bearing stream with no
trailing data, valid JSON/schema data, and the same scope/security validation
as ingest. Unknown versions, malformed/truncated streams, and oversized output
are typed corruption failures rather than partial results.

SQLite's dynamic typing lets the upgraded reader accept legacy text JSON and
version-1 blobs in the same table. Ingest validates first and compresses only
after validation. It does not rewrite legacy rows, and duplicate replay compares
decoded events so representation does not change idempotency. A bounded 5,000
entry decoded-event cache stores both the exact persisted representation and
the decoded event; any stored-byte change invalidates the entry and is decoded
again, so the cache cannot conceal corruption.

Deploy compatible readers before enabling upgraded writers. An older binary
cannot read new blob rows. For rollback, stop writers and either restore the
pre-deployment database snapshot or use the upgraded reader to export/reseed a
legacy store; do not start an old binary against a mixed database. Forward
rollback is safe because legacy rows remain untouched.

## Deterministic test matrix

| Failure or threat | Automated evidence |
| --- | --- |
| Steady lifecycle/security stream | 1,000 sequential security events, zero gaps/loss, signed export |
| Burst and query pressure | Real-time campaign accepts batches up to 1,000/s and measures ingest/query p95 |
| Duplicate delivery and reconnect | Idempotent duplicate acknowledgement and checkpointed sequence resume |
| Ring/spool quota and sink outage | Agent spool/collector hard-quota, bounded-queue, restart/replay tests in `agent-rs` |
| Lower-priority loss | Missing sequence remains below durable ACK and creates durable gap plus `telemetry.loss` count/range |
| Collector/clock/control-plane restart | Coverage tests expose restart, stale collector, clock error, and durable database reopen |
| Tenant/source spoofing | Authenticated ingest scope rejects mismatched hierarchy before transaction |
| PID/cgroup reuse and NAT | Tenant-scoped correlation retains process generation and `nat_observed` metadata |
| Secret sentinel | Prohibited metadata field is rejected; sentinel is absent from query and signed export |
| Hostile rendering | Activity UI uses text nodes; hostile-markup browser fixture remains in the #714 suite |
| Manifest mutation/removal/stale key | Anchored Merkle/HMAC verification fails for changed, missing, or wrong-key input |
| Query/index/export authorization | Authenticated operator and tenant-bound API/governance tests cover all three surfaces |

## Runtime matrix

| Tier | Campaign posture |
| --- | --- |
| QEMU/KVM and Cloud Hypervisor | Guest Linux collector plus runtime/network sources; run full campaign |
| Docker on Linux | Runtime/host Linux sources; run full campaign and retain NAT attribution evidence |
| Native Linux | Host-layer Linux sources; report host-global boundary and unsupported guest classes |
| Docker Desktop | Label Linux-VM sources `docker-desktop-linux-vm`; never claim native macOS container parity |
| Native macOS | Run metadata normalizer now; Endpoint Security source-delivery evidence requires full Xcode, Apple entitlement approval, signing/notarization, and host consent |

## Evidence status

The checked-in contract evidence proves campaign execution, resource-field
collection, durable resume, and no sequence loss over a short real-time run. It
is not a seven-day report. The optimized 10,000-event burst used 8.85% of one
core, 98.5 MiB maximum resident memory, 111.6 ms p95 durable batch ingest, and
21.3 ms p95 query latency. After recording the 13.7 MiB live database/WAL peak,
the campaign performed a real truncate-checkpoint and measured 926.9 bytes per
event in 8.84 MiB of durable database plus shared-memory state. CPU, memory,
ingest latency, and the unchanged 1 KiB/event target all pass. See
`evidence/activity-reliability-contract-715.json`.

Issue #715 must remain open until a real rolling operational window or controlled
campaign completes for every claimed production tier, missing failure paths are
covered by bounded synthetic evidence, and the resulting reports meet or
formally revise #707's CPU, memory, latency, spool, storage, and loss budgets.
