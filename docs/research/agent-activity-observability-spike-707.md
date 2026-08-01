# Agent activity observability research spike (#707)

**Status:** Complete research recommendation with bounded proof of concept

**Baseline:** `origin/main` at `b429906` (2026-08-01)

**Scope:** QEMU/KVM, Cloud Hypervisor, Docker, and native-host runtimes

**Decision:** Adopt a correlated, metadata-first activity-event pipeline; keep
content capture opt-in, separately stored, and short-lived.

## Executive recommendation

Agentic Sandbox already records several valuable but independent streams:
management logs, lifecycle events, security audit records, PTY transcripts,
agentshare run files, agent/runtime metrics, CLI intent/outcome audit records,
and AIWG mission events. These streams cannot yet answer one simple forensic
question reliably: **what did this agent do, through which tool and process,
against which destination or resource, and what happened next?**

Build a normalized activity pipeline around four independent evidence planes:

1. **Semantic session evidence** from the control plane and provider adapter.
2. **Observed process/file evidence** from a guest or host OS collector.
3. **Observed network-flow evidence** at the sandbox runtime boundary.
4. **Runtime/system evidence** from cgroups, VMM/container events, journals,
   kernel logs, and pressure/OOM signals.

Every record carries stable sandbox correlation identifiers, source layer,
trust level, sensitivity, retention class, collector sequence, and two clocks.
No single evidence plane is treated as complete. Provider callbacks explain
intent, while runtime/OS collectors independently observe effects.

The default tier records metadata, not prompts, keystrokes, environment values,
file contents, packet payloads, or TLS plaintext. Full-content capture is an
explicit forensic mode with authorization, scope, expiration, encryption, and
an audit record. This preserves #620's guardrails while making activity
reconstruction possible.

The executable PoC is
[`scripts/observability/activity_timeline_poc.py`](../../scripts/observability/activity_timeline_poc.py).
Its checked-in fixture correlates one session, tool invocation, process exec,
network flow, resource sample, process exit, and collector-loss event. The
[rendered timeline](activity-observability-poc-timeline-707.md) shows both an
inferred sequence gap and explicit loss reporting.

## Scope and method

This spike combined source inspection of management, agent, CLI, runtime, API,
agentshare, and UI capture paths; review of existing security and observability
documentation; comparison against the primary sources in
[References](#references); and an executable, dependency-free event normalizer.

It did **not** install audit rules, load eBPF programs, enable packet capture,
change host networking, or deploy a production telemetry backend.

## Current-state inventory

| Surface | Current evidence and persistence | Useful coverage | Material gap |
| --- | --- | --- | --- |
| Management tracing | `management/src/telemetry/logging.rs` emits structured logs; `log_buffer.rs` retains 2,000 entries for `/api/v1/logs`; file output can rotate | HTTP/gRPC/WS operations and diagnostics | Ring eviction lacks per-record sequence; fields lack the full sandbox correlation set |
| Lifecycle event store | `management/src/http/events.rs` records VM, container, agent, command, PTY, and reconciliation events; 100 hot/source, then `events.jsonl`; SSE reports lag | Runtime and control-plane transitions; optional trace ID | No common event ID, source trust, sensitivity, tenant/session hierarchy, or collector sequence |
| AIWG executor events | `management/src/aiwg_serve/mod.rs` emits `mission.*` and executor events | Mission/session semantics | Provider/tool callbacks are not normalized with OS/runtime effects |
| PTY replay/transcript | `management/src/session/{registry,transcript,redaction}.rs` keeps bounded replay and spills evicted output/keyframes to private JSONL | Session output chronology and search | Content-sensitive; cannot prove non-PTY exec, process ancestry, network, or file activity |
| Agentshare run record | `agent-rs/src/main.rs` writes `stdout.log`, `stderr.log`, `commands.log`, `metadata.json`, and cgroup-aware `metrics.json`; private modes and seven-day pruning | Run dispatch, output, metadata, resource snapshot | Dispatch log is not every process/syscall; schemas and IDs differ from event/audit streams |
| Security audit | `management/src/audit/audit.rs` uses UUIDv7, sequence, actor/resource/outcome, optional trace ID, rate limiting, 90-day default retention, and previous-record hashes | Sensitive control actions, PTY/transcript and gateway access | No general process/file/network/kernel classes; a locally recomputable chain is not independently signed proof |
| CLI audit | `cli/src/audit/mod.rs` writes local intent/outcome pairs with verb, target, duration, and error | Operator actions through `sandboxctl` | Other clients and direct runtime actions bypass it; no server-issued correlation IDs |
| Metrics | Management Prometheus, agent textfile exporter, dashboard metrics, and agentshare snapshot | Service, command, session, container, cgroup CPU/memory, and storage health | Aggregates cannot reconstruct actions; no correlated OOM/kernel/process/network evidence |
| OpenTelemetry | `management/src/telemetry/otel.rs` has optional OTLP tracing | Standards-based export hook | Traces only; current wiring does not provide the unified activity model |
| Runtime lifecycle | libvirt callbacks, Cloud Hypervisor polling, Docker polling, and host-supervisor metadata | Start/stop/crash/readiness and inventory | Fidelity differs by backend; no guest process or network-flow observation |
| Dashboard/API | Metrics, logs, events, transcript, WebSocket/SSE, and v2 admin mirrors | Existing data is inspectable | No unified timeline, coverage/loss status, or content-authorization flow |

Strengths to preserve:

- Security audit already has ordered IDs, sequence, trace ID, retention, rate
  limiting, and hash linking.
- Event and PTY fan-out expose subscriber lag rather than silently claiming
  completeness.
- #620 made agent-readable metrics sandbox-scoped and established transcript
  permissions, quota-backed storage, and pruning.
- Container networking labels both the Linux default-deny and weaker macOS
  Docker Desktop compatibility posture.
- Prometheus exposes event eviction/archive failures and transcript errors.

## Gap matrix

| Evidence | QEMU/KVM | Cloud Hypervisor | Docker | Native host | Trust boundary / blind spot |
| --- | --- | --- | --- | --- | --- |
| Session/mission lifecycle | Partial | Partial | Partial | Partial | Control-plane semantics do not observe effects; provider/agent can omit tool activity |
| PTY stdout/stderr | Full for managed PTY | Full for managed PTY | Full through agent path | Full for managed backend | Direct exec, daemon children, detached processes, and non-PTY APIs are absent |
| Provider tool calls | None normalized | None normalized | None normalized | None normalized | Useful intent but self-reported and content-sensitive |
| OS command/process exec | None; dispatch log only | None; dispatch log only | None at kernel/runtime level | Supervisor root process only | Shell history misses direct `execve`, library spawns, and direct syscalls |
| Process exit/ancestry | None | None | None | Partial | PID reuse/namespaces require boot ID, start time, cgroup, and parent identity |
| File access/mutation | None | None | None | None | High volume; paths may be sensitive; reads are especially noisy |
| DNS/network flows | None | None | Policy labels only | None | TLS hides content; NAT and VM boundaries weaken process attribution |
| Packet/app payload | None by design | None by design | None by design | None by design | Credentials/PII risk; encryption must not be bypassed silently |
| Runtime lifecycle | Partial via libvirt | Partial via state polling | Partial via Docker polling | Partial via supervisor | Polling misses short transitions; no guest semantics |
| Resource metrics | Partial | Partial | Partial/cgroup-aware | Partial/platform-specific | Aggregation loses action causality |
| Kernel/journal/`dmesg` | None | None | None; container must not read host kernel log | None | Host kernel sees VMM/container effects, not guest-kernel details |
| OOM/pressure/security | None correlated | None correlated | None correlated | None correlated | Needs cgroup/instance mapping at event time |
| Loss/tamper | Partial ring/archive metrics and audit chain | Partial | Partial | Partial | No end-to-end sequence, clock error, signed batch, or unified coverage query |

## Runtime-specific collection map

### QEMU/KVM

- **Guest:** a least-privileged agent collector can read selected journal,
  Linux Audit, cgroup/process, and guest kernel/OOM records. eBPF is optional
  higher fidelity when the guest kernel and policy permit it.
- **Host/runtime:** libvirt callbacks, QEMU QMP, QEMU process cgroups, tap/vnet
  flow metadata, nftables decisions, and host pressure provide evidence
  independent of the guest.
- Host eBPF sees QEMU, not guest `execve`; guest and host evidence must remain
  distinct trust sources.

### Cloud Hypervisor

- Use the same guest collector profile as QEMU/KVM.
- Add Cloud Hypervisor API lifecycle/error signals plus process/cgroup/tap
  evidence. Retain state polling as reconciliation, not the sole event source.
- The host cannot infer guest processes from the VMM process alone.

### Docker

- On Linux, Docker Engine events cover lifecycle; cgroup IDs/namespaces let a
  host Audit/eBPF collector attribute process activity. Observe flows on the
  managed bridge/veth or egress gateway, not from inside the workload.
- Provider callbacks enrich host evidence but remain self-reported.
- Docker Desktop on macOS runs containers in Docker's Linux VM. A macOS host
  process collector cannot see container `execve`. Without a collector in that
  VM/environment, report process/flow attribution as unsupported.
- Never grant a workload host `dmesg`, audit, BPF, packet-capture, Docker-socket,
  host-PID, or journal access merely to improve telemetry.

### Native host

- **Linux:** filter Audit/eBPF/journal/cgroup/PSI/OOM and network evidence by the
  supervisor-owned process group/cgroup. Use a distinct collector identity.
- **macOS:** Linux Audit/eBPF/cgroup/`dmesg` techniques do not apply. Process and
  file events need Endpoint Security entitlement/approval; OS logs use Unified
  Logging; network observation needs separately authorized Network Extension or
  capture. Until deployed, claim supervisor/provider/session/process-group
  evidence only.
- Monitoring does not turn the least-isolated host runtime into a sandbox.

## Candidate technology comparison

| Mechanism | Evidence/fidelity | Privilege/portability | Cost | Limitation | Recommendation |
| --- | --- | --- | --- | --- | --- |
| Provider callbacks | High-semantic tool intent/result | Provider-specific, usually unprivileged | Low volume; content may be large | Self-reported, bypassable, secret risk | Ingest allowlisted metadata; never alone |
| PTY/shell hooks | Interactive content | Portable but shell/session-specific | Potentially huge | Misses direct exec/syscalls/detached work | Separate restricted-content stream |
| Linux Audit | Kernel-observed exec/file/security per rules | Linux, root-managed policy | High if broad; backlog can drop | Guest root can change guest policy; argv/path sensitivity | Audit-first Linux baseline with loss metrics |
| eBPF | Process/file/network/OOM/cgroup | Linux, privileged, kernel-dependent | Efficient when filtered; ring can drop | Collector expands host attack surface | Preferred high-fidelity option after audited PoC |
| procfs/cgroup v2/PSI | Resource/process snapshots | Linux, lower privilege when scoped | Low | Races, PID reuse, misses events | Always-on resource plane |
| journald/kernel log | Service/kernel/audit/OOM records | Linux/systemd, controlled access | Moderate/rate-limited | Guest/host scope and rate loss | Allowlist units/fields; preserve cursor/loss |
| Docker events | Daemon lifecycle | Docker API is highly privileged | Low | No guest semantics | Consume only through management daemon boundary |
| libvirt/QMP/CH API | VMM lifecycle/device/error | Host-only/backend-specific | Low | No guest semantics | Normalize with source sequence/reconcile state |
| conntrack/nftables/eBPF flow | DNS/5-tuple/bytes/policy | Linux runtime boundary | Moderate | NAT, encrypted DNS, attribution races | Default network plane; prefer enforced gateway decisions |
| Packet capture | Headers and optional payload | Elevated/platform-specific | Very high | Payload secrets/PII; TLS encrypted | Off by default; case-scoped C2 only |
| OpenTelemetry | Transport/batching/export model | Broad ecosystem | Configurable | Does not create missing kernel evidence or immutable audit | Export mapping, not source-of-truth schema |

## Proposed activity event contract

The machine-readable draft is
[`activity-event-v1.schema.json`](activity-event-v1.schema.json). It follows
CloudEvents-style named events, W3C trace correlation, and the OpenTelemetry log
model's timestamp/observed-time distinction, while adding sandbox trust,
sensitivity, retention, and integrity.

| Field | Rule |
| --- | --- |
| `schema_version` | `activity.event/v1`; major changes are breaking |
| `event_id` | Source-generated UUIDv7 |
| `event_name` / `plane` | Namespaced action and one of session/action/network/runtime/system/integrity |
| `occurred_at` / `observed_at` | Preserve source and collector-receipt RFC3339 clocks |
| `source` | Collector, guest/runtime/host/control-plane/provider layer, runtime, trust, optional clock error |
| `correlation` | Tenant, host, instance, agent plus optional session/mission/task/tool/command/process/trace/span |
| `sensitivity` | Metadata, restricted content, or secret-prohibited |
| `retention_class` | Standard, security, forensic hold, or ephemeral |
| `payload` | Event-specific allowlist after source-side filtering |
| `integrity` | Collector sequence and optional source/timeline hashes/signature |

```text
tenant_id
└── host_id
    └── instance_id (runtime + boot identity)
        └── agent_id
            └── session_id
                └── mission_id / task_id
                    └── tool_call_id
                        └── command_id
                            └── process_id = boot_id + pid + start_time
```

Trace/span context links requests but does not replace durable domain IDs. PID
alone is never a stable process identity.

### Event-class policy

| Event class | Source / trust | Required correlation | Retention | Known blind spot |
| --- | --- | --- | --- | --- |
| `session.*`, `mission.*`, `task.*` | Control plane / attested | tenant/host/instance/agent/session and mission/task | Security, 90d | Cannot prove OS effects |
| `agent.tool.*` | Provider / self-reported | session/tool call/trace-span | Standard 30d; restricted args ephemeral | Provider can omit/rename tools |
| `process.*` | Guest/host kernel collector / observed | instance/process; session/tool where propagated | Security, 90d | Guest source can be tampered; argv sensitive |
| `file.*` | Kernel collector / observed | instance/process | Metadata standard; forensic opt-in | Reads are high-volume; paths disclose data |
| `network.*` | Runtime/gateway / observed or attested | instance/process where reliable | Security, 90d | Encryption/NAT limit attribution/content |
| `runtime.lifecycle` | VMM/Docker/supervisor / attested | host/instance/agent | Security, 90d | No guest semantics |
| `runtime.resource.sample` | cgroup/VMM/runtime / observed | host/instance; process if scoped | Standard, 30d | Sampling misses spikes |
| `system.*` | Guest/host kernel/journal / observed | host/instance/cgroup/process | Security, 90d | Rate limits and distinct guest/host scopes |
| `telemetry.*` | Every collection stage / attested | host/instance/collector | Security, >=90d | Compromised collector may suppress its own loss |
| Content streams | Dedicated content store / source-specific | Full session/case and actor | Ephemeral 7d or hold | Redaction imperfect; encrypted traffic remains encrypted |

## Collection and data-flow architecture

```text
 provider/session          guest OS             runtime/host
 callbacks + PTY      audit/eBPF/journal     VMM/Docker/flow/cgroup
        │                    │                       │
        └──────────────┬─────┴───────────────────────┘
                       ▼
              per-source bounded spool
          sequence + drop counters + source time
                       │
                       ▼
         normalization and source-side redaction
     schema validation + correlation + sensitivity gate
                       │  mTLS / UDS / vsock
                       ▼
              control-plane ingest gateway
         tenant authorization + rate/backpressure gate
                  │                   │
                  ▼                   ▼
       metadata event journal    restricted content store
       hash-linked signed batch  separately encrypted/authorized
                  │                   │
                  └─────────┬─────────┘
                            ▼
              index/query/export + loss status
                   dashboard / API / SIEM
```

### Buffering, backpressure, and loss

- Each collector owns a bounded disk spool and monotonic sequence. It reports
  queue depth, oldest age, bytes, drops, restarts, and last acknowledged
  sequence.
- Metadata outranks content. Under pressure, stop/sample content first, then
  high-volume file reads, while retaining lifecycle, policy, security, and
  `telemetry.loss` records.
- The gateway acknowledges durable receipt. Retry is idempotent by `event_id`.
- Every query returns coverage, gaps, clock bounds, and collector health.
- A collector unable to persist loss state reconnects as degraded, never as
  complete.

Preserve `occurred_at` and `observed_at`, source boot ID, monotonic offset, NTP
state, and estimated clock error. Detect backward jumps as `telemetry.clock`.
Do not invent a total order across sources when error bounds overlap.

### Integrity

Hash linking detects missing or mutated normalized records after a known
checkpoint, but hashes alone are not tamper-proof: an attacker controlling the
file can recompute them. Production should hash-link per collector, close
bounded batches with sequence/count/root, sign roots with a key unavailable to
the workload, anchor signed manifests in append-only/object-lock storage, and
security-audit verification/export. The PoC labels its chain as
post-normalization integrity, not source authenticity.

## Fidelity tiers and safe defaults

| Tier | Default | Captures | Excludes | Use |
| --- | --- | --- | --- | --- |
| **M0 operational metadata** | On | Lifecycle, tool name, process identity/executable, flow/DNS/policy metadata, resources, kernel/OOM/security metadata, loss/clock/config | Prompts, argv content, keystrokes, environment, file/packet content | Routine monitoring and triage |
| **M1 enhanced forensic metadata** | Explicit profile | Allowlisted/hashed arguments, file mutation paths, denser process/network/resources | Raw credentials, unrestricted reads/content, TLS decryption | High-assurance readiness |
| **C2 restricted content** | Off | Exactly authorized PTY/prompt/file/packet streams | Unscoped host/tenant data and silent TLS interception | Time-bounded incident case |

M0 is the minimum activity-monitoring claim. A runtime lacking a collector must
report unsupported event classes instead of substituting PTY text.

## Privacy, retention, and access control

### Data classes

- **Metadata:** IDs/times/names, executable identity, allowlisted/hash arguments,
  5-tuples/bytes, resource values, policy outcome.
- **Restricted content:** PTY, prompt/response, full argv, sensitive paths, file
  excerpts, packet payload.
- **Secret-prohibited:** tokens, cookies, keys, credential files, authorization
  headers, and raw environment dumps. Reject/redact rather than retain.

| Class | Default retention | Storage | Access |
| --- | --- | --- | --- |
| Standard metadata | 30 days | Encrypted journal/index | Tenant-scoped operator/read-only roles |
| Security/loss/policy | 90 days | Encrypted append-only journal + signed manifests | Security operator; export audited |
| Restricted content | Seven days, matching agentshare | Separate encrypted store | Explicit `content.read`; every read audited |
| Forensic hold | Expiration required; 30-day review | Separate case key and immutable manifest | Named case members; optional dual-control export |

Deletion removes indexes, objects, and data keys. Flash, COW, or thin storage
deletion is not a physical-overwrite claim; cryptographic erasure requires
destroying a dedicated key.

Collection policy is per tenant/profile, and its digest appears in
`telemetry.config`. Content authorization names actor, reason/case, exact
sources, start, maximum duration, retention, and automatic expiration. Sandbox
users receive only their intended sandbox metadata; host, cross-tenant, and
security-control data stay control-plane side. Query/export/hold/policy change,
redaction failure, and deletion are audited.

## Threat model

| Threat | Example | Required control | Residual risk |
| --- | --- | --- | --- |
| Spoofing | Fake tool event or instance ID | Workload identity; collector-assigned source/instance; self-report label | Compromised guest collector can lie locally |
| Tampering | Edit spool/archive | Separate identity; sequence/hash; signed batches; remote anchor | Guest root may tamper before export |
| Repudiation | Deny query/export | Actor/outcome audit | Compromised control plane can affect data and audit without remote anchor |
| Disclosure | Secrets in argv/PTY/prompt/env/packet/path | Source allowlist, redaction, scanner, separate content store, encryption | Recognition is imperfect; minimize first |
| Denial of service | Flood stdout/file/flows | Quotas, priorities, sampling, disk spool, drop counters, rate limits | Flood can reduce low-priority visibility |
| Privilege escalation | Collector becomes host attack path | Minimal audited collector; no agent BPF/audit/Docker access; sandbox collector service | Host collector remains high value |
| Cross-tenant leak | Shared bridge/journal/index | Trusted tenant labeling, scoped authorization, cross-tenant tests | Early mapping error can misattribute |
| Evasion | Bypass shell, kill collector, skew clock | Independent host/runtime evidence, watchdog, boot/sequence/clock state | Guest semantics disappear if guest collector is owned |
| False chronology | Skew reverses exec/flow | Two clocks, monotonic source, error bounds, causal IDs | Cross-host order remains approximate |
| Content deception | Control bytes/fake log lines | Structured fields, escaping, raw-byte preservation, visible source/trust | Humans can over-trust untrusted content |

## Network monitoring decision

Default records are metadata: instance and reliable process/cgroup identity,
address family/protocol/5-tuple, DNS question/result from an enforced resolver,
first/last time, bytes/packets, TCP result, allow/deny and rule ID, gateway, TLS
observation, and `payload_captured=false`.

TLS, QUIC, encrypted DNS, tunnels, and multiplexing limit interpretation. The
design does not claim URLs or application actions inside encrypted flows. TLS
interception is a separate security architecture and not recommended here.
Prefer enforced egress-gateway decisions, supplemented by
conntrack/nftables/eBPF observation. Packet capture is C2-only with filter,
size, duration, case, and separate storage.

## Kernel and system coverage

Required Linux sources are selected Audit/eBPF process/security events;
allowlisted journald units/fields; separately labeled guest and host kernel
records including `dmesg`-equivalent messages; cgroup v2 CPU/memory/I/O/PID and
pressure; OOM kill, `memory.events`, PSI, seccomp/LSM/audit; VMM/container/
supervisor lifecycle; and collector failure/restart.

Do not give Docker workloads `CAP_SYSLOG`, audit control, BPF/perf, host PID,
host journal, or packet-capture privileges. Host collectors filter by cgroup/
namespace. VM guest and host kernel records remain separate because they
describe different kernels.

## Proof of concept

Files:

- [`activity_timeline_poc.py`](../../scripts/observability/activity_timeline_poc.py)
  — validator, redactor, correlator, gap detector, hash linker, renderer, and
  benchmark.
- [`activity-events.jsonl`](../../scripts/observability/fixtures/activity-events.jsonl)
  — seven cross-plane events.
- [`test_activity_timeline_poc.py`](../../scripts/observability/test_activity_timeline_poc.py)
  — dependency-free regression tests.
- [`activity-event-v1.schema.json`](activity-event-v1.schema.json) — proposed
  contract.
- [Generated timeline](activity-observability-poc-timeline-707.md) and
  [benchmark](activity-observability-poc-benchmark-707.json).

```bash
python3 -m unittest scripts/observability/test_activity_timeline_poc.py -v

python3 scripts/observability/activity_timeline_poc.py timeline \
  scripts/observability/fixtures/activity-events.jsonl \
  --output /tmp/activity-timeline.md

python3 scripts/observability/activity_timeline_poc.py benchmark \
  --event-counts 100,1000,10000 --repetitions 3 \
  --output /tmp/activity-benchmark.json
```

The PoC proves shared correlation across the four planes, visible source/trust,
pre-persistence redaction, sequence-gap and explicit-loss reporting, and a
mutation-sensitive normalized hash chain. It does not prove privileged
collection overhead, source completeness/authenticity, remote durable storage,
multi-tenant authorization, content visibility, or API stability.

## Benchmark and production budgets

The checked-in run used Linux, Python 3.12.3, and 20 visible CPUs. It includes
copying, redaction, timestamp parsing, sorting, gap detection, canonical hashing,
serialization, and `tracemalloc`; it excludes collection, network, compression,
remote storage, indexing, and query.

| Events | Wall p50/p95 | CPU p50 | Throughput p50 | Max heap | Serialized input/output | I/O expansion |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | 0.024s/0.026s | 0.024s | 4,125/s | 0.14 MB | 62/80 KB | 1.28x |
| 1,000 | 0.244s/0.246s | 0.244s | 4,102/s | 1.37 MB | 626/795 KB | 1.27x |
| 10,000 | 2.549s/2.574s | 2.549s | 3,922/s | 13.35 MB | 6.28/7.97 MB | 1.27x |

The input/output columns measure serialized I/O volume, not disk or network
device latency. Normalization expands the synthetic input about 1.27x because
it adds derived integrity fields and timeline metadata. The offline Python
validator is not a hot-path design. At roughly 0.8 KB/event of stored output,
10 events/s is about 0.69 GB/day before compression/indexes; 100 events/s is
about 6.9 GB/day. Broad file/packet collection can be far higher. Collector,
spool, disk, network, and index I/O remain explicit full-pipeline measurements
for #715; presenting the in-memory PoC as those results would be misleading.

| Dimension | M0 target | Burst/failure requirement |
| --- | --- | --- |
| Collector CPU | <=2% of one core/sandbox at 100 events/s | <=10% at 1,000 events/s for 60s |
| Memory | <=64 MiB guest; <=128 MiB host service | Bounded during exporter outage |
| Action latency | p95 added exec/network latency <=2 ms | Telemetry never deadlocks workload |
| Ingest latency | p95 durable receipt <=2s | Recover five-minute backlog within 10 min |
| Loss | Zero lifecycle/security/policy loss at 100/s | Every lower-priority drop reports count/range |
| Spool | >=15 min configured M0 peak, hard quota | Content sheds before metadata |
| Storage | <=1 KiB/event pre-compression M0 target | Measure index/cardinality separately |

These are follow-up acceptance targets, not PoC results.

## Operator queries

Illustrative future syntax:

```text
activity timeline --instance instance-demo --session session-demo
activity query --tool-call tool-demo \
  --event agent.tool.invoked,process.exec,network.flow,process.exited
activity query --mission mission-demo --outcome denied,degraded,failure
activity coverage --instance instance-demo --from 2026-08-01T12:00:00Z
activity query --instance instance-demo --event system.oom,system.kernel
activity content read --session session-demo --case IR-123 --reason "approved triage"
```

Workflow: check coverage/clock bounds; start from control-plane IDs; compare
self-reported tool intent with observed effects; inspect runtime/OOM/policy/
kernel records; escalate to authorized content only if metadata is insufficient;
export a signed manifest containing query, actor, IDs, roots, loss, and clocks.

## Phased implementation plan

| Phase | Deliverable | Dependencies | Effort | Exit evidence |
| --- | --- | --- | ---: | --- |
| 0 | Contract, trust taxonomy, policy schema, fixtures | ADR-012/current IDs | 1-2 weeks | Versioned schema and compatibility tests |
| 1 | Ingest, bounded spool, durable ACK, loss/clock, query API | Phase 0 | 2-3 weeks | Restart/outage/loss E2E all runtimes |
| 2 | Linux process/kernel/resource collector; Audit baseline and eBPF evaluation | 0-1 + security review | 2-4 weeks | VM/CH/Docker/native Linux matrix |
| 3 | DNS/flow/policy collector | 0-1 + network policy source | 2-3 weeks | Attribution/encryption/NAT/drop/volume tests |
| 4 | Redaction/RBAC, content store, signing, retention/hold/export | 0-1 | 2-3 weeks | Governance tests and threat review |
| 5 | Timeline/coverage UI and SIEM/OTLP export | 1-4 | 2-3 weeks | Example investigations without raw content |
| 6 | macOS Endpoint Security/Unified Logging/Network Extension parity | Stable contract + entitlement plan | 3-5 weeks | Explicit support matrix and signed collector |
| 7 | Performance, long-duration, outage, flood, tamper, cross-tenant validation | All | 1-2 weeks + seven-day run | Budgets proven or revised |

## Decisions and rejected shortcuts

| Question | Decision |
| --- | --- |
| Canonical model | `activity.event/v1` with domain IDs plus W3C trace/span; not free-form logs |
| Evidence origin | Semantic intent at provider/control plane and effects at guest/runtime/host |
| Shell history/PTY | Supporting, incomplete, content-sensitive evidence only |
| Direct syscalls | Kernel/runtime observation on supported Linux; explicit unsupported status elsewhere |
| Network | Flow/DNS/policy metadata default; no TLS bypass/payload default |
| `dmesg` | Allowlisted guest and host kernel records, never host log access inside container |
| OpenTelemetry | Export/transport mapping, not source collection or immutable integrity |
| Hash chain | Useful only when signed/anchored outside attacker control |
| Environment/argv | Secret-prohibited by default; allowlist/digest then restricted escalation |
| Universal collector | Rejected; runtime/OS boundaries need different collectors/trust |
| Production deployment now | Rejected; #707 calls for research, bounded PoC, and plan |

## Follow-up issue slices

Duplicate detection found no equivalent open work. Implementation is split into:

1. [#710 — activity contract and loss-aware ingest/query foundation](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/710).
2. [#711 — Linux process/kernel/resource collectors](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/711).
3. [#712 — per-sandbox DNS/flow/policy metadata](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/712).
4. [#713 — governance, redaction, signed batches, retention, and forensic hold](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/713).
5. [#714 — correlated timeline, coverage API/UI, and export](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/714).
6. [#715 — full-pipeline performance, overload, tamper, and tenant-isolation tests](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/715).
7. [#716 — macOS Endpoint Security and Unified Logging parity](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/716).

## Completion checklist

- [x] Current management, agent, runtime, API, agentshare, and UI inventory.
- [x] Runtime/evidence gap matrix and trust boundaries.
- [x] Technology comparison with fidelity, portability, privilege, cost, security.
- [x] Normalized schema and correlation hierarchy.
- [x] Data flow, buffering, backpressure, loss, storage, index, export.
- [x] Threat model and governance policy.
- [x] Safe fidelity/retention/redaction/access defaults.
- [x] Cross-plane, loss-aware PoC and mock timeline.
- [x] Repeatable benchmark and production budgets.
- [x] Phased plan, dependencies, risks, effort.
- [x] Operator queries and assessment procedure.
- [x] QEMU/KVM, Cloud Hypervisor, Docker, Linux/macOS host limits.
- [x] Metadata vs restricted content separation.
- [x] No reliance on shell history, PTY, or self-report alone.
- [x] Event source/trust/correlation/retention/blind spots.
- [x] Encrypted-network and payload boundary.
- [x] Kernel, journal, `dmesg`, security, OOM/pressure, guest/host scope.
- [x] Follow-up issues filed and linked.
- [x] Local repository format, link, lint, script, and Rust test checks pass.
- Exact-main CI is delivery evidence recorded on #707, not a research artifact.

## References

Primary specifications and platform documentation:

- [OpenTelemetry Logs Data Model](https://opentelemetry.io/docs/specs/otel/logs/data-model/)
- [W3C Trace Context](https://www.w3.org/TR/trace-context/)
- [CloudEvents specification](https://github.com/cloudevents/spec/blob/main/cloudevents/spec.md)
- [Linux BPF ring buffer](https://docs.kernel.org/bpf/ringbuf.html)
- [Linux cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html)
- [`auditd(8)`](https://man7.org/linux/man-pages/man8/auditd.8.html) and [`audit.rules(7)`](https://man7.org/linux/man-pages/man7/audit.rules.7.html)
- [systemd journal fields](https://www.freedesktop.org/software/systemd/man/latest/systemd.journal-fields.html)
- [conntrack-tools manual](https://conntrack-tools.netfilter.org/manual.html)
- [Docker Engine system events](https://docs.docker.com/reference/api/engine/version/v1.51/#tag/System/operation/SystemEvents)
- [libvirt domain event API](https://libvirt.org/html/libvirt-libvirt-domain.html#virConnectDomainEventRegisterAny)
- [QEMU QMP reference](https://www.qemu.org/docs/master/interop/qemu-qmp-ref.html)
- [Cloud Hypervisor API](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/api.md)
- [Apple Endpoint Security](https://developer.apple.com/documentation/endpointsecurity), [Unified Logging](https://developer.apple.com/documentation/os/logging), and [Network Extension](https://developer.apple.com/documentation/networkextension)
- [NIST SP 800-92](https://csrc.nist.gov/pubs/sp/800/92/final)
- [OWASP Logging Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html)

Repository evidence:

- [`docs/telemetry.md`](../telemetry.md), [`docs/transport-audit.md`](../transport-audit.md), [`docs/agentshare.md`](../agentshare.md), and [`docs/pty-rendering.md`](../pty-rendering.md)
- [`docs/security/container-runtime-boundary.md`](../security/container-runtime-boundary.md)
- [`docs/runtimes/overview.md`](../runtimes/overview.md) and [`docs/runtimes/host-supervisor.md`](../runtimes/host-supervisor.md)
- [`management/src/http/events.rs`](../../management/src/http/events.rs), [`management/src/audit/audit.rs`](../../management/src/audit/audit.rs), and [`management/src/session/transcript.rs`](../../management/src/session/transcript.rs)
- [`agent-rs/src/main.rs`](../../agent-rs/src/main.rs)
