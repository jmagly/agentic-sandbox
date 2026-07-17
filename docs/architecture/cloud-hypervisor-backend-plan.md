# Cloud Hypervisor Backend — Implementation & Rollout Plan

**Decision:** ADR-030 (`.aiwg/architecture/adr/ADR-030-adopt-cloud-hypervisor-backend.md`, Accepted).
**Epic:** [#646](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/646).
**Basis:** spikes #639 (snapshot), #642 (sub-second start), #644 (CH PoC).
**Status:** Phase 0 implementation in progress. `backends/cloud-hypervisor.sh`,
`cloud-hypervisor-pins.json`, and `install-cloud-hypervisor.sh` now cover the initial backend,
host-prereq, standalone-disk, and explicit-tap plumbing for #647-#649. Scripts `checkpoint-vm.sh`
(#643) and `snapshot-seal.sh` (#645) already exist and feed Phase 2.

## Goal

Add **Cloud Hypervisor** as an additive VM backend so sandboxes can be handed out in **well under a
second** (warm-pool restore ~0.1 s, fork-from-warm-base) while keeping VM-grade isolation and the
existing storage (virtiofs) + transport (vsock) model. The libvirt/q35 backend remains the default and
the stable fallback; CH is opt-in via `AGENTIC_BACKEND=cloud-hypervisor`.

## Why this is low-risk to integrate

The VM lifecycle is **already abstracted in shell** — `images/qemu/lib/platform.sh` dispatches 10 ops
to `backends/<name>.sh` (`libvirt.sh` full; `proxmox.sh` a stub skeleton). The Rust management server
is **decoupled from the hypervisor for provisioning** — it execs `provision-vm.sh`. So the *create*
path is mostly a new backend script; the real work is (a) networking taps, (b) CH-equivalent
*observation*/teardown on the Rust + scripts side, (c) GPU translation, and (d) the fast-start/fork
payoff. See the #644 PoC doc for measured numbers and the machine-model constraints.

## Target architecture

```
mgmt server (Rust) ── execs ──▶ provision-vm.sh ──▶ platform.sh (AGENTIC_BACKEND)
                                                     ├─ backends/libvirt.sh        (default, q35 UEFI)
                                                     └─ backends/cloud-hypervisor.sh (NEW, fast path)
                                                            │
        cloud-hypervisor --kernel CLOUDHV.fd              │  virtiofsd per mount (--fs)
        --disk standalone.qcow2 --vsock cid=N --net tap ◀──┘  vsock cid from registry (#595)
        --api-socket (ch-remote: info/pause/snapshot/restore)
```

Key facts carried from the PoC (#644) and Phase 0 smoke testing:
- Boot via edk2 `CLOUDHV.fd` so the **existing agent qcow2 boots directly**. The
  Rust Hypervisor Firmware path is still supported as an override, but it failed
  the Ubuntu 24.04 LVM image smoke test by booting the kernel without the guest initrd.
- CH reads qcow2 **standalone only** (rejects backing chains → per-VM flatten/prepare step).
- `memory_restore_mode=ondemand` (userfaultfd) needs `vm.unprivileged_userfaultfd=1` or `CAP_SYS_PTRACE`.
- virtiofs is a per-mount vhost-user `virtiofsd` (CH doesn't spawn it implicitly like libvirt).
- Networking taps are explicit (libvirt did them implicitly).

## Work breakdown (→ issues)

| Phase | Item | Issue |
|---|---|---|
| 0 | `backends/cloud-hypervisor.sh` — 10 ops, firmware boot, `--fs`/`--vsock`/`--disk`, vsock gate | #647 (CH-1) |
| 0 | Host prereqs (pin CH+fw, userfaultfd sysctl), backend selection, standalone-disk prepare | #648 (CH-2) |
| 0 | Networking — explicit tap/bridge + reuse MAC/IP/DHCP model | #649 (CH-3) |
| 1 | Rust + scripts observation parity — state/IP/events, destroy/reap | #650 (CH-4) |
| 1 | e2e/CI matrix leg + loadout/agentshare parity | #651 (CH-9) |
| 2 | Snapshot/restore + warm pool (sub-second hand-out) — extends #643 | #652 (CH-5) — implemented by `images/qemu/ch-faststart.sh` + admin v2 CH endpoints |
| 2 | Fork-from-warm-base — ondemand + per-child COW; closes #644 fork item | #653 (CH-6) — implemented by `ch-faststart.sh fork` |
| 2 | Secret hygiene / enroll-on-restore — pre-enrollment clean base; consumes #645 | #654 (CH-7) — implemented by clean-base provenance + restore guards |
| 3 | GPU passthrough (VFIO `--device`) — with #641 | #655 (CH-8) |

**Sequencing:** Phase 0 (#647→#648→#649) yields a booting CH VM; Phase 1 (#650, #651) makes it
usable + tested at parity; Phase 2 (#652→#653, #654 in parallel) delivers the payoff the whole
adoption is for; Phase 3 (#655) reaches GPU parity. GPU workloads stay on libvirt until #655.

## Key technical decisions

1. **Additive backend, not migration.** q35 stays default; CH is selected per workload. Zero
   disruption to the current flow; instant rollback (switch `AGENTIC_BACKEND`).
2. **Firmware boot over direct-kernel** — reuses the agent image's own kernel/rootfs; no kernel
   extraction or module-version coupling.
3. **Per-VM standalone disk** (flatten/reflink), because CH rejects qcow2 backing chains. On a CoW
   filesystem, reflink copies keep this cheap; else `qemu-img convert`.
4. **Snapshot the pre-enrollment clean base; inject identity on restore** — same posture as #639/#645;
   makes warm-pool/fork residue-free and keeps secrets out of snapshot files.
5. **Expose fast-start through scripts and the management API.** `ch-faststart.sh` owns
   snapshot/restore/fork/warm-pool orchestration; admin v2 exposes async operation endpoints under
   `/api/v2/admin/cloud-hypervisor/...` and records operation results through the existing
   operation store.
6. **Reuse everything above the hypervisor** — base image, loadouts, agentshare, vsock CID registry,
   enrollment, health endpoints — unchanged.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Networking taps/DHCP diverge from libvirt behavior | Reuse `lib/network.sh` MAC/IP model; dnsmasq on the bridge; leak-check in reap (CH-4) |
| Observation gap (no libvirt event stream) | CH `ch-remote info` polling / libvirt-independent monitor feeding the same internal events (CH-4) |
| userfaultfd disabled on target hosts | sysctl drop-in in provisioning (CH-2); copy-mode restore (0.134 s) still sub-second as fallback |
| GPU parity lag | Keep GPU on libvirt until CH-8; document in `docs/runtime-parity.md` |
| Snapshot secret exposure | Pre-enrollment clean base + `snapshot-seal.sh` at rest (CH-7/#645) |
| CH/firmware supply chain | Pin + checksum binaries like `iso-pins.json` (CH-2) |

## Prerequisites to bake into provisioning

- `cloud-hypervisor`, `ch-remote`, `CLOUDHV.fd` — pinned + checksum-verified.
- `vm.unprivileged_userfaultfd=1` sysctl drop-in (or run CH with `CAP_SYS_PTRACE`).
- Standalone (non-backing-chain) per-VM disks.
- Per-VM tap on the sandbox bridge; per-VM API socket; fresh vsock CID per VM/child.

## References

- ADR-030 (decision) · ADR-001 (hybrid runtime) · ADR-023 (transport-per-runtime).
- `docs/research/cloud-hypervisor-poc-644.md` (measured PoC) · `docs/research/memory-snapshot-restore-spike.md` (q35 baseline).
- `images/qemu/lib/platform.sh`, `images/qemu/backends/` (backend contract).
- `images/qemu/checkpoint-vm.sh` (#643), `images/qemu/snapshot-seal.sh` (#645).
