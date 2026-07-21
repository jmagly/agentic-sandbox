# Local-only dual-GPU VFIO validation

## Purpose

Validate physical GPU passthrough, managed teardown, quarantine behavior, and cross-tenant VRAM
residue for issues #658 and #655 without exposing an active workstation to automated driver changes.
Titan has no spare GPU, and Grissom is an active workstation, so physical execution is local-only
until a dedicated host is provisioned. The procedure produces structured evidence without collecting
credentials or machine serial numbers.

## Scope

This runbook applies only during an operator-controlled maintenance window on a Linux/KVM host with
two explicitly assigned GPU roles: one service GPU that must remain on its native driver, and one test
GPU whose complete IOMMU group is temporarily dedicated to VFIO. It does not authorize CI-controlled
execution, detaching a GPU held by a graphical session, weakening ACS checks, bypassing PCI reset, or
releasing a quarantined claim after a failed teardown.

The current Grissom candidate has an Intel Iris Xe service GPU (`0000:00:02.0`, `i915`) and an NVIDIA
RTX 3060 Laptop GPU (`0000:01:00.0`, `nvidia`) with its audio function (`0000:01:00.1`) isolated in
IOMMU group 16. The NVIDIA primary exposes a reset interface. This is only a discovered candidate,
not an approved inventory: Xorg currently holds the NVIDIA and DRM device nodes, ACS approval has not
been recorded, and the SSH/residue-test prerequisites are not installed.

The repository supplies:

- `.gitea/workflows/ci.yaml`: no physical-GPU job or dispatch input; automated CI cannot execute the
  local entrypoint;
- `scripts/vfio-gpu-runner-guard.sh`: inventory, preflight, and postflight assertions;
- `scripts/run-vfio-gpu-validation.sh`: global-lock, two-tenant lifecycle orchestration;
- `configs/vfio-gpu-runner.example.json`: inventory and policy template;
- `configs/vfio-gpu-runners/grissom.workstation-draft.json`: discovered Grissom topology with
  `acs_reviewed: false`, intentionally unable to authorize a run.

## Prerequisites

1. Preserve out-of-band management access and prove it does not depend on the test GPU.
2. Enable VT-d or AMD-Vi and IOMMU grouping. Do not use an ACS override kernel parameter as evidence
   of hardware isolation.
3. Confirm the test GPU is headless, every IOMMU-group member can be assigned together, and the
   primary function exposes `/sys/bus/pci/devices/<BDF>/reset`.
4. Install pinned Cloud Hypervisor v53.0, KVM/VFIO, libvirt/QEMU, Cargo, `jq`, `fuser`, `flock`, SSH,
   `lspci`, and the canonical agent base image.
5. Establish an operator-controlled local maintenance window. Save workstation state, arrange SSH or
   other independent management through the service GPU/network, and explicitly log out the graphical
   session. Automation must not stop Xorg or the display manager.
6. Install an owner-only guest SSH key and a device-specific residue tool. The tool interface is:
   `tool fill` for tenant A and `tool probe-before-write` for tenant B; each emits JSON with
   `{"result":"pass"}` only when its phase succeeds.
7. Copy the example inventory to `configs/vfio-gpu-runners/<host-class>.json`, replace every
   placeholder, record the exact group/device-node allowlist and native drivers, obtain an ACS review,
   and commit the sanitized inventory. Install the same reviewed file at
   `/etc/agentic-sandbox/vfio-gpu-runner.json` owned by root and not group/other writable.

The local operator needs root access for the validation entrypoint. Do not add Grissom as a GPU CI
runner or grant a CI account passwordless access. Treat changes to the CI exclusion, the two VFIO
scripts, or the installed inventory as privileged infrastructure changes requiring review.

## Procedure

1. Compare the reviewed inventory with the host before any graphical-session or driver change:

   ```bash
   sudo scripts/vfio-gpu-runner-guard.sh inventory \
     --config /etc/agentic-sandbox/vfio-gpu-runner.json | jq .
   sudo scripts/vfio-gpu-runner-guard.sh preflight \
     --config /etc/agentic-sandbox/vfio-gpu-runner.json | jq .
   ```

   `preflight` is expected to fail while Xorg or another process holds any test-GPU node. Do not
   override that failure.
2. Confirm the inventory contains CPU, board/firmware, kernel, both GPU PCI identities/drivers, exact
   IOMMU membership, and reset method. It intentionally omits DMI serials, UUIDs, credentials, and
   private-key material.
3. After the operator has ended the graphical session and independently confirmed workstation access,
   rerun `preflight`. Proceed only when it reports `result: pass`. Then invoke the local entrypoint
   from a clean `main` checkout with an explicit run identifier:

   ```bash
   run_id="local-$(date --utc +%Y%m%dT%H%M%SZ)"
   sudo scripts/run-vfio-gpu-validation.sh \
     --config /etc/agentic-sandbox/vfio-gpu-runner.json \
     --confirmation RUN-LOCAL-VFIO \
     --run-id "$run_id" \
     --artifact-dir "/var/tmp/agentic-vfio-evidence/run-$run_id" \
     --repository roctinam/agentic-sandbox \
     --ref refs/heads/main \
     --actor roctinam \
     --event local_manual
   ```

   The exact confirmation and `local_manual` event are mandatory. No CI job invokes this entrypoint.
4. The command acquires `/run/lock/agentic-sandbox-vfio-gpu.lock` for its entire lifetime. A second
   invocation fails rather than sharing the group.
5. The command provisions tenant A, proves guest PCI enumeration and driver functionality, fills the
   device-specific VRAM pattern, and destroys A through `scripts/destroy-vm.sh` with the Cloud
   Hypervisor backend.
6. Postflight must prove native-driver restoration, claim removal, VFIO-node removal, no target VMM,
   no VM state directory, and an unchanged service-GPU driver.
7. The command repeats with independent tenant B. PCI enumeration skips `nvidia-smi`, then the residue
   tool runs `probe-before-write` before the configured driver-functionality command. Tenant B must
   attest `prewrite: true`; use a host-class guest image/tool that prevents automatic driver
   initialization before that attestation.
8. Before the physical run, execute `images/qemu/tests/test-cloud-hypervisor-backend.sh`; its
   synthetic reset-failure regression must show that reset failure retains the durable claim/recovery
   journal rather than reissuing the group.

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

Review the local evidence before attaching the non-secret bundle to #658 and #655. Commit the
sanitized approved inventory and update `docs/runtime-parity.md` only after the physical results pass.

## Rollback

Normal rollback is managed teardown of only the exact locally created names
`vfio-<run-id>-a` and `vfio-<run-id>-b`. Do not delete their state directories or claim files by
hand: the recovery journal is required to restore the original drivers safely.

If teardown fails, leave the group quarantined, keep local GPU testing paused, preserve the VM state
and `/var/lib/agentic-sandbox/vms/.vfio-claims/iommu-<group>` evidence, and use out-of-band access. A
host power cycle is the final reset boundary when device reset or driver restoration cannot be
proven. Only after the power cycle and inspection may an operator use the documented managed
force-release path; never remove the claim directory as a shortcut.

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
- **Graphical session owns the test GPU:** end the maintenance attempt. The operator may deliberately
  log out and retry later; automation must not kill Xorg or stop the display manager.
- **Evidence promotion fails:** preserve `/var/tmp/agentic-vfio-evidence/run-<id>` locally and attach
  only the reviewed non-secret files.

## Evidence

The local command writes sanitized hardware inventories, guest enumeration, functionality and residue
results, managed teardown logs, a summary result, and SHA256 manifest beneath
`/var/tmp/agentic-vfio-evidence/run-<id>`. It must not contain SSH keys, bootstrap tokens,
`/etc/agentic-sandbox` secret files, full environment dumps, DMI serials, or arbitrary guest memory.
Promote the reviewed, non-secret evidence needed for tracker and long-term provenance manually.

## Future CI enablement

Do not add a physical workflow job for Titan or Grissom. A future CI host must have a dedicated test GPU,
independent service/management GPU, approved IOMMU inventory, installed residue utility and SSH
material, narrow sudo policy, and a unique restricted runner label. Enabling CI then requires an
intentional workflow implementation and review of the runner label, dispatch authorization, evidence
retention, and entrypoint confirmation/event contract. That deferred infrastructure work is tracked
in #659.
