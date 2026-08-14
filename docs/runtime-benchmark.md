# Runtime performance benchmark

Issue #660 requires one representative workload to be measured on the host,
Docker, QEMU/libvirt, and Cloud Hypervisor. The benchmark runner is deliberately
adapter-based: it sends the exact same Python program and workload parameters to
each runtime on stdin, while the adapter owns transport to the already selected
runtime target.

The configuration must define all four runtime names. A runtime that cannot run
must carry a specific `not_run_reason`; omitting it or silently substituting a
different runtime is an error. Every measured runtime requires at least three
samples. Adapter commands are argv arrays and are never evaluated by a shell.
Credential-bearing arguments are rejected, and retained evidence contains only
the adapter configuration digest rather than local paths or endpoint details.

Run from a clean checkout:

```bash
scripts/run-runtime-benchmark-local.sh \
  --output /path/to/runtime-benchmark-evidence
```

The local wrapper preflights Docker, libvirt/KVM, the project-pinned Cloud
Hypervisor installation, and at least 8 GiB of available memory. It creates one
uniquely named disposable container and two uniquely named 2-vCPU/2-GiB VMs,
then performs exact-name cleanup on success, failure, or interruption. It also
retains the generated adapter config alongside the evidence. Existing runtime
targets are never selected or modified.

To run the lower-level collector with a separately reviewed environment:

```bash
python3 scripts/benchmark-runtimes.py \
  --config /path/to/reviewed-runtime-benchmark.json \
  --output /path/to/runtime-benchmark-evidence
```

Each adapter command must invoke `python3 -` in its target runtime. The runner
embeds validated numeric workload parameters into the stdin program, so no
environment forwarding is required. Typical command shapes are:

```json
[
  {"name":"host","command":["python3","-"]},
  {"name":"docker","command":["docker","exec","-i","runtime-bench-docker","python3","-"]},
  {"name":"qemu-libvirt","command":["scripts/runtime-benchmark-ssh-adapter.sh","runtime-bench-qemu"]},
  {"name":"cloud-hypervisor","not_run_reason":"pinned VMM or compatible image unavailable on this host"}
]
```

Use an optional `prepare_command` that returns only after the target is ready to
capture provision-to-ready latency. Its matching `cleanup_command` is attempted
after sampling. When no prepare command exists, the report labels launch timing
as not separated; command overhead must not be presented as cold launch time.

The output directory contains:

- `raw.json`: sanitized environment metadata and all sample rows;
- `samples.csv`: the same sample rows for independent analysis;
- `summary.json`: p50/p95 aggregates and host-relative comparisons;
- `REPORT.md`: reviewable summary with explicit `NOT RUN` and evidence limits.

The workload measures deterministic CPU completion time and process CPU time,
process maximum RSS, sequential write/read throughput with an fsync latency,
and JSON/hash task throughput. Process RSS excludes Docker daemon and VMM
resident memory. A final RISK-004 decision therefore also needs runtime-level
memory/capacity evidence and true cold/warm launch measurements from reviewed
prepare adapters; the runner states these limits instead of inferring them.
