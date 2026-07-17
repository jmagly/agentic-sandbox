# ADR-030: Adopt Cloud Hypervisor as a Fast-Start / Fork VM Backend

## Status

Accepted

## Date

2026-07-17

## Context

Interactive agent workloads need a sandbox handed out in well under a second while keeping VM-grade
isolation, and we want to fork many sandboxes from one warm, agent-ready base. The current
QEMU/libvirt **q35 UEFI** backend cannot deliver either:

- Cold provision is seconds-to-minutes and a recurring source of flakiness (#614, #597).
- Memory snapshot/restore on q35 is **3.7–6.2 s** and cannot fork RAM — and **virtiofs (vhost-user)
  blocks `virsh save`/migrate entirely**, while UEFI/pflash + `memfd` block internal snapshots
  (spike #639, `docs/research/memory-snapshot-restore-spike.md`).

Three planning spikes framed the decision: memory snapshot (#639), sub-second start (#642), and this
adoption. A PoC (#644, `docs/research/cloud-hypervisor-poc-644.md`) measured **Cloud Hypervisor v53.0**
booting the actual agent base image:

| Phase | Cloud Hypervisor | QEMU/q35 (#639) |
|---|---|---|
| boot → running | **~0.5 s** | ~22 s |
| snapshot | **~0.06 s** (sparse, ≈ touched RAM) | 8–29 s |
| restore → resumed | **0.043 s** (userfaultfd ondemand) / **0.134 s** (copy) | 3.7–6.2 s |

Cloud Hypervisor also supports **virtiofs (`--fs`) + vsock (`--vsock`) + snapshot/restore +
userfaultfd on-demand restore** natively (confirmed from the v53.0 binary), so it does **not** have
Firecracker's disqualifying virtiofs gap.

### Requirements Driving This Decision

- **Preserve VM isolation** — this must not weaken the isolation boundary vs q35 (highest weight).
- **Sub-second hand-out + restore** — the explicit goal of #642/#639.
- **Fork-from-warm-base** — N COW children from one enrolled/warm base.
- **Keep the storage + transport model** — virtiofs (global/inbox) and vsock (ADR-023, CID registry)
  are load-bearing; any fast path must support both.
- **Additive, low-risk integration** — reuse the existing base image and the `platform.sh` backend
  abstraction; keep libvirt/q35 as the stable default.

### Options Evaluated

The current libvirt/q35 path **stays** as the default/stable backend; this decision picks the
**fast-start / fork** mechanism to add alongside it (the #642 option space).

| Option | Isolation | Speed/Fork | Reliability | Cost | Weighted Total |
|--------|-----------|-----------|-------------|------|----------------|
| **A: Cloud Hypervisor backend** | 5 | 5 | 4 | 3 | **4.60** |
| B: QEMU `microvm` machine type | 5 | 3 | 4 | 3 | 3.90 |
| C: Warm-pool only on existing q35 | 5 | 2 | 5 | 4 | 3.85 |
| D: Firecracker | 5 | 5 | 2* | 2 | *disqualified* |

Weights: Isolation 0.35, Speed/Fork 0.35, Reliability 0.20, Cost 0.10. (Re-weighted from ADR-001's
build-time weights because this decision is explicitly about start-time + fork **without** sacrificing
isolation; the slow, mature path is retained separately.)

*D (Firecracker) has **no virtiofs** — it cannot satisfy our global/inbox storage model, a hard
functional fail regardless of score. B (QEMU microvm) improves boot but still hits the virtiofs
migrate block and has no userfaultfd fork. C (warm pool) pre-boots to hide cold-provision cost but
gives no sub-second *restore* and no fork, at an idle-capacity cost.*

### Team / Integration Context

- The shell VM lifecycle is **already abstracted** (`images/qemu/lib/platform.sh` + `backends/*.sh`,
  10 dispatch ops; `backends/proxmox.sh` is a stub skeleton). A CH backend is a new
  `backends/cloud-hypervisor.sh` implementing the same contract.
- The Rust management server is **decoupled from the hypervisor for provisioning** — it execs
  `provision-vm.sh`, so the create path needs little Rust change. The virsh-bound *observation*
  code (`libvirt_events.rs`, `vm-event-bridge.rs`, `reap-e2e-vms.sh`, `get_vm_ip`/`vm_power_state`)
  needs CH equivalents.
- The base image (`ubuntu-server-24.04-agent.qcow2`) is hypervisor-agnostic; CH consumes qcow2
  directly (**standalone only** — it rejects backing chains, `MaxNestingDepthExceeded`).

## Decision

Adopt **Cloud Hypervisor** as an **additive** sandbox VM backend for the fast-start / fork path,
selected via the existing `AGENTIC_BACKEND` mechanism, implemented as `backends/cloud-hypervisor.sh`
behind the `platform.sh` contract. The libvirt/q35 backend remains the default and the stable fallback
(and covers GPU/features until CH parity lands). Cloud Hypervisor becomes the substrate for:

- **Fast hand-out / warm pool** — restore a pre-enrollment warm base per hand-out in ~0.1 s
  (extends #643 semantics onto CH).
- **Fork-from-warm-base** — userfaultfd on-demand restore + per-child COW disk + fresh vsock CID
  (graduates the remaining #644 measurement into implementation).
- **Checkpoint/restore** across host restarts.

Boot uses `rust-hypervisor-firmware` so the existing agent image boots directly. Secret handling
follows #639/#645: **snapshot a pre-enrollment clean base, inject identity on restore**
(fresh CID #595, ephemeral secret #619, mTLS #617); never persist a post-enrollment snapshot
unencrypted (`snapshot-seal.sh`).

This ADR **extends** ADR-001 (which chose QEMU over alternatives on a security-weighted matrix) —
Cloud Hypervisor is a KVM-based VMM in the same isolation class, added for a capability ADR-001 did
not need. It composes with ADR-023 (transport-per-runtime: CH presents the virtio-vsock device with
the allocated CID).

## Consequences

**Positive**
- Sub-second hand-out/restore (~30–140× faster than q35) and true fork-from-warm-base.
- Smaller device model → arguably a smaller attack surface than full QEMU q35, at equal isolation.
- Reuses the base image, loadouts, agentshare, vsock CID registry, and enrollment path.
- Additive: zero disruption to the existing q35 flow; libvirt stays the default.

**Negative / costs**
- New backend surface to build and maintain (`cloud-hypervisor.sh`), plus **explicit tap/bridge
  networking** (libvirt did this implicitly) and **CH-equivalent observation** (state/IP/events,
  destroy/reap) on the Rust + scripts side.
- **GPU** passthrough must be re-expressed as CH `--device` VFIO (vs libvirt `<hostdev>`), tracked
  with #641 — until then GPU workloads stay on the libvirt backend.
- Operational prereqs to bake in: `vm.unprivileged_userfaultfd=1` (or `CAP_SYS_PTRACE`), standalone
  (non-backing-chain) disks, pinned CH + firmware binaries.
- A second backend to test → CI needs an `AGENTIC_BACKEND=cloud-hypervisor` matrix leg and a CH-aware
  reap path (current e2e is virsh-bound).

**Neutral**
- Coexistence, not migration: q35 and CH run side-by-side, selected per workload. Not migrating the
  libvirt path or touching Proxmox (#119/#120).

## References

- Spikes: #639 (`docs/research/memory-snapshot-restore-spike.md`), #642, #644
  (`docs/research/cloud-hypervisor-poc-644.md`).
- Rollout plan: `docs/architecture/cloud-hypervisor-backend-plan.md`.
- Prior/related ADRs: ADR-001 (hybrid runtime), ADR-023 (transport-per-runtime security),
  ADR-026 (enrollment & secret retirement).
- Related issues: #643 (checkpoint/warm-pool), #645 (snapshot secret hygiene), #641 (GPU),
  #595 (vsock CID registry), #617/#619 (secrets), #622 (shared-kernel isolation baseline).
