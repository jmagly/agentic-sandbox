# Linux activity collector

The Linux collector normalizes host-side Linux Audit, eBPF process lifecycle,
cgroup v2 `memory.events`, pressure stall information (PSI), and allowlisted
journal observations into `activity.event/v1`. The collector module is
`agent-rs/src/linux_activity.rs`; durable delivery uses `ActivitySpool`.

## Security boundary

The source adapter belongs beside the sandbox supervisor, never inside a tenant
workload. Workloads are not granted `CAP_AUDIT_READ`, `CAP_BPF`, `CAP_PERFMON`,
host PID namespace access, journal group membership, kernel log access, or host
cgroup filesystem access. QEMU/KVM and Cloud Hypervisor guest collectors use
guest-local sources. Their events use `source.layer=guest`; host/runtime evidence
uses `host` or `runtime` and is never relabeled as guest evidence.

Before queueing, the normalizer replaces executable paths, cgroup names, argv,
Audit proctitle, raw journal messages, boot IDs, and restart reasons with SHA-256
digests. A conservative executable basename is the only path component retained.
The journal adapter accepts only configured units and stores a record digest,
priority, and unit. The management metadata endpoint independently rejects
secret-bearing field names.

## Source and runtime matrix

| Runtime | Process exec | Process exit | Resource/OOM | Kernel/journal | Source layer |
|---|---|---|---|---|---|
| QEMU/KVM Linux guest | Audit; optional eBPF | eBPF tracepoint; Audit exit is incomplete | guest cgroup v2 + PSI | guest allowlist | `guest` |
| Cloud Hypervisor Linux guest | Audit; optional eBPF | eBPF tracepoint; Audit exit is incomplete | guest cgroup v2 + PSI | guest allowlist | `guest` |
| Docker/Linux | host Audit attributed by cgroup; optional eBPF | eBPF tracepoint | host cgroup v2 + PSI | host allowlist | `host`/`runtime` |
| Native Linux | host Audit filtered by supervisor cgroup; optional eBPF | eBPF tracepoint | host cgroup v2 + PSI | host allowlist | `host` |

Audit `execve` (59) and `execveat` (322) records observe direct and non-shell
execution independently of PTY capture, shell history, and command wrappers.
PID reuse is disambiguated with the digest of boot ID, PID, and process start
time. Agent/instance/host/tenant and propagated command or trace identifiers are
carried by the stable event correlation envelope.

The eBPF adapter should use stable process tracepoints and cgroup attribution. It
is optional because supported kernel/BTF combinations and locked-down hosts vary;
Audit remains the baseline. File-open telemetry, full syscall streams, raw kernel
records, packet payloads, and non-allowlisted journal units are explicitly
unsupported in this milestone and produce `telemetry.unsupported` when requested.

## Loss, restart, and delivery

The in-memory normalization queue is bounded (4,096 by default). Capacity loss is
counted and emitted as `telemetry.loss` after space becomes available. Collector
restart is emitted as `telemetry.collector_restart`. OOM and `memory.events`
deltas, PSI totals/averages, source layer, collector identity, and monotonic spool
sequence make an outage or restart visible in the same activity timeline.

The durable JSONL spool uses fsync, a hard byte quota, idempotent replay, and a
persisted sequence watermark. Management acknowledgements remove only durably
accepted records. Operators must size the byte quota for at least 15 minutes at
their measured peak rate and alert before the quota is exhausted.

## Performance evidence

Run the repeatable normalizer benchmark with:

```sh
cd agent-rs
cargo run --release --example linux_activity_benchmark -- 100000
```

The original 1 KiB pre-compression planning target is revised to a 1.5 KiB hard
test ceiling for a fully correlated process event. The stable schema and four
scope identifiers account for much of the envelope; dropping them would weaken
tenant isolation and incident correlation. The benchmark reports normalization
throughput, mean event size, queue depth, and drops. CPU, RSS, ingest latency,
backlog recovery, and kernel-source loss remain environment measurements in the
#715 seven-day validation matrix; a microbenchmark is not presented as evidence
for those system-level budgets.

The checked-in result is
`docs/observability/evidence/linux-activity-collector-benchmark-711.json`.

## Verification

`cargo test --lib linux_activity` covers direct Audit exec, eBPF-style exit,
QEMU/KVM, Cloud Hypervisor, Docker, native-host attribution, guest/host source
separation, cgroup OOM, PSI, allowlisted journal handling, raw secret suppression,
queue loss, collector restart, UUIDv7, event size, and durable sequence recovery.
