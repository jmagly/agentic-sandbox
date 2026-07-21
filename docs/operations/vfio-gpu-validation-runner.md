# Dual-GPU VFIO validation runner

## Purpose

Provide the exclusive, host-native Gitea Actions lane required to validate physical GPU passthrough,
managed teardown, quarantine behavior, and cross-tenant VRAM residue for issues #658 and #655. The
lane is manual-only and produces structured evidence without collecting credentials or machine serial
numbers.

## Scope

This runbook applies only to a DevOps-owned Linux/KVM host with two explicitly assigned GPU roles:
one service GPU that must remain on its native driver, and one headless test GPU whose complete IOMMU
group is dedicated to VFIO. It does not authorize detaching a workstation display GPU, weakening ACS
checks, bypassing PCI reset, or releasing a quarantined claim after a failed teardown.

The repository supplies:

- `.gitea/workflows/ci.yaml`: restricted manual-only physical-GPU job added to the existing Titan CI
  workflow;
- `scripts/vfio-gpu-runner-guard.sh`: inventory, preflight, and postflight assertions;
- `scripts/run-vfio-gpu-validation.sh`: global-lock, two-tenant lifecycle orchestration;
- `configs/vfio-gpu-runner.example.json`: inventory and policy template.

## Prerequisites

1. Preserve out-of-band management access and prove it does not depend on the test GPU.
2. Enable VT-d or AMD-Vi and IOMMU grouping. Do not use an ACS override kernel parameter as evidence
   of hardware isolation.
3. Confirm the test GPU is headless, every IOMMU-group member can be assigned together, and the
   primary function exposes `/sys/bus/pci/devices/<BDF>/reset`.
4. Install pinned Cloud Hypervisor v53.0, KVM/VFIO, libvirt/QEMU, Cargo, `jq`, `fuser`, `flock`, SSH,
   `lspci`, and the canonical agent base image.
5. Use the existing repository-scoped Titan runner. The physical lane is a distinct job selected only
   by the manual confirmation input; pushes, pull requests, and ordinary dispatches never enter it.
6. Install an owner-only guest SSH key and a device-specific residue tool. The tool interface is:
   `tool fill` for tenant A and `tool probe-before-write` for tenant B; each emits JSON with
   `{"result":"pass"}` only when its phase succeeds.
7. Copy the example inventory to `configs/vfio-gpu-runners/<host-class>.json`, replace every
   placeholder, record the exact group/device-node allowlist and native drivers, obtain an ACS review,
   and commit the sanitized inventory. Install the same reviewed file at
   `/etc/agentic-sandbox/vfio-gpu-runner.json` owned by root and not group/other writable.

The runner account needs narrowly reviewed passwordless sudo for the validation entrypoint and the
final evidence `test`, `cp`, and `chown` operations used by the workflow. It must not receive a general
passwordless shell. Treat changes to this workflow, the two VFIO scripts, or the installed inventory
as privileged infrastructure changes requiring DevOps review.

## Procedure

1. Compare the committed inventory with the host before runner registration:

   ```bash
   sudo scripts/vfio-gpu-runner-guard.sh inventory \
     --config /etc/agentic-sandbox/vfio-gpu-runner.json | jq .
   sudo scripts/vfio-gpu-runner-guard.sh preflight \
     --config /etc/agentic-sandbox/vfio-gpu-runner.json | jq .
   ```

2. Confirm the inventory contains CPU, board/firmware, kernel, both GPU PCI identities/drivers, exact
   IOMMU membership, and reset method. It intentionally omits DMI serials, UUIDs, credentials, and
   private-key material.
3. In Gitea, dispatch **CI** from `main` and set `vfio_gpu_confirmation` to the exact value
   `RUN-EXCLUSIVE-VFIO`. Only actors in the installed inventory are accepted, and the physical job
   waits for the ordinary Titan lint/test gates.
4. The job acquires `/run/lock/agentic-sandbox-vfio-gpu.lock` for its entire lifetime. A second job
   fails rather than sharing the group.
5. The job provisions tenant A, proves guest PCI enumeration and driver functionality, fills the
   device-specific VRAM pattern, and destroys A through `scripts/destroy-vm.sh` with the Cloud
   Hypervisor backend.
6. Postflight must prove native-driver restoration, claim removal, VFIO-node removal, no target VMM,
   no VM state directory, and an unchanged service-GPU driver.
7. The job repeats with independent tenant B. PCI enumeration skips `nvidia-smi`, then the residue
   tool runs `probe-before-write` before the configured driver-functionality command. Tenant B must
   attest `prewrite: true`; use a host-class guest image/tool that prevents automatic driver
   initialization before that attestation.
8. The synthetic reset-failure regression runs in the same workflow and must show that reset failure
   retains the durable claim/recovery journal rather than reissuing the group.

## Verification

A passing run requires all of the following:

- preflight and both postflight JSON documents report `result: pass`;
- both `gpu-validation.json` files match the exact host vendor/device ID and live CH device path;
- both driver-functionality outputs are non-empty and successful;
- tenant-A fill and tenant-B pre-write probe JSON report `result: pass`;
- `inventory-before.json` and `inventory-after.json` show the service GPU on the same native driver;
- managed destroy logs complete without a retained claim, stale `/dev/vfio/<group>`, VMM, or VM state;
- `result.json` reports both tenants and all four high-level gates as true;
- `evidence.sha256` verifies every retained artifact.

Link the successful run and retained artifact to #658 and #655, commit the sanitized inventory, and
update `docs/runtime-parity.md` only after reviewing the physical results.

## Rollback

Normal rollback is managed teardown of only the exact workflow-created names
`vfio-<run-id>-a` and `vfio-<run-id>-b`. Do not delete their state directories or claim files by
hand: the recovery journal is required to restore the original drivers safely.

If teardown fails, leave the group quarantined, pause new Titan workflows, preserve the VM state and
`/var/lib/agentic-sandbox/vms/.vfio-claims/iommu-<group>` evidence, and use out-of-band access. A host
power cycle is the final reset boundary when device reset or driver restoration cannot be proven.
Only after the power cycle and inspection may an operator use the documented managed force-release
path; never remove the claim directory as a shortcut.

## Troubleshooting

- **Preflight reports active device node:** stop the named display/compute workload. Never override
  the check while rotating tenants.
- **IOMMU group differs from inventory:** disable the lane and repeat the ACS/topology review. Do not
  auto-update the allowlist.
- **Service GPU driver changed:** treat this as a host-safety failure; recover console/management via
  out-of-band access before any further VFIO action.
- **No reset interface or reset failure:** quarantine the group. The host class is single-tenant until
  a power cycle or a vendor-supported reset procedure is validated.
- **Native driver fails to bind:** retain the claim and recovery journal, inspect kernel logs and the
  recorded original driver/override, then retry only through managed teardown.
- **Artifact upload fails:** the validation result remains authoritative; preserve
  `/var/tmp/agentic-vfio-evidence/run-<id>` locally and upload it without credential files.

## Evidence

Gitea retains the `vfio-gpu-hardware-evidence-<run-id>` artifact for 30 days. It contains sanitized
hardware inventories, guest enumeration, functionality and residue results, managed teardown logs,
the synthetic quarantine regression log, a summary result, and SHA256 manifest. It must not contain
SSH keys, bootstrap tokens, `/etc/agentic-sandbox` secret files, full environment dumps, DMI serials,
or arbitrary guest memory. Promote the reviewed, non-secret evidence needed for long-term provenance
to `docs/research/evidence/`.
