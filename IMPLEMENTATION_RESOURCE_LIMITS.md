# Resource Quota Implementation - Gitea Issue #86

## Implementation Summary

This document details the implementation of cgroup v2 resource limits and disk quotas for the agentic-sandbox project, based on `docs/security/resource-quota-design.md`.

## Status: PARTIALLY COMPLETE

### Completed Components

1. **Test Suite** ✅
   - File: `/home/roctinam/dev/agentic-sandbox/tests/e2e/test_resource_limits.py`
   - Status: Comprehensive E2E tests already implemented
   - Coverage: Memory, PID, FD, disk quota, I/O throttling, cgroup metrics

2. **Systemd Service with cgroup v2 Limits** ✅
   - File: `/home/roctinam/dev/agentic-sandbox/agent-rs/systemd/agent-client.service`
   - Status: Already has full cgroup v2 resource limits
   - Limits configured:
     - MemoryMax=7680M, MemoryHigh=6144M, MemorySwapMax=0
     - TasksMax=2048
     - CPUQuota=400%, CPUWeight=100
     - IOWeight=500, IOReadBandwidthMax=/dev/vda 500M, IOWriteBandwidthMax=/dev/vda 200M
     - LimitNOFILE=65536, LimitCORE=0

3. **Disk Quota Management Script** ✅
   - File: `/home/roctinam/dev/agentic-sandbox/scripts/setup-disk-quotas.sh`
   - Status: Fully implemented XFS project quota manager
   - Features:
     - Initial quota setup
     - Per-task quota creation/removal
     - Quota reporting
     - Project ID management (1000-1999 system, 2000-9999 tasks)

### Remaining Work

4. **provision-vm.sh Enhancements** 🔄 IN PROGRESS
   - File: `/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh`
   - Status: Needs modifications for:
     a. CLI options for resource limits
     b. libvirt XML resource tuning elements
     c. Resource limit calculation and parameter passing

---

## Required Changes to provision-vm.sh

### 1. Add CLI Options (in main() function)

Add new variables after line 2348:

```bash
# Resource limit options (default: auto-calculated from VM resources)
local mem_limit=""
local mem_soft=""
local cpu_quota=""
local tasks_max="2048"
local io_weight="500"
local io_read_limit="500M"
local io_write_limit="200M"
local nofile_limit="65536"
local disk_quota="50G"
```

### 2. Add Argument Parsing Cases (after line 2413)

Add before `-n|--dry-run)`:

```bash
            --mem-limit)
                mem_limit="$2"
                shift 2
                ;;
            --mem-soft)
                mem_soft="$2"
                shift 2
                ;;
            --cpu-quota)
                cpu_quota="$2"
                shift 2
                ;;
            --tasks-max)
                tasks_max="$2"
                shift 2
                ;;
            --io-weight)
                io_weight="$2"
                shift 2
                ;;
            --io-read-limit)
                io_read_limit="$2"
                shift 2
                ;;
            --io-write-limit)
                io_write_limit="$2"
                shift 2
                ;;
            --nofile-limit)
                nofile_limit="$2"
                shift 2
                ;;
            --disk-quota)
                disk_quota="$2"
                shift 2
                ;;
```

### 3. Update usage() Function (around line 283)

Add resource limits section after `-h, --help`:

```
Resource Limits (cgroup v2 and libvirt tuning):
  --mem-limit SIZE      Memory hard limit for agent service (default: auto from --memory)
  --mem-soft SIZE       Memory soft limit (default: 75% of --mem-limit)
  --cpu-quota PERCENT   CPU quota as percentage (default: cpus * 100%)
  --tasks-max NUM       Maximum PIDs/tasks (default: 2048)
  --io-weight NUM       I/O scheduling weight 100-10000 (default: 500)
  --io-read-limit SIZE  Read bandwidth limit (default: 500M)
  --io-write-limit SIZE Write bandwidth limit (default: 200M)
  --nofile-limit NUM    File descriptor limit (default: 65536)
  --disk-quota SIZE     Disk quota for inbox (default: 50G)
```

### 4. Add Resource Limit Helper Functions (before provision_vm())

Insert around line 1690:

```bash
# Parse size string (8G, 512M, etc.) to bytes
parse_size_to_bytes() {
    local size_str="$1"
    local number="${size_str%[GMK]*}"
    local unit="${size_str##*[0-9]}"

    case "${unit^^}" in
        G|GB)
            echo $((number * 1024 * 1024 * 1024))
            ;;
        M|MB)
            echo $((number * 1024 * 1024))
            ;;
        K|KB)
            echo $((number * 1024))
            ;;
        *)
            echo "$number"
            ;;
    esac
}

# Parse size string to MiB
parse_size_to_mb() {
    local bytes=$(parse_size_to_bytes "$1")
    echo $((bytes / 1024 / 1024))
}

# Calculate resource limits from user options
calculate_resource_limits() {
    local vm_cpus="$1"
    local vm_memory_mb="$2"
    local user_mem_limit="$3"
    local user_mem_soft="$4"
    local user_cpu_quota="$5"
    local user_io_read="$6"
    local user_io_write="$7"

    # Memory limit: default to 94% of VM memory
    if [[ -z "$user_mem_limit" ]]; then
        mem_limit_mb=$((vm_memory_mb * 94 / 100))
    else
        mem_limit_mb=$(parse_size_to_mb "$user_mem_limit")
    fi

    # Memory soft limit: default to 75% of hard limit
    if [[ -z "$user_mem_soft" ]]; then
        mem_soft_mb=$((mem_limit_mb * 75 / 100))
    else
        mem_soft_mb=$(parse_size_to_mb "$user_mem_soft")
    fi

    # CPU quota: default to cpus * 100%
    if [[ -z "$user_cpu_quota" ]]; then
        cpu_quota_pct=$((vm_cpus * 100))
    else
        cpu_quota_pct="$user_cpu_quota"
    fi

    # I/O limits: convert to bytes/sec
    if [[ -z "$user_io_read" ]]; then
        io_read_bps=524288000
    else
        io_read_bps=$(parse_size_to_bytes "$user_io_read")
    fi

    if [[ -z "$user_io_write" ]]; then
        io_write_bps=209715200
    else
        io_write_bps=$(parse_size_to_bytes "$user_io_write")
    fi

    echo "$mem_limit_mb $mem_soft_mb $cpu_quota_pct $io_read_bps $io_write_bps"
}
```

### 5. Invoke Resource Calculation (in provision_vm(), before define_vm call)

Around line 2139, add before `log_info "Defining VM in libvirt..."`:

```bash
    # Calculate resource limits for libvirt XML
    local limits
    limits=$(calculate_resource_limits "$cpus" "$memory_mb" "$mem_limit" "$mem_soft" "$cpu_quota" "$io_read_limit" "$io_write_limit")
    read -r mem_limit_mb mem_soft_mb cpu_quota_pct io_read_bps io_write_bps <<< "$limits"
```

### 6. Update define_vm() Parameters (line 1784)

Add new parameters after outbox_path:

```bash
define_vm() {
    local vm_name="$1"
    local disk_path="$2"
    local cloud_init_iso="$3"
    local cpus="$4"
    local memory_mb="$5"
    local network="$6"
    local mac_address="${7:-}"
    local use_agentshare="${8:-false}"
    local inbox_path="${9:-}"
    local outbox_path="${10:-}"
    # Resource limits (NEW)
    local mem_limit_mb="${11:-$memory_mb}"
    local cpu_quota_pct="${12:-$((cpus * 100))}"
    local io_weight="${13:-500}"
    local io_read_bps="${14:-524288000}"
    local io_write_bps="${15:-209715200}"
```

### 7. Update define_vm() XML Generation (around line 1845)

Add resource tuning elements after `<vcpu>` line, before `<os>`:

```xml
  <vcpu placement='static'>$cpus</vcpu>$memoryBacking

  <!-- Resource limits: prevent VM from exhausting host resources -->
  <memtune>
    <hard_limit unit='MiB'>$((mem_limit_mb + 256))</hard_limit>
    <soft_limit unit='MiB'>$mem_limit_mb</soft_limit>
  </memtune>

  <!-- CPU tuning: fair scheduling and quota enforcement -->
  <cputune>
    <shares>$((cpus * 1024))</shares>
    <period>100000</period>
    <quota>$((cpu_quota_pct * 1000))</quota>
  </cputune>

  <!-- Block I/O tuning: prevent storage abuse -->
  <blkiotune>
    <weight>$io_weight</weight>
    <device>
      <path>/dev/vda</path>
      <read_bytes_sec>$io_read_bps</read_bytes_sec>
      <write_bytes_sec>$io_write_bps</write_bytes_sec>
    </device>
  </blkiotune>

  <os>
```

### 8. Update define_vm() Call (line 2142)

Change from:

```bash
xml_path=$(define_vm "$vm_name" "$disk_path" "$cloud_init_iso" "$cpus" "$memory_mb" "$network" "$mac_address" "$use_agentshare" "$inbox_path" "$outbox_path")
```

To:

```bash
xml_path=$(define_vm "$vm_name" "$disk_path" "$cloud_init_iso" "$cpus" "$memory_mb" "$network" "$mac_address" "$use_agentshare" "$inbox_path" "$outbox_path" "$mem_limit_mb" "$cpu_quota_pct" "$io_weight" "$io_read_bps" "$io_write_bps")
```

---

## Testing

### Unit Tests

The tests are already in place at `tests/e2e/test_resource_limits.py`. To run:

```bash
cd /home/roctinam/dev/agentic-sandbox
pytest tests/e2e/test_resource_limits.py -v
```

### Manual Testing

1. **Provision VM with default limits**:
```bash
./images/qemu/provision-vm.sh --profile agentic-dev --start agent-test-01
```

2. **Provision VM with custom limits**:
```bash
./images/qemu/provision-vm.sh \
  --cpus 8 --memory 16G \
  --mem-limit 15G --cpu-quota 800 --tasks-max 4096 \
  --io-write-limit 400M \
  --profile agentic-dev --start agent-heavy-01
```

3. **Verify libvirt XML**:
```bash
virsh dumpxml agent-test-01 | grep -A 10 "memtune\|cputune\|blkiotune"
```

4. **Verify cgroup limits in VM**:
```bash
ssh agent@192.168.122.201 'cat /sys/fs/cgroup/memory.max /sys/fs/cgroup/pids.max'
```

5. **Test resource exhaustion** (should be contained):
```bash
ssh agent@192.168.122.201 'python3 -c "x = bytearray(10 * 1024**3)"'  # Should OOM
ssh agent@192.168.122.201 'timeout 5 bash -c ":(){ :|:& };:"'  # Fork bomb
```

---

## Verification Checklist

- [ ] provision-vm.sh accepts new CLI options
- [ ] Resource limits are auto-calculated when not specified
- [ ] libvirt XML includes memtune, cputune, blkiotune elements
- [ ] VM starts successfully with resource limits applied
- [ ] virsh dumpxml shows correct resource tuning
- [ ] cgroup v2 limits visible in VM at /sys/fs/cgroup/*
- [ ] Memory exhaustion triggers OOM (not system crash)
- [ ] Fork bomb is contained by TasksMax
- [ ] VM remains responsive under resource pressure
- [ ] E2E tests pass: `pytest tests/e2e/test_resource_limits.py`

---

## Files Modified/Created

### Created
- `/home/roctinam/dev/agentic-sandbox/tests/e2e/test_resource_limits.py` ✅ (already exists)
- `/home/roctinam/dev/agentic-sandbox/scripts/setup-disk-quotas.sh` ✅ (already exists)
- `/home/roctinam/dev/agentic-sandbox/IMPLEMENTATION_RESOURCE_LIMITS.md` (this file)

### Modified (planned)
- `/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh` 🔄
  - Added CLI options for resource limits
  - Added helper functions for size parsing and calculation
  - Updated define_vm() function signature and XML generation
  - Updated define_vm() call with resource parameters

### Already Correct
- `/home/roctinam/dev/agentic-sandbox/agent-rs/systemd/agent-client.service` ✅
  - Already has all required cgroup v2 limits

---

## Implementation Approach

The implementation follows test-driven development (TDD):

1. **Tests First** ✅ - Comprehensive E2E tests already written
2. **Systemd Service** ✅ - cgroup limits already configured
3. **Disk Quotas** ✅ - Management script already implemented
4. **VM Provisioning** 🔄 - Need to add libvirt tuning (in progress)

The design ensures:
- **Backwards Compatibility**: All new options are optional with sensible defaults
- **Defense in Depth**: Three layers (libvirt, cgroup, disk quotas)
- **Fail Safe**: Resource exhaustion kills process, not VM or host

---

## Next Steps

1. Apply the changes to `provision-vm.sh` as detailed above
2. Test provisioning with default and custom limits
3. Run E2E test suite to verify all limits are enforced
4. Update documentation (BUILD.md, README.md) with resource limit options
5. Close Gitea issue #86

---

## Reference

- Design Document: `docs/security/resource-quota-design.md`
- Gitea Issue: https://git.integrolabs.net/roctinam/agentic-sandbox/issues/86
- Test Suite: `tests/e2e/test_resource_limits.py`
- Systemd Service: `agent-rs/systemd/agent-client.service`
- Disk Quotas: `scripts/setup-disk-quotas.sh`
