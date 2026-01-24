# UC-003: Secure VM Sandbox for Untrusted Agent

## Use Case Overview

**ID**: UC-003
**Priority**: High
**Status**: Partially Implemented (QEMU structure defined, needs testing)
**Last Updated**: 2026-01-05

## Summary

Security engineer runs untrusted or experimental agent code in a QEMU/KVM virtual machine with hardware-level isolation. Even if agent exploits software vulnerabilities, hardware boundary prevents host compromise.

## Actors

**Primary**: Security Engineer
**Secondary**: Untrusted Agent (inside VM)
**Supporting**: QEMU/KVM Hypervisor, libvirt Manager

## Stakeholders and Interests

- **Security Engineer**: Requires maximum isolation for untrusted code analysis
- **Security Team**: Needs assurance that agent exploits cannot compromise host
- **Research Team**: Wants to experiment with third-party agent frameworks safely
- **Compliance**: Requires air-gapped execution for sensitive workloads

## Preconditions

- QEMU 8.0+ with KVM support installed on host
- libvirt 9.0+ configured and running
- Hardware virtualization enabled in BIOS (Intel VT-x or AMD-V)
- VM image built (ubuntu-agent.qcow2) and available
- Sufficient host resources (8GB RAM, 4 CPU cores minimum for VM)
- Engineer has permissions to create/manage VMs via virsh

## Postconditions

**Success**:
- VM launched with full hardware isolation
- Agent running in completely separate OS instance
- No kernel sharing between host and VM
- Agent exploits contained within VM boundary
- VM destroyed after task with no persistent state (if configured)

**Failure**:
- VM fails to launch with clear error message
- Resources cleaned up (no orphaned VM definitions)
- Error logged for troubleshooting

## Main Success Scenario

1. Security engineer runs: `./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent --untrusted`
2. Sandbox launcher generates libvirt XML from template
3. System configures VM with maximum isolation:
   - Separate kernel, no host kernel sharing
   - Isolated network bridge (no external access)
   - VirtIO drivers for I/O (performance + security)
   - UEFI secure boot enabled
   - Memory balloon for dynamic resource adjustment
4. libvirt creates KVM-accelerated VM with 8GB RAM, 4 vCPU
5. VM boots Ubuntu 24.04 LTS from qcow2 system disk
6. Engineer connects to serial console: `virsh console ubuntu-agent`
7. Engineer manually executes untrusted agent code inside VM
8. Agent attempts container escape exploit (kernel vulnerability)
9. Exploit succeeds at compromising VM kernel
10. Hardware isolation prevents hypervisor access
11. Host remains unaffected, other VMs unaffected
12. Engineer destroys VM: `virsh destroy ubuntu-agent && virsh undefine ubuntu-agent`
13. All VM state removed, host clean

**Expected Duration**: VM launch <2 minutes, analysis session hours to days

## Alternative Flows

**2a. Hardware virtualization not available**:
- QEMU falls back to software emulation (very slow)
- System warns: "KVM not available, performance degraded 10x+"
- Engineer can proceed or abort

**4a. Insufficient host memory for VM**:
- libvirt fails to allocate 8GB RAM
- System displays error with current memory usage
- Suggests reducing VM memory or stopping other VMs

**5a. VM boot fails (corrupted disk image)**:
- UEFI fails to find bootable device
- Serial console shows boot error
- Engineer can rebuild VM image or restore from backup

**7a. Agent requires GPU acceleration**:
- Engineer stops VM, modifies XML for GPU passthrough
- Engineer relaunches with: `--gpu passthrough`
- NVIDIA GPU passed through to VM via VFIO

**8a. Agent attempts network exfiltration**:
- Isolated network bridge blocks external connectivity
- Agent receives network unreachable error
- Audit log records connection attempt

## Exception Flows

**E1. Hypervisor vulnerability (VM escape attempt)**:
- Agent exploits QEMU/KVM bug to break VM boundary
- Hypervisor exploit succeeds (rare, requires 0-day)
- Host kernel may be compromised
- Incident response: Isolate host, forensic analysis, patch QEMU/kernel

**E2. Resource exhaustion inside VM (fork bomb)**:
- Agent spawns thousands of processes inside VM
- VM becomes unresponsive, host unaffected
- Engineer destroys VM via virsh (force shutdown)
- New VM launched for continued analysis

**E3. Disk image fills workspace**:
- qcow2 disk grows to maximum size (50GB default)
- VM filesystem full, operations fail
- Engineer can resize disk or clean up VM filesystem

**E4. Serial console hangs**:
- VirtIO console driver crash or kernel panic
- Engineer cannot access VM via virsh console
- Engineer can use VNC graphics console as fallback
- Force shutdown via virsh destroy

## Business Rules

**BR-001**: Untrusted workloads must use QEMU, never Docker (hardware isolation required)
**BR-002**: VM must use isolated network (no external access without explicit configuration)
**BR-003**: Workspace disk separate from system disk (easy to preserve or destroy)
**BR-004**: VM must be destroyed after untrusted code analysis (no persistent VM state)
**BR-005**: GPU passthrough requires IOMMU groups configured in host BIOS

## Special Requirements

### Performance
- VM launch latency: <2 minutes from command to console available
- VirtIO I/O: Near-native disk and network performance (90%+ of bare metal)
- CPU performance: 80%+ of host CPU speed with KVM acceleration
- GPU passthrough: Near-native GPU performance for ML/rendering workloads

### Security
- Hardware isolation: Separate kernel, no host kernel sharing
- UEFI secure boot: Verify VM kernel signature on boot
- Network isolation: Internal bridge only, no external access by default
- Resource isolation: VM cannot exhaust host memory or CPU (cgroups for QEMU process)
- Audit logging: VM lifecycle events (start, stop, crashes)

### Reliability
- VM stability: Graceful shutdown on host reboot
- Crash recovery: Auto-restart disabled (manual control for security analysis)
- Disk corruption protection: qcow2 snapshots for rollback

## Technology and Data Variations

**VM Image Variations**:
- ubuntu-agent: Ubuntu 24.04 LTS with development tools
- debian-agent: Debian stable for alternative distro testing
- windows-agent: Windows 11 VM (future, requires licensing)

**Disk Configurations**:
- Thin provisioning: qcow2 allocates disk space on-demand (default)
- Pre-allocated: Full disk size allocated upfront (better performance)
- Ephemeral: Workspace disk destroyed on VM shutdown (maximum security)

**Network Configurations**:
- Isolated: No external connectivity (default for untrusted workloads)
- Bridged: Access to host network (for integration testing)
- NAT: Outbound internet via host NAT (for package downloads)

## Open Issues

**OI-001**: GPU passthrough not yet tested (configuration present, needs validation)
**OI-002**: Windows VM support not implemented (licensing, driver complexity)
**OI-003**: Nested virtualization not tested (agent running VMs inside VM)
**OI-004**: Live migration between hosts not supported (single-host only)
**OI-005**: Checkpoint/resume for long-running analysis not implemented

## Frequency of Occurrence

- **Expected**: 2-5 untrusted agent analyses per week per security engineer
- **Peak**: 10-20 analyses during security research sprints
- **Team-wide**: 10-30 VM launches per month for 3 security engineers

## Assumptions

**A-001**: Host has hardware virtualization support (Intel VT-x or AMD-V)
**A-002**: Untrusted agent code is Linux-compatible (Ubuntu-based VM)
**A-003**: Security engineer has CLI familiarity with virsh, QEMU
**A-004**: VM network isolation sufficient (no advanced firewall evasion by agent)
**A-005**: Host kernel and QEMU patched against known hypervisor vulnerabilities

## Acceptance Criteria

- [ ] VM launches successfully in <2 minutes
- [ ] Serial console accessible via virsh console
- [ ] Agent can run arbitrary code inside VM without host access
- [ ] Simulated kernel exploit contained within VM (host unaffected)
- [ ] Network isolation blocks unauthorized external connections
- [ ] Resource limits prevent VM from exhausting host memory/CPU
- [ ] VM destruction removes all VM state (no orphaned files)
- [ ] Multiple VMs (2+) run concurrently without interference
- [ ] GPU passthrough works for CUDA-enabled workloads (if configured)
- [ ] Workspace disk persists across VM restarts (if not ephemeral)

## Notes

- This use case provides maximum security for untrusted code
- Performance overhead (10-20%) acceptable trade-off for hardware isolation
- QEMU/KVM vulnerabilities are rare but not impossible (monitor CVEs)
- Consider using nested virtualization for agent-in-agent scenarios (future)
- GPU passthrough enables ML model training in isolated VM (high value)

## Related Use Cases

- **UC-001**: Launch Autonomous Coding Agent (QEMU alternative to Docker)
- **UC-002**: Git Repository Operations via Proxy (same proxy model applies to VMs)
- **UC-004**: Resource-Limited Sandbox Execution (VM resource limits enforced)
