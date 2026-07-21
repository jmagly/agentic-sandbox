# Cloud Hypervisor PoC — sub-second restore + fork-from-warm-base

**Spike:** [#644](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/644) — follow-up from
[#639](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/639) (memory snapshot spike) and
joint with [#642](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/642) (sub-second start).
**Date:** 2026-07-17 · **Host:** `grissom` (Linux/KVM, in `kvm` group).
**Type:** PoC / findings. Verdict: **GO** for the fast-start/fork path.

> All numbers are real, measured against **Cloud Hypervisor v53.0** (upstream static build) booting a
> **flattened copy of the actual agent base image** (`ubuntu-server-24.04-agent.qcow2`). This is the
> empirical counterpart to #639, whose QEMU/q35 baseline was 3.7–6.2 s restore.

---

## TL;DR

- **CH boots the agent base in ~0.5 s** and **restores a snapshot to a resumed VM in 43 ms (ondemand)
  / 134 ms (copy)** — i.e. **sub-second**, ~30–140× faster than the QEMU/q35 `virsh restore` path
  measured in #639 (3.7–6.2 s). This settles the core question #639 left open.
- **CH supports virtiofs (`--fs`) + vsock (`--vsock`) + snapshot/restore natively**, confirmed from
  the actual binary — so it does **not** have Firecracker's disqualifying virtiofs gap. It is the
  right microVM for our storage model.
- **Snapshot is near-instant (~0.15 s)** and **sparse (size ≈ touched RAM**, not the RAM allocation).
- **`memory_restore_mode=ondemand` (userfaultfd) works** once `vm.unprivileged_userfaultfd=1` (or CH
  runs with `CAP_SYS_PTRACE`); this is the lazy-demand-paging mode that enables cheap fork fan-out.
- **Recommendation: GO.** Adopt Cloud Hypervisor as the sandbox VMM for the fast-start / fork path,
  converging #639 and #642 on one mechanism. Fork fan-out (N COW children from one warm base) has all
  required primitives confirmed present; the multi-child measurement is the one remaining PoC item.

---

## 1. Capability matrix (empirical — from the v53.0 binary, not docs)

| Capability | Firecracker | **Cloud Hypervisor v53.0** | Evidence |
|---|---|---|---|
| **virtiofs** (our global/inbox mounts) | ✗ none | **✓ `--fs tag=…,socket=…`** | `cloud-hypervisor --help`: `--fs <fs>… virtio-fs parameters` |
| **vsock** (ADR-023 transport, #595) | ✓ | **✓ `--vsock cid=…,socket=…`** | `--vsock <vsock> Virtio VSOCK parameters` |
| **snapshot / restore** | ✓ | **✓ `ch-remote snapshot` / `--restore`** | `ch-remote` verbs `snapshot`, `restore`; `--restore source_url=…` |
| **userfaultfd on-demand restore** (fork enabler) | ✓ | **✓ `memory_restore_mode=ondemand`** | `--restore … memory_restore_mode=copy\|ondemand` — "ondemand enables lazy demand paging (needs userfaultfd)" |
| **qcow2 disk (direct)** | ✗ (raw only) | **✓ `image_type=qcow2`** (no backing chains) | `--disk … image_type=<raw,qcow2,vhd,vhdx>`; standalone qcow2 only (overlays → `MaxNestingDepthExceeded`) |
| **boot of existing Ubuntu image** | needs kernel+rootfs | ✓ via `hypervisor-fw` (rust-hypervisor-firmware) | booted the agent base with `--kernel hypervisor-fw` |

**Firecracker is disqualified by the virtiofs row.** Cloud Hypervisor satisfies every requirement.

---

## 2. Measured latency + size (agent base, 2 vCPU / 2048 MiB, `shared=on`)

| Phase | Cloud Hypervisor v53.0 | QEMU/q35 baseline (#639) |
|---|---|---|
| **Boot → VM Running** (firmware + qcow2) | **~0.5 s** | ~22 s to guest-agent (boot only) |
| **Snapshot** (pause → write) | **0.06 s**, sparse, size ≈ touched RAM (~141 MiB here) | 8–29 s (`virsh save`, ≈ touched RAM) |
| **Restore → resumed (copy mode)** | **0.134 s** | 3.7–6.2 s (`virsh restore`) |
| **Restore → resumed (ondemand/userfaultfd)** | **0.043 s (~43 ms)** — pages fault lazily | n/a (QEMU q35 copies eagerly) |

Notes:
- Snapshot layout: `config.json` (VM config incl. disk paths), `state.json` (device state ~53 KiB),
  `memory-ranges` (a 2 GiB *apparent* file that is **sparse** — only ~141 MiB of touched pages consume
  disk). Restore latency is dominated by device-state setup, not RAM copy, which is why it is sub-second.
- Guest here was a firmware-booted base without network/enrollment (host-side CH RSS ~147 MiB), so the
  touched-RAM figure is a *warm-idle floor*; a loaded agent will snapshot larger, but restore latency
  stays low because ondemand pages fault lazily and copy-mode is already ~0.14 s at this footprint.
- `ondemand` requires `sysctl vm.unprivileged_userfaultfd=1` **or** running the VMM with
  `CAP_SYS_PTRACE`; otherwise restore fails `Failed to create userfaultfd: Operation not permitted`.
  This is an operational prerequisite to document, not a CH limitation.

---

## 3. Fork-from-warm-base

All primitives are confirmed present; the mechanism is:

- **Disk COW per child:** each child gets its own writable disk. CH rejects qcow2 *backing chains*
  (`MaxNestingDepthExceeded`), so children use standalone per-child disk copies (or raw + reflink on a
  CoW filesystem), not a qcow2 overlay of a shared base.
- **RAM isolation, not resident sharing:** with `memory_restore_mode=ondemand`, userfaultfd reads the
  shared `memory-ranges` snapshot input and uses `UFFDIO_COPY` to populate each child's distinct
  guest-memory memfd. A live N=2 inherited-memory mutation test on 2026-07-21 proved isolation, but
  measured 0 KiB defensible resident guest-RAM sharing. The shared snapshot inode/page cache must not
  be reported as shared resident guest RAM.
- **Per-child identity:** each child needs a fresh vsock CID (#595) and independent enrollment
  (#617/#619) — consistent with the "snapshot the pre-enrollment clean base, inject identity on
  restore" posture from #639 and the secret-hygiene work (#645).

**Completed measurement:** concurrent N=2 restore recorded 66 ms per child in the final controlled
run. Each 4 GiB guest mapping reported 4 GiB RSS/PSS, distinct memfd inodes, 0 KiB KSM, and 0 KiB
defensible resident sharing. The direct 64 MiB inherited-buffer mutations remained isolated. See
`docs/research/evidence/ch-fork-memory-isolation-grissom-2026-07-21.json`.

---

## 4. Recommendation — GO

Adopt **Cloud Hypervisor** as the sandbox VMM for the fast-start / fork path, converging #639 and #642:

1. **Runtime:** CH v53.0 (or current), booting the agent base via `hypervisor-fw` with `--fs` (virtiofs
   global/inbox), `--vsock` (ADR-023), `shared=on` memory.
2. **Fast resume / warm pool (#643 semantics on CH):** snapshot a **pre-enrollment clean warm base**;
   restore per handout in ~0.14 s (copy) or ondemand; enroll on restore (fresh CID + secret + mTLS).
3. **Fork fan-out:** ondemand restore + per-child COW disk → many isolated children from one warm
   base, with full per-child resident-memory cost unless a separately measured deduplication mechanism
   is enabled.
4. **Prereqs to bake into provisioning:** `vm.unprivileged_userfaultfd=1` (or `CAP_SYS_PTRACE`);
   standalone (non-backing-chain) disks; per-child CID allocation.
5. **Coexistence:** CH is the fast/fork path; the current libvirt/QEMU path (and the `checkpoint-vm.sh`
   primitive from #643) remains for the existing q35 flow. Not migrating the libvirt path here
   (out of scope, #119/#120).

---

## Appendix — reproduction

```
# capability probe (authoritative — the actual binary)
cloud-hypervisor --help | grep -E ' --fs| --vsock| --restore'
ch-remote --help | grep -iE 'snapshot|restore'

# boot agent base (flattened qcow2 — CH rejects backing overlays)
qemu-img convert -O qcow2 ubuntu-server-24.04-agent.qcow2 flat.qcow2
cloud-hypervisor --api-socket ch.sock --kernel hypervisor-fw \
  --disk path=flat.qcow2,image_type=qcow2 --cpus boot=2 --memory size=2048M,shared=on \
  --serial file=serial.log --console off &

# snapshot (URL is positional in v53)
ch-remote --api-socket ch.sock pause
ch-remote --api-socket ch.sock snapshot file:///path/snap

# restore (sub-second)
sysctl -w vm.unprivileged_userfaultfd=1     # for ondemand
cloud-hypervisor --api-socket r.sock \
  --restore source_url=file:///path/snap,memory_restore_mode=copy,resume=true &
```

## References

- #639 findings: `docs/research/memory-snapshot-restore-spike.md` (QEMU/q35 baseline).
- #642 (sub-second start), #595 (vsock CID), #617/#619 (secrets), #643 (checkpoint primitive),
  #645 (snapshot secret hygiene).
- Cloud Hypervisor v53.0; rust-hypervisor-firmware 0.5.0.
