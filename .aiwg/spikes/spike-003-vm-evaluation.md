# Spike 003: QEMU/Firecracker Evaluation

**Status:** Complete
**Date:** 2026-01-24
**Duration:** 30 minutes

## Objective

Determine whether to use Firecracker microVMs or QEMU/libvirt for VM-based isolation on this workstation.

## Environment Check

| Component | Status | Notes |
|-----------|--------|-------|
| KVM | **Available** | `/dev/kvm` accessible |
| User permissions | **OK** | User in kvm, libvirt groups |
| QEMU | **Installed** | `/usr/bin/qemu-system-x86_64` |
| libvirt | **Installed** | `/usr/bin/virsh`, existing VMs running |
| Firecracker | **Installed** | v1.14.1 downloaded to `bin/firecracker` |

## Existing Infrastructure

```bash
$ virsh list --all
 Id   Name               State
-----------------------------------
 1    dev-daily          running
 2    vault001           running
 -    dev-sandbox        shut off
 ...
```

There's already a `dev-sandbox` VM defined in libvirt. The user has experience with libvirt-managed VMs.

## Firecracker vs QEMU Comparison

| Feature | Firecracker | QEMU/libvirt |
|---------|-------------|--------------|
| Boot time | ~125ms | ~3-5 seconds |
| Memory overhead | ~5MB per VM | ~50-100MB per VM |
| Image format | Custom rootfs/kernel | qcow2, standard formats |
| Network | TAP devices, manual | libvirt-managed bridges |
| GPU passthrough | Not supported | Supported |
| Live migration | Not supported | Supported |
| Tooling | API-driven, minimal | virsh, virt-manager |
| Learning curve | Steeper | Lower (already in use) |

## Key Findings

### Firecracker
- **Pros**: Fast boot, low overhead, production-proven at AWS scale
- **Cons**: No pre-built images easily available, requires custom rootfs build, no GPU passthrough
- **Use case**: High-density, short-lived VMs (serverless)

### QEMU/libvirt
- **Pros**: Already working, familiar, GPU passthrough for ML workloads
- **Cons**: Higher overhead, slower boot
- **Use case**: Long-running VMs, GPU workloads, development

## Recommendation

**Use QEMU/libvirt as primary VM runtime** for this workstation because:

1. **Already operational** - libvirt is running with existing VMs
2. **GPU passthrough needed** - For ML agent workloads
3. **Lower learning curve** - User already has libvirt experience
4. **Flexible image management** - qcow2 snapshots, resize, etc.

**Keep Firecracker as optional** for:
- High-density scenarios (many small VMs)
- Production multi-tenant environments
- When sub-second boot times matter

## Runtime Selection Logic

```python
def select_runtime(spec):
    # GPU required → QEMU only
    if spec.requires_gpu:
        return "qemu"

    # User explicit preference
    if spec.runtime == "firecracker":
        return "firecracker" if firecracker_available() else "qemu"
    if spec.runtime == "qemu":
        return "qemu"

    # Default: Docker for most cases
    if spec.isolation == "container":
        return "docker"

    # VM default: QEMU (more features)
    return "qemu"
```

## Files Created

- `bin/firecracker` - Firecracker binary v1.14.1

## Next Steps

1. **Use existing libvirt setup** for VM sandboxes
2. **Adapt `runtimes/qemu/ubuntu-agent.xml`** for sandbox use
3. **Add Firecracker support later** if density becomes a concern
4. **Document image building** for both runtimes

## QEMU VM Configuration

The project already has `runtimes/qemu/ubuntu-agent.xml` with:
- 8GB RAM, 4 vCPU
- VirtIO disk and network
- GPU passthrough (commented out)
- Isolated network bridge

This can be adapted for sandbox use with minimal changes.
