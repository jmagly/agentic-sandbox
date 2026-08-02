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

The report records CPU time, maximum resident memory, database/WAL bytes,
ingest p95, query p95, accepted security-event count, durable sequence, gaps,
durable losses, and signed-export count. A complete seven-day result must also
attach host-level I/O and collector-specific source-delivery latency metrics.

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
is not a seven-day report. The optimized 10,000-event burst used 6.63% of one
core, 97.8 MiB maximum resident memory, and 81.7 ms p95 durable batch ingest,
which meet the #707 burst CPU, host-memory, and durable-ingest budgets. SQLite
used 1,564.6 bytes/event, so the 1 KiB/event storage target is not met and has
not been silently revised. See
`evidence/activity-reliability-contract-715.json`.

Issue #715 must remain open until a real rolling operational window or controlled
campaign completes for every claimed production tier, missing failure paths are
covered by bounded synthetic evidence, and the resulting reports meet or
formally revise #707's CPU, memory, latency, spool, storage, and loss budgets.
