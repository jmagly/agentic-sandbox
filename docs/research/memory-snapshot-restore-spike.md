# Memory Snapshot / Restore for Fast Resume, Fork, and Checkpoint

**Spike:** [#639](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/639) — `spike(runtime): memory snapshot/restore for fast resume, fork, and checkpoint`
**Sibling spike:** [#642](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/642) — sub-second sandbox start (tightly coupled; "likely one mechanism")
**Research date:** 2026-07-16
**Researcher:** Claude Code (research + planning spike)
**Type:** Findings + recommendation. No implementation — see follow-up issues.
**Test host:** `grissom` — QEMU 8.2.2 / libvirt 10.0.0, Linux KVM, KSM enabled, 62 GiB RAM.

> **Benchmarks in this doc are real**, run on `grissom` against the actual agent base image
> (`ubuntu-server-24.04-agent.qcow2`) using a domain that mirrors `images/qemu/provision-vm.sh`
> (`define_vm`) exactly: q35 UEFI, 8 GiB, qcow2 overlay, virtiofs (`memfd` shared), vsock, virtio-net,
> qemu-guest-agent. Numbers that could **not** be reproduced on this host (full cloud-init + agent
> enrollment cost) are labelled as such and cited from the referenced issues, not fabricated.

---

## Executive summary / TL;DR

1. **On the current QEMU/libvirt q35 stack, "millisecond restore" is not achievable.** The stock
   fast-resume primitive (`virsh save`/`managedsave` = migrate-to-file) restores an **idle** warm
   base to a live guest in **~3.7 s**, and a lightly-loaded agent VM (~2 GiB live) in **~6.2 s**.
   That is a large win over cold provision (seconds-to-minutes with cloud-init + enrollment, per
   #614/#597), but it is **seconds, not milliseconds**.

2. **virtiofs categorically blocks `virsh save`/`managedsave`/`migrate` on this stack.** With a
   virtiofs mount attached, save fails hard:
   `Migration disabled: vhost-user backend lacks VHOST_USER_PROTOCOL_F_LOG_SHMFD feature`. Since our
   default profile *depends* on virtiofs (global/inbox mounts), **any snapshot design on the current
   stack must detach virtiofs before snapshot and re-attach on restore** (live detach works, rc=0).

3. **Internal qcow2 snapshots (`savevm`) are doubly blocked** on our machine model — both by the
   `memfd` shared-memory backing virtiofs requires *and* by UEFI/pflash firmware
   (`internal snapshots of a VM with pflash based firmware are not supported`). Internal snapshots are
   off the table regardless of virtiofs.

4. **Disk fork is nearly free; RAM fork is not.** qcow2 backing overlays give copy-on-write disk
   forks (external disk-only snapshot succeeds; initial overlay ~200 KiB). But QEMU q35 restore
   **copies the full RAM image into each child** — there is no shared-RO-base RAM fork. KSM (on by
   default here) can *dedup* identical base pages lazily after the fact, but that is best-effort
   reclaim, not a fork primitive, and it does nothing for restore latency.

5. **True sub-second restore + real fork-from-warm-base is a microVM property, and points at the
   same answer as #642: Cloud Hypervisor.** It is the only microVM that supports **virtiofs + vsock +
   native snapshot/restore** together, with demand-paged (userfaultfd) restore and snapshot fan-out.
   Firecracker has snapshot/fork but **no virtiofs** (a hard blocker for our storage model). This
   converges #639 and #642 onto one mechanism, as the issues anticipated.

6. **A memory snapshot is a secrets-at-rest object.** Guest RAM at capture contains the control-plane
   mTLS client key and Claude OAuth tokens (#617) and the bootstrap bearer token (#619). The
   **recommended posture is to snapshot only a pre-credential clean warm base** and inject
   secrets/identity on restore via the existing enrollment path — which also eliminates cross-tenant
   residue for forks and dovetails with the credential-broker direction already scoped in #517/#617.

**Recommendation in one line:** ship a **detach-virtiofs → `virsh save`/`restore` checkpoint +
warm-pool** capability on the current stack now (seconds-scale resume, single-VM checkpoint), and
**adopt Cloud Hypervisor** as the target runtime for genuine sub-second restore and
fork-from-warm-base — one mechanism shared with #642. Snapshot the **pre-enrollment** base only.

---

## 1. Machine model under test (why the mechanism choice is constrained)

From `images/qemu/provision-vm.sh` (`define_vm`, `_libvirt_os_xml`), the default profile is:

| Element | Value | Snapshot-relevant consequence |
|---|---|---|
| Machine | `q35`, `<domain type='kvm'>`, `host-passthrough` CPU | Full-machine emulation; not a minimal microVM device model |
| Firmware | **UEFI** — OVMF `pflash` + per-VM NVRAM (`*_VARS.fd`) | **Blocks internal (`savevm`) snapshots**; NVRAM is a *separate file* that must travel with any snapshot |
| Memory | default **8 GiB** (`DEFAULT_MEMORY=8G`) | Snapshot size is bounded by *touched* RAM, not the 8 GiB allocation |
| Memory backing | `memfd` + `access mode='shared'` (**required** for virtiofs) | Blocks internal snapshots; interacts with migration |
| Disk | single **qcow2** `vda` (virtio, `cache=writeback`) + cloud-init CDROM | qcow2 → backing-chain COW forks available for the *disk* |
| Shares | **virtiofs** ×2–3 (`global-ro`, `inbox`, optional `outbox`) | **vhost-user external process (`virtiofsd`); blocks `virsh save`/migrate** (see §2) |
| Transport | **vsock** static CID (ADR-023, registry #595) | virtio-vsock *is* migratable (in the RAM stream); but per-VM CID is host config → forks need fresh CIDs |
| Mgmt | qemu-guest-agent channel, serial/console pty, VNC | guest-agent used as the "guest is usable again" probe in benchmarks |

The two load-bearing constraints for snapshotting are **virtiofs (vhost-user)** and **UEFI/pflash +
`memfd`** — both inherent to how we run today.

---

## 2. Mechanism comparison (empirical, this exact machine model)

| Mechanism | Captures full guest RAM + device state? | Works on our profile? | Evidence |
|---|---|---|---|
| **`virsh save` / `managedsave`** (QEMU migrate-to-file) | Yes (RAM + device state; NVRAM separate) | **Only with virtiofs detached** | `save`/`managedsave` **rc=1** with virtiofs: `Migration disabled: vhost-user backend lacks VHOST_USER_PROTOCOL_F_LOG_SHMFD feature`. After `virsh detach-device` of the virtiofs mounts (rc=0), save/restore works — §3 numbers. |
| **`migrate "exec:cat > snap"`** (migrate-to-file variant) | Yes | Same as above (same migration path) | Identical `VHOST_USER_PROTOCOL_F_LOG_SHMFD` gate — it is the migration subsystem, not the CLI verb, that virtiofs blocks. |
| **Internal snapshot (`savevm` / `snapshot-create-as` with `--memspec … internal`)** | Yes (RAM stored *inside* qcow2) | **No — doubly blocked** | `memory filename … requires external snapshot` (memfd), and separately `internal snapshots of a VM with pflash based firmware are not supported` (UEFI). |
| **External *disk-only* snapshot (`--disk-only`, qcow2 overlay)** | Disk only (no RAM) | **Yes** (rc=0) | `Domain snapshot … created`; fresh overlay ~200 KiB. Useful for disk COW / fork, not for RAM state. |
| **External snapshot *with memory* (`--memspec … external`)** | Yes | Bounded by the same migration gate as `virsh save` (virtiofs must be detached) | Not separately timed; shares the migrate-to-file backend. |
| **CRIU** (process/namespace checkpoint) | For the **container/bridge** sandbox mode only — N/A to the VM path | Parallel path, see §6 | — |

**Bottom line:** on the current stack the *only* full-state RAM mechanism is the migrate-to-file
family (`virsh save`/`managedsave`/`migrate`), and it requires virtiofs to be **detached** first.
Internal snapshots are impossible here.

---

## 3. Benchmark — latency + size (the required numbers)

**Setup:** virtiofs-detached twin of the default profile (q35 UEFI, 8 GiB, vsock, qcow2 overlay of
the real agent base). This is the *realistic snapshot target*, since virtiofs cannot be attached
during save. RAM was dirtied with **incompressible** data (`/dev/urandom` → `/dev/shm`) to defeat
migration zero-page elision — i.e. a worst-case, not a best-case, size measurement. "restore→agent"
= wall-clock from `virsh restore` to the guest agent answering `guest-ping` (guest actually usable).

| Guest state | Guest RSS | **Snapshot size** | **Save time** | **Restore → usable** |
|---|---|---|---|---|
| Warm idle | ~850 MiB | **666 MiB** | 7.97 s | **3.74 s** |
| +1 GiB live | ~1.6 GiB | 1466 MiB | 16.81 s | 4.21 s |
| +2 GiB live | ~2.7 GiB | 2593 MiB | 28.56 s | **6.16 s** |

Baselines on the same host/profile:

- **Cold boot → guest-agent-ready: ~22–23 s** (boot only; *excludes* cloud-init + agent enrollment).
- **Full cold provision** (cloud-init + enrollment) is the "seconds-to-minutes" flaky path the issue
  cites (#614 no-timeout virsh wedge, #597 VM left shut off / no enroll) — not reproduced here
  because it needs the management-server + enrollment stack; cited, not measured.

**Derived facts:**

- **Snapshot size ≈ touched (non-zero) RAM.** ~0.95–1.0 MiB on disk per MiB of live incompressible
  data; zero pages are elided. An idle enrolled warm base is on the order of **~0.7 GiB**; budget
  **~1 GiB of snapshot per 1 GiB of live agent working set**. Real agent RAM (page cache, model
  context) is partly compressible, so this is an upper bound.
- **Save throughput ≈ 85–90 MiB/s** (default `save_image_format`, uncompressed, single-threaded, to
  the root LV). Save time scales linearly with touched RAM and is the dominant cost.
- **Restore → usable ≈ 3.7 s (idle) to 6.2 s (2 GiB live)**, with a **~3.5 s fixed floor** (RAM
  stream read + guest scheduler/agent resettle) on top of stream size. **This is not sub-second.**
- **Restore is cheaper than save** (sequential page-in vs dirty-scan+write), but the fixed resettle
  floor dominates at small sizes.

**Answer to the issue's core question — "is restore truly sub-second?"** **No, not on QEMU q35 +
`virsh restore`.** Sub-second restore requires demand-paged restore (userfaultfd) as in Firecracker /
Cloud Hypervisor, where the guest resumes immediately and pages fault in lazily. See §4/§7.

---

## 4. Fork-from-warm-base — what actually gets shared vs copied

| Layer | Shareable across N children on QEMU q35? | Mechanism | Verdict |
|---|---|---|---|
| **Disk** | **Yes** | qcow2 backing overlay — one RO base, per-child COW overlay | Empirically works (external disk snapshot rc=0; ~200 KiB initial overlay). Near-free. |
| **RAM** | **No (not natively)** | `virsh restore` copies the whole RAM image per child | Each child pays the full restore cost and full RAM footprint. |
| **RAM (lazy dedup)** | Partial, best-effort | **KSM** (on here, `run=1`) merges identical pages in the background | Reclaims memory over seconds/minutes; **not** a fork primitive; **no** restore-latency benefit. |
| **RAM (true COW fork)** | Not on our stock path | `memory-backend-file,share=on` + `x-ignore-shared`; or Firecracker/Cloud-Hypervisor snapshot fan-out with userfaultfd | Available on **Cloud Hypervisor** with virtiofs+vsock → §7. |
| **vsock CID** | **No** | Per-VM host config (registry #595) | Each fork needs a **fresh CID**; cannot share one base CID. Config-level, not RAM. |
| **UEFI NVRAM** | Copy per child | `*_VARS.fd` per domain | Small; must be cloned with each child. |

So on the current stack a "fork" is: **shared qcow2 disk base + full per-child RAM restore + fresh
CID + cloned NVRAM** — i.e. disk is forked, RAM is not. Genuine RAM-sharing fork-from-warm-base is a
reason to move to Cloud Hypervisor (converges with #642).

---

## 5. Interaction with virtiofs and the vsock CID registry (#595)

- **virtiofs does not survive save/restore and in fact blocks it** (§2). virtiofsd is an external
  vhost-user process whose state is not in the migration stream, and Ubuntu 24.04 / QEMU 8.2
  virtiofsd does not advertise `VHOST_USER_PROTOCOL_F_LOG_SHMFD`, so libvirt refuses the migration
  outright. **Required restore logic:** detach virtiofs before snapshot; on restore, (re)attach the
  virtiofs mounts (`virsh attach-device`) and the guest re-mounts `agentglobal`/`agentinbox`. This is
  a device *re-attach*, not just a guest remount — the host-side backend must be re-established.
- **vsock survives restore in principle** (virtio-vsock device state is in the RAM stream), but the
  **host-side CID must not double-allocate**. For single-VM resume the CID is unchanged and fine. For
  **forks, every child must get a fresh CID** from the registry, and the guest agent-client must
  re-register (ADR-023 transport). Note the registry bug tracked in #595 (writer/parser order) is
  orthogonal but must be sound before fan-out — a fork storm that appends CIDs will amplify any
  registry fragility.

---

## 6. Container/bridge sandbox mode — is CRIU viable?

Snapshotting is **VM-only** on the primary path. For the container/bridge mode (`docs/container-runtime.md`,
low-isolation baseline, #622), **CRIU** is the parallel checkpoint/restore primitive:

- CRIU can checkpoint a process tree + namespaces to disk and restore it, and integrates with
  `runc`/`podman` (`--checkpoint`/`--restore`).
- **Caveats for our workload:** CRIU restore of a container that holds live TCP/gRPC/mTLS sessions,
  open vsock, and bind-mounted `agentshare` is fragile; external connections generally must be
  re-established, and GPU/passthrough state is not covered. The credential exposure is *worse* than
  the VM case: the checkpoint image contains the workload's entire address space including the
  in-memory mTLS key and OAuth tokens (#617), on the host filesystem.
- **Verdict:** CRIU is viable as a *container-mode* fast-resume experiment but is **not** the primary
  recommendation; the VM path (and its Cloud Hypervisor evolution) is where isolation + snapshot
  align. Treat CRIU as a separate, lower-priority follow-up if container-mode warm resume is wanted.

---

## 7. Security — a snapshot is a secrets-at-rest object

A memory snapshot contains **everything in guest RAM at capture**, verbatim. On our sandboxes that
includes, per the security assessment:

- the **control-plane mTLS client key** and **Claude OAuth access/refresh tokens** (#617, SBX-002 —
  today readable by the root workload; also therefore resident in RAM and captured), and
- the **bootstrap bearer token** in PID 1's environment (#619, SBX-004 — short-lived/single-use, but
  present in RAM if captured pre-expiry).

Consequences and the recommended posture:

| Concern | Risk | Recommended handling |
|---|---|---|
| **Snapshot at rest** | Snapshot file = plaintext secrets + full workload memory on the host FS | **Snapshot the pre-credential clean warm base only** (before enrollment injects mTLS key / OAuth / bootstrap token). If a *post-credential* snapshot is ever taken, it MUST be encrypted at rest (LUKS/`age`/`fscrypt`) and access-controlled to the sidecar UID — see `.claude/rules/no-unauthenticated-encryption.md`, `crypto-flag-verification.md`. |
| **Scrub vs snapshot-clean-base** | Scrubbing secrets out of a live snapshot is unreliable (copies linger in freed pages, caches, socket buffers) | **Prefer snapshot-clean-base + inject-on-restore** over scrub. Restore path re-runs enrollment: fresh ephemeral secret (#619), fresh mTLS identity (#617), fresh vsock CID (#595). This is the same "credentials never land in the workload" direction as the credential-broker work (#517/#617). |
| **Cross-tenant residue on fork** | Forking one base to N children shares that base's RAM contents → any secret in the base leaks into every child | A **clean pre-credential base has no tenant secrets to leak.** Each child enrolls independently post-restore. Default threat model is single-host, VM-lateral (project deployment default) — a clean base keeps lateral residue to non-secret OS/page-cache state. |
| **Snapshot provenance/tamper** | A swapped/edited base snapshot could inject a backdoored warm base into every fork | Treat base snapshots as release artifacts: hash + sign, verify before restore (mirrors the base-image manifest discipline already in `/mnt/ops/base-images/manifest.json`). |

**Net:** the security analysis and the performance analysis point the same way — **snapshot the
pre-enrollment warm base, inject identity on restore.** That is faster to capture (no secret-scrub
step), safe at rest (no secrets in the file), and residue-free for forks.

---

## 8. Recommendation

### 8.1 Mechanism (now, current stack)
Adopt **`virsh save`/`restore` (migrate-to-file) with a detach-virtiofs wrapper** as the single-VM
**checkpoint + fast-resume** primitive:
1. On checkpoint: `virsh detach-device` the virtiofs mounts → `virsh save <file>` (or `managedsave`)
   → persist the NVRAM `*_VARS.fd` alongside.
2. On restore: `virsh restore <file>` → re-attach virtiofs → guest re-mounts `agentglobal`/`agentinbox`.
3. Snapshot the **pre-enrollment** base; enroll on restore (fresh CID + ephemeral secret + mTLS).

This delivers **~3.7–6 s resume** vs seconds-to-minutes cold provision — a real, shippable win for
checkpoint/restore across host restarts, and the basis of a **warm pool** (#642's "warm pool" option:
keep pre-booted pre-enrollment bases; on handout, restore + enroll).

### 8.2 Fork model (now)
Disk-only: qcow2 backing overlay per child (works, near-free). **RAM is copied per child** on this
stack — acceptable for small fan-out, not for large fan-out. Do **not** rely on KSM as a fork
primitive.

### 8.3 Target runtime (for genuine sub-second + RAM fork) — converge with #642
Adopt **Cloud Hypervisor** as the sandbox VMM for the fast-start/fork path. It is the only microVM
that supports **virtiofs + vsock + native snapshot/restore** together, with **userfaultfd demand-paged
restore** (immediate resume, lazy page-in → sub-second) and **snapshot fan-out** for true
fork-from-warm-base (shared RO base RAM + per-child COW). **Firecracker is disqualified for us by its
lack of virtiofs.** This is explicitly the "one mechanism for #639 and #642" the issues call for, and
matches the Firecracker/microVM direction already in `docs/research/platform-comparison.md` (adjusted
for our virtiofs dependency, which pushes microVM choice from Firecracker → Cloud Hypervisor).

### 8.4 Secret handling (both stacks)
**Snapshot pre-credential clean warm base; inject identity on restore.** Never snapshot a
post-enrollment VM to an unencrypted file. Hash+sign base snapshots and verify before restore.

### 8.5 Container mode
CRIU is a **separate, lower-priority** experiment for container-mode warm resume; not on the VM
critical path.

---

## 9. Follow-up implementation issues

Filed and linked from #639:

- **Checkpoint/fast-resume on the current stack** — `virsh save`/`restore` with detach/re-attach
  virtiofs, NVRAM persistence, pre-enrollment base + enroll-on-restore. (Enables #642 "warm pool".)
- **Evaluate & PoC Cloud Hypervisor** as the sandbox VMM (virtiofs + vsock + snapshot/restore + fork)
  — joint with #642; produce the virtiofs/vsock support matrix + a userfaultfd restore-latency PoC.
- **Snapshot secret hygiene** — pre-enrollment clean-base capture, snapshot-at-rest
  encryption + signing/verification, enroll-on-restore wiring (ties to #517/#617/#619).

---

## Appendix A — reproduction

The benchmark is a self-contained script that builds a default-profile domain (mirroring
`define_vm`), boots it, times `virsh save`/`restore` at three RAM-fill levels with incompressible
data, and probes the internal/external/managedsave compatibility matrix. It cleans up the domain,
NVRAM, and scratch artifacts, and does **not** touch the sandbox vsock CID registry (it uses a
libvirt-direct CID of 639, absent from `/var/lib/agentic-sandbox/vms/.vsock-cid-registry`). Method:
q35 UEFI, 8 GiB, 4 vCPU, qcow2 overlay of `ubuntu-server-24.04-agent.qcow2`; RAM dirtied via
`/dev/urandom → /dev/shm`; "usable" = qemu-guest-agent answers `guest-ping`.

## Appendix B — key raw evidence

```
# virtiofs blocks migrate-to-file (virsh save / managedsave), rc=1:
error: Requested operation is not valid: cannot migrate domain:
  Migration disabled: vhost-user backend lacks VHOST_USER_PROTOCOL_F_LOG_SHMFD feature.

# internal snapshot doubly blocked:
error: XML error: memory filename '…/int.mem' requires external snapshot        # memfd shared mem
error: Operation not supported: internal snapshots of a VM with pflash based firmware are not supported  # UEFI

# external disk-only snapshot works (disk COW), rc=0:
Domain snapshot extdisktest created

# live virtiofs detach works (enables detach-before-save), rc=0:
Device detached successfully

# real save/restore (virtiofs-detached twin, incompressible RAM):
| state      | guest RSS MiB | save MiB | save s | restore->agent s |
| idle       | 849           | 666      | 7.97   | 3.74             |
| touched_1g | 1576          | 1466     | 16.81  | 4.21             |
| touched_2g | 2691          | 2593     | 28.56  | 6.16             |
```

## References

- Issue #639 (this spike), #642 (sub-second start, sibling), #614/#597 (cold-provision flakiness).
- Security: #617 (SBX-002 mTLS key + OAuth in workload), #619 (SBX-004 bootstrap token), #503
  (hardening epic), #517 (credential proxy direction).
- Transport: #595 (vsock CID registry), ADR-023 (vsock transport).
- `images/qemu/provision-vm.sh` — `define_vm`, `_libvirt_os_xml` (machine model under test).
- `docs/research/platform-comparison.md` — prior microVM/Firecracker platform survey.
- Out of scope (per #639): implementation, live cross-host migration, Proxmox backend (#119/#120).
