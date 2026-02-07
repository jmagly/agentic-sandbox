# Resource Quota and cgroup v2 Implementation Design

**Document Version**: 1.0
**Date**: 2026-01-31
**Author**: Security Architect
**Status**: Draft - Ready for Review

---

## Executive Summary

This document defines a comprehensive resource quota system for agentic-sandbox VMs to address the current security gaps:

| Gap | Risk Level | Impact |
|-----|------------|--------|
| No memory limits | HIGH | VM can exhaust host memory, triggering OOM across system |
| No disk quotas | HIGH | 1.9TB shared storage can be filled by single agent |
| No CPU throttling | MEDIUM | Runaway process can starve other VMs |
| Unlimited PIDs | HIGH | Fork bomb can crash VM and stress host |
| Unlimited file descriptors | MEDIUM | FD exhaustion can cause service failures |

The design provides defense-in-depth through three layers:
1. **VM-level limits** (libvirt/QEMU) - Hard memory and vCPU caps
2. **cgroup v2 limits** (systemd) - Process, memory, and I/O controls inside VM
3. **Disk quotas** (XFS project quotas) - Per-task storage limits on virtiofs mounts

---

## 1. Architecture Overview

```
+==============================================================================+
|                              HOST SYSTEM                                      |
|                                                                               |
|  +-------------------------------------------------------------------------+  |
|  |                    LAYER 1: libvirt Resource Limits                     |  |
|  |  - Hard memory ceiling per VM (no overcommit)                           |  |
|  |  - vCPU pinning and scheduling weight                                   |  |
|  |  - Block I/O throttling per VM                                          |  |
|  +-------------------------------------------------------------------------+  |
|                                     |                                         |
|  +-------------------------------------------------------------------------+  |
|  |                    LAYER 2: XFS Project Quotas                          |  |
|  |  - /srv/agentshare mounted with prjquota                                |  |
|  |  - Per-task project IDs with size limits                                |  |
|  |  - Quota enforcement at filesystem level                                |  |
|  +-------------------------------------------------------------------------+  |
|                                     |                                         |
+=====================================|=========================================+
                                      |
+=====================================|=========================================+
|                              VM GUEST                                         |
|                                     |                                         |
|  +-------------------------------------------------------------------------+  |
|  |                    LAYER 3: systemd cgroup v2 Limits                    |  |
|  |  - MemoryMax, MemoryHigh (soft limit with OOM protection)               |  |
|  |  - TasksMax (PID limit)                                                 |  |
|  |  - CPUWeight, CPUQuota                                                  |  |
|  |  - IOWeight, IOReadBandwidthMax, IOWriteBandwidthMax                    |  |
|  |  - LimitNOFILE (file descriptor limit)                                  |  |
|  +-------------------------------------------------------------------------+  |
+===============================================================================+
```

---

## 2. Resource Limit Specifications

### 2.1 Default Resource Profiles

| Profile | Memory | vCPU | Disk | PIDs | FDs | I/O Weight |
|---------|--------|------|------|------|-----|------------|
| **standard** | 8GB | 4 | 50GB | 2048 | 65536 | 500 |
| **minimal** | 4GB | 2 | 20GB | 1024 | 32768 | 250 |
| **heavy** | 16GB | 8 | 100GB | 4096 | 131072 | 750 |

### 2.2 Limit Rationale

#### Memory Limits

| Setting | Value | Justification |
|---------|-------|---------------|
| `MemoryMax` (hard) | 7.5GB (94% of VM) | Leave 500MB for kernel, systemd |
| `MemoryHigh` (soft) | 6GB (75% of VM) | Trigger throttling before OOM |
| `MemorySwapMax` | 0 | Disable swap to prevent unpredictable latency |

**Why 7.5GB for 8GB VM?**
- Linux kernel requires ~100-200MB
- systemd and essential services: ~100MB
- QEMU guest agent: ~50MB
- Buffer for cgroup accounting overhead

#### PID/Task Limits

| Setting | Value | Justification |
|---------|-------|---------------|
| `TasksMax` | 2048 | Sufficient for Claude Code + build tools |
| Fork bomb threshold | ~1000 | Detected before hitting limit |

**Calculation:**
- Claude Code: ~50-100 processes (node, git, language servers)
- Docker build: ~200-500 processes
- Concurrent npm/cargo: ~100-300 processes
- Safety margin: 2x expected peak

#### CPU Limits

| Setting | Value | Justification |
|---------|-------|---------------|
| `CPUQuota` | 400% (4 cores) | Match VM vCPU allocation |
| `CPUWeight` | 100 (default) | Fair sharing with other services |

**Why CPUQuota = vCPUs * 100%?**
- Allows full utilization of allocated resources
- Prevents stealing from host kernel or other VMs
- libvirt already limits at hypervisor level

#### I/O Limits

| Setting | Value | Justification |
|---------|-------|---------------|
| `IOWeight` | 500 | Balanced priority (100-10000 scale) |
| `IOReadBandwidthMax` | 500MB/s | Prevent SSD saturation |
| `IOWriteBandwidthMax` | 200MB/s | Protect shared storage write path |

**virtiofs I/O Considerations:**
- virtiofs writes go through host page cache
- Large burst writes can pressure host memory
- Rate limiting prevents cache thrashing

#### File Descriptor Limits

| Setting | Value | Justification |
|---------|-------|---------------|
| `LimitNOFILE` (soft) | 65536 | Sufficient for most dev tools |
| `LimitNOFILE` (hard) | 65536 | Same as soft (no escalation) |

**Risk without limit:**
- Process can open unlimited FDs
- Each FD consumes kernel memory
- Can exhaust system-wide FD limit

---

## 3. Implementation Details

### 3.1 libvirt XML Enhancements

Update `define_vm()` in `/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh`:

```xml
<domain type='kvm'>
  <name>$vm_name</name>
  <memory unit='MiB'>$memory_mb</memory>
  <currentMemory unit='MiB'>$memory_mb</currentMemory>
  <vcpu placement='static'>$cpus</vcpu>

  <!-- Memory tuning: No overcommit, locked pages -->
  <memtune>
    <hard_limit unit='MiB'>$((memory_mb + 256))</hard_limit>
    <soft_limit unit='MiB'>$memory_mb</soft_limit>
  </memtune>

  <!-- CPU tuning: Scheduling and shares -->
  <cputune>
    <shares>$((cpus * 1024))</shares>
    <period>100000</period>
    <quota>$((cpus * 100000))</quota>
  </cputune>

  <!-- Block I/O tuning -->
  <blkiotune>
    <weight>500</weight>
    <device>
      <path>/path/to/vda</path>
      <read_bytes_sec>524288000</read_bytes_sec>
      <write_bytes_sec>209715200</write_bytes_sec>
    </device>
  </blkiotune>

  <!-- ... rest of domain definition ... -->
</domain>
```

### 3.2 systemd Service with cgroup v2 Limits

Create `/home/roctinam/dev/agentic-sandbox/agent-rs/systemd/agent-client-hardened.service`:

```ini
[Unit]
Description=Agentic Sandbox Agent Client (Hardened)
Documentation=https://git.integrolabs.net/roctinam/agentic-sandbox
After=network-online.target qemu-guest-agent.service
Wants=network-online.target

[Service]
Type=simple
User=agent
Group=agent
WorkingDirectory=/home/agent

# Environment
EnvironmentFile=/etc/agentic-sandbox/agent.env

# Executable
ExecStart=/opt/agentic-sandbox/bin/agent-client

# Restart policy
Restart=always
RestartSec=5
StartLimitBurst=5
StartLimitIntervalSec=300

# ============================================================================
# RESOURCE LIMITS (cgroup v2)
# ============================================================================

# Memory limits - hard ceiling prevents OOM affecting other processes
MemoryMax=7680M
MemoryHigh=6144M
MemorySwapMax=0

# Task/PID limit - fork bomb defense
TasksMax=2048

# CPU limits - match VM allocation
CPUQuota=400%
CPUWeight=100

# I/O limits - prevent storage abuse
IOWeight=500
IOReadBandwidthMax=/dev/vda 500M
IOWriteBandwidthMax=/dev/vda 200M

# File descriptor limits
LimitNOFILE=65536

# Core dumps disabled (security)
LimitCORE=0

# ============================================================================
# SECURITY HARDENING
# ============================================================================

# Prevent privilege escalation
NoNewPrivileges=true

# Filesystem protection
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/home/agent /tmp /mnt/inbox /mnt/outbox /var/log
PrivateTmp=true
PrivateDevices=true

# Network restrictions (allow all for agent operations)
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK

# System call filtering (allow needed syscalls)
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources
SystemCallErrorNumber=EPERM

# Misc hardening
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictRealtime=true
RestrictSUIDSGID=true
MemoryDenyWriteExecute=false  # Required for JIT (Node.js, etc.)

# ============================================================================
# LOGGING
# ============================================================================

StandardOutput=journal
StandardError=journal
SyslogIdentifier=agent-client

# Audit resource limit events
AuditResourceLimits=true

[Install]
WantedBy=multi-user.target
```

### 3.3 XFS Project Quota Configuration

#### 3.3.1 Host Setup Script

Create `/home/roctinam/dev/agentic-sandbox/scripts/setup-disk-quotas.sh`:

```bash
#!/bin/bash
# setup-disk-quotas.sh - Initialize XFS project quotas for agentshare
#
# Prerequisites:
#   - /srv/agentshare mounted on XFS filesystem
#   - Mount option 'prjquota' enabled
#
# Usage: sudo ./setup-disk-quotas.sh

set -euo pipefail

AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"
PROJID_FILE="/etc/projid"
PROJECTS_FILE="/etc/projects"

# Project ID allocation
# 1000-1999: Reserved for system
# 2000-9999: Task inboxes (task-{uuid} -> project ID)
# 10000+: Future use
PROJECT_ID_START=2000
PROJECT_ID_MAX=9999

# Default quota per task
DEFAULT_QUOTA_GB=50
GLOBAL_QUOTA_GB=100

log_info() { echo "[INFO] $*"; }
log_error() { echo "[ERROR] $*" >&2; }

# Check XFS mount options
check_xfs_prjquota() {
    local mount_point="$1"

    if ! mount | grep -q "$mount_point.*xfs"; then
        log_error "$mount_point is not an XFS filesystem"
        echo "To create XFS with quotas:"
        echo "  mkfs.xfs /dev/sdX"
        echo "  mount -o prjquota /dev/sdX $mount_point"
        exit 1
    fi

    if ! mount | grep -q "$mount_point.*prjquota"; then
        log_error "$mount_point not mounted with prjquota option"
        echo "Add 'prjquota' to mount options in /etc/fstab:"
        echo "  /dev/sdX  $mount_point  xfs  defaults,prjquota  0  0"
        echo "Then remount: mount -o remount,prjquota $mount_point"
        exit 1
    fi

    log_info "XFS prjquota verified for $mount_point"
}

# Initialize quota files
init_quota_files() {
    touch "$PROJID_FILE" "$PROJECTS_FILE"
    chmod 644 "$PROJID_FILE" "$PROJECTS_FILE"
}

# Add project for global share
setup_global_quota() {
    local project_name="agentshare_global"
    local project_id=1001
    local path="$AGENTSHARE_ROOT/global"

    # Add to projid
    if ! grep -q "^${project_name}:" "$PROJID_FILE"; then
        echo "${project_name}:${project_id}" >> "$PROJID_FILE"
    fi

    # Add to projects
    if ! grep -q "^${project_id}:" "$PROJECTS_FILE"; then
        echo "${project_id}:${path}" >> "$PROJECTS_FILE"
    fi

    # Initialize project
    xfs_quota -x -c "project -s ${project_name}" "$AGENTSHARE_ROOT"

    # Set quota (soft = 90%, hard = 100%)
    local soft_kb=$((GLOBAL_QUOTA_GB * 1024 * 1024 * 90 / 100))
    local hard_kb=$((GLOBAL_QUOTA_GB * 1024 * 1024))
    xfs_quota -x -c "limit -p bsoft=${soft_kb}k bhard=${hard_kb}k ${project_name}" "$AGENTSHARE_ROOT"

    log_info "Global quota set: ${GLOBAL_QUOTA_GB}GB for $path"
}

# Create task-specific quota
create_task_quota() {
    local task_id="$1"
    local quota_gb="${2:-$DEFAULT_QUOTA_GB}"

    # Generate unique project ID from task UUID
    local hash=$(echo -n "$task_id" | md5sum | cut -c1-8)
    local project_id=$((16#$hash % (PROJECT_ID_MAX - PROJECT_ID_START) + PROJECT_ID_START))
    local project_name="task_${task_id:0:8}"

    local inbox_path="$AGENTSHARE_ROOT/tasks/${task_id}/inbox"
    local outbox_path="$AGENTSHARE_ROOT/tasks/${task_id}/outbox"

    # Create directories
    mkdir -p "$inbox_path" "$outbox_path"

    # Add to projid (if not exists)
    if ! grep -q "^${project_name}:" "$PROJID_FILE"; then
        echo "${project_name}:${project_id}" >> "$PROJID_FILE"
    fi

    # Add inbox to projects
    if ! grep -q "^${project_id}:${inbox_path}" "$PROJECTS_FILE"; then
        echo "${project_id}:${inbox_path}" >> "$PROJECTS_FILE"
    fi

    # Initialize project
    xfs_quota -x -c "project -s ${project_name}" "$AGENTSHARE_ROOT"

    # Set quota
    local soft_kb=$((quota_gb * 1024 * 1024 * 90 / 100))
    local hard_kb=$((quota_gb * 1024 * 1024))
    xfs_quota -x -c "limit -p bsoft=${soft_kb}k bhard=${hard_kb}k ${project_name}" "$AGENTSHARE_ROOT"

    echo "$project_id"
}

# Remove task quota
remove_task_quota() {
    local task_id="$1"
    local project_name="task_${task_id:0:8}"

    # Remove quota limit
    xfs_quota -x -c "limit -p bsoft=0 bhard=0 ${project_name}" "$AGENTSHARE_ROOT" 2>/dev/null || true

    # Remove from files
    sed -i "/^${project_name}:/d" "$PROJID_FILE" 2>/dev/null || true
    sed -i "/${task_id}/d" "$PROJECTS_FILE" 2>/dev/null || true
}

# Report quota usage
report_quotas() {
    echo "=== XFS Project Quota Report ==="
    xfs_quota -x -c "report -p -h" "$AGENTSHARE_ROOT"
}

# Main
main() {
    case "${1:-setup}" in
        setup)
            check_xfs_prjquota "$AGENTSHARE_ROOT"
            init_quota_files
            setup_global_quota
            report_quotas
            ;;
        create)
            create_task_quota "$2" "${3:-$DEFAULT_QUOTA_GB}"
            ;;
        remove)
            remove_task_quota "$2"
            ;;
        report)
            report_quotas
            ;;
        *)
            echo "Usage: $0 {setup|create <task-id> [quota-gb]|remove <task-id>|report}"
            exit 1
            ;;
    esac
}

main "$@"
```

#### 3.3.2 Alternative: ext4 Quota (if XFS not available)

For ext4 filesystems, use user/group quotas mapped to task UIDs:

```bash
# /etc/fstab
/dev/sdX  /srv/agentshare  ext4  defaults,usrquota,grpquota  0  2

# Enable quotas
quotacheck -cug /srv/agentshare
quotaon /srv/agentshare

# Set per-user quota (map task to UID)
setquota -u $TASK_UID 0 50G 0 0 /srv/agentshare
```

### 3.4 Cloud-init Integration

Update cloud-init in `provision-vm.sh` to deploy hardened service:

```yaml
write_files:
  # Resource limits configuration
  - path: /etc/agentic-sandbox/resource-limits.conf
    permissions: '0644'
    content: |
      # Agent resource limits (read by agent-client)
      MEMORY_LIMIT_MB=7680
      MEMORY_SOFT_MB=6144
      TASKS_MAX=2048
      CPU_QUOTA_PERCENT=400
      IO_READ_MBPS=500
      IO_WRITE_MBPS=200
      NOFILE_LIMIT=65536

  # Hardened systemd service
  - path: /etc/systemd/system/agentic-agent.service
    content: |
      [Unit]
      Description=Agentic Sandbox Agent Client (Hardened)
      After=network-online.target
      Wants=network-online.target

      [Service]
      Type=simple
      User=agent
      Group=agent
      EnvironmentFile=/etc/agentic-sandbox/agent.env
      ExecStart=/opt/agentic-sandbox/bin/agent-client

      # Resource limits
      MemoryMax=7680M
      MemoryHigh=6144M
      MemorySwapMax=0
      TasksMax=2048
      CPUQuota=400%
      IOWeight=500
      LimitNOFILE=65536
      LimitCORE=0

      # Security
      NoNewPrivileges=true
      ProtectSystem=strict
      ProtectHome=read-only
      ReadWritePaths=/home/agent /tmp /mnt/inbox /mnt/outbox
      PrivateTmp=true

      Restart=always
      RestartSec=5

      [Install]
      WantedBy=multi-user.target

runcmd:
  # Verify cgroup v2 is available
  - |
    if ! grep -q "cgroup2" /proc/filesystems; then
      echo "WARNING: cgroup v2 not available, resource limits may not work"
    fi

  # Apply sysctl limits
  - sysctl -w vm.max_map_count=262144
  - sysctl -w fs.file-max=500000
  - echo "vm.max_map_count=262144" >> /etc/sysctl.d/99-agent.conf
  - echo "fs.file-max=500000" >> /etc/sysctl.d/99-agent.conf

  # Configure systemd default limits
  - mkdir -p /etc/systemd/system.conf.d
  - |
    cat > /etc/systemd/system.conf.d/agent-limits.conf <<EOF
    [Manager]
    DefaultTasksMax=2048
    DefaultLimitNOFILE=65536
    EOF

  # Reload and start
  - systemctl daemon-reload
  - systemctl enable agentic-agent
  - systemctl start agentic-agent
```

---

## 4. Failure Mode Analysis

### 4.1 Memory Exhaustion

| Scenario | Trigger | System Response | Agent Experience |
|----------|---------|-----------------|------------------|
| Gradual growth | Allocation crosses MemoryHigh | Throttling, reclaim pressure | Slowdown, may trigger GC |
| Rapid allocation | Allocation crosses MemoryMax | OOM killer terminates process | Process killed, agent restarts |
| Memory leak | Slow growth over hours | Eventually hits MemoryMax | Same as rapid allocation |

**Recommended Agent Behavior:**
- Monitor `/sys/fs/cgroup/memory.current`
- Implement graceful degradation at 80% usage
- Checkpoint long-running tasks periodically

### 4.2 PID Exhaustion (Fork Bomb)

| Scenario | Trigger | System Response | Agent Experience |
|----------|---------|-----------------|------------------|
| Fork bomb | Malicious/buggy code | `fork()` returns EAGAIN | Process cannot spawn children |
| Build tool explosion | cargo build -j 256 | Excess workers fail to spawn | Build continues with fewer workers |

**Detection:**
```bash
# From host, monitor task count
cat /sys/fs/cgroup/system.slice/*/pids.current
```

### 4.3 Disk Quota Exceeded

| Scenario | Trigger | System Response | Agent Experience |
|----------|---------|-----------------|------------------|
| Large clone | `git clone huge-repo` | Write returns EDQUOT | Clone fails with "No space left" |
| Build artifacts | `cargo build --release` | Writes fail at quota | Build fails, cache unusable |

**Graceful Handling:**
- Agent should check available space before large operations
- Report quota status in progress updates

### 4.4 I/O Throttling

| Scenario | Trigger | System Response | Agent Experience |
|----------|---------|-----------------|------------------|
| Large file copy | `cp -r /mnt/global/dataset .` | I/O limited to configured rate | Copy takes longer |
| Concurrent writes | Multiple processes writing | I/O shared within limit | All processes slowed |

**Note:** I/O throttling is "soft" - operations complete eventually, just slower.

---

## 5. Monitoring and Observability

### 5.1 Metrics to Expose

```rust
// management/src/metrics.rs additions
struct AgentResourceMetrics {
    // Memory
    memory_usage_bytes: Gauge,
    memory_limit_bytes: Gauge,
    memory_oom_kills: Counter,

    // CPU
    cpu_usage_seconds: Counter,
    cpu_throttled_seconds: Counter,

    // Tasks
    tasks_current: Gauge,
    tasks_limit: Gauge,

    // I/O
    io_read_bytes: Counter,
    io_write_bytes: Counter,

    // Disk
    disk_usage_bytes: Gauge,
    disk_quota_bytes: Gauge,
}
```

### 5.2 Health Check Enhancements

Update `/opt/agentic-sandbox/health/health-server.py`:

```python
def collect_health(self):
    cgroup_path = "/sys/fs/cgroup"

    return {
        "status": "healthy",
        "resources": {
            "memory": {
                "current_mb": self._read_cgroup_bytes(f"{cgroup_path}/memory.current") / 1024 / 1024,
                "max_mb": self._read_cgroup_bytes(f"{cgroup_path}/memory.max") / 1024 / 1024,
                "usage_pct": self._memory_usage_percent(),
            },
            "tasks": {
                "current": self._read_cgroup_int(f"{cgroup_path}/pids.current"),
                "max": self._read_cgroup_int(f"{cgroup_path}/pids.max"),
            },
            "disk": {
                "inbox_used_mb": self._get_disk_usage("/mnt/inbox") / 1024 / 1024,
                "quota_mb": self._get_disk_quota("/mnt/inbox") / 1024 / 1024,
            }
        }
    }
```

### 5.3 Alert Thresholds

| Metric | Warning | Critical | Action |
|--------|---------|----------|--------|
| `memory_usage_pct` | 80% | 90% | Notify, prepare for OOM |
| `tasks_current / tasks_max` | 75% | 90% | Warn about fork bomb risk |
| `disk_usage_pct` | 80% | 95% | Warn, block new writes |
| `cpu_throttled_pct` | 50% | 80% | Inform of performance impact |

---

## 6. Testing Strategy

### 6.1 Unit Tests

```rust
// management/src/orchestrator/resource_limits_test.rs

#[test]
fn test_parse_memory_limit() {
    assert_eq!(parse_memory_limit("8G"), 8 * 1024 * 1024 * 1024);
    assert_eq!(parse_memory_limit("512M"), 512 * 1024 * 1024);
}

#[test]
fn test_quota_allocation() {
    let quota = TaskQuota::new("task-123", QuotaProfile::Standard);
    assert_eq!(quota.disk_gb, 50);
    assert_eq!(quota.pids, 2048);
}
```

### 6.2 Integration Tests

```python
# tests/e2e/test_resource_limits.py

class TestResourceLimits:
    """Resource limit enforcement tests"""

    def test_memory_limit_enforcement(self, vm):
        """Verify OOM kill when exceeding memory limit"""
        result = vm.ssh("python3 -c 'x = [0] * (10 * 1024 * 1024 * 1024)'")
        assert result.returncode != 0
        assert "Killed" in result.stderr or "MemoryError" in result.stderr

    def test_pid_limit_enforcement(self, vm):
        """Verify fork bomb is contained"""
        # This should eventually fail, not crash the VM
        result = vm.ssh("bash -c 'while true; do /bin/true & done'", timeout=10)
        assert "Resource temporarily unavailable" in result.stderr

        # VM should still be responsive
        assert vm.ssh("echo ok").stdout.strip() == "ok"

    def test_disk_quota_enforcement(self, vm):
        """Verify disk quota prevents overfill"""
        # Try to write 60GB to a 50GB quota
        result = vm.ssh("dd if=/dev/zero of=/mnt/inbox/bigfile bs=1M count=61440")
        assert result.returncode != 0
        assert "No space left" in result.stderr or "Disk quota exceeded" in result.stderr

    def test_io_throttling(self, vm):
        """Verify I/O is rate-limited"""
        # Write 1GB and measure time
        start = time.time()
        vm.ssh("dd if=/dev/zero of=/mnt/inbox/testfile bs=1M count=1024 oflag=direct")
        elapsed = time.time() - start

        # With 200MB/s limit, should take at least 5 seconds
        assert elapsed >= 5.0
```

### 6.3 Chaos Tests

```bash
# scripts/chaos/test-resource-limits.sh

#!/bin/bash
# Chaos test: resource exhaustion scenarios

VM_NAME="${1:-agent-01}"
VM_IP=$(virsh domifaddr "$VM_NAME" | grep -oE '([0-9]+\.){3}[0-9]+')

echo "=== Chaos Test: Resource Limits ==="

# Test 1: Fork bomb (should be contained)
echo "[1/4] Testing fork bomb containment..."
timeout 30 ssh agent@$VM_IP "bash -c ':(){ :|:& };:'" 2>&1 || true
# Verify VM is still alive
if ssh -o ConnectTimeout=5 agent@$VM_IP "echo ok" | grep -q ok; then
    echo "  PASS: VM survived fork bomb"
else
    echo "  FAIL: VM became unresponsive"
fi

# Test 2: Memory exhaustion
echo "[2/4] Testing memory limit..."
ssh agent@$VM_IP "python3 -c 'import sys; x=[0]*sys.maxsize'" 2>&1 | head -5
if ssh agent@$VM_IP "echo ok" | grep -q ok; then
    echo "  PASS: VM survived memory exhaustion"
else
    echo "  FAIL: VM became unresponsive"
fi

# Test 3: Disk fill attempt
echo "[3/4] Testing disk quota..."
ssh agent@$VM_IP "dd if=/dev/zero of=/mnt/inbox/fill bs=1G count=100 2>&1" | tail -3
used=$(ssh agent@$VM_IP "du -sh /mnt/inbox | cut -f1")
echo "  Disk usage after fill attempt: $used"

# Test 4: FD exhaustion
echo "[4/4] Testing file descriptor limit..."
ssh agent@$VM_IP "python3 -c 'import os; fds=[os.open(\"/dev/null\", os.O_RDONLY) for _ in range(100000)]'" 2>&1 | tail -3
if ssh agent@$VM_IP "echo ok" | grep -q ok; then
    echo "  PASS: VM survived FD exhaustion"
else
    echo "  FAIL: VM became unresponsive"
fi

echo "=== Chaos test complete ==="
```

---

## 7. Migration Plan

### Phase 1: libvirt Limits (Week 1)

1. Update `define_vm()` to include `<memtune>`, `<cputune>`, `<blkiotune>`
2. Add command-line options: `--mem-limit`, `--cpu-quota`, `--io-limit`
3. Test with existing VMs (verify no breaking changes)
4. Update documentation

### Phase 2: systemd cgroup Limits (Week 2)

1. Create hardened systemd service file
2. Update cloud-init to deploy new service
3. Add health check resource reporting
4. Test all failure modes

### Phase 3: Disk Quotas (Week 3)

1. Verify/create XFS filesystem for agentshare
2. Implement quota management scripts
3. Integrate with task provisioning
4. Add quota reporting to orchestrator

### Phase 4: Monitoring (Week 4)

1. Add resource metrics to management server
2. Create alerting rules
3. Build dashboard visualizations
4. Document runbooks for quota events

---

## 8. Security Considerations

### 8.1 cgroup Escape Risks

| Risk | Mitigation |
|------|------------|
| cgroup namespace manipulation | Disable user namespaces in VM |
| `/sys/fs/cgroup` write access | Mount as read-only for agent user |
| CAP_SYS_ADMIN in container | VM boundary isolates from host cgroups |

### 8.2 Quota Bypass Risks

| Risk | Mitigation |
|------|------------|
| Hardlinks to other projects | XFS project quotas count at inode level |
| Sparse file attacks | Account for allocated blocks, not file size |
| Symlink to unquoted path | virtiofs mount isolation prevents escape |

---

## 9. Configuration Reference

### 9.1 Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `AGENT_MEMORY_MAX_MB` | 7680 | Hard memory limit |
| `AGENT_MEMORY_HIGH_MB` | 6144 | Soft memory limit |
| `AGENT_TASKS_MAX` | 2048 | Maximum processes |
| `AGENT_CPU_QUOTA_PCT` | 400 | CPU quota (100 = 1 core) |
| `AGENT_IO_READ_MBPS` | 500 | Read bandwidth limit |
| `AGENT_IO_WRITE_MBPS` | 200 | Write bandwidth limit |
| `AGENT_DISK_QUOTA_GB` | 50 | Disk quota for inbox |
| `AGENT_NOFILE_LIMIT` | 65536 | File descriptor limit |

### 9.2 Command-Line Options

```bash
./provision-vm.sh agent-01 \
  --profile agentic-dev \
  --cpus 4 \
  --memory 8G \
  --disk 50G \
  --mem-limit 7680M \    # NEW: cgroup memory max
  --tasks-max 2048 \     # NEW: PID limit
  --io-limit 200M \      # NEW: I/O write limit
  --agentshare \
  --start
```

---

## 10. Appendices

### Appendix A: cgroup v2 Verification

```bash
# Check cgroup v2 is enabled
cat /sys/fs/cgroup/cgroup.controllers
# Should show: cpuset cpu io memory hugetlb pids rdma

# Check systemd is using cgroup v2
cat /proc/1/cgroup
# Should show single line: 0::/init.scope

# In VM, check agent service cgroup
systemctl status agentic-agent
# Shows cgroup path like: /system.slice/agentic-agent.service
```

### Appendix B: XFS Quota Commands

```bash
# Initialize quotas
xfs_quota -x -c "project -s myproject" /srv/agentshare

# Set limits
xfs_quota -x -c "limit -p bsoft=45g bhard=50g myproject" /srv/agentshare

# Check usage
xfs_quota -x -c "report -p -h" /srv/agentshare

# Remove limits
xfs_quota -x -c "limit -p bsoft=0 bhard=0 myproject" /srv/agentshare
```

### Appendix C: Troubleshooting

| Symptom | Likely Cause | Resolution |
|---------|--------------|------------|
| Service fails with "Failed to set up service cgroups" | cgroup v2 not enabled | Boot with `systemd.unified_cgroup_hierarchy=1` |
| MemoryMax ignored | Kernel too old | Requires Linux 4.5+ |
| IOBandwidth has no effect | virtio driver issue | Use `IOWeight` instead |
| Quota shows wrong usage | Delayed accounting | Run `xfs_quota -x -c sync` |

---

## Document Approval

| Role | Name | Date | Status |
|------|------|------|--------|
| Author | Security Architect | 2026-01-31 | Complete |
| Technical Review | - | Pending | - |
| Security Review | - | Pending | - |
| Approval | - | Pending | - |
