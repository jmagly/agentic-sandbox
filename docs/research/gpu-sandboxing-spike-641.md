# GPU Sandboxing and Passthrough Recommendation

**Issues:** #641 (research) · #655 (Cloud Hypervisor implementation)
**Decision date:** 2026-07-17
**Current implementation:** whole-device VFIO passthrough for Cloud Hypervisor, with reset-gated
cold hand-outs.

## Decision

Use a dedicated, ACS-isolated GPU through VFIO for the current strong-isolation path. A GPU and
every companion function in its IOMMU group are assigned to exactly one VM. The Cloud Hypervisor
backend owns binding to `vfio-pci`, resets the GPU before a hand-out and during teardown, restores
the original host drivers, and keeps a durable per-IOMMU-group claim to prevent double assignment.

GPU-backed Cloud Hypervisor VMs are not eligible for snapshot, restore, fork, or warm-pool flows.
The generic `vfio-pci` driver does not expose migratable device state, and a physical GPU cannot be
shared by forked children. Reusing one of those paths would risk stale device/VRAM state and
exclusive-device contention. GPU workloads therefore use a fresh cold VM after successful reset.

## Options matrix

| Option | Isolation | Shareability | Cost / constraints | Recommendation |
| --- | --- | --- | --- | --- |
| Full-device VFIO | Strongest available: VM boundary plus IOMMU DMA isolation | One IOMMU group per VM | Host loses the device; needs IOMMU/ACS, spare GPU, reset support, and guest driver | **Use now** for untrusted or cross-tenant workloads |
| NVIDIA MIG | Hardware partitions compute and memory resources | Multiple GPU instances per supported GPU | Restricted NVIDIA hardware/driver matrix; operational lifecycle and reset behavior vary by generation | Evaluate for multi-GPU production capacity after a separate isolation validation |
| NVIDIA vGPU / mediated device | VM boundary with vendor-managed sharing | Multiple vGPUs per physical GPU | Supported GPU/hypervisor matrix and commercial licensing; additional host driver/control plane | Consider only when utilization justifies licensing and a supported CH integration exists |
| Generic mdev | Depends on the vendor parent driver | Potentially shareable | No general-purpose NVIDIA consumer-GPU path; lifecycle is vendor-specific | Do not select as the baseline |
| Container GPU | Weakest for hostile tenants: shares host kernel and host GPU driver | High | Fast and inexpensive; no VM/IOMMU boundary between workload and host driver | Trusted single-team development only |

Linux VFIO treats the IOMMU group as the ownership unit; a group is viable only when all devices
are bound to a VFIO driver. Cloud Hypervisor likewise requires every member of a shared group
(commonly a GPU plus its audio function) to be bound and passed to the guest. See the
[Linux VFIO documentation](https://www.kernel.org/doc/html/latest/driver-api/vfio.html) and
[Cloud Hypervisor VFIO HOWTO](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/vfio.md).

MIG and vGPU remain capacity options rather than transparent substitutes. NVIDIA documents the
generation-specific reset behavior in its
[MIG guide](https://docs.nvidia.com/datacenter/tesla/mig-user-guide/getting-started-with-mig.html)
and the restricted hardware and software matrix in its
[vGPU supported-products documentation](https://docs.nvidia.com/vgpu/latest/product-support-matrix/).

## Recommendation by host class

### Single-GPU developer workstation

- Do not detach a GPU that owns the host display.
- Prefer CPU execution or container GPU access only for trusted local code.
- If the GPU is headless/dedicated, allow VFIO only as a single-tenant reservation.
- If the device exposes no PCI reset, do not rotate it between tenants. A host power cycle is the
  safe boundary; `AGENTIC_CH_VFIO_ALLOW_NO_RESET=1` is only for a reviewed single-tenant host.
  After use, the claim stays quarantined until a host power cycle and an explicit
  `AGENTIC_CH_VFIO_FORCE_RELEASE_AFTER_POWER_CYCLE=1` managed teardown.

### Multi-GPU Linux/KVM host

- Reserve at least one headless compute GPU for the host and one for sandbox VFIO.
- Put passthrough GPUs in ACS-isolated slots. The backend rejects IOMMU groups containing unrelated
  PCI slots by default.
- Schedule one VM per claimed IOMMU group. Queue requests while the group claim exists.
- Require the sysfs PCI `reset` interface and run the cold hand-out verification for every approved
  GPU/firmware/host-kernel class.
- Evaluate MIG/vGPU only when concurrent utilization is worth the added hardware, licensing, and
  control-plane constraints.

### Apple silicon

The current passthrough implementation is Linux/KVM-specific. The project has no Metal or ANE
passthrough contract for the Apple container/runtime path, so accelerated Apple workloads remain
outside this decision.

## Binding and teardown contract

For a configured PCI address such as `0000:41:00.0`, the CH backend:

1. Validates the BDF, device, and IOMMU-group link.
2. Rejects a group containing a different PCI slot unless
   `AGENTIC_CH_VFIO_ALLOW_UNSAFE_GROUP=1` was explicitly approved.
3. Atomically claims `<VM_STORAGE_DIR>/.vfio-claims/iommu-<group>`.
4. Refuses the hand-out while the primary GPU's DRM/NVIDIA device node is open by a host process.
5. Records the original driver and `driver_override` for every group member.
6. Loads `vfio-pci`, unbinds native drivers, sets `driver_override=vfio-pci`, and binds the whole
   group.
7. Waits for devtmpfs to create `/dev/vfio/<group>`, grants the claimed backend account owner-only
   access when the dynamic node is root-only, and verifies it is a readable/writable character device.
8. Requires and invokes the primary GPU's sysfs `reset` before launch.
9. Adds one `--device path=/sys/bus/pci/devices/<BDF>/` argument per group member.
10. On destroy/reap, stops the VMM, resets the GPU again, unbinds `vfio-pci`, restores each original
   override and driver, and releases the group claim.

Linux documents `driver_override` as an explicit match override that does not itself bind or unbind
a device, which is why the backend performs every transition explicitly. Linux also exposes a
`reset` sysfs file only when the device supports function reset. See
[driver binding](https://docs.kernel.org/driver-api/driver-model/binding.html) and the
[PCI sysfs ABI](https://docs.kernel.org/admin-guide/abi-testing.html).

## Cross-tenant residue and reset

- A successful PCI reset is the required hand-out boundary. Missing reset support fails closed.
- Teardown attempts reset even when restoring a host driver later fails, and reports a non-zero
  status so VM state is not silently treated as safely recycled. Any reset or driver-restore
  failure preserves the claim and original-driver records for recovery.
- A durable group claim prevents concurrent use and survives a stopped VMM. Only managed destroy
  or reaping releases it.
- Snapshot/restore, fork, and warm-pool commands reject GPU/VFIO source metadata. Generic
  `vfio-pci` is non-migratable in Cloud Hypervisor, and duplicating one physical device across
  children is invalid.
- A reset is not assumed to erase tenant data unless the host/device combination passes the
  validation runbook below. Devices that fail residue validation are single-tenant only.

## Scheduling and quota

The scheduling unit is an IOMMU group, not an individual PCI function. The current durable claim is
the local exclusion primitive; higher-level schedulers should treat claim contention as
`resource busy`, queue fairly, and charge the entire claim duration to the tenant. MIG/vGPU
profiles would require a separate inventory and quota model.

## Host-class validation

### Current host evidence

On `grissom` (2026-07-17), the pinned Cloud Hypervisor v53.0 and `CLOUDHV.fd` assets matched
the repository SHA256 pins. A flattened canonical agent base booted Linux under the real VMM when
firmware was supplied with `--firmware` and the boot/cloud-init disks were fixed at PCI slots 1 and
2, matching the edk2 boot paths. Cloud Hypervisor documents firmware boot separately from direct
kernel boot in its [official README](https://github.com/cloud-hypervisor/cloud-hypervisor#booting-linux).

The managed VFIO lifecycle was also exercised against an unused, isolated, reset-capable Realtek
RTS5260 card reader in IOMMU group 17. The device was bound to `vfio-pci`, passed through with
Cloud Hypervisor, enumerated in the guest as `10ec:5260`, and then restored to `rtsx_pci` after
confirmed VMM exit. The group claim and `/dev/vfio/17` node were removed. Structured evidence is in
[`evidence/ch-vfio-proxy-grissom-2026-07-17.json`](evidence/ch-vfio-proxy-grissom-2026-07-17.json).
This proves the real generic VFIO lifecycle, but it does **not** substitute for GPU enumeration or
cross-tenant GPU residue evidence.

The NVIDIA group 16 GPU and Intel group 0 iGPU are both owned by the active graphical session.
Detaching either would disrupt the workstation, so GPU acceptance remains gated on a maintenance
window or a dedicated GPU host.

Run these checks before approving a GPU class:

```bash
# Host prerequisites and isolation
test -e /sys/bus/pci/devices/0000:41:00.0/iommu_group
readlink -f /sys/bus/pci/devices/0000:41:00.0/iommu_group
ls -1 /sys/bus/pci/devices/0000:41:00.0/iommu_group/devices
test -e /sys/bus/pci/devices/0000:41:00.0/reset

# Provision and start with AGENTIC_BACKEND=cloud-hypervisor, then prove the
# vendor/device ID is enumerated inside the guest and retain JSON evidence.
images/qemu/tests/verify-ch-gpu-passthrough.sh VM_NAME \
  --host GUEST_IP --user agent --key ~/.ssh/agentic_sandbox

# Destroy through the managed path; do not remove the VM directory first.
AGENTIC_BACKEND=cloud-hypervisor scripts/destroy-vm.sh VM_NAME --force
```

The verifier rejects a non-display-class primary, queries the live CH API for the exact device
path, requires every IOMMU-group member to be bound to `vfio-pci`, and records the group, reset
method, guest `lspci` line, and optional `nvidia-smi` output.

For cross-tenant approval, run a tenant-A VRAM fill/probe workload, destroy the VM, then launch an
independent tenant-B VM and execute the device-specific residue probe before any tenant-B writes.
Retain the GPU model, VBIOS, host firmware, kernel, `reset_method`, driver versions, and result.
Passing PCI enumeration alone proves assignment, not VRAM sanitization.
